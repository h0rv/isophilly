from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
from collections import defaultdict
from collections.abc import Iterator, Sequence
from dataclasses import asdict, dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import BinaryIO, Literal
from urllib.parse import urljoin

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
LIDAR_DIR = ROOT / "data" / "lidar-2025"
INVENTORY_PATH = LIDAR_DIR / "inventory.json"
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


@dataclass(frozen=True, slots=True)
class Inventory:
    schema_version: int
    source_url: str
    listing_sha256: str
    fetched_at: str
    city_sha256: str
    building_sha256: str
    tiles: tuple[Tile, ...]


type TileStatus = Literal["downloading", "downloaded", "derived", "released", "outside"]


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


def parse_listing(content: bytes, source_url: str = PASDA_LAS_URL) -> tuple[Tile, ...]:
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
        tiles.append(_tile_from_name(label, urljoin(source_url, href), size))
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


def parse_inventory(value: object) -> Inventory:
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
        parsed = _tile_from_name(name, url, size, selected)
        raw_bounds = raw.get("approximate_bounds_ft")
        if not isinstance(raw_bounds, list) or len(raw_bounds) != 4:
            raise LidarError(f"LiDAR inventory bounds are invalid for {name}")
        if tuple(float(item) for item in raw_bounds) != parsed.approximate_bounds_ft:
            raise LidarError(f"LiDAR inventory bounds do not match filename for {name}")
        tiles.append(parsed)
    fields = (
        "source_url",
        "listing_sha256",
        "fetched_at",
        "city_sha256",
        "building_sha256",
    )
    if any(not isinstance(value.get(field), str) for field in fields):
        raise LidarError("LiDAR inventory provenance is invalid")
    return Inventory(
        2,
        str(value["source_url"]),
        str(value["listing_sha256"]),
        str(value["fetched_at"]),
        str(value["city_sha256"]),
        str(value["building_sha256"]),
        tuple(tiles),
    )


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
) -> Inventory:
    owns_client = client is None
    if client is None:
        client = httpx.Client(
            headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=120
        )
    try:
        response = client.get(PASDA_LAS_URL)
        response.raise_for_status()
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
            np.asarray(chunk["z"], dtype=np.float64) * header.scales[2] + header.offsets[2],
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


def _collect_matches(
    tree: shapely.STRtree,
    x: np.ndarray,
    y: np.ndarray,
    z: np.ndarray,
    destination: dict[int, list[np.ndarray]],
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
        destination[building_index].append(z[point_indices[indices]])


def derive_evidence(
    las_path: Path,
    buildings: gpd.GeoDataFrame,
    output_path: Path,
) -> int:
    header = load_las_header(las_path)
    if buildings.crs is None or buildings.crs.to_epsg() != CITY_CRS_FEET:
        raise LidarError(f"building evidence requires EPSG:{CITY_CRS_FEET} footprints")
    buildings = buildings.loc[buildings.geometry.intersects(box(*header.bounds_ft))].copy()
    if buildings.empty:
        table = pa.Table.from_pylist([], schema=EVIDENCE_SCHEMA)
    else:
        geometries = np.asarray(buildings.geometry.array, dtype=object)
        roof_tree = shapely.STRtree(geometries)
        ground_tree = shapely.STRtree(shapely.buffer(geometries, 12.0))
        roof_values: dict[int, list[np.ndarray]] = defaultdict(list)
        ground_values: dict[int, list[np.ndarray]] = defaultdict(list)
        for x, y, z, classification in iter_las_points(las_path, header):
            roof_mask = classification == 6
            _collect_matches(roof_tree, x[roof_mask], y[roof_mask], z[roof_mask], roof_values)
            ground_mask = classification == 2
            _collect_matches(
                ground_tree, x[ground_mask], y[ground_mask], z[ground_mask], ground_values
            )
        rows: list[dict[str, object]] = []
        identifiers = buildings["building_id"].tolist()
        source_digests = buildings["source_sha256"].tolist()
        for index, building_id in enumerate(identifiers):
            roof = np.concatenate(roof_values[index]) if roof_values[index] else np.array([])
            ground = np.concatenate(ground_values[index]) if ground_values[index] else np.array([])
            if len(roof) < 10 or len(ground) < 5:
                continue
            roof_quantiles = np.quantile(roof, (0.1, 0.5, 0.9)) * US_SURVEY_FOOT_METERS
            ground_m = float(np.quantile(ground, 0.25) * US_SURVEY_FOOT_METERS)
            spread = float(roof_quantiles[2] - roof_quantiles[0])
            count = int(len(roof))
            quality = "high" if count >= 100 and len(ground) >= 20 else "usable"
            rows.append(
                {
                    "building_id": str(building_id),
                    "source_footprints_sha256": str(source_digests[index]),
                    "tile": las_path.name,
                    "building_point_count": count,
                    "ground_point_count": int(len(ground)),
                    "ground_elevation_m": ground_m,
                    "roof_p10_m": float(roof_quantiles[0]),
                    "roof_p50_m": float(roof_quantiles[1]),
                    "roof_p90_m": float(roof_quantiles[2]),
                    "height_p90_m": float(roof_quantiles[2] - ground_m),
                    "roof_spread_m": spread,
                    "quality": quality,
                }
            )
        table = pa.Table.from_pylist(rows, schema=EVIDENCE_SCHEMA)
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
        if offset and response.status_code == 206:
            content_range = response.headers.get("content-range", "")
            match = CONTENT_RANGE.fullmatch(content_range)
            if (
                match is None
                or int(match.group("start")) != offset
                or int(match.group("end")) < offset
                or int(match.group("total")) != tile.bytes
            ):
                raise LidarError(
                    f"invalid Content-Range for resumed {tile.name}: {content_range!r}"
                )
        elif offset:
            offset = 0
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
    partial.replace(destination)
    _record_progress(inventory, tile, "downloaded", bytes=tile.bytes, sha256=sha256)
    return destination, sha256


def _load_tile_buildings(header: LasHeader) -> gpd.GeoDataFrame:
    if not FOOTPRINTS_PATH.exists():
        raise LidarError(f"missing {FOOTPRINTS_PATH}; run `poe lidar-plan`")
    return gpd.read_parquet(FOOTPRINTS_PATH, bbox=header.bounds_ft)


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in "0123456789abcdef" for character in value)
    )


def _artifact_metadata(tile: Tile) -> dict[str, object]:
    metadata_path = DERIVED_DIR / f"{tile.name}.json"
    try:
        value = json.loads(metadata_path.read_text())
    except (FileNotFoundError, json.JSONDecodeError) as error:
        raise LidarError(f"missing or corrupt derived metadata for {tile.name}") from error
    if not isinstance(value, dict):
        raise LidarError(f"invalid derived metadata for {tile.name}")
    return value


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
    if metadata.get("result") == "outside":
        return metadata
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
    if output.exists() and metadata_path.exists():
        try:
            metadata = validate_tile_artifact(tile, inventory)
        except LidarError:
            pass
        else:
            raw = RAW_LAS_DIR / tile.name
            if discard_raw and raw.exists():
                raw.unlink()
            status: TileStatus = "released" if discard_raw or not raw.exists() else "derived"
            _record_progress(inventory, tile, status, **metadata)
            return
    raw, source_sha256 = download_tile(tile, inventory, client)
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


def merge_evidence(inventory: Inventory, *, allow_partial: bool = False) -> int:
    paths: list[Path] = []
    invalid: list[str] = []
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
    if invalid and not allow_partial:
        examples = "; ".join(invalid[:3])
        raise LidarError(
            f"cannot merge an incomplete selection: {len(invalid):,} of "
            f"{sum(tile.selected for tile in inventory.tiles):,} tiles are missing or invalid; "
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
    _write_json_atomic(
        destination.with_suffix(".json"),
        {
            "schema_version": 1,
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "selected_tiles": sum(tile.selected for tile in inventory.tiles),
            "validated_tiles": sum(tile.selected for tile in inventory.tiles) - len(invalid),
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
        or metadata.get("output_sha256") != sha256_file(path)
        or metadata.get("output_bytes") != path.stat().st_size
        or metadata.get("source_footprints_sha256") != expected_source_sha256
    ):
        raise LidarError("merged LiDAR evidence provenance is invalid")
    partial = metadata.get("partial")
    if not isinstance(partial, bool):
        raise LidarError("merged LiDAR evidence does not declare completeness")
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
    return (
        f"PASDA listed {len(inventory.tiles):,} tiles; selected {len(selected):,} "
        f"({sum(tile.bytes for tile in selected) / 1024**3:.2f} GiB); "
        f"pending {len(pending):,}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Plan and stream PASDA 2025 LiDAR into compact building evidence"
    )
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan", help="pin the official inventory and prepare footprints")
    plan.add_argument("--refresh", action="store_true", help="replace an existing inventory")
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


if __name__ == "__main__":
    main()
