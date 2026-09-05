from __future__ import annotations

import hashlib
import math
import re
import struct
from collections.abc import Iterator
from itertools import repeat
from numbers import Real

import geopandas as gpd
from pyproj import Transformer
from shapely import make_valid
from shapely.geometry import GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon
from shapely.geometry import Point as ShapelyPoint
from shapely.geometry.base import BaseGeometry

from .config import (
    BUILDING_SIMPLIFY_METERS,
    CITY_SIMPLIFY_METERS,
    DEFAULT_HEIGHT_METERS,
    EPSG,
    GROUND_SIMPLIFY_METERS,
    MAX_HEIGHT_METERS,
    MIN_BUILDING_AREA_METERS,
    MIN_HEIGHT_METERS,
    STREET_TREE_ACCEPTED_COUNT,
    STREET_TREE_PAYLOAD_SHA256,
    STREET_TREE_SOURCE_RECORD_COUNT,
)
from .models import Building, Ring, StreetTree, TransportKind, TransportLine, TreeForm

HEIGHT_FIELDS = ("approx_hgt", "max_hgt")
TREE_FIELDS = ("objectid", "tree_name", "tree_dbh", "year", "loc_y", "loc_x", "geometry")
INCHES_TO_METERS = 0.0254
DEFAULT_TREE_DIAMETER_METERS = 0.15
MIN_TREE_DIAMETER_METERS = 0.0254
MAX_TREE_DIAMETER_METERS = 2.0
TREE_LOCATION_TOLERANCE_METERS = 1.0
TRANSPORT_SIMPLIFY_METERS = 0.9144  # 3 ft
CONIFER_GENERA = frozenset(
    {
        "PINUS",
        "PICEA",
        "JUNIPERUS",
        "THUJA",
        "TAXODIUM",
        "TSUGA",
        "CHAMAECYPARIS",
        "CEDRUS",
        "CRYPTOMERIA",
        "ABIES",
        "PSEUDOTSUGA",
        "CUPRESSUS",
        "METASEQUOIA",
        "LARIX",
        "TAXUS",
    }
)
COLUMNAR_MODIFIERS = frozenset({"COLUMNAR", "FASTIGIATE", "UPRIGHT", "NARROW", "PYRAMIDAL"})
WEEPING_MODIFIERS = frozenset({"WEEPING", "PENDULA"})
COMMON_NAME_WORDS = re.compile(r"[A-Z]+")


def transport_lines(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[TransportLine]:
    """Keep only the three city-ranked road classes that read at city scale.

    Street centerlines are reference geometry, not surveyed curb lines.  Local
    streets would turn the aerial into a noisy wireframe, so classes 4 and
    above stay in the photograph.  The City source has no rail geometry; rail
    remains represented by its aerial/land-cover surface until a pinned rail
    source is added deliberately.
    """
    if "class" not in frame:
        raise ValueError("street centerline source has no class field")
    result: list[TransportLine] = []
    for kind_value, geometry in zip(frame["class"], projected(frame).geometry, strict=True):
        if isinstance(kind_value, bool) or not isinstance(kind_value, Real):
            continue
        try:
            kind = TransportKind(int(kind_value))
        except ValueError:
            continue
        if kind_value != int(kind_value) or geometry is None or geometry.is_empty:
            continue
        clipped = geometry.intersection(city).simplify(
            TRANSPORT_SIMPLIFY_METERS, preserve_topology=True
        )
        geometries = (
            [clipped]
            if isinstance(clipped, LineString)
            else list(clipped.geoms)
            if isinstance(clipped, (MultiLineString, GeometryCollection))
            else []
        )
        for line in geometries:
            if not isinstance(line, LineString):
                continue
            points = tuple((float(x), float(y)) for x, y in line.coords)
            if len(points) >= 2:
                result.append(TransportLine(kind=kind, points=points))
    return sorted(result, key=lambda line: (int(line.kind), line.points))


def projected(frame: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    if frame.crs is None:
        raise ValueError("source has no coordinate reference system")
    return frame.to_crs(epsg=EPSG)


def polygons(geometry: BaseGeometry | None, tolerance: float) -> Iterator[Polygon]:
    if geometry is None or geometry.is_empty:
        return
    fixed = geometry if geometry.is_valid else make_valid(geometry)
    simplified = fixed.simplify(tolerance, preserve_topology=True)
    if isinstance(simplified, Polygon):
        yield simplified
    elif isinstance(simplified, (MultiPolygon, GeometryCollection)):
        for part in simplified.geoms:
            yield from polygons(part, 0.0)


def exterior(polygon: Polygon) -> Ring | None:
    points = tuple((float(x), float(y)) for x, y in polygon.exterior.coords[:-1])
    return points if len(points) >= 3 else None


def city_rings(frame: gpd.GeoDataFrame) -> list[Ring]:
    return [
        outline
        for geometry in projected(frame).geometry
        for polygon in polygons(geometry, CITY_SIMPLIFY_METERS)
        if (outline := exterior(polygon))
    ]


def ground_rings(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[Ring]:
    return [
        outline
        for geometry in projected(frame).geometry
        for source_polygon in polygons(geometry, 0.0)
        for polygon in polygons(source_polygon.intersection(city), GROUND_SIMPLIFY_METERS)
        if (outline := exterior(polygon))
    ]


def height_from_values(values: Iterator[object]) -> float:
    for value in values:
        if isinstance(value, Real) and not isinstance(value, bool):
            meters = float(value) * 0.3048006096012192
            if MIN_HEIGHT_METERS <= meters <= MAX_HEIGHT_METERS:
                return meters
    return DEFAULT_HEIGHT_METERS


def footprint_id(geometry: BaseGeometry) -> str:
    return hashlib.sha256(make_valid(geometry).normalize().wkb).hexdigest()[:24]


def buildings(
    frame: gpd.GeoDataFrame,
    city: BaseGeometry,
    height_evidence: dict[str, float] | None = None,
) -> list[Building]:
    result: list[Building] = []
    identifiers = [
        footprint_id(geometry) if geometry is not None and not geometry.is_empty else None
        for geometry in frame.geometry
    ]
    frame = projected(frame)
    approximate = frame[HEIGHT_FIELDS[0]] if HEIGHT_FIELDS[0] in frame else repeat(None)
    maximum = frame[HEIGHT_FIELDS[1]] if HEIGHT_FIELDS[1] in frame else repeat(None)
    for geometry, approximate_height, maximum_height, identifier in zip(
        frame.geometry, approximate, maximum, identifiers, strict=True
    ):
        if geometry is None or geometry.is_empty or not geometry.intersects(city):
            continue
        clipped = geometry if city.covers(geometry) else geometry.intersection(city)
        measured_height = (
            height_evidence.get(identifier)
            if height_evidence is not None and identifier is not None
            else None
        )
        height = (
            measured_height
            if measured_height is not None
            else height_from_values(iter((approximate_height, maximum_height)))
        )
        for polygon in polygons(clipped, BUILDING_SIMPLIFY_METERS):
            if polygon.area < MIN_BUILDING_AREA_METERS:
                continue
            if outline := exterior(polygon):
                result.append(Building(height, outline))
    return result


def tree_diameter(value: object) -> float:
    if isinstance(value, Real) and not isinstance(value, bool):
        diameter = float(value) * INCHES_TO_METERS
        if MIN_TREE_DIAMETER_METERS <= diameter <= MAX_TREE_DIAMETER_METERS:
            return diameter
    return DEFAULT_TREE_DIAMETER_METERS


def normalized_tree_name(value: object) -> tuple[str, str] | None:
    """Parse the narrow, audited ``SCIENTIFIC - COMMON`` inventory spelling.

    Whitespace is made deterministic, but only the literal ASCII separator is
    accepted.  In particular, an en dash is not silently reinterpreted as a
    safe taxonomy boundary.
    """
    if not isinstance(value, str):
        return None
    normalized = " ".join(value.upper().split())
    if normalized in {"", "UNKNOWN", "UNKNOWN UNKNOWN - UNKNOWN"}:
        return None
    parts = normalized.split(" - ")
    if len(parts) != 2 or not all(parts):
        return None
    return parts[0], parts[1]


def tree_form(value: object) -> TreeForm:
    """Classify only explicit, safe source labels into a visual form."""
    parsed = normalized_tree_name(value)
    if parsed is None:
        return TreeForm.DEFAULT
    scientific, common = parsed
    scientific_tokens = scientific.split()
    if not scientific_tokens:
        return TreeForm.DEFAULT
    genus = scientific_tokens[0]
    common_words = frozenset(COMMON_NAME_WORDS.findall(common))
    if genus == "SHRUB":
        return TreeForm.SHRUB
    if genus in {"PALM", "UNKNOWN"}:
        return TreeForm.DEFAULT
    if common_words & COLUMNAR_MODIFIERS:
        return TreeForm.COLUMNAR
    if common_words & WEEPING_MODIFIERS:
        return TreeForm.WEEPING
    if genus in CONIFER_GENERA:
        return TreeForm.CONIFER
    return TreeForm.DEFAULT


def street_trees(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[StreetTree]:
    if tuple(frame.columns) != TREE_FIELDS:
        raise ValueError(
            "2025 tree inventory schema changed: "
            f"expected {TREE_FIELDS!r}, received {tuple(frame.columns)!r}"
        )
    if len(frame) != STREET_TREE_SOURCE_RECORD_COUNT:
        raise ValueError(
            f"tree inventory contains {len(frame):,} records; "
            f"expected exactly {STREET_TREE_SOURCE_RECORD_COUNT:,}"
        )
    if frame["objectid"].isna().any() or frame["objectid"].duplicated().any():
        raise ValueError("tree inventory object IDs must be present and unique")
    years = set(frame["year"].dropna().astype(str))
    if frame["year"].isna().any() or years != {"2025"}:
        raise ValueError(f"tree inventory contains unexpected years: {sorted(years)!r}")

    locations = list(zip(frame["loc_x"], frame["loc_y"], strict=True))
    if any(
        not isinstance(value, Real) or isinstance(value, bool) or not math.isfinite(float(value))
        for location in locations
        for value in location
    ):
        raise ValueError("tree inventory longitude/latitude fields must be finite numbers")
    transformer = Transformer.from_crs(4326, EPSG, always_xy=True)
    location_x, location_y = transformer.transform(
        [float(location[0]) for location in locations],
        [float(location[1]) for location in locations],
    )
    projected_frame = projected(frame)
    trees: list[tuple[int, StreetTree]] = []
    for object_id, tree_name, geometry, diameter, expected_x, expected_y in zip(
        frame["objectid"],
        frame["tree_name"],
        projected_frame.geometry,
        frame["tree_dbh"],
        location_x,
        location_y,
        strict=True,
    ):
        if (
            not isinstance(object_id, Real)
            or isinstance(object_id, bool)
            or not float(object_id).is_integer()
            or object_id < 1
        ):
            raise ValueError(f"tree inventory has invalid object ID: {object_id!r}")
        if not isinstance(geometry, ShapelyPoint) or geometry.is_empty:
            raise ValueError(f"tree {object_id!r} does not have point geometry")
        if (
            math.hypot(geometry.x - expected_x, geometry.y - expected_y)
            > TREE_LOCATION_TOLERANCE_METERS
        ):
            raise ValueError(f"tree {object_id!r} geometry disagrees with loc_x/loc_y")
        if not city.covers(geometry):
            continue
        trees.append(
            (
                int(object_id),
                StreetTree(
                    (float(geometry.x), float(geometry.y)),
                    tree_diameter(diameter),
                    tree_form(tree_name),
                ),
            )
        )
    trees.sort(key=lambda item: item[0])
    return [tree for _, tree in trees]


def validate_street_tree_output(trees: list[StreetTree]) -> None:
    digest = hashlib.sha256()
    for tree in trees:
        digest.update(
            struct.pack("<fffB", tree.point[0], tree.point[1], tree.diameter_m, int(tree.form))
        )
    if len(trees) != STREET_TREE_ACCEPTED_COUNT or digest.hexdigest() != STREET_TREE_PAYLOAD_SHA256:
        raise ValueError(
            "retained street-tree coordinates changed; review the tree and City Limits "
            "snapshots before updating the accepted count and payload digest"
        )
