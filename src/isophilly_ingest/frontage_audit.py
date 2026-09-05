"""Read-only, deterministic City frontage-candidate audit.

This module intentionally does not decide party-wall masks: that information
exists only while the Rust renderer has its neighboring-building context.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
from collections import Counter, defaultdict
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Final

import pyogrio
from pyproj import Transformer
from shapely import from_wkb
from shapely import transform as shapely_transform
from shapely.geometry import GeometryCollection, LineString, MultiLineString, Polygon, box
from shapely.strtree import STRtree

from .config import EPSG, RAW_DIR, SOURCES, Source

MIN_STREET_DISTANCE_M: Final = 3.0
MAX_STREET_DISTANCE_M: Final = 30.0
MIN_EDGE_LENGTH_M: Final = 3.048
MAX_EDGE_LENGTH_M: Final = 9.144
MAX_PARALLEL_ANGLE_DEGREES: Final = 20.0
UNAMBIGUOUS_ADVANTAGE_M: Final = 2.0
UNKNOWN_FRONTAGE_EDGE: Final = 255

# Finite, reviewed synonyms.  This is normalization, never fuzzy matching.
DIRECTION_SYNONYMS: Final = {
    "N": "N",
    "NORTH": "N",
    "S": "S",
    "SOUTH": "S",
    "E": "E",
    "EAST": "E",
    "W": "W",
    "WEST": "W",
    "NE": "NE",
    "NORTHEAST": "NE",
    "NW": "NW",
    "NORTHWEST": "NW",
    "SE": "SE",
    "SOUTHEAST": "SE",
    "SW": "SW",
    "SOUTHWEST": "SW",
}
TYPE_SYNONYMS: Final = {
    "ALLEY": "ALY",
    "ALY": "ALY",
    "AVENUE": "AVE",
    "AVE": "AVE",
    "BOULEVARD": "BLVD",
    "BLVD": "BLVD",
    "CIRCLE": "CIR",
    "CIR": "CIR",
    "COURT": "CT",
    "CT": "CT",
    "DRIVE": "DR",
    "DR": "DR",
    "HIGHWAY": "HWY",
    "HWY": "HWY",
    "LANE": "LN",
    "LN": "LN",
    "PARKWAY": "PKWY",
    "PKWY": "PKWY",
    "PLACE": "PL",
    "PL": "PL",
    "ROAD": "RD",
    "RD": "RD",
    "SQUARE": "SQ",
    "SQ": "SQ",
    "STREET": "ST",
    "ST": "ST",
    "TERRACE": "TER",
    "TER": "TER",
    "WAY": "WAY",
}
RULES: Final = {
    "epsg": EPSG,
    "street_classes": [1, 2, 3, 4, 5],
    "street_distance_m": [MIN_STREET_DISTANCE_M, MAX_STREET_DISTANCE_M],
    "candidate_edge_length_m": [MIN_EDGE_LENGTH_M, MAX_EDGE_LENGTH_M],
    "parallel_angle_degrees": MAX_PARALLEL_ANGLE_DEGREES,
    "unambiguous_advantage_m": UNAMBIGUOUS_ADVANTAGE_M,
    "direction_synonyms": DIRECTION_SYNONYMS,
    "type_synonyms": TYPE_SYNONYMS,
    "address_policy": "positive single number or same-parity range; trailing # unit removed",
}
ADDRESS_PATTERN: Final = re.compile(
    r"^\s*(?P<first>[1-9][0-9]{0,5})(?:\s*[-–]\s*(?P<last>[1-9][0-9]{0,5}))?\s+(?P<label>.+?)\s*$"
)


@dataclass(frozen=True, slots=True)
class HouseRange:
    first: int
    last: int

    def __post_init__(self) -> None:
        if self.first > self.last or self.first % 2 != self.last % 2:
            raise ValueError("house ranges must be ordered and have one parity")

    def fits(self, address: HouseRange) -> bool:
        return (
            self.first % 2 == address.first % 2
            and self.first <= address.first <= address.last <= self.last
        )


@dataclass(frozen=True, slots=True)
class ParsedAddress:
    house_range: HouseRange
    street_label: str


@dataclass(frozen=True, slots=True)
class StreetSegment:
    object_id: int
    street_label: str
    street_class: int
    ranges: tuple[HouseRange, ...]
    geometry: LineString

    def matches(self, address: ParsedAddress) -> bool:
        return self.street_label == address.street_label and any(
            house_range.fits(address.house_range) for house_range in self.ranges
        )


@dataclass(frozen=True, slots=True)
class EdgeCandidate:
    edge: LineString
    street: StreetSegment
    distance_m: float
    angle_degrees: float

    @property
    def edge_key(self) -> tuple[float, float, float, float]:
        first, second = self.edge.coords
        start = (round(float(first[0]), 6), round(float(first[1]), 6))
        end = (round(float(second[0]), 6), round(float(second[1]), 6))
        return (*min(start, end), *max(start, end))


@dataclass(frozen=True, slots=True)
class EdgeSelection:
    candidate: EdgeCandidate | None
    rejected_reason: str | None


@dataclass(frozen=True, slots=True)
class InputSnapshot:
    path: Path
    sha256: str
    size: int


@dataclass(frozen=True, slots=True)
class StreetIndex:
    segments: tuple[StreetSegment, ...]
    tree: STRtree
    ids_by_name: dict[str, frozenset[int]]

    def nearby_matching(
        self, polygon: Polygon, address: ParsedAddress
    ) -> tuple[StreetSegment, ...]:
        permitted = self.ids_by_name.get(address.street_label, frozenset())
        min_x, min_y, max_x, max_y = polygon.bounds
        search_bounds = box(
            min_x - MAX_STREET_DISTANCE_M,
            min_y - MAX_STREET_DISTANCE_M,
            max_x + MAX_STREET_DISTANCE_M,
            max_y + MAX_STREET_DISTANCE_M,
        )
        return tuple(
            self.segments[int(index)]
            for index in self.tree.query(search_bounds)
            if int(index) in permitted and self.segments[int(index)].matches(address)
        )


def normalize_street_label(value: str) -> str:
    tokens = [token.rstrip(".,").upper() for token in value.split()]
    tokens = [token for token in tokens if token]
    if not tokens:
        return ""
    if tokens[0] in DIRECTION_SYNONYMS:
        tokens[0] = DIRECTION_SYNONYMS[tokens[0]]
    if tokens[-1] in DIRECTION_SYNONYMS:
        tokens[-1] = DIRECTION_SYNONYMS[tokens[-1]]
    type_index = -2 if tokens[-1] in DIRECTION_SYNONYMS.values() and len(tokens) > 1 else -1
    if tokens[type_index] in TYPE_SYNONYMS:
        tokens[type_index] = TYPE_SYNONYMS[tokens[type_index]]
    return " ".join(tokens)


def parse_address(value: object) -> ParsedAddress | None:
    if not isinstance(value, str):
        return None
    match = ADDRESS_PATTERN.fullmatch(value.split("#", maxsplit=1)[0])
    if match is None:
        return None
    first = int(match.group("first"))
    last_text = match.group("last")
    try:
        house_range = HouseRange(first, first if last_text is None else int(last_text))
    except ValueError:
        return None
    street_label = normalize_street_label(match.group("label"))
    tokens = street_label.split()
    type_index = -2 if tokens and tokens[-1] in DIRECTION_SYNONYMS.values() else -1
    if not street_label or not tokens or tokens[type_index] not in TYPE_SYNONYMS.values():
        return None
    return ParsedAddress(house_range, street_label)


def parse_street_range(first: object, last: object) -> HouseRange | None:
    if (
        isinstance(first, bool)
        or isinstance(last, bool)
        or not isinstance(first, int)
        or not isinstance(last, int)
    ):
        return None
    if first < 1 or last < 1:
        return None
    try:
        return HouseRange(min(first, last), max(first, last))
    except ValueError:
        return None


def _line_segments(geometry: object) -> Iterator[LineString]:
    lines: Iterable[LineString]
    if isinstance(geometry, LineString):
        lines = (geometry,)
    elif isinstance(geometry, (MultiLineString, GeometryCollection)):
        lines = (part for part in geometry.geoms if isinstance(part, LineString))
    else:
        return
    for line in lines:
        for start, end in zip(line.coords, line.coords[1:], strict=False):
            segment = LineString((start, end))
            if segment.length > 0:
                yield segment


def _arrow_batches(
    path: Path, columns: tuple[str, ...]
) -> Iterator[tuple[dict[str, list[object]], str]]:
    with pyogrio.open_arrow(path, columns=list(columns), use_pyarrow=True, batch_size=65_536) as (
        metadata,
        reader,
    ):
        geometry_name = str(metadata["geometry_name"] or "wkb_geometry")
        for batch in reader:
            yield (
                {name: batch.column(name).to_pylist() for name in (*columns, geometry_name)},
                geometry_name,
            )


def _source_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def resolve_snapshot(source: Source, explicit_path: Path | None) -> InputSnapshot:
    """Find a valid local source, selecting multiple caches lexically, never by mtime."""
    candidates = (
        [explicit_path]
        if explicit_path is not None
        else sorted(RAW_DIR.glob(f"{source.filename}-*.{source.extension}"))
    )
    valid: list[InputSnapshot] = []
    for candidate in candidates:
        if (
            candidate is None
            or not candidate.is_file()
            or not source.accepts_size(candidate.stat().st_size)
        ):
            continue
        digest = _source_digest(candidate)
        if source.accepts_digest(digest):
            valid.append(InputSnapshot(candidate, digest, candidate.stat().st_size))
    if not valid:
        requested = explicit_path if explicit_path is not None else source.filename
        raise ValueError(f"no valid local {source.name} snapshot for {requested}")
    return min(valid, key=lambda snapshot: (snapshot.path.name, snapshot.sha256))


def build_street_index(path: Path) -> StreetIndex:
    columns = ("objectid", "streetlabe", "class", "l_f_add", "l_t_add", "r_f_add", "r_t_add")
    transformer = Transformer.from_crs(4326, EPSG, always_xy=True)
    segments: list[StreetSegment] = []
    ids_by_name: defaultdict[str, set[int]] = defaultdict(set)
    for values, geometry_name in _arrow_batches(path, columns):
        for object_id, label, street_class, l_first, l_last, r_first, r_last, wkb in zip(
            values["objectid"],
            values["streetlabe"],
            values["class"],
            values["l_f_add"],
            values["l_t_add"],
            values["r_f_add"],
            values["r_t_add"],
            values[geometry_name],
            strict=True,
        ):
            if (
                isinstance(object_id, bool)
                or not isinstance(object_id, int)
                or not isinstance(label, str)
                or isinstance(street_class, bool)
                or not isinstance(street_class, int)
                or street_class not in {1, 2, 3, 4, 5}
                or not isinstance(wkb, bytes)
            ):
                continue
            ranges = tuple(
                item
                for item in (
                    parse_street_range(l_first, l_last),
                    parse_street_range(r_first, r_last),
                )
                if item is not None
            )
            street_label = normalize_street_label(label)
            if not ranges or not street_label:
                continue
            projected = shapely_transform(from_wkb(wkb), transformer.transform, interleaved=False)
            for geometry in _line_segments(projected):
                segment_index = len(segments)
                segments.append(
                    StreetSegment(object_id, street_label, street_class, ranges, geometry)
                )
                ids_by_name[street_label].add(segment_index)
    if not segments:
        raise ValueError("street centerline source yielded no indexed class 1-5 segments")
    frozen_segments = tuple(segments)
    return StreetIndex(
        frozen_segments,
        STRtree([segment.geometry for segment in frozen_segments]),
        {name: frozenset(ids) for name, ids in ids_by_name.items()},
    )


def _edge_angle_degrees(edge: LineString, street: LineString) -> float:
    edge_start, edge_end = edge.coords
    street_start, street_end = street.coords
    edge_angle = math.degrees(math.atan2(edge_end[1] - edge_start[1], edge_end[0] - edge_start[0]))
    street_angle = math.degrees(
        math.atan2(street_end[1] - street_start[1], street_end[0] - street_start[0])
    )
    difference = abs((edge_angle - street_angle) % 180.0)
    return min(difference, 180.0 - difference)


def ring_edges(polygon: Polygon) -> tuple[LineString, ...]:
    points = tuple(polygon.exterior.coords)
    return tuple(
        edge
        for start, end in zip(points, points[1:], strict=False)
        if (edge := LineString((start, end))).length > 0
    )


def select_frontage_edge(candidates: Sequence[EdgeCandidate]) -> EdgeSelection:
    """Choose one edge or reject a corner/near-tie, independent of ring order."""
    best_by_edge: dict[tuple[float, float, float, float], EdgeCandidate] = {}
    for candidate in candidates:
        previous = best_by_edge.get(candidate.edge_key)
        if previous is None or (
            candidate.distance_m,
            candidate.street.object_id,
            candidate.angle_degrees,
        ) < (previous.distance_m, previous.street.object_id, previous.angle_degrees):
            best_by_edge[candidate.edge_key] = candidate
    ordered = sorted(
        best_by_edge.values(),
        key=lambda item: (
            item.distance_m,
            item.edge_key,
            item.street.object_id,
            item.angle_degrees,
        ),
    )
    if not ordered:
        return EdgeSelection(None, "no_eligible_edge")
    if len(ordered) > 1 and ordered[1].distance_m - ordered[0].distance_m < UNAMBIGUOUS_ADVANTAGE_M:
        return EdgeSelection(None, "ambiguous_edge_tie")
    return EdgeSelection(ordered[0], None)


def _candidate_for_polygon(
    polygon: Polygon, nearby: Sequence[StreetSegment]
) -> tuple[EdgeSelection, str | None]:
    if not nearby:
        return EdgeSelection(None, "no_nearby_range_matched_street"), "distance"
    if (
        not MIN_STREET_DISTANCE_M
        <= min(item.geometry.distance(polygon.exterior) for item in nearby)
        <= MAX_STREET_DISTANCE_M
    ):
        return EdgeSelection(None, "street_ring_distance_outside_3_30m"), "distance"
    candidates: list[EdgeCandidate] = []
    saw_edge_length = False
    saw_candidate_distance = False
    saw_parallel = False
    for edge in ring_edges(polygon):
        if not MIN_EDGE_LENGTH_M <= edge.length <= MAX_EDGE_LENGTH_M:
            continue
        saw_edge_length = True
        for street in nearby:
            distance_m = edge.distance(street.geometry)
            if not MIN_STREET_DISTANCE_M <= distance_m <= MAX_STREET_DISTANCE_M:
                continue
            saw_candidate_distance = True
            angle = _edge_angle_degrees(edge, street.geometry)
            if angle > MAX_PARALLEL_ANGLE_DEGREES:
                continue
            saw_parallel = True
            candidates.append(EdgeCandidate(edge, street, distance_m, angle))
    selection = select_frontage_edge(candidates)
    if selection.candidate is not None:
        return selection, None
    if not saw_edge_length:
        return EdgeSelection(None, "no_candidate_edge_length"), "geometry"
    if not saw_candidate_distance:
        return selection, "distance"
    if not saw_parallel:
        return selection, "geometry"
    return selection, "ambiguous"


def frontage_edge_for_polygon(
    polygon: Polygon, address: ParsedAddress | None, street_index: StreetIndex
) -> int | None:
    """Return the final polygon-ring edge selected by the audited rules.

    Callers must supply the exact polygon that will be packed.  This deliberately
    avoids mapping a raw-source edge through clipping, repair, simplification,
    or polygon explosion.
    """
    if address is None:
        return None
    nearby = street_index.nearby_matching(polygon, address)
    if not nearby:
        return None
    selection, _ = _candidate_for_polygon(polygon, nearby)
    if selection.candidate is None:
        return None
    for index, edge in enumerate(ring_edges(polygon)):
        if edge.equals_exact(selection.candidate.edge, tolerance=0.0):
            return index if index < UNKNOWN_FRONTAGE_EDGE else None
    raise ValueError("selected frontage edge is not an edge of the final polygon")


def _candidate_record(
    object_id: object, address: ParsedAddress, candidate: EdgeCandidate
) -> dict[str, object]:
    return {
        "building_objectid": object_id,
        "edge": list(candidate.edge_key),
        "house_range": [address.house_range.first, address.house_range.last],
        "parallel_angle_degrees": round(candidate.angle_degrees, 6),
        "street_centerline_objectid": candidate.street.object_id,
        "street_class": candidate.street.street_class,
        "street_distance_m": round(candidate.distance_m, 6),
        "street_label": address.street_label,
    }


def audit(
    buildings: InputSnapshot, streets: InputSnapshot, *, include_candidates: bool = False
) -> dict[str, object]:
    """Audit in pinned source order; candidates are opt-in to keep the default compact."""
    street_index = build_street_index(streets.path)
    transformer = Transformer.from_crs(4326, EPSG, always_xy=True)
    counts: Counter[str] = Counter()
    rejected: Counter[str] = Counter()
    accepted_digest = hashlib.sha256()
    records: list[dict[str, object]] = []
    for values, geometry_name in _arrow_batches(buildings.path, ("objectid", "address")):
        for object_id, raw_address, wkb in zip(
            values["objectid"], values["address"], values[geometry_name], strict=True
        ):
            counts["total_records"] += 1
            if raw_address is None:
                rejected["null_address"] += 1
                continue
            counts["non_null_addresses"] += 1
            address = parse_address(raw_address)
            if address is None:
                rejected["unparseable_address"] += 1
                continue
            counts["parseable_addresses"] += 1
            if address.street_label not in street_index.ids_by_name:
                rejected["no_exact_street_name"] += 1
                continue
            counts["name_matched"] += 1
            if not isinstance(wkb, bytes):
                rejected["missing_geometry"] += 1
                counts["geometry_rejections"] += 1
                continue
            source_geometry = from_wkb(wkb)
            if not isinstance(source_geometry, Polygon):
                rejected["source_geometry_not_polygon"] += 1
                counts["geometry_rejections"] += 1
                continue
            polygon = shapely_transform(source_geometry, transformer.transform, interleaved=False)
            if not isinstance(polygon, Polygon) or polygon.is_empty or not polygon.is_valid:
                rejected["invalid_projected_polygon"] += 1
                counts["geometry_rejections"] += 1
                continue
            nearby = street_index.nearby_matching(polygon, address)
            if not nearby:
                rejected["no_range_matched_street"] += 1
                continue
            counts["range_matched"] += 1
            selection, category = _candidate_for_polygon(polygon, nearby)
            if selection.candidate is None:
                rejected[selection.rejected_reason or "unknown_geometry_rejection"] += 1
                if category == "distance":
                    counts["distance_rejections"] += 1
                elif category == "geometry":
                    counts["geometry_rejections"] += 1
                elif category == "ambiguous":
                    counts["ambiguous_candidates"] += 1
                continue
            counts["unique_candidates"] += 1
            record = _candidate_record(object_id, address, selection.candidate)
            accepted_digest.update(
                json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
            )
            accepted_digest.update(b"\n")
            if include_candidates:
                records.append(record)
    counts["accepted_records"] = counts["unique_candidates"]
    rules_sha256 = hashlib.sha256(
        json.dumps(RULES, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return {
        "accepted_records_sha256": accepted_digest.hexdigest(),
        "audit": "frontage-candidates-v1",
        "candidate_order": "pinned building-footprint source feature order",
        "candidates": records if include_candidates else None,
        "counts": {key: counts[key] for key in sorted(counts)},
        "inputs": {
            "building_footprints": {
                "bytes": buildings.size,
                "file": buildings.path.name,
                "sha256": buildings.sha256,
            },
            "street_centerlines": {
                "bytes": streets.size,
                "file": streets.path.name,
                "sha256": streets.sha256,
            },
        },
        "projection": {"epsg": EPSG, "units": "metres"},
        "rejected_reasons": {key: rejected[key] for key in sorted(rejected)},
        "renderer_context": {
            "party_wall_masks": "not evaluated; renderer context is required",
            "raw_edge_coordinates": (
                "audit evidence only; v12 must recompute after projection, city clipping, "
                "make_valid, 0.35m simplification, and polygon explosion before using packed "
                "ring indices"
            ),
        },
        "rules": RULES,
        "rules_sha256": rules_sha256,
    }


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--buildings", type=Path, help="explicit local building-footprint GeoJSON")
    parser.add_argument("--streets", type=Path, help="explicit local street-centerline GeoJSON")
    parser.add_argument(
        "--output", type=Path, help="explicit report path; stdout is always written"
    )
    parser.add_argument(
        "--include-candidates",
        action="store_true",
        help="include accepted raw-source evidence records; default output is a compact summary",
    )
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    options = parse_args(arguments)
    try:
        report = audit(
            resolve_snapshot(SOURCES.buildings, options.buildings),
            resolve_snapshot(SOURCES.streets, options.streets),
            include_candidates=options.include_candidates,
        )
    except (OSError, ValueError) as error:
        print(f"frontage audit failed: {error}", file=sys.stderr)
        return 2
    encoded = json.dumps(report, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"
    if options.output is not None:
        options.output.write_text(encoded)
    sys.stdout.write(encoded)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
