from __future__ import annotations

import argparse
import hashlib
import json
import math
import struct
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Final

import geopandas as gpd
import numpy as np
import pyarrow.parquet as pq
from pyproj import Transformer
from shapely.geometry.base import BaseGeometry

from .config import EPSG
from .lidar import FOOTPRINTS_PATH, MERGED_EVIDENCE_PATH, preflight_merge_read, sha256_file
from .models import Bounds

MAGIC: Final = b"ISOTERN1"
SCHEMA_VERSION: Final = 1
PREFIX: Final = struct.Struct("<8sII")
ARTIFACT_NAME: Final = "terrain-v1.isoterrain"
CELL_SIZE_METERS: Final = 256.0
GROUND_POINT_COUNT_MINIMUM: Final = 20
GROUND_ELEVATION_MIN_METERS: Final = -5.0
GROUND_ELEVATION_MAX_METERS: Final = 150.0
DIRECT_SAMPLE_MINIMUM: Final = 3
FILL_NEIGHBOR_COUNT: Final = 5
FILL_DISTANCE_METERS: Final = 1_500.0
COVERAGE_UNSUPPORTED: Final = 0
COVERAGE_DIRECT: Final = 1
COVERAGE_INTERPOLATED: Final = 2
COVERAGE_REJECTED_GAP: Final = 3
EXPECTED_REJECTED_TILES: Final = (
    "26822E227832N.las",
    "26848E238392N.las",
    "26954E227832N.las",
    "27086E256872N.las",
    "27086E259512N.las",
    "27086E262152N.las",
    "27086E264792N.las",
    "27086E267432N.las",
)


class TerrainError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class Gap:
    tile: str
    min_x_feet: float
    min_y_feet: float
    max_x_feet: float
    max_y_feet: float

    def canonical_value(self) -> dict[str, object]:
        return {
            "bounds_ft": [self.min_x_feet, self.min_y_feet, self.max_x_feet, self.max_y_feet],
            "tile": self.tile,
        }


@dataclass(frozen=True, slots=True)
class EvidenceManifest:
    parquet_sha256: str
    footprint_sha256: str
    source_coverage_complete: bool
    gaps: tuple[Gap, ...]
    manifest_sha256: str


@dataclass(frozen=True, slots=True)
class TerrainGrid:
    min_x: float
    min_y: float
    width: int
    height: int

    @property
    def cell_count(self) -> int:
        return self.width * self.height

    def cell_index(self, x: float, y: float) -> int:
        column = min(max(int((x - self.min_x) // CELL_SIZE_METERS), 0), self.width - 1)
        row = min(max(int((y - self.min_y) // CELL_SIZE_METERS), 0), self.height - 1)
        return row * self.width + column

    def center(self, index: int) -> tuple[float, float]:
        row, column = divmod(index, self.width)
        return (
            self.min_x + (column + 0.5) * CELL_SIZE_METERS,
            self.min_y + (row + 0.5) * CELL_SIZE_METERS,
        )

    def intersects(self, gap: tuple[float, float, float, float], index: int) -> bool:
        row, column = divmod(index, self.width)
        min_x = self.min_x + column * CELL_SIZE_METERS
        min_y = self.min_y + row * CELL_SIZE_METERS
        max_x = min_x + CELL_SIZE_METERS
        max_y = min_y + CELL_SIZE_METERS
        gap_min_x, gap_min_y, gap_max_x, gap_max_y = gap
        return min_x < gap_max_x and max_x > gap_min_x and min_y < gap_max_y and max_y > gap_min_y


@dataclass(frozen=True, slots=True)
class TerrainBuild:
    path: Path
    sha256: str
    grid: TerrainGrid
    direct_cells: int
    interpolated_cells: int
    rejected_gap_cells: int
    unsupported_cells: int


@dataclass(frozen=True, slots=True)
class TerrainArtifact:
    grid: TerrainGrid
    elevations_centimeters: tuple[int, ...]
    coverage: bytes
    sha256: str


def _is_sha256(value: object) -> bool:
    return (
        isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)
    )


def _raw_object(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TerrainError(f"terrain {label} must be an object")
    return value


def _raw_list(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise TerrainError(f"terrain {label} must be a list")
    return value


def _raw_string(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise TerrainError(f"terrain {label} must be a string")
    return value


def _raw_bool(value: object, label: str) -> bool:
    if not isinstance(value, bool):
        raise TerrainError(f"terrain {label} must be a boolean")
    return value


def _raw_number(value: object, label: str) -> float:
    if (
        not isinstance(value, int | float)
        or isinstance(value, bool)
        or not math.isfinite(float(value))
    ):
        raise TerrainError(f"terrain {label} must be a finite number")
    return float(value)


def _canonical_sha256(value: object) -> str:
    return hashlib.sha256(
        json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def parse_evidence_manifest(path: Path, evidence_path: Path) -> EvidenceManifest:
    try:
        raw = _raw_object(json.loads(path.read_text()), "evidence manifest")
    except (OSError, json.JSONDecodeError) as error:
        raise TerrainError(f"cannot read terrain evidence manifest: {path}") from error
    output_file = _raw_string(raw.get("output_file"), "evidence output_file")
    output_sha256 = _raw_string(raw.get("output_sha256"), "evidence output_sha256")
    source_sha256 = _raw_string(raw.get("source_footprints_sha256"), "evidence source footprints")
    complete = _raw_bool(raw.get("source_coverage_complete"), "evidence source coverage")
    rejected_count = raw.get("rejected_source_count")
    if (
        not isinstance(rejected_count, int)
        or isinstance(rejected_count, bool)
        or rejected_count < 0
    ):
        raise TerrainError("terrain evidence rejected source count is invalid")
    if (
        output_file != evidence_path.name
        or not _is_sha256(output_sha256)
        or not _is_sha256(source_sha256)
    ):
        raise TerrainError("terrain evidence manifest does not bind its Parquet input")
    gaps: list[Gap] = []
    for raw_gap in _raw_list(raw.get("rejected_source_gaps"), "evidence rejected source gaps"):
        gap = _raw_object(raw_gap, "rejected source gap")
        bounds = _raw_list(gap.get("bounds_ft"), "rejected source gap bounds")
        if len(bounds) != 4:
            raise TerrainError("terrain rejected source gap has invalid bounds")
        values = tuple(_raw_number(value, "rejected source gap bound") for value in bounds)
        if values[0] >= values[2] or values[1] >= values[3]:
            raise TerrainError("terrain rejected source gap bounds are unordered")
        gaps.append(Gap(_raw_string(gap.get("tile"), "rejected source gap tile"), *values))
    gap_tiles = tuple(gap.tile for gap in gaps)
    if len(gaps) != rejected_count or list(gap_tiles) != sorted(gap_tiles):
        raise TerrainError("terrain evidence rejected source gaps are not canonical")
    if complete or gap_tiles != EXPECTED_REJECTED_TILES:
        raise TerrainError("terrain evidence source-gap authority changed; review terrain pins")
    return EvidenceManifest(
        output_sha256,
        source_sha256,
        complete,
        tuple(gaps),
        sha256_file(path),
    )


def _grid(bounds: Bounds) -> TerrainGrid:
    min_x = math.floor(bounds.min_x / CELL_SIZE_METERS) * CELL_SIZE_METERS
    min_y = math.floor(bounds.min_y / CELL_SIZE_METERS) * CELL_SIZE_METERS
    width = math.ceil((bounds.max_x - min_x) / CELL_SIZE_METERS)
    height = math.ceil((bounds.max_y - min_y) / CELL_SIZE_METERS)
    if not 0 < width <= 512 or not 0 < height <= 512:
        raise TerrainError("terrain grid dimensions are unsupported")
    return TerrainGrid(min_x, min_y, width, height)


def _accepted_observations(path: Path, source_sha256: str) -> dict[str, float]:
    try:
        table = pq.read_table(
            path,
            columns=[
                "building_id",
                "source_footprints_sha256",
                "ground_point_count",
                "ground_elevation_m",
            ],
        )
    except Exception as error:
        raise TerrainError(f"cannot read terrain evidence: {path}") from error
    result: dict[str, float] = {}
    for raw in table.to_pylist():
        row = _raw_object(raw, "evidence row")
        identifier = _raw_string(row.get("building_id"), "evidence building ID")
        row_source = _raw_string(row.get("source_footprints_sha256"), "evidence footprint SHA-256")
        count = row.get("ground_point_count")
        elevation = _raw_number(row.get("ground_elevation_m"), "evidence ground elevation")
        if row_source != source_sha256:
            raise TerrainError("terrain evidence was derived from another footprint snapshot")
        if not isinstance(count, int) or isinstance(count, bool):
            raise TerrainError("terrain evidence ground point count is invalid")
        if (
            count >= GROUND_POINT_COUNT_MINIMUM
            and GROUND_ELEVATION_MIN_METERS <= elevation <= GROUND_ELEVATION_MAX_METERS
        ):
            if identifier in result:
                raise TerrainError("terrain evidence has duplicate building IDs")
            result[identifier] = elevation
    if not result:
        raise TerrainError("terrain evidence has no accepted ground elevations")
    return result


def _sample_positions(
    footprints: Path, elevations: dict[str, float], source_sha256: str
) -> tuple[tuple[float, float, float], ...]:
    try:
        frame = gpd.read_parquet(footprints, columns=["building_id", "source_sha256", "geometry"])
    except (OSError, ValueError) as error:
        raise TerrainError(f"cannot read terrain footprint index: {footprints}") from error
    if frame.crs is None or frame.crs.to_epsg() != 6565:
        raise TerrainError("terrain footprint index must use EPSG:6565")
    if not {"building_id", "source_sha256", "geometry"}.issubset(frame.columns):
        raise TerrainError("terrain footprint index schema is invalid")
    selected = frame.loc[frame["building_id"].isin(elevations)]
    positions: dict[str, tuple[float, float]] = {}
    for identifier, row_source, geometry in zip(
        selected["building_id"], selected["source_sha256"], selected.geometry, strict=True
    ):
        if not isinstance(identifier, str) or not isinstance(row_source, str):
            raise TerrainError("terrain footprint index identifiers are invalid")
        if row_source != source_sha256:
            raise TerrainError("terrain footprint index provenance is invalid")
        if not isinstance(geometry, BaseGeometry) or geometry.is_empty:
            raise TerrainError("terrain footprint index geometry is invalid")
        point = geometry.representative_point()
        coordinates = (float(point.x), float(point.y))
        previous = positions.get(identifier)
        if previous is not None and previous != coordinates:
            raise TerrainError("terrain duplicate footprint ID has conflicting geometries")
        positions[identifier] = coordinates
    if set(positions) != set(elevations):
        raise TerrainError("terrain evidence and footprint index do not match")
    transformer = Transformer.from_crs(6565, EPSG, always_xy=True)
    identifiers = sorted(positions)
    xs, ys = transformer.transform(
        [positions[identifier][0] for identifier in identifiers],
        [positions[identifier][1] for identifier in identifiers],
    )
    return tuple(
        (float(x), float(y), elevations[identifier])
        for identifier, x, y in zip(identifiers, xs, ys, strict=True)
    )


def _projected_gaps(gaps: tuple[Gap, ...]) -> tuple[tuple[float, float, float, float], ...]:
    transformer = Transformer.from_crs(6565, EPSG, always_xy=True)
    result: list[tuple[float, float, float, float]] = []
    for gap in gaps:
        xs, ys = transformer.transform(
            [gap.min_x_feet, gap.max_x_feet, gap.max_x_feet, gap.min_x_feet],
            [gap.min_y_feet, gap.min_y_feet, gap.max_y_feet, gap.max_y_feet],
        )
        result.append((min(xs), min(ys), max(xs), max(ys)))
    return tuple(result)


def _filled_grid(
    grid: TerrainGrid,
    samples: tuple[tuple[float, float, float], ...],
    gaps: tuple[tuple[float, float, float, float], ...],
) -> tuple[np.ndarray, bytes]:
    values: defaultdict[int, list[float]] = defaultdict(list)
    for x, y, elevation in samples:
        values[grid.cell_index(x, y)].append(elevation)
    elevations = np.zeros(grid.cell_count, dtype=np.float64)
    coverage = bytearray([COVERAGE_UNSUPPORTED] * grid.cell_count)
    rejected = {
        index
        for index in range(grid.cell_count)
        if any(grid.intersects(gap, index) for gap in gaps)
    }
    for index, samples_in_cell in values.items():
        if index not in rejected and len(samples_in_cell) >= DIRECT_SAMPLE_MINIMUM:
            elevations[index] = float(np.median(np.asarray(samples_in_cell, dtype=np.float64)))
            coverage[index] = COVERAGE_DIRECT
    direct = np.asarray(
        [index for index, value in enumerate(coverage) if value == COVERAGE_DIRECT], dtype=np.int64
    )
    if len(direct) == 0:
        raise TerrainError("terrain grid has no directly supported cells")
    direct_centers = np.asarray([grid.center(int(index)) for index in direct], dtype=np.float64)
    direct_values = elevations[direct]
    for index, flag in enumerate(coverage):
        if flag == COVERAGE_DIRECT:
            continue
        center = np.asarray(grid.center(index), dtype=np.float64)
        squared = np.sum((direct_centers - center) ** 2, axis=1)
        nearby = np.flatnonzero(squared <= FILL_DISTANCE_METERS**2)
        if len(nearby) == 0:
            continue
        ordered = nearby[np.argsort(squared[nearby], kind="stable")[:FILL_NEIGHBOR_COUNT]]
        distances = squared[ordered]
        if np.any(distances == 0.0):
            elevations[index] = float(direct_values[ordered[np.argmin(distances)]])
        else:
            weights = 1.0 / distances
            elevations[index] = float(np.sum(direct_values[ordered] * weights) / np.sum(weights))
        coverage[index] = COVERAGE_REJECTED_GAP if index in rejected else COVERAGE_INTERPOLATED
    smoothed = elevations.copy()
    for index, flag in enumerate(coverage):
        if flag == COVERAGE_UNSUPPORTED:
            continue
        row, column = divmod(index, grid.width)
        neighborhood = [
            elevations[neighbor_row * grid.width + neighbor_column]
            for neighbor_row in range(max(0, row - 1), min(grid.height, row + 2))
            for neighbor_column in range(max(0, column - 1), min(grid.width, column + 2))
            if coverage[neighbor_row * grid.width + neighbor_column] != COVERAGE_UNSUPPORTED
        ]
        smoothed[index] = float(np.median(np.asarray(neighborhood, dtype=np.float64)))
    return smoothed, bytes(coverage)


def _header(
    grid: TerrainGrid, manifest: EvidenceManifest, payload: bytes, coverage: bytes
) -> dict[str, object]:
    gaps = [gap.canonical_value() for gap in manifest.gaps]
    return {
        "acceptance": {
            "ground_elevation_max_m": GROUND_ELEVATION_MAX_METERS,
            "ground_elevation_min_m": GROUND_ELEVATION_MIN_METERS,
            "ground_point_count_min": GROUND_POINT_COUNT_MINIMUM,
        },
        "coverage": {
            "0": "unsupported",
            "1": "direct",
            "2": "interpolated",
            "3": "rejected_gap_interpolated",
        },
        "evidence": {
            "manifest_sha256": manifest.manifest_sha256,
            "parquet_sha256": manifest.parquet_sha256,
            "rejected_source_count": len(manifest.gaps),
            "rejected_source_gaps_sha256": _canonical_sha256(gaps),
            "rejected_source_tiles": [gap.tile for gap in manifest.gaps],
            "source_coverage_complete": manifest.source_coverage_complete,
            "source_footprints_sha256": manifest.footprint_sha256,
        },
        "grid": {
            "cell_size_m": CELL_SIZE_METERS,
            "epsg": EPSG,
            "height": grid.height,
            "min_x": grid.min_x,
            "min_y": grid.min_y,
            "row_order": "south_to_north",
            "sample_location": "cell_center",
            "width": grid.width,
        },
        "interpolation": {
            "direct_min_samples": DIRECT_SAMPLE_MINIMUM,
            "fill": "inverse_distance_squared_5_nearest_direct_cells_within_1500m",
            "smoothing": "single_3x3_median",
        },
        "payload_sha256": hashlib.sha256(payload).hexdigest(),
        "schema_version": SCHEMA_VERSION,
        "vertical_reference": "relative_visual_relief_only",
    }


def build_terrain(
    destination: Path,
    bounds: Bounds,
    *,
    evidence_path: Path = MERGED_EVIDENCE_PATH,
    footprints_path: Path = FOOTPRINTS_PATH,
) -> TerrainBuild:
    preflight_merge_read(evidence_path)
    manifest_path = evidence_path.with_suffix(".json")
    manifest = parse_evidence_manifest(manifest_path, evidence_path)
    if manifest.parquet_sha256 != sha256_file(evidence_path):
        raise TerrainError("terrain evidence Parquet checksum differs from its manifest")
    grid = _grid(bounds)
    accepted = _accepted_observations(evidence_path, manifest.footprint_sha256)
    samples = _sample_positions(footprints_path, accepted, manifest.footprint_sha256)
    elevations, coverage = _filled_grid(grid, samples, _projected_gaps(manifest.gaps))
    centimeters = np.rint(elevations * 100.0).astype("<i2")
    payload = centimeters.tobytes() + coverage
    header = _header(grid, manifest, payload, coverage)
    header_bytes = json.dumps(header, sort_keys=True, separators=(",", ":")).encode("utf-8")
    if len(header_bytes) > 64 * 1024:
        raise TerrainError("terrain header is too large")
    temporary = destination.with_suffix(f"{destination.suffix}.part")
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary.write_bytes(
        PREFIX.pack(MAGIC, SCHEMA_VERSION, len(header_bytes)) + header_bytes + payload
    )
    artifact = load_terrain(temporary)
    temporary.replace(destination)
    return TerrainBuild(
        destination,
        artifact.sha256,
        grid,
        coverage.count(COVERAGE_DIRECT),
        coverage.count(COVERAGE_INTERPOLATED),
        coverage.count(COVERAGE_REJECTED_GAP),
        coverage.count(COVERAGE_UNSUPPORTED),
    )


def load_terrain(path: Path) -> TerrainArtifact:
    try:
        data = path.read_bytes()
    except OSError as error:
        raise TerrainError(f"cannot read terrain artifact: {path}") from error
    if len(data) < PREFIX.size:
        raise TerrainError("terrain artifact is truncated")
    magic, schema_version, header_length = PREFIX.unpack(data[: PREFIX.size])
    if magic != MAGIC or schema_version != SCHEMA_VERSION or header_length > 64 * 1024:
        raise TerrainError("terrain artifact prefix is invalid")
    header_end = PREFIX.size + header_length
    try:
        raw = _raw_object(json.loads(data[PREFIX.size : header_end]), "artifact header")
    except json.JSONDecodeError as error:
        raise TerrainError("terrain artifact header is invalid JSON") from error
    raw_grid = _raw_object(raw.get("grid"), "artifact grid")
    width = raw_grid.get("width")
    height = raw_grid.get("height")
    min_x = _raw_number(raw_grid.get("min_x"), "artifact grid min_x")
    min_y = _raw_number(raw_grid.get("min_y"), "artifact grid min_y")
    if (
        raw.get("schema_version") != SCHEMA_VERSION
        or raw_grid.get("epsg") != EPSG
        or raw_grid.get("cell_size_m") != CELL_SIZE_METERS
        or raw_grid.get("row_order") != "south_to_north"
        or raw_grid.get("sample_location") != "cell_center"
        or not isinstance(width, int)
        or isinstance(width, bool)
        or not isinstance(height, int)
        or isinstance(height, bool)
        or not 0 < width <= 512
        or not 0 < height <= 512
    ):
        raise TerrainError("terrain artifact grid is invalid")
    grid = TerrainGrid(min_x, min_y, width, height)
    payload = data[header_end:]
    height_bytes = grid.cell_count * 2
    if len(payload) != height_bytes + grid.cell_count:
        raise TerrainError("terrain artifact payload size is invalid")
    payload_sha256 = _raw_string(raw.get("payload_sha256"), "artifact payload SHA-256")
    if not _is_sha256(payload_sha256) or hashlib.sha256(payload).hexdigest() != payload_sha256:
        raise TerrainError("terrain artifact payload checksum is invalid")
    elevations = tuple(np.frombuffer(payload[:height_bytes], dtype="<i2").tolist())
    coverage = payload[height_bytes:]
    if any(value not in {0, 1, 2, 3} for value in coverage):
        raise TerrainError("terrain artifact coverage is invalid")
    return TerrainArtifact(grid, elevations, coverage, hashlib.sha256(data).hexdigest())


def build_if_evidence_present(destination: Path, bounds: Bounds) -> TerrainBuild | None:
    if not MERGED_EVIDENCE_PATH.exists():
        return None
    return build_terrain(destination, bounds)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Audit the offline IsoPhilly terrain-relief artifact"
    )
    parser.add_argument("command", choices=("audit",))
    parser.add_argument("--path", type=Path, default=Path("data/clean") / ARTIFACT_NAME)
    arguments = parser.parse_args()
    artifact = load_terrain(arguments.path)
    print(
        json.dumps(
            {
                "sha256": artifact.sha256,
                "width": artifact.grid.width,
                "height": artifact.grid.height,
                "direct_cells": artifact.coverage.count(COVERAGE_DIRECT),
                "interpolated_cells": artifact.coverage.count(COVERAGE_INTERPOLATED),
                "rejected_gap_cells": artifact.coverage.count(COVERAGE_REJECTED_GAP),
                "unsupported_cells": artifact.coverage.count(COVERAGE_UNSUPPORTED),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    main()
