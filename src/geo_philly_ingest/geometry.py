from __future__ import annotations

from collections.abc import Iterator
from numbers import Real

import geopandas as gpd
import pandas as pd
from shapely import make_valid
from shapely.geometry import GeometryCollection, LineString, MultiLineString, MultiPolygon, Polygon
from shapely.geometry.base import BaseGeometry

from .config import (
    BUILDING_SIMPLIFY_METERS,
    DEFAULT_HEIGHT_METERS,
    EPSG,
    GROUND_SIMPLIFY_METERS,
    MAX_HEIGHT_METERS,
    MIN_BUILDING_AREA_METERS,
    MIN_HEIGHT_METERS,
    STREET_SIMPLIFY_METERS,
)
from .models import Building, Ring, Street

HEIGHT_FIELDS = ("approx_hgt", "max_hgt")
STREET_CLASSES = frozenset({1, 2, 3, 4, 5, 9, 10})


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


def lines(geometry: BaseGeometry | None) -> Iterator[LineString]:
    if geometry is None or geometry.is_empty:
        return
    simplified = geometry.simplify(STREET_SIMPLIFY_METERS, preserve_topology=True)
    if isinstance(simplified, LineString):
        yield simplified
    elif isinstance(simplified, (MultiLineString, GeometryCollection)):
        for part in simplified.geoms:
            yield from lines(part)


def exterior(polygon: Polygon) -> Ring | None:
    points = tuple((float(x), float(y)) for x, y in polygon.exterior.coords[:-1])
    return points if len(points) >= 3 else None


def city_rings(frame: gpd.GeoDataFrame) -> list[Ring]:
    return [
        outline
        for geometry in projected(frame).geometry
        for polygon in polygons(geometry, GROUND_SIMPLIFY_METERS)
        if (outline := exterior(polygon))
    ]


def ground_rings(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[Ring]:
    clipped = projected(frame).geometry.intersection(city)
    return [
        outline
        for geometry in clipped
        for polygon in polygons(geometry, GROUND_SIMPLIFY_METERS)
        if (outline := exterior(polygon))
    ]


def building_height(row: pd.Series) -> float:
    for field in HEIGHT_FIELDS:
        value: object = row.get(field)
        if isinstance(value, Real) and not isinstance(value, bool):
            meters = float(value) * 0.3048006096012192
            if MIN_HEIGHT_METERS <= meters <= MAX_HEIGHT_METERS:
                return meters
    return DEFAULT_HEIGHT_METERS


def buildings(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[Building]:
    result: list[Building] = []
    for _, row in projected(frame).iterrows():
        geometry: BaseGeometry | None = row.geometry
        if geometry is None or geometry.is_empty or not geometry.intersects(city):
            continue
        clipped = geometry if city.covers(geometry) else geometry.intersection(city)
        for polygon in polygons(clipped, BUILDING_SIMPLIFY_METERS):
            if polygon.area < MIN_BUILDING_AREA_METERS:
                continue
            if outline := exterior(polygon):
                result.append(Building(building_height(row), outline))
    return result


def streets(frame: gpd.GeoDataFrame, city: BaseGeometry) -> list[Street]:
    result: list[Street] = []
    for _, row in projected(frame).iterrows():
        street_class: object = row.get("class")
        if not isinstance(street_class, Real):
            continue
        parsed_class = int(float(street_class))
        if parsed_class not in STREET_CLASSES:
            continue
        geometry: BaseGeometry | None = row.geometry
        if geometry is None or geometry.is_empty:
            continue
        for line in lines(geometry.intersection(city)):
            points = tuple((float(x), float(y)) for x, y in line.coords)
            if len(points) >= 2:
                result.append(Street(parsed_class, points))
    return result
