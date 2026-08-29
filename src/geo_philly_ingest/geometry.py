from __future__ import annotations

from collections.abc import Iterator
from itertools import repeat
from numbers import Real

import geopandas as gpd
from shapely import make_valid
from shapely.geometry import GeometryCollection, MultiPolygon, Polygon
from shapely.geometry.base import BaseGeometry

from .config import (
    BUILDING_SIMPLIFY_METERS,
    CITY_SIMPLIFY_METERS,
    DEFAULT_HEIGHT_METERS,
    EPSG,
    MAX_HEIGHT_METERS,
    MIN_BUILDING_AREA_METERS,
    MIN_HEIGHT_METERS,
)
from .models import Building, Ring

HEIGHT_FIELDS = ("approx_hgt", "max_hgt")


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


def height_from_values(values: Iterator[object]) -> float:
    for value in values:
        if isinstance(value, Real) and not isinstance(value, bool):
            meters = float(value) * 0.3048006096012192
            if MIN_HEIGHT_METERS <= meters <= MAX_HEIGHT_METERS:
                return meters
    return DEFAULT_HEIGHT_METERS


def buildings(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[Building]:
    result: list[Building] = []
    frame = projected(frame)
    approximate = frame[HEIGHT_FIELDS[0]] if HEIGHT_FIELDS[0] in frame else repeat(None)
    maximum = frame[HEIGHT_FIELDS[1]] if HEIGHT_FIELDS[1] in frame else repeat(None)
    for geometry, approximate_height, maximum_height in zip(
        frame.geometry, approximate, maximum, strict=True
    ):
        if geometry is None or geometry.is_empty or not geometry.intersects(city):
            continue
        clipped = geometry if city.covers(geometry) else geometry.intersection(city)
        height = height_from_values(iter((approximate_height, maximum_height)))
        for polygon in polygons(clipped, BUILDING_SIMPLIFY_METERS):
            if polygon.area < MIN_BUILDING_AREA_METERS:
                continue
            if outline := exterior(polygon):
                result.append(Building(height, outline))
    return result
