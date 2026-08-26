from __future__ import annotations

import asyncio
import struct
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from io import BufferedWriter
from pathlib import Path

import geopandas as gpd
import pandas as pd
from shapely import affinity
from shapely.geometry import MultiPolygon, Polygon
from shapely.geometry.base import BaseGeometry

ROOT = Path(__file__).resolve().parent.parent.parent
OUT_BIN = ROOT / "data" / "clean" / "philly.bin"

MAGIC = b"GEOPHILY"
VERSION = 1
EPSG = 2272
FEET_TO_METERS = 0.3048006096012192
DEFAULT_HEIGHT_METERS = 8.0
SIMPLIFY_TOLERANCE_METERS = 0.5
MIN_BUILDING_AREA_METERS = 10.0
HEIGHT_FIELDS = ("approx_hgt", "approx_height", "max_hgt", "height")

type Ring = tuple[tuple[float, float], ...]


@dataclass(frozen=True, slots=True)
class Building:
    height: float
    ring: Ring


@dataclass(frozen=True, slots=True)
class Bounds:
    min_x: float
    min_y: float
    max_x: float
    max_y: float


async def load_geojson(dataset: str, resource: str) -> gpd.GeoDataFrame:
    from philly import Philly

    loaded = await Philly().load(dataset, resource, format="geojson")
    if not isinstance(loaded, Mapping):
        raise TypeError(f"{dataset} did not load as GeoJSON")
    features = loaded.get("features")
    if not isinstance(features, list):
        raise TypeError(f"{dataset} GeoJSON has no feature list")
    return gpd.GeoDataFrame.from_features(features, crs="EPSG:4326")


def meters(gdf: gpd.GeoDataFrame) -> gpd.GeoDataFrame:
    source = gdf.set_crs("EPSG:4326") if gdf.crs is None else gdf
    projected = source.to_crs(epsg=EPSG)
    result = projected.copy()
    result["geometry"] = projected.geometry.apply(
        lambda geometry: (
            affinity.scale(geometry, xfact=FEET_TO_METERS, yfact=FEET_TO_METERS, origin=(0, 0))
            if geometry is not None and not geometry.is_empty
            else geometry
        )
    )
    return result


def prepared_polygons(geometry: BaseGeometry | None) -> Iterator[Polygon]:
    if geometry is None or geometry.is_empty:
        return
    fixed = geometry if geometry.is_valid else geometry.buffer(0)
    simplified = fixed.simplify(SIMPLIFY_TOLERANCE_METERS, preserve_topology=True)
    if isinstance(simplified, Polygon):
        yield simplified
    elif isinstance(simplified, MultiPolygon):
        yield from simplified.geoms


def polygons(gdf: gpd.GeoDataFrame) -> Iterator[Polygon]:
    for geometry in gdf.geometry:
        yield from prepared_polygons(geometry)


def ring(polygon: Polygon) -> Ring | None:
    points = tuple((float(x), float(y)) for x, y in polygon.exterior.coords[:-1])
    return points if len(points) >= 3 else None


def height(row: pd.Series) -> float:
    for field in HEIGHT_FIELDS:
        value: object = row.get(field)
        if isinstance(value, (int, float)) and 0 < value < 2_000:
            return float(value) * FEET_TO_METERS
    return DEFAULT_HEIGHT_METERS


def buildings(gdf: gpd.GeoDataFrame) -> list[Building]:
    result: list[Building] = []
    for _, row in gdf.iterrows():
        for polygon in prepared_polygons(row.geometry):
            if polygon.area < MIN_BUILDING_AREA_METERS:
                continue
            if outline := ring(polygon):
                result.append(Building(height(row), outline))
    return result


def bounds(*layers: list[Ring]) -> Bounds:
    points = [point for layer in layers for ring in layer for point in ring]
    xs, ys = zip(*points, strict=True)
    return Bounds(min(xs), min(ys), max(xs), max(ys))


def write_ring(file: BufferedWriter, outline: Ring) -> None:
    file.write(struct.pack("<I", len(outline)))
    for x, y in outline:
        file.write(struct.pack("<ff", x, y))


def pack(buildings: list[Building], water: list[Ring], parks: list[Ring]) -> None:
    city_bounds = bounds([building.ring for building in buildings], water, parks)
    OUT_BIN.parent.mkdir(parents=True, exist_ok=True)
    with OUT_BIN.open("wb") as file:
        file.write(MAGIC)
        file.write(struct.pack("<IIIII", VERSION, EPSG, len(buildings), len(water), len(parks)))
        file.write(
            struct.pack(
                "<dddd",
                city_bounds.min_x,
                city_bounds.min_y,
                city_bounds.max_x,
                city_bounds.max_y,
            )
        )
        for building in buildings:
            file.write(struct.pack("<f", building.height))
            write_ring(file, building.ring)
        for outline in water + parks:
            write_ring(file, outline)
    print(f"wrote {OUT_BIN} ({OUT_BIN.stat().st_size / 1_000_000:.1f} MB)")


async def main_async() -> None:
    building_data = await load_geojson("Building Footprints", "Building Footprints (GeoJSON)")
    water_data = await load_geojson("Hydrology", "Hydrology - Polygon (GeoJSON)")
    park_data = await load_geojson("PPR Properties", "PPR Properties (GeoJSON)")
    packed_buildings = buildings(meters(building_data))
    packed_water = [
        outline for polygon in polygons(meters(water_data)) if (outline := ring(polygon))
    ]
    packed_parks = [
        outline for polygon in polygons(meters(park_data)) if (outline := ring(polygon))
    ]
    pack(packed_buildings, packed_water, packed_parks)


def main() -> None:
    asyncio.run(main_async())


if __name__ == "__main__":
    main()
