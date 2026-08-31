from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import shutil
import struct
from collections import OrderedDict
from collections.abc import Iterator, Sequence
from contextlib import suppress
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import BinaryIO, Literal
from urllib.parse import urljoin, urlsplit

import geopandas as gpd
import httpx
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import shapely
from shapely.geometry import box
from shapely.geometry.base import BaseGeometry

from .config import ROOT, SOURCES, Source
from .download import USER_AGENT, cached_snapshot
from .geometry import footprint_id

PASDA_LAS_URL = "https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/"
AUDITED_LISTING_SHA256 = "cbc710dacbf13902a168c6af262734e8a07d79565d15463faf4edf4d7a5f31b5"
AUDITED_CITY_SHA256 = "b12d1e6e62ce72b5c409792e2535a3b90c6bcfa2d2d6c28455cd750f7db8c942"
AUDITED_BUILDING_SHA256 = "9e1a96e6287d1253a0f4d92d6f8fb83931776a0c8c43df4525b46a3b1ceef352"
# Canonical JSON of the authority fields and every ordered tile, excluding the
# retrieval timestamp and redundant summaries. See semantic_inventory_sha256.
AUDITED_INVENTORY_SHA256 = "0a04f12d90a4393c09152d2655947456c7b531b5c67c62b51c4b92bf5d9cec96"
LIDAR_DIR = ROOT / "data" / "lidar-2025"
INVENTORY_PATH = LIDAR_DIR / "inventory.json"
AUDIT_CANDIDATE_PATH = LIDAR_DIR / "inventory.audit-candidate.json"
PROGRESS_PATH = LIDAR_DIR / "progress.json"
FOOTPRINTS_PATH = LIDAR_DIR / "footprints.parquet"
RAW_LAS_DIR = LIDAR_DIR / "raw"
DERIVED_DIR = LIDAR_DIR / "derived"
MERGED_EVIDENCE_PATH = LIDAR_DIR / "building-evidence.parquet"
PARTIAL_EVIDENCE_PATH = LIDAR_DIR / "building-evidence.partial.parquet"

# Filenames expose a rounded Pennsylvania South State Plane lower-left corner.
# Actual LAS bounds are authoritative after download. The conservative padding
# prevents a boundary tile from being omitted because the easting is rounded.
TILE_NAME = re.compile(r"^(?P<easting>\d{5})E(?P<northing>\d{6})N\.las$")
LISTING_ROW = re.compile(
    r"(?P<size>\d+)\s*<a\s+href=[\"'](?P<href>[^\"']+\.las)[\"'][^>]*>"
    r"(?P<label>[^<]+)</a>",
    re.IGNORECASE,
)
CONTENT_RANGE = re.compile(r"^bytes (?P<start>\d+)-(?P<end>\d+)/(?P<total>\d+)$")
TILE_SIZE_FEET = 2_640.0
EASTING_ROUNDING_FEET = 100.0
# PASDA metadata identifies NAD83(2011) Pennsylvania South in US survey feet.
CITY_CRS_FEET = 6565
US_SURVEY_FOOT_METERS = 0.3048006096012192
DOWNLOAD_CHUNK_BYTES = 4 * 1024 * 1024
POINT_CHUNK_SIZE = 750_000
MAX_OPEN_SPILL_FILES = 64


class LidarError(RuntimeError):
    pass


@dataclass(frozen=True, slots=True)
class Tile:
    name: str
    url: str
    bytes: int
    approximate_bounds_ft: tuple[float, float, float, float]
    selected: bool


@dataclass(frozen=True, slots=True)
class LasHeader:
    version: str
    point_format: int
    point_record_bytes: int
    point_count: int
    point_data_offset: int
    scales: tuple[float, float, float]
    offsets: tuple[float, float, float]
    bounds_ft: tuple[float, float, float, float]


class InvalidLasSourceError(LidarError):
    """An exact pinned download whose LAS structure is physically invalid."""

    def __init__(
        self,
        path: Path,
        source_sha256: str,
        reason: str,
        *,
        header: LasHeader | None = None,
        expected_minimum_bytes: int | None = None,
    ) -> None:
        super().__init__(reason)
        self.path = path
        self.source_sha256 = source_sha256
        self.actual_bytes = path.stat().st_size
        self.header = header
        self.expected_minimum_bytes = expected_minimum_bytes


@dataclass(frozen=True, slots=True)
class Inventory:
    schema_version: int
    source_url: str
    listing_sha256: str
    fetched_at: str
    city_sha256: str
    building_sha256: str
    tiles: tuple[Tile, ...]


type TileStatus = Literal[
    "downloading",
    "downloaded",
    "derived",
    "released",
    "outside",
    "rejected_source",
]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _write_json_atomic(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(f"{path.suffix}.part")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def _tile_from_name(name: str, url: str, size: int, selected: bool = False) -> Tile:
    match = TILE_NAME.fullmatch(name)
    if match is None:
        raise LidarError(f"unexpected PASDA LAS filename: {name}")
    easting = float(int(match.group("easting")) * 100)
    northing = float(int(match.group("northing")))
    bounds = (
        easting - EASTING_ROUNDING_FEET,
        northing - EASTING_ROUNDING_FEET,
        easting + TILE_SIZE_FEET + EASTING_ROUNDING_FEET,
        northing + TILE_SIZE_FEET + EASTING_ROUNDING_FEET,
    )
    return Tile(name, url, size, bounds, selected)


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def _validate_tile_url(name: str, url: str) -> None:
    parsed = urlsplit(url)
    source = urlsplit(PASDA_LAS_URL)
    if (
        parsed.scheme != source.scheme
        or parsed.netloc != source.netloc
        or parsed.query
        or parsed.fragment
        or parsed.path != f"{source.path}{name}"
        or url != f"{PASDA_LAS_URL}{name}"
    ):
        raise LidarError(f"LiDAR tile URL is outside the audited PASDA LAS directory: {url}")


def parse_listing(content: bytes, source_url: str = PASDA_LAS_URL) -> tuple[Tile, ...]:
    if source_url != PASDA_LAS_URL:
        raise LidarError(f"unexpected PASDA LAS listing URL: {source_url}")
    text = content.decode("utf-8", errors="strict")
    tiles: list[Tile] = []
    for match in LISTING_ROW.finditer(text):
        label = match.group("label").strip()
        href = match.group("href").strip()
        if label != Path(href).name:
            raise LidarError(f"PASDA listing label does not match link: {label!r}, {href!r}")
        size = int(match.group("size"))
        if size <= 0:
            raise LidarError(f"PASDA listing reports an empty LAS file: {label}")
        url = urljoin(source_url, href)
        _validate_tile_url(label, url)
        tiles.append(_tile_from_name(label, url, size))
    tiles.sort(key=lambda tile: tile.name)
    if not tiles:
        raise LidarError("PASDA listing contained no LAS files")
    if len({tile.name for tile in tiles}) != len(tiles):
        raise LidarError("PASDA listing contains duplicate LAS filenames")
    return tuple(tiles)


def _default_snapshot(source: Source) -> Path:
    snapshot = cached_snapshot(source)
    if snapshot is None:
        raise LidarError(f"no validated cached {source.name} snapshot; run `poe ingest` first")
    return snapshot.path


def _city_geometry(city_path: Path) -> BaseGeometry:
    frame = gpd.read_file(city_path)
    if frame.empty or frame.crs is None:
        raise LidarError(f"city boundary is empty or has no CRS: {city_path}")
    geometry = shapely.union_all(frame.to_crs(CITY_CRS_FEET).geometry)
    if geometry.is_empty:
        raise LidarError("city boundary produced empty projected geometry")
    return geometry


def select_city_tiles(tiles: Sequence[Tile], city: BaseGeometry) -> tuple[Tile, ...]:
    result: list[Tile] = []
    for tile in tiles:
        intersects = city.intersects(box(*tile.approximate_bounds_ft))
        result.append(Tile(tile.name, tile.url, tile.bytes, tile.approximate_bounds_ft, intersects))
    return tuple(result)


def inventory_dict(inventory: Inventory) -> dict[str, object]:
    selected = sum(tile.selected for tile in inventory.tiles)
    return {
        "schema_version": inventory.schema_version,
        "source_url": inventory.source_url,
        "listing_sha256": inventory.listing_sha256,
        "fetched_at": inventory.fetched_at,
        "city_sha256": inventory.city_sha256,
        "building_sha256": inventory.building_sha256,
        "counts": {"listed": len(inventory.tiles), "selected": selected},
        "bytes": {
            "listed": sum(tile.bytes for tile in inventory.tiles),
            "selected": sum(tile.bytes for tile in inventory.tiles if tile.selected),
        },
        "tiles": [asdict(tile) for tile in inventory.tiles],
    }


def semantic_inventory_sha256(inventory: Inventory) -> str:
    value = {
        "schema_version": inventory.schema_version,
        "source_url": inventory.source_url,
        "listing_sha256": inventory.listing_sha256,
        "city_sha256": inventory.city_sha256,
        "building_sha256": inventory.building_sha256,
        "tiles": [asdict(tile) for tile in inventory.tiles],
    }
    payload = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def validate_audited_inventory(inventory: Inventory) -> None:
    expected_fields = (
        ("source URL", inventory.source_url, PASDA_LAS_URL),
        ("listing SHA-256", inventory.listing_sha256, AUDITED_LISTING_SHA256),
        ("City SHA-256", inventory.city_sha256, AUDITED_CITY_SHA256),
        ("building SHA-256", inventory.building_sha256, AUDITED_BUILDING_SHA256),
        (
            "semantic inventory SHA-256",
            semantic_inventory_sha256(inventory),
            AUDITED_INVENTORY_SHA256,
        ),
    )
    for label, actual, expected in expected_fields:
        if actual != expected:
            raise LidarError(
                f"LiDAR {label} is outside the checked-in 2026-08-30 audit pin: "
                f"{actual!r} != {expected!r}; create and review an audit candidate"
            )


def parse_inventory(value: object, *, require_audited: bool = True) -> Inventory:
    if not isinstance(value, dict) or value.get("schema_version") != 2:
        raise LidarError("unsupported LiDAR inventory schema")
    raw_tiles = value.get("tiles")
    if not isinstance(raw_tiles, list):
        raise LidarError("LiDAR inventory has no tile list")
    tiles: list[Tile] = []
    for raw in raw_tiles:
        if not isinstance(raw, dict):
            raise LidarError("LiDAR inventory contains an invalid tile")
        name = raw.get("name")
        url = raw.get("url")
        size = raw.get("bytes")
        selected = raw.get("selected")
        if (
            not isinstance(name, str)
            or not isinstance(url, str)
            or not isinstance(size, int)
            or not isinstance(selected, bool)
        ):
            raise LidarError("LiDAR inventory tile fields are invalid")
        if size <= 0:
            raise LidarError(f"LiDAR inventory size is invalid for {name}")
        _validate_tile_url(name, url)
        parsed = _tile_from_name(name, url, size, selected)
        raw_bounds = raw.get("approximate_bounds_ft")
        if not isinstance(raw_bounds, list) or len(raw_bounds) != 4:
            raise LidarError(f"LiDAR inventory bounds are invalid for {name}")
        if tuple(float(item) for item in raw_bounds) != parsed.approximate_bounds_ft:
            raise LidarError(f"LiDAR inventory bounds do not match filename for {name}")
        tiles.append(parsed)
    if [tile.name for tile in tiles] != sorted(tile.name for tile in tiles):
        raise LidarError("LiDAR inventory tiles are not deterministically sorted")
    if len({tile.name for tile in tiles}) != len(tiles):
        raise LidarError("LiDAR inventory contains duplicate tile names")
    if value.get("source_url") != PASDA_LAS_URL:
        raise LidarError("LiDAR inventory has an unexpected source URL")
    if not isinstance(value.get("fetched_at"), str) or not value["fetched_at"]:
        raise LidarError("LiDAR inventory provenance is invalid")
    for field in ("listing_sha256", "city_sha256", "building_sha256"):
        if not _is_sha256(value.get(field)):
            raise LidarError(f"LiDAR inventory {field} is not a lowercase SHA-256")
    expected_counts = {"listed": len(tiles), "selected": sum(tile.selected for tile in tiles)}
    expected_bytes = {
        "listed": sum(tile.bytes for tile in tiles),
        "selected": sum(tile.bytes for tile in tiles if tile.selected),
    }
    if value.get("counts") != expected_counts or value.get("bytes") != expected_bytes:
        raise LidarError("LiDAR inventory count or byte summaries do not match its tiles")
    inventory = Inventory(
        2,
        str(value["source_url"]),
        str(value["listing_sha256"]),
        str(value["fetched_at"]),
        str(value["city_sha256"]),
        str(value["building_sha256"]),
        tuple(tiles),
    )
    if require_audited:
        validate_audited_inventory(inventory)
    return inventory


def load_inventory(path: Path = INVENTORY_PATH) -> Inventory:
    try:
        return parse_inventory(json.loads(path.read_text()))
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise LidarError(
            f"missing or corrupt LiDAR inventory: {path}; run `poe lidar-plan`"
        ) from error


def validate_active_sources(inventory: Inventory) -> None:
    for source, expected in (
        (SOURCES.city, inventory.city_sha256),
        (SOURCES.buildings, inventory.building_sha256),
    ):
        snapshot = cached_snapshot(source)
        if snapshot is None or snapshot.sha256 != expected:
            raise LidarError(
                f"active {source.name} snapshot differs from the LiDAR plan; "
                "run `python -m isophilly_ingest.lidar plan --refresh`"
            )


def create_inventory(
    city_path: Path,
    building_path: Path,
    *,
    output_path: Path = INVENTORY_PATH,
    client: httpx.Client | None = None,
    audit_candidate: bool = False,
) -> Inventory:
    owns_client = client is None
    if client is None:
        client = httpx.Client(
            headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=120
        )
    try:
        response = client.get(PASDA_LAS_URL)
        response.raise_for_status()
        if str(response.url) != PASDA_LAS_URL:
            raise LidarError(f"PASDA listing redirected outside its audited URL: {response.url}")
    finally:
        if owns_client:
            client.close()
    listing = response.content
    tiles = select_city_tiles(parse_listing(listing), _city_geometry(city_path))
    inventory = Inventory(
        2,
        PASDA_LAS_URL,
        hashlib.sha256(listing).hexdigest(),
        datetime.now(UTC).isoformat(),
        sha256_file(city_path),
        sha256_file(building_path),
        tiles,
    )
    if not audit_candidate:
        validate_audited_inventory(inventory)
    _write_json_atomic(output_path, inventory_dict(inventory))
    return inventory


def prepare_footprints(building_path: Path, city_path: Path) -> None:
    city = _city_geometry(city_path)
    frame = gpd.read_file(building_path)
    if frame.empty or frame.crs is None:
        raise LidarError("building footprints are empty or have no CRS")
    frame["building_id"] = [footprint_id(geometry) for geometry in frame.geometry]
    frame = frame.to_crs(CITY_CRS_FEET)
    frame = frame.loc[frame.geometry.intersects(city), ["building_id", "geometry"]].copy()
    frame["source_sha256"] = sha256_file(building_path)
    FOOTPRINTS_PATH.parent.mkdir(parents=True, exist_ok=True)
    temporary = FOOTPRINTS_PATH.with_suffix(".parquet.part")
    frame.to_parquet(temporary, index=False, compression="zstd", write_covering_bbox=True)
    temporary.replace(FOOTPRINTS_PATH)


def read_las_header(file: BinaryIO) -> LasHeader:
    data = file.read(375)
    if len(data) < 227 or data[:4] != b"LASF":
        raise LidarError("file is not a complete LAS header")
    major, minor = struct.unpack_from("<BB", data, 24)
    raw_format = data[104]
    if raw_format & 0x80:
        raise LidarError("compressed LAZ point records are not supported")
    point_format = raw_format & 0x3F
    record_bytes = struct.unpack_from("<H", data, 105)[0]
    legacy_count = struct.unpack_from("<I", data, 107)[0]
    data_offset = struct.unpack_from("<I", data, 96)[0]
    scales = struct.unpack_from("<ddd", data, 131)
    offsets = struct.unpack_from("<ddd", data, 155)
    max_x, min_x, max_y, min_y = struct.unpack_from("<dddd", data, 179)
    point_count = legacy_count
    if (major, minor) >= (1, 4):
        if len(data) < 255:
            raise LidarError("LAS 1.4 header is truncated")
        extended_count = struct.unpack_from("<Q", data, 247)[0]
        point_count = extended_count or legacy_count
    if point_format > 10 or record_bytes < 20 or point_count <= 0:
        raise LidarError("unsupported or empty LAS point layout")
    return LasHeader(
        f"{major}.{minor}",
        point_format,
        record_bytes,
        point_count,
        data_offset,
        scales,
        offsets,
        (min_x, min_y, max_x, max_y),
    )


def load_las_header(path: Path) -> LasHeader:
    with path.open("rb") as file:
        header = read_las_header(file)
    minimum_size = header.point_data_offset + header.point_count * header.point_record_bytes
    if path.stat().st_size < minimum_size:
        raise LidarError(
            f"LAS point data is truncated: {path.stat().st_size:,} < {minimum_size:,} bytes"
        )
    return header


def _validate_exact_las_source(path: Path, source_sha256: str) -> LasHeader:
    try:
        with path.open("rb") as file:
            header = read_las_header(file)
    except LidarError as error:
        raise InvalidLasSourceError(path, source_sha256, str(error)) from error
    minimum_size = header.point_data_offset + header.point_count * header.point_record_bytes
    if path.stat().st_size < minimum_size:
        reason = f"LAS point data is truncated: {path.stat().st_size:,} < {minimum_size:,} bytes"
        raise InvalidLasSourceError(
            path,
            source_sha256,
            reason,
            header=header,
            expected_minimum_bytes=minimum_size,
        )
    return header


def _point_dtype(header: LasHeader) -> np.dtype:
    classification_offset = 16 if header.point_format >= 6 else 15
    if header.point_record_bytes <= classification_offset:
        raise LidarError("LAS point record is too short for classification")
    return np.dtype(
        {
            "names": ["x", "y", "z", "classification"],
            "formats": ["<i4", "<i4", "<i4", "u1"],
            "offsets": [0, 4, 8, classification_offset],
            "itemsize": header.point_record_bytes,
        }
    )


def iter_las_points(path: Path, header: LasHeader) -> Iterator[tuple[np.ndarray, ...]]:
    points = np.memmap(
        path,
        dtype=_point_dtype(header),
        mode="r",
        offset=header.point_data_offset,
        shape=(header.point_count,),
    )
    for start in range(0, header.point_count, POINT_CHUNK_SIZE):
        chunk = points[start : start + POINT_CHUNK_SIZE]
        yield (
            np.asarray(chunk["x"], dtype=np.float64) * header.scales[0] + header.offsets[0],
            np.asarray(chunk["y"], dtype=np.float64) * header.scales[1] + header.offsets[1],
            np.asarray(chunk["z"], dtype=np.int32),
            np.asarray(chunk["classification"]),
        )


EVIDENCE_SCHEMA = pa.schema(
    [
        ("building_id", pa.string()),
        ("source_footprints_sha256", pa.string()),
        ("tile", pa.string()),
        ("building_point_count", pa.int64()),
        ("ground_point_count", pa.int64()),
        ("ground_elevation_m", pa.float32()),
        ("roof_p10_m", pa.float32()),
        ("roof_p50_m", pa.float32()),
        ("roof_p90_m", pa.float32()),
        ("height_p90_m", pa.float32()),
        ("roof_spread_m", pa.float32()),
        ("quality", pa.string()),
    ]
)


class _SampleSpool:
    """Bounded-handle, exact int32 sample spill for one source tile."""

    def __init__(self, root: Path) -> None:
        self.root = root
        self.counts: dict[tuple[int, str], int] = {}
        self.bytes_written = 0
        self._handles: OrderedDict[Path, BinaryIO] = OrderedDict()

    def _path(self, building_index: int, kind: str) -> Path:
        return self.root / f"{building_index:08d}.{kind}.i32"

    def append(self, building_index: int, kind: str, values: np.ndarray) -> None:
        if not len(values):
            return
        path = self._path(building_index, kind)
        handle = self._handles.pop(path, None)
        if handle is None:
            if len(self._handles) >= MAX_OPEN_SPILL_FILES:
                _, oldest = self._handles.popitem(last=False)
                oldest.close()
            handle = path.open("ab")
        self._handles[path] = handle
        payload = np.asarray(values, dtype="<i4").tobytes()
        handle.write(payload)
        key = (building_index, kind)
        self.counts[key] = self.counts.get(key, 0) + len(values)
        self.bytes_written += len(payload)

    def close(self) -> None:
        for handle in self._handles.values():
            handle.close()
        self._handles.clear()

    def values(self, building_index: int, kind: str) -> np.memmap | None:
        count = self.counts.get((building_index, kind), 0)
        if not count:
            return None
        return np.memmap(self._path(building_index, kind), dtype="<i4", mode="r", shape=(count,))


def _spill_matches(
    tree: shapely.STRtree,
    x: np.ndarray,
    y: np.ndarray,
    raw_z: np.ndarray,
    spool: _SampleSpool,
    kind: str,
) -> None:
    if not len(x):
        return
    pairs = tree.query(shapely.points(x, y), predicate="within")
    if pairs.shape[1] == 0:
        return
    point_indices, building_indices = pairs
    order = np.argsort(building_indices, kind="stable")
    building_indices = building_indices[order]
    point_indices = point_indices[order]
    boundaries = np.flatnonzero(np.diff(building_indices)) + 1
    for indices in np.split(np.arange(len(building_indices)), boundaries):
        building_index = int(building_indices[indices[0]])
        spool.append(building_index, kind, raw_z[point_indices[indices]])


def _remove_spill_directory(path: Path) -> None:
    if path.is_symlink():
        path.unlink()
    elif path.exists():
        shutil.rmtree(path)


def _spilled_rows(
    las_path: Path,
    header: LasHeader,
    buildings: gpd.GeoDataFrame,
    spool: _SampleSpool,
) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    identifiers = buildings["building_id"].tolist()
    source_digests = buildings["source_sha256"].tolist()
    for index, building_id in enumerate(identifiers):
        roof = spool.values(index, "roof")
        ground = spool.values(index, "ground")
        roof_count = 0 if roof is None else len(roof)
        ground_count = 0 if ground is None else len(ground)
        if roof_count < 10 or ground_count < 5:
            continue
        roof_array = np.asarray(roof, dtype=np.int32)
        ground_array = np.asarray(ground, dtype=np.int32)
        raw_roof_quantiles = np.quantile(roof_array, np.asarray((0.1, 0.5, 0.9), dtype=np.float64))
        raw_ground_quantile = float(np.quantile(ground_array, 0.25))
        roof_quantiles = (
            raw_roof_quantiles * header.scales[2] + header.offsets[2]
        ) * US_SURVEY_FOOT_METERS
        ground_m = (
            raw_ground_quantile * header.scales[2] + header.offsets[2]
        ) * US_SURVEY_FOOT_METERS
        spread = float(roof_quantiles[2] - roof_quantiles[0])
        rows.append(
            {
                "building_id": str(building_id),
                "source_footprints_sha256": str(source_digests[index]),
                "tile": las_path.name,
                "building_point_count": roof_count,
                "ground_point_count": ground_count,
                "ground_elevation_m": float(ground_m),
                "roof_p10_m": float(roof_quantiles[0]),
                "roof_p50_m": float(roof_quantiles[1]),
                "roof_p90_m": float(roof_quantiles[2]),
                "height_p90_m": float(roof_quantiles[2] - ground_m),
                "roof_spread_m": spread,
                "quality": "high" if roof_count >= 100 and ground_count >= 20 else "usable",
            }
        )
    return rows


def _derive_spilled_rows(
    las_path: Path, header: LasHeader, buildings: gpd.GeoDataFrame, work_path: Path
) -> tuple[list[dict[str, object]], int]:
    geometries = np.asarray(buildings.geometry.array, dtype=object)
    roof_tree = shapely.STRtree(geometries)
    ground_tree = shapely.STRtree(shapely.buffer(geometries, 12.0))
    spool = _SampleSpool(work_path)
    try:
        for x, y, raw_z, classification in iter_las_points(las_path, header):
            roof_mask = classification == 6
            _spill_matches(
                roof_tree,
                x[roof_mask],
                y[roof_mask],
                raw_z[roof_mask],
                spool,
                "roof",
            )
            ground_mask = classification == 2
            _spill_matches(
                ground_tree,
                x[ground_mask],
                y[ground_mask],
                raw_z[ground_mask],
                spool,
                "ground",
            )
        spool.close()
        return _spilled_rows(las_path, header, buildings, spool), spool.bytes_written
    finally:
        spool.close()


def derive_evidence(
    las_path: Path,
    buildings: gpd.GeoDataFrame,
    output_path: Path,
) -> int:
    header = load_las_header(las_path)
    if buildings.crs is None or buildings.crs.to_epsg() != CITY_CRS_FEET:
        raise LidarError(f"building evidence requires EPSG:{CITY_CRS_FEET} footprints")
    buildings = buildings.loc[buildings.geometry.intersects(box(*header.bounds_ft))].copy()
    work_parent = output_path.parent / ".lidar-work"
    work_path = work_parent / las_path.name
    _remove_spill_directory(work_path)
    work_path.mkdir(parents=True)
    try:
        rows, spill_bytes = _derive_spilled_rows(las_path, header, buildings, work_path)
        metadata = {
            b"lidar_point_passes": b"1",
            b"lidar_spill_bytes": str(spill_bytes).encode(),
            b"lidar_spill_encoding": b"little-endian-int32-z",
        }
        table = pa.Table.from_pylist(rows, schema=EVIDENCE_SCHEMA).replace_schema_metadata(metadata)
    finally:
        _remove_spill_directory(work_path)
        with suppress(OSError):
            work_parent.rmdir()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = output_path.with_suffix(".parquet.part")
    pq.write_table(table, temporary, compression="zstd")
    temporary.replace(output_path)
    return table.num_rows


def _new_progress(inventory: Inventory) -> dict[str, object]:
    return {
        "schema_version": 2,
        "inventory_listing_sha256": inventory.listing_sha256,
        "inventory_city_sha256": inventory.city_sha256,
        "inventory_building_sha256": inventory.building_sha256,
        "tiles": {},
    }


def _load_progress(inventory: Inventory) -> dict[str, object]:
    try:
        value = json.loads(PROGRESS_PATH.read_text())
    except FileNotFoundError:
        return _new_progress(inventory)
    if not isinstance(value, dict) or not isinstance(value.get("tiles"), dict):
        raise LidarError(f"invalid progress manifest: {PROGRESS_PATH}")
    if (
        value.get("schema_version") != 2
        or value.get("inventory_listing_sha256") != inventory.listing_sha256
        or value.get("inventory_city_sha256") != inventory.city_sha256
        or value.get("inventory_building_sha256") != inventory.building_sha256
    ):
        return _new_progress(inventory)
    return value


def _record_progress(
    inventory: Inventory, tile: Tile, status: TileStatus, **details: object
) -> None:
    progress = _load_progress(inventory)
    tiles = progress["tiles"]
    if not isinstance(tiles, dict):
        raise AssertionError("progress parser guarantees a tile mapping")
    tiles[tile.name] = {
        "status": status,
        "expected_bytes": tile.bytes,
        "updated_at": datetime.now(UTC).isoformat(),
        **details,
    }
    _write_json_atomic(PROGRESS_PATH, progress)


def download_tile(tile: Tile, inventory: Inventory, client: httpx.Client) -> tuple[Path, str]:
    RAW_LAS_DIR.mkdir(parents=True, exist_ok=True)
    destination = RAW_LAS_DIR / tile.name
    partial = destination.with_suffix(".las.part")
    progress = _load_progress(inventory)
    progress_tiles = progress["tiles"]
    if not isinstance(progress_tiles, dict):
        raise AssertionError("progress parser guarantees a tile mapping")
    entry = progress_tiles.get(tile.name)
    if destination.exists():
        digest = sha256_file(destination)
        if (
            isinstance(entry, dict)
            and destination.stat().st_size == tile.bytes
            and entry.get("sha256", entry.get("source_sha256")) == digest
        ):
            _validate_exact_las_source(destination, digest)
            return destination, digest
        destination.unlink()
    if partial.exists() and (
        not isinstance(entry, dict)
        or entry.get("status") != "downloading"
        or entry.get("downloaded_bytes") != partial.stat().st_size
    ):
        partial.unlink()
    if partial.exists() and partial.stat().st_size > tile.bytes:
        raise LidarError(f"partial LAS is larger than pinned inventory: {partial}")
    offset = partial.stat().st_size if partial.exists() else 0
    _record_progress(inventory, tile, "downloading", downloaded_bytes=offset)
    headers = {"Range": f"bytes={offset}-"} if offset else {}
    with client.stream("GET", tile.url, headers=headers) as response:
        response.raise_for_status()
        if str(response.url) != tile.url:
            raise LidarError(f"LiDAR tile redirected outside its pinned URL: {response.url}")
        content_length = response.headers.get("content-length", "")
        if offset and response.status_code == 206:
            content_range = response.headers.get("content-range", "")
            match = CONTENT_RANGE.fullmatch(content_range)
            expected_response_bytes = tile.bytes - offset
            if (
                match is None
                or int(match.group("start")) != offset
                or int(match.group("end")) != tile.bytes - 1
                or int(match.group("total")) != tile.bytes
                or content_length != str(expected_response_bytes)
            ):
                raise LidarError(
                    f"invalid resumed response for {tile.name}: Content-Range "
                    f"{content_range!r}, Content-Length {content_length!r}"
                )
        elif offset:
            offset = 0
            if response.status_code != 200 or content_length != str(tile.bytes):
                raise LidarError(
                    f"invalid replacement response for {tile.name}: status "
                    f"{response.status_code}, Content-Length {content_length!r}"
                )
        elif response.status_code != 200 or content_length != str(tile.bytes):
            raise LidarError(
                f"invalid download response for {tile.name}: status {response.status_code}, "
                f"Content-Length {content_length!r}; expected {tile.bytes}"
            )
        mode = "ab" if offset else "wb"
        with partial.open(mode) as file:
            for chunk in response.iter_bytes(DOWNLOAD_CHUNK_BYTES):
                file.write(chunk)
    if partial.stat().st_size != tile.bytes:
        raise LidarError(
            f"incomplete LAS download for {tile.name}: "
            f"{partial.stat().st_size:,} of {tile.bytes:,} bytes"
        )
    sha256 = sha256_file(partial)
    _validate_exact_las_source(partial, sha256)
    partial.replace(destination)
    _record_progress(inventory, tile, "downloaded", bytes=tile.bytes, sha256=sha256)
    return destination, sha256


def recheck_rejected_sources(
    inventory: Inventory, client: httpx.Client, *, discard_raw: bool
) -> tuple[int, int]:
    """Re-fetch terminal truncations without clearing their evidence first."""
    checked = 0
    repaired = 0
    RAW_LAS_DIR.mkdir(parents=True, exist_ok=True)
    for tile in inventory.tiles:
        if not tile.selected:
            continue
        try:
            metadata = validate_tile_artifact(tile, inventory)
        except LidarError:
            continue
        if metadata.get("result") != "rejected_source":
            continue
        checked += 1
        candidate = RAW_LAS_DIR / f"{tile.name}.recheck.part"
        candidate.unlink(missing_ok=True)
        with client.stream("GET", tile.url) as response:
            response.raise_for_status()
            if str(response.url) != tile.url:
                raise LidarError(f"LiDAR recheck redirected outside its pinned URL: {response.url}")
            if response.status_code != 200 or response.headers.get("content-length") != str(
                tile.bytes
            ):
                raise LidarError(f"invalid recheck response for {tile.name}")
            with candidate.open("wb") as file:
                for chunk in response.iter_bytes(DOWNLOAD_CHUNK_BYTES):
                    file.write(chunk)
        if candidate.stat().st_size != tile.bytes:
            candidate.unlink(missing_ok=True)
            raise LidarError(f"incomplete recheck response for {tile.name}")
        digest = sha256_file(candidate)
        try:
            _validate_exact_las_source(candidate, digest)
        except InvalidLasSourceError:
            candidate.unlink(missing_ok=True)
            continue
        destination = RAW_LAS_DIR / tile.name
        candidate.replace(destination)
        _record_progress(inventory, tile, "downloaded", bytes=tile.bytes, sha256=digest)
        (DERIVED_DIR / f"{tile.name}.json").unlink()
        process_tile(tile, inventory, client, discard_raw=discard_raw)
        repaired += 1
    return checked, repaired


def _load_tile_buildings(header: LasHeader) -> gpd.GeoDataFrame:
    if not FOOTPRINTS_PATH.exists():
        raise LidarError(f"missing {FOOTPRINTS_PATH}; run `poe lidar-plan`")
    return gpd.read_parquet(FOOTPRINTS_PATH, bbox=header.bounds_ft)


def _artifact_metadata(tile: Tile) -> dict[str, object]:
    metadata_path = DERIVED_DIR / f"{tile.name}.json"
    try:
        value = json.loads(metadata_path.read_text())
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise LidarError(f"missing or corrupt derived metadata for {tile.name}") from error
    if not isinstance(value, dict):
        raise LidarError(f"invalid derived metadata for {tile.name}")
    return value


def _parse_artifact_header(value: object, tile: Tile) -> LasHeader:
    if not isinstance(value, dict):
        raise LidarError(f"derived {tile.name} has no complete LAS header")
    try:
        version = value["version"]
        point_format = value["point_format"]
        point_record_bytes = value["point_record_bytes"]
        point_count = value["point_count"]
        point_data_offset = value["point_data_offset"]
        scales = tuple(float(item) for item in value["scales"])
        offsets = tuple(float(item) for item in value["offsets"])
        bounds = tuple(float(item) for item in value["bounds_ft"])
    except (KeyError, TypeError, ValueError) as error:
        raise LidarError(f"derived {tile.name} has an invalid LAS header") from error
    integer_fields = (point_format, point_record_bytes, point_count, point_data_offset)
    if (
        not isinstance(version, str)
        or re.fullmatch(r"\d+\.\d+", version) is None
        or any(not isinstance(item, int) or isinstance(item, bool) for item in integer_fields)
        or not 0 <= point_format <= 10
        or point_record_bytes < 20
        or point_count <= 0
        or point_data_offset < 227
        or len(scales) != 3
        or len(offsets) != 3
        or len(bounds) != 4
        or not all(math.isfinite(item) for item in (*scales, *offsets, *bounds))
        or not all(item > 0 for item in scales)
        or bounds[0] > bounds[2]
        or bounds[1] > bounds[3]
    ):
        raise LidarError(f"derived {tile.name} has an invalid LAS header")
    return LasHeader(
        version,
        point_format,
        point_record_bytes,
        point_count,
        point_data_offset,
        scales,
        offsets,
        bounds,
    )


def validate_tile_artifact(tile: Tile, inventory: Inventory) -> dict[str, object]:
    metadata = _artifact_metadata(tile)
    if not tile.selected or tile not in inventory.tiles:
        raise LidarError(f"derived tile is not selected by the active inventory: {tile.name}")
    expected = {
        "inventory_listing_sha256": inventory.listing_sha256,
        "inventory_city_sha256": inventory.city_sha256,
        "inventory_building_sha256": inventory.building_sha256,
        "source_url": tile.url,
        "source_bytes": tile.bytes,
    }
    for field, value in expected.items():
        if metadata.get(field) != value:
            raise LidarError(f"derived {tile.name} has stale or invalid {field}")
    if not _is_sha256(metadata.get("source_sha256")):
        raise LidarError(f"derived {tile.name} has no valid source SHA-256")
    header = _parse_artifact_header(metadata.get("las"), tile)
    expected_minimum = header.point_data_offset + header.point_count * header.point_record_bytes
    result = metadata.get("result")
    if result == "rejected_source":
        actual_bytes = metadata.get("actual_bytes")
        if not isinstance(actual_bytes, int) or actual_bytes != tile.bytes:
            raise LidarError(f"rejected {tile.name} does not match the pinned source size")
        if not isinstance(metadata.get("error"), str) or not metadata["error"]:
            raise LidarError(f"rejected {tile.name} has no structural error")
        if (
            metadata.get("expected_minimum_bytes") != expected_minimum
            or expected_minimum <= actual_bytes
        ):
            raise LidarError(f"rejected {tile.name} has an invalid expected minimum size")
        return metadata
    if result == "outside":
        city = _city_geometry(_default_snapshot(SOURCES.city))
        if city.intersects(box(*header.bounds_ft)):
            raise LidarError(f"outside {tile.name} actually intersects the pinned City boundary")
        return metadata
    if result != "derived":
        raise LidarError(f"derived {tile.name} has an unknown result")
    output = DERIVED_DIR / f"{tile.name}.parquet"
    if metadata.get("output_file") != output.name or not output.is_file():
        raise LidarError(f"derived Parquet is missing for {tile.name}")
    if metadata.get("output_bytes") != output.stat().st_size:
        raise LidarError(f"derived Parquet size changed for {tile.name}")
    if metadata.get("output_sha256") != sha256_file(output):
        raise LidarError(f"derived Parquet checksum changed for {tile.name}")
    try:
        table = pq.read_table(output, columns=["tile", "source_footprints_sha256"])
    except (OSError, pa.ArrowException) as error:
        raise LidarError(f"derived Parquet is unreadable for {tile.name}") from error
    if table.num_rows != metadata.get("rows"):
        raise LidarError(f"derived row count changed for {tile.name}")
    if set(table.column("tile").to_pylist()) - {tile.name}:
        raise LidarError(f"derived Parquet contains another source tile: {tile.name}")
    footprints = set(table.column("source_footprints_sha256").to_pylist())
    if footprints and footprints != {metadata.get("source_footprints_sha256")}:
        raise LidarError(f"derived footprint provenance changed for {tile.name}")
    return metadata


def process_tile(
    tile: Tile, inventory: Inventory, client: httpx.Client, *, discard_raw: bool
) -> None:
    output = DERIVED_DIR / f"{tile.name}.parquet"
    metadata_path = DERIVED_DIR / f"{tile.name}.json"
    if metadata_path.exists():
        try:
            metadata = validate_tile_artifact(tile, inventory)
        except LidarError:
            pass
        else:
            raw = RAW_LAS_DIR / tile.name
            result = metadata.get("result")
            if (discard_raw or result == "rejected_source") and raw.exists():
                raw.unlink()
            if result == "rejected_source":
                status: TileStatus = "rejected_source"
            elif result == "outside":
                status = "outside"
            else:
                status = "released" if discard_raw or not raw.exists() else "derived"
            _record_progress(inventory, tile, status, **metadata)
            return
    try:
        raw, source_sha256 = download_tile(tile, inventory, client)
    except InvalidLasSourceError as error:
        if error.header is None or error.expected_minimum_bytes is None:
            # Only a self-consistent parsed header proves a stable upstream
            # truncation. Other malformed responses remain retryable.
            raise
        metadata = {
            "result": "rejected_source",
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "source_url": tile.url,
            "source_bytes": tile.bytes,
            "source_sha256": error.source_sha256,
            "actual_bytes": error.actual_bytes,
            "expected_minimum_bytes": error.expected_minimum_bytes,
            "error": str(error),
            "las": asdict(error.header) if error.header is not None else None,
        }
        _write_json_atomic(metadata_path, metadata)
        validate_tile_artifact(tile, inventory)
        error.path.unlink()
        _record_progress(inventory, tile, "rejected_source", **metadata)
        return
    header = load_las_header(raw)
    city = _city_geometry(_default_snapshot(SOURCES.city))
    if not city.intersects(box(*header.bounds_ft)):
        metadata = {
            "result": "outside",
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "source_url": tile.url,
            "source_bytes": tile.bytes,
            "source_sha256": source_sha256,
            "las": asdict(header),
        }
        _write_json_atomic(metadata_path, metadata)
        _record_progress(inventory, tile, "outside", **metadata)
        if discard_raw:
            raw.unlink()
        return
    tile_buildings = _load_tile_buildings(header)
    source_footprints_sha256 = sha256_file(_default_snapshot(SOURCES.buildings))
    source_digests = set(tile_buildings["source_sha256"].tolist())
    if source_digests and source_digests != {source_footprints_sha256}:
        raise LidarError(f"footprint index has invalid provenance for {tile.name}")
    rows = derive_evidence(raw, tile_buildings, output)
    metadata = {
        "result": "derived",
        "inventory_listing_sha256": inventory.listing_sha256,
        "inventory_city_sha256": inventory.city_sha256,
        "inventory_building_sha256": inventory.building_sha256,
        "source_url": tile.url,
        "source_bytes": tile.bytes,
        "source_sha256": source_sha256,
        "source_footprints_sha256": source_footprints_sha256,
        "output_file": output.name,
        "output_bytes": output.stat().st_size,
        "output_sha256": sha256_file(output),
        "rows": rows,
        "las": asdict(header),
    }
    _write_json_atomic(metadata_path, metadata)
    validate_tile_artifact(tile, inventory)
    _record_progress(inventory, tile, "derived", **metadata)
    if discard_raw:
        if sha256_file(output) != metadata["output_sha256"]:
            raise LidarError("refusing to discard raw LAS: derived checksum changed")
        raw.unlink()
        _record_progress(inventory, tile, "released", **metadata)


def pending_tiles(inventory: Inventory) -> tuple[Tile, ...]:
    complete: set[str] = set()
    for tile in inventory.tiles:
        if not tile.selected:
            continue
        try:
            validate_tile_artifact(tile, inventory)
        except LidarError:
            continue
        complete.add(tile.name)
    return tuple(
        sorted(
            (tile for tile in inventory.tiles if tile.selected and tile.name not in complete),
            key=lambda tile: (tile.bytes, tile.name),
        )
    )


def _rejected_source_gap(
    tile: Tile, metadata: dict[str, object], inventory: Inventory
) -> dict[str, object]:
    las = metadata.get("las")
    if not isinstance(las, dict):
        raise LidarError(f"rejected source has no LAS bounds: {tile.name}")
    raw_bounds = las.get("bounds_ft")
    if not isinstance(raw_bounds, list) or len(raw_bounds) != 4:
        raise LidarError(f"rejected source has invalid LAS bounds: {tile.name}")
    bounds = tuple(float(value) for value in raw_bounds)
    if not FOOTPRINTS_PATH.is_file():
        raise LidarError(f"missing {FOOTPRINTS_PATH}; cannot audit rejected-source gap")
    buildings = gpd.read_parquet(FOOTPRINTS_PATH, bbox=bounds)
    if not buildings.empty:
        sources = set(buildings["source_sha256"].tolist())
        if sources != {inventory.building_sha256}:
            raise LidarError(f"footprint index has invalid provenance for gap {tile.name}")
        buildings = buildings.loc[buildings.geometry.intersects(box(*bounds))]
    return {
        "tile": tile.name,
        "bounds_ft": list(bounds),
        "affected_footprints": int(buildings["building_id"].nunique()),
    }


def merge_evidence(inventory: Inventory, *, allow_partial: bool = False) -> int:
    paths: list[Path] = []
    invalid: list[str] = []
    rejected: list[str] = []
    for tile in inventory.tiles:
        if not tile.selected:
            continue
        try:
            metadata = validate_tile_artifact(tile, inventory)
        except LidarError as error:
            invalid.append(f"{tile.name}: {error}")
            continue
        if metadata.get("result") == "derived":
            paths.append(DERIVED_DIR / f"{tile.name}.parquet")
        elif metadata.get("result") == "rejected_source":
            rejected.append(tile.name)
    if invalid and not allow_partial:
        examples = "; ".join(invalid[:3])
        raise LidarError(
            f"cannot merge an incomplete selection: {len(invalid):,} missing or invalid of "
            f"{sum(tile.selected for tile in inventory.tiles):,} selected tiles; "
            f"examples: {examples}; finish the queue or pass --allow-partial"
        )
    if not paths:
        raise LidarError("no validated derived LiDAR evidence to merge")
    partial = bool(invalid)
    destination = PARTIAL_EVIDENCE_PATH if partial else MERGED_EVIDENCE_PATH
    table = pa.concat_tables([pq.read_table(path, schema=EVIDENCE_SCHEMA) for path in paths])
    frame = table.to_pandas()
    frame["accepted"] = (
        (frame["quality"] == "high")
        & (frame["building_point_count"] >= 100)
        & (frame["ground_point_count"] >= 20)
        & frame["height_p90_m"].between(2.4, 400.0)
        & frame["roof_spread_m"].between(0.0, 3.0)
        & (frame["roof_spread_m"] <= (frame["height_p90_m"] * 0.35).clip(lower=1.0))
    )
    frame["high_quality"] = frame["quality"] == "high"
    frame = frame.sort_values(
        [
            "building_id",
            "accepted",
            "high_quality",
            "ground_point_count",
            "building_point_count",
            "tile",
        ],
        ascending=[True, False, False, False, False, True],
        kind="stable",
    ).drop_duplicates("building_id", keep="first")
    frame = frame.drop(columns=["accepted", "high_quality"])
    result = pa.Table.from_pandas(frame, schema=EVIDENCE_SCHEMA, preserve_index=False)
    footprints = set(result.column("source_footprints_sha256").to_pylist())
    if len(footprints) != 1:
        raise LidarError("merged evidence has inconsistent footprint provenance")
    temporary = destination.with_suffix(".parquet.part")
    pq.write_table(result, temporary, compression="zstd")
    temporary.replace(destination)
    selected_tiles = sum(tile.selected for tile in inventory.tiles)
    rejected_gaps = [
        _rejected_source_gap(
            tile,
            validate_tile_artifact(tile, inventory),
            inventory,
        )
        for tile in inventory.tiles
        if tile.name in rejected
    ]
    _write_json_atomic(
        destination.with_suffix(".json"),
        {
            "schema_version": 2,
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "selected_tiles": selected_tiles,
            "accounted_tiles": selected_tiles - len(invalid),
            "evidence_tiles": len(paths),
            "source_coverage_complete": not rejected,
            "rejected_source_count": len(rejected),
            "rejected_source_tiles": rejected,
            "rejected_source_gaps": rejected_gaps,
            "partial": partial,
            "source_footprints_sha256": next(iter(footprints)),
            "output_file": destination.name,
            "output_bytes": destination.stat().st_size,
            "output_sha256": sha256_file(destination),
            "rows": result.num_rows,
        },
    )
    return result.num_rows


def load_height_evidence(
    path: Path, expected_source_sha256: str, *, allow_partial: bool = False
) -> dict[str, float]:
    """Load only dense, bounded evidence derived from the current footprints."""
    try:
        metadata = json.loads(path.with_suffix(".json").read_text())
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise LidarError(f"missing or corrupt merged LiDAR metadata for {path}") from error
    if (
        not isinstance(metadata, dict)
        or metadata.get("schema_version") != 2
        or metadata.get("output_sha256") != sha256_file(path)
        or metadata.get("output_bytes") != path.stat().st_size
        or metadata.get("source_footprints_sha256") != expected_source_sha256
    ):
        raise LidarError("merged LiDAR evidence provenance is invalid")
    partial = metadata.get("partial")
    if not isinstance(partial, bool):
        raise LidarError("merged LiDAR evidence does not declare completeness")
    source_coverage_complete = metadata.get("source_coverage_complete")
    rejected_source_count = metadata.get("rejected_source_count")
    rejected_sources = metadata.get("rejected_source_tiles")
    rejected_gaps = metadata.get("rejected_source_gaps")
    selected_tiles = metadata.get("selected_tiles")
    accounted_tiles = metadata.get("accounted_tiles")
    if (
        not isinstance(source_coverage_complete, bool)
        or not isinstance(rejected_source_count, int)
        or not isinstance(rejected_sources, list)
        or not all(isinstance(name, str) for name in rejected_sources)
        or not isinstance(rejected_gaps, list)
        or not isinstance(selected_tiles, int)
        or not isinstance(accounted_tiles, int)
        or source_coverage_complete != (len(rejected_sources) == 0)
        or rejected_source_count != len(rejected_sources)
    ):
        raise LidarError("merged LiDAR evidence has invalid source-coverage metadata")
    gap_tiles: list[str] = []
    for gap in rejected_gaps:
        if not isinstance(gap, dict):
            raise LidarError("merged LiDAR evidence has invalid rejected-source gap")
        tile = gap.get("tile")
        bounds = gap.get("bounds_ft")
        affected = gap.get("affected_footprints")
        if (
            not isinstance(tile, str)
            or not isinstance(bounds, list)
            or len(bounds) != 4
            or not all(isinstance(value, int | float) for value in bounds)
            or not isinstance(affected, int)
            or affected < 0
        ):
            raise LidarError("merged LiDAR evidence has invalid rejected-source gap")
        gap_tiles.append(tile)
    if sorted(gap_tiles) != sorted(rejected_sources):
        raise LidarError("merged LiDAR evidence rejected-source gaps do not match its tile list")
    if not partial and accounted_tiles != selected_tiles:
        raise LidarError("complete local LiDAR merge does not account for every selected tile")
    if partial and not allow_partial:
        raise LidarError(
            "normal ingest requires a complete LiDAR merge; partial evidence is diagnostic only"
        )
    if INVENTORY_PATH.exists():
        inventory = load_inventory()
        if (
            metadata.get("inventory_listing_sha256") != inventory.listing_sha256
            or metadata.get("inventory_city_sha256") != inventory.city_sha256
            or metadata.get("inventory_building_sha256") != inventory.building_sha256
        ):
            raise LidarError("merged LiDAR evidence belongs to a different active inventory")
    table = pq.read_table(
        path,
        columns=[
            "building_id",
            "source_footprints_sha256",
            "height_p90_m",
            "building_point_count",
            "ground_point_count",
            "roof_spread_m",
            "quality",
        ],
    )
    rows = table.to_pylist()
    sources = {row["source_footprints_sha256"] for row in rows}
    if sources and sources != {expected_source_sha256}:
        raise LidarError(
            "LiDAR evidence was derived from a different building-footprint snapshot; "
            "rerun `poe lidar-plan`, the LiDAR queue, and `poe lidar-merge`"
        )
    result: dict[str, float] = {}
    for row in rows:
        height = float(row["height_p90_m"])
        spread = float(row["roof_spread_m"])
        if (
            row["quality"] == "high"
            and int(row["building_point_count"]) >= 100
            and int(row["ground_point_count"]) >= 20
            and 2.4 <= height <= 400.0
            and 0.0 <= spread <= 3.0
            and spread <= max(1.0, height * 0.35)
        ):
            result[str(row["building_id"])] = height
    return result


def _summary(inventory: Inventory) -> str:
    selected = tuple(tile for tile in inventory.tiles if tile.selected)
    pending = pending_tiles(inventory)
    rejected_sources = 0
    for tile in selected:
        try:
            metadata = validate_tile_artifact(tile, inventory)
        except LidarError:
            continue
        if metadata.get("result") == "rejected_source":
            rejected_sources += 1
    return (
        f"PASDA listed {len(inventory.tiles):,} tiles; selected {len(selected):,} "
        f"({sum(tile.bytes for tile in selected) / 1024**3:.2f} GiB); "
        f"pending {len(pending):,}; rejected sources {rejected_sources:,}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Plan and stream PASDA 2025 LiDAR into compact building evidence"
    )
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan", help="pin the official inventory and prepare footprints")
    plan.add_argument(
        "--refresh",
        action="store_true",
        help="re-fetch only if it still matches the checked-in audited pin",
    )
    commands.add_parser(
        "audit-candidate",
        help="write a non-active candidate and print the semantic pin for review",
    )
    run = commands.add_parser("run", help="process selected tiles sequentially and resumably")
    limit = run.add_mutually_exclusive_group(required=True)
    limit.add_argument("--max-tiles", type=int)
    limit.add_argument("--all", action="store_true")
    run.add_argument("--discard-raw", action="store_true")
    commands.add_parser("status", help="show pinned and completed tile counts")
    merge = commands.add_parser("merge", help="merge per-tile evidence by building")
    merge.add_argument(
        "--allow-partial",
        action="store_true",
        help="explicitly merge only currently validated tiles",
    )
    recheck = commands.add_parser(
        "recheck-rejected", help="safely test whether PASDA repaired rejected source bytes"
    )
    recheck.add_argument("--discard-raw", action="store_true")
    arguments = parser.parse_args()

    if arguments.command == "plan":
        city_path = _default_snapshot(SOURCES.city)
        building_path = _default_snapshot(SOURCES.buildings)
        if INVENTORY_PATH.exists() and not arguments.refresh:
            inventory = load_inventory()
        else:
            inventory = create_inventory(city_path, building_path)
        validate_active_sources(inventory)
        if not FOOTPRINTS_PATH.exists() or arguments.refresh:
            prepare_footprints(building_path, city_path)
        print(_summary(inventory))
    elif arguments.command == "audit-candidate":
        city_path = _default_snapshot(SOURCES.city)
        building_path = _default_snapshot(SOURCES.buildings)
        inventory = create_inventory(
            city_path,
            building_path,
            output_path=AUDIT_CANDIDATE_PATH,
            audit_candidate=True,
        )
        print(f"wrote non-active audit candidate: {AUDIT_CANDIDATE_PATH}")
        print(f"semantic inventory SHA-256: {semantic_inventory_sha256(inventory)}")
    elif arguments.command == "run":
        inventory = load_inventory()
        validate_active_sources(inventory)
        queue = pending_tiles(inventory)
        if not arguments.all:
            if arguments.max_tiles is None or arguments.max_tiles < 1:
                parser.error("--max-tiles must be at least 1")
            queue = queue[: arguments.max_tiles]
        with httpx.Client(
            headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=None
        ) as client:
            for position, tile in enumerate(queue, start=1):
                print(f"[{position}/{len(queue)}] {tile.name} ({tile.bytes / 1024**2:.1f} MiB)")
                process_tile(tile, inventory, client, discard_raw=arguments.discard_raw)
        print(_summary(inventory))
    elif arguments.command == "status":
        inventory = load_inventory()
        validate_active_sources(inventory)
        print(_summary(inventory))
    elif arguments.command == "merge":
        inventory = load_inventory()
        validate_active_sources(inventory)
        print(
            f"merged evidence for "
            f"{merge_evidence(inventory, allow_partial=arguments.allow_partial):,} "
            f"buildings"
        )
    elif arguments.command == "recheck-rejected":
        inventory = load_inventory()
        validate_active_sources(inventory)
        with httpx.Client(
            headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=None
        ) as client:
            checked, repaired = recheck_rejected_sources(
                inventory, client, discard_raw=arguments.discard_raw
            )
        print(f"rechecked {checked:,} rejected sources; repaired {repaired:,}")


if __name__ == "__main__":
    main()
