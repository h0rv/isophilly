from __future__ import annotations

import asyncio
import hashlib
import json
import struct
from io import BufferedWriter
from pathlib import Path

import geopandas as gpd
from shapely import union_all
from shapely.geometry.base import BaseGeometry

from .config import CLEAN_DIR, EPSG, METADATA_JSON, SOURCES, STREETS_BIN, WORLD_BIN
from .download import download_all
from .geometry import buildings, city_rings, ground_rings, projected, streets
from .models import Bounds, Building, Ring, Snapshot, Street

WORLD_MAGIC = b"GEOPHILY"
STREET_MAGIC = b"GEOSTRPH"
VERSION = 1


def load(snapshot: Snapshot) -> gpd.GeoDataFrame:
    frame = gpd.read_file(snapshot.path)
    if frame.empty:
        raise ValueError(f"{snapshot.name} source is empty")
    return frame


def write_ring(file: BufferedWriter, outline: Ring) -> None:
    file.write(struct.pack("<I", len(outline)))
    for x, y in outline:
        file.write(struct.pack("<ff", x, y))


def pack_world(
    packed_buildings: list[Building], water: list[Ring], parks: list[Ring], bounds: Bounds
) -> None:
    with WORLD_BIN.open("wb") as file:
        file.write(WORLD_MAGIC)
        file.write(
            struct.pack(
                "<IIIII",
                VERSION,
                EPSG,
                len(packed_buildings),
                len(water),
                len(parks),
            )
        )
        file.write(struct.pack("<dddd", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y))
        for building in packed_buildings:
            file.write(struct.pack("<f", building.height))
            write_ring(file, building.ring)
        for outline in water + parks:
            write_ring(file, outline)


def pack_streets(packed_streets: list[Street], bounds: Bounds) -> None:
    with STREETS_BIN.open("wb") as file:
        file.write(STREET_MAGIC)
        file.write(struct.pack("<III", VERSION, EPSG, len(packed_streets)))
        file.write(struct.pack("<dddd", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y))
        for street in packed_streets:
            file.write(struct.pack("<B", street.street_class))
            write_ring(file, street.points)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def write_metadata(
    snapshots: dict[str, Snapshot],
    bounds: Bounds,
    packed_buildings: list[Building],
    water: list[Ring],
    parks: list[Ring],
    packed_streets: list[Street],
) -> None:
    heights = [building.height for building in packed_buildings]
    metadata = {
        "schema_version": VERSION,
        "crs": {"epsg": EPSG, "units": "metres"},
        "bounds_m": {
            "min_x": bounds.min_x,
            "min_y": bounds.min_y,
            "max_x": bounds.max_x,
            "max_y": bounds.max_y,
            "width": bounds.width,
            "height": bounds.height,
        },
        "counts": {
            "buildings": len(packed_buildings),
            "water": len(water),
            "parks": len(parks),
            "streets": len(packed_streets),
        },
        "height_m": {"min": min(heights), "max": max(heights)},
        "artifacts": {
            WORLD_BIN.name: {"bytes": WORLD_BIN.stat().st_size, "sha256": sha256(WORLD_BIN)},
            STREETS_BIN.name: {
                "bytes": STREETS_BIN.stat().st_size,
                "sha256": sha256(STREETS_BIN),
            },
        },
        "sources": [snapshots[source.filename].metadata() for source in SOURCES.all()],
    }
    METADATA_JSON.write_text(json.dumps(metadata, indent=2) + "\n")


def city_geometry(snapshot: Snapshot) -> tuple[BaseGeometry, Bounds]:
    frame = load(snapshot)
    city = union_all(projected(frame).geometry)
    return city, Bounds.from_rings(city_rings(frame))


async def main_async() -> None:
    snapshots = await download_all(SOURCES.all())
    city, bounds = city_geometry(snapshots[SOURCES.city.filename])
    packed_buildings = buildings(load(snapshots[SOURCES.buildings.filename]), city)
    water = ground_rings(load(snapshots[SOURCES.water.filename]), city)
    parks = ground_rings(load(snapshots[SOURCES.parks.filename]), city)
    packed_streets = streets(load(snapshots[SOURCES.streets.filename]), city)

    CLEAN_DIR.mkdir(parents=True, exist_ok=True)
    pack_world(packed_buildings, water, parks, bounds)
    pack_streets(packed_streets, bounds)
    write_metadata(snapshots, bounds, packed_buildings, water, parks, packed_streets)
    print(
        f"wrote {WORLD_BIN} ({WORLD_BIN.stat().st_size / 1_000_000:.1f} MB) and "
        f"{STREETS_BIN} ({STREETS_BIN.stat().st_size / 1_000_000:.1f} MB)"
    )


def main() -> None:
    asyncio.run(main_async())


if __name__ == "__main__":
    main()
