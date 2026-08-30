from __future__ import annotations

import json
import re
from collections.abc import Mapping
from math import isfinite
from numbers import Real

import geopandas as gpd
from shapely.geometry import Polygon

from .config import EPSG, MAX_HEIGHT_METERS, MIN_BUILDING_AREA_METERS, MIN_HEIGHT_METERS
from .geometry import exterior, polygons
from .models import BuildingPart, RoofShape, Snapshot

METERS_PER_FOOT = 0.3048
METERS_PER_LEVEL = 3.2
DEFAULT_ROOF_HEIGHT_METERS = 2.4
_NUMBER = re.compile(r"^\s*(-?\d+(?:\.\d+)?)\s*([a-zA-Z']*)\s*$")
_ROOF_SHAPES = {
    "flat": RoofShape.FLAT,
    "gabled": RoofShape.GABLED,
    "skillion": RoofShape.GABLED,
    "saltbox": RoofShape.GABLED,
    "quadruple_saltbox": RoofShape.GABLED,
    "quadruple:saltbox": RoofShape.GABLED,
    "hipped": RoofShape.HIPPED,
    "round hipped": RoofShape.HIPPED,
    "pyramidal": RoofShape.PYRAMIDAL,
    "dome": RoofShape.DOME,
    "round": RoofShape.DOME,
    "cone": RoofShape.CONE,
    "mansard": RoofShape.MANSARD,
    "basilical": RoofShape.MANSARD,
}


def _mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ValueError(f"{label} must be an object with string keys")
    return {str(key): item for key, item in value.items()}


def source_metadata(snapshot: Snapshot) -> dict[str, str]:
    payload: object = json.loads(snapshot.path.read_text())
    response = _mapping(payload, "OpenStreetMap response")
    result: dict[str, str] = {}
    generator = response.get("generator")
    if isinstance(generator, str):
        result["generator"] = generator
    osm3s = response.get("osm3s")
    if osm3s is not None:
        timestamp = _mapping(osm3s, "OpenStreetMap metadata").get("timestamp_osm_base")
        if isinstance(timestamp, str):
            result["timestamp_osm_base"] = timestamp
    return result


def _sequence(value: object, label: str) -> list[object]:
    if not isinstance(value, list):
        raise ValueError(f"{label} must be an array")
    return value


def _tags(value: object) -> dict[str, str]:
    raw = _mapping(value, "OpenStreetMap tags")
    if not all(isinstance(item, str) for item in raw.values()):
        raise ValueError("OpenStreetMap tags must contain strings")
    return {key: item for key, item in raw.items() if isinstance(item, str)}


def parse_length(value: str | None) -> float | None:
    if value is None:
        return None
    match = _NUMBER.fullmatch(value)
    if match is None:
        return None
    amount = float(match.group(1))
    unit = match.group(2).lower()
    if unit in {"ft", "foot", "feet", "'"}:
        amount *= METERS_PER_FOOT
    elif unit not in {"", "m", "meter", "meters", "metre", "metres"}:
        return None
    return amount if amount >= 0.0 else None


def parse_levels(value: str | None) -> float | None:
    if value is None:
        return None
    match = _NUMBER.fullmatch(value)
    if match is None or match.group(2):
        return None
    levels = float(match.group(1))
    return levels if levels >= 0.0 else None


def _levels(tags: Mapping[str, str], key: str) -> float | None:
    value = parse_levels(tags.get(key))
    return None if value is None else value * METERS_PER_LEVEL


def _height(tags: Mapping[str, str]) -> float | None:
    explicit = parse_length(tags.get("height"))
    height = explicit if explicit is not None else _levels(tags, "building:levels")
    if height is None or not MIN_HEIGHT_METERS <= height <= MAX_HEIGHT_METERS:
        return None
    return height


def _roof(tags: Mapping[str, str], height: float, min_height: float) -> tuple[RoofShape, float]:
    shape = _ROOF_SHAPES.get(tags.get("roof:shape", "flat"), RoofShape.FLAT)
    explicit_height = parse_length(tags.get("roof:height"))
    explicit = explicit_height if explicit_height is not None else _levels(tags, "roof:levels")
    if shape is RoofShape.FLAT:
        return shape, 0.0
    roof_height = (
        explicit if explicit is not None else min(DEFAULT_ROOF_HEIGHT_METERS, height * 0.25)
    )
    return shape, min(roof_height, (height - min_height) * 0.45)


def _point(value: object) -> tuple[float, float]:
    raw = _mapping(value, "OpenStreetMap point")
    lon = raw.get("lon")
    lat = raw.get("lat")
    if (
        not isinstance(lon, Real)
        or isinstance(lon, bool)
        or not isinstance(lat, Real)
        or isinstance(lat, bool)
    ):
        raise ValueError("OpenStreetMap point needs numeric lon and lat")
    point = float(lon), float(lat)
    if not isfinite(point[0]) or not isfinite(point[1]):
        raise ValueError("OpenStreetMap point coordinates must be finite")
    if not -180.0 <= point[0] <= 180.0 or not -90.0 <= point[1] <= 90.0:
        raise ValueError("OpenStreetMap point coordinates are out of range")
    return point


def building_parts(snapshot: Snapshot) -> list[BuildingPart]:
    payload: object = json.loads(snapshot.path.read_text())
    response = _mapping(payload, "OpenStreetMap response")
    remark = response.get("remark")
    if isinstance(remark, str) and remark:
        raise ValueError(f"OpenStreetMap response failed: {remark}")
    elements = _sequence(response.get("elements"), "elements")
    if not elements:
        raise ValueError("OpenStreetMap response contains no building parts")
    parsed: list[tuple[int, dict[str, str], Polygon]] = []
    for value in elements:
        element = _mapping(value, "OpenStreetMap element")
        if element.get("type") != "way":
            continue
        osm_id = element.get("id")
        geometry = element.get("geometry")
        if not isinstance(osm_id, int) or isinstance(osm_id, bool) or geometry is None:
            continue
        tags = _tags(element.get("tags", {}))
        if "building:part" not in tags:
            continue
        points = tuple(_point(point) for point in _sequence(geometry, "geometry"))
        if len(points) < 4:
            continue
        polygon = Polygon(points)
        if not polygon.is_empty:
            parsed.append((osm_id, tags, polygon))

    if not parsed:
        raise ValueError("OpenStreetMap response contains no usable building parts")

    frame = gpd.GeoDataFrame(
        {"osm_id": [item[0] for item in parsed]},
        geometry=[item[2] for item in parsed],
        crs=4326,
    ).to_crs(epsg=EPSG)
    result: list[BuildingPart] = []
    for (osm_id, tags, _), geometry in zip(parsed, frame.geometry, strict=True):
        height = _height(tags)
        if height is None:
            continue
        explicit_min_height = parse_length(tags.get("min_height"))
        level_min_height = _levels(tags, "building:min_level")
        min_height = (
            explicit_min_height
            if explicit_min_height is not None
            else level_min_height
            if level_min_height is not None
            else 0.0
        )
        if height <= min_height:
            continue
        roof_shape, roof_height = _roof(tags, height, min_height)
        for polygon in polygons(geometry, 0.1):
            if polygon.area < MIN_BUILDING_AREA_METERS:
                continue
            outline = exterior(polygon)
            if outline is not None:
                result.append(
                    BuildingPart(
                        osm_id=osm_id,
                        height=height,
                        min_height=min_height,
                        roof_height=roof_height,
                        roof_shape=roof_shape,
                        ring=outline,
                    )
                )
    if not result:
        raise ValueError("OpenStreetMap response contains no height-backed building parts")
    return sorted(result, key=lambda part: (part.osm_id, part.ring))
