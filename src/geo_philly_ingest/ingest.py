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

from .config import (
    CLEAN_DIR,
    EPSG,
    METADATA_JSON,
    MIN_BUILDING_COUNT,
    SOURCES,
    STREETS_BIN,
    WORLD_BIN,
)
from .download import download_all
from .geometry import buildings, city_rings, ground_rings, projected, streets
from .mesh import building_meshes
from .models import Bounds, Building, BuildingMesh, BuildingPart, MeshFace, Ring, Snapshot, Street
from .osm import building_parts, source_metadata

WORLD_MAGIC = b"GEOPHILY"
STREET_MAGIC = b"GEOSTRPH"
VERSION = 4
STREET_VERSION = 1


def load(snapshot: Snapshot) -> gpd.GeoDataFrame:
    frame = gpd.read_file(snapshot.path)
    if frame.empty:
        raise ValueError(f"{snapshot.name} source is empty")
    return frame


def write_ring(file: BufferedWriter, outline: Ring) -> None:
    file.write(struct.pack("<I", len(outline)))
    for x, y in outline:
        file.write(struct.pack("<ff", x, y))


def write_face(file: BufferedWriter, face: MeshFace) -> None:
    file.write(struct.pack("<I", len(face.points)))
    for x, y, z in face.points:
        file.write(struct.pack("<fff", x, y, z))


def pack_world(
    packed_buildings: list[Building],
    parts: list[BuildingPart],
    meshes: list[BuildingMesh],
    water: list[Ring],
    parks: list[Ring],
    bounds: Bounds,
) -> None:
    with WORLD_BIN.open("wb") as file:
        file.write(WORLD_MAGIC)
        file.write(
            struct.pack(
                "<IIIIIII",
                VERSION,
                EPSG,
                len(packed_buildings),
                len(parts),
                len(meshes),
                len(water),
                len(parks),
            )
        )
        file.write(struct.pack("<dddd", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y))
        for building in packed_buildings:
            file.write(struct.pack("<f", building.height))
            write_ring(file, building.ring)
        for part in parts:
            file.write(
                struct.pack(
                    "<QfffB",
                    part.osm_id,
                    part.height,
                    part.min_height,
                    part.roof_height,
                    int(part.roof_shape),
                )
            )
            color = part.facade_color
            file.write(bytes((*color, 255)) if color is not None else bytes(4))
            write_ring(file, part.ring)
        for mesh in meshes:
            file.write(struct.pack("<IfI", mesh.source_id, mesh.height, len(mesh.faces)))
            write_ring(file, mesh.footprint)
            for face in mesh.faces:
                write_face(file, face)
        for outline in water + parks:
            write_ring(file, outline)


def pack_streets(packed_streets: list[Street], bounds: Bounds) -> None:
    with STREETS_BIN.open("wb") as file:
        file.write(STREET_MAGIC)
        file.write(struct.pack("<III", STREET_VERSION, EPSG, len(packed_streets)))
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
    parts: list[BuildingPart],
    meshes: list[BuildingMesh],
    water: list[Ring],
    parks: list[Ring],
    packed_streets: list[Street],
) -> None:
    heights = [building.height for building in packed_buildings]
    part_heights = [part.height for part in parts]
    mesh_heights = [mesh.height for mesh in meshes]
    sources = []
    for source in SOURCES.all():
        snapshot = snapshots[source.filename]
        metadata = snapshot.metadata()
        if source is SOURCES.building_parts:
            metadata.update(source_metadata(snapshot))
        sources.append(metadata)
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
            "building_parts": len(parts),
            "building_part_facade_colors": sum(part.facade_color is not None for part in parts),
            "building_meshes": len(meshes),
            "building_mesh_faces": sum(len(mesh.faces) for mesh in meshes),
            "water": len(water),
            "parks": len(parks),
            "streets": len(packed_streets),
        },
        "height_m": {
            "buildings": {"min": min(heights), "max": max(heights)},
            "building_parts": {"min": min(part_heights), "max": max(part_heights)},
            "building_meshes": {"min": min(mesh_heights), "max": max(mesh_heights)},
        },
        "artifacts": {
            WORLD_BIN.name: {"bytes": WORLD_BIN.stat().st_size, "sha256": sha256(WORLD_BIN)},
            STREETS_BIN.name: {
                "bytes": STREETS_BIN.stat().st_size,
                "sha256": sha256(STREETS_BIN),
            },
        },
        "sources": sources,
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
    if len(packed_buildings) < MIN_BUILDING_COUNT:
        raise ValueError(
            f"building source produced only {len(packed_buildings):,} usable footprints; "
            f"expected at least {MIN_BUILDING_COUNT:,}"
        )
    parts = building_parts(snapshots[SOURCES.building_parts.filename])
    meshes = building_meshes(snapshots[SOURCES.downtown_meshes.filename])
    water = ground_rings(load(snapshots[SOURCES.water.filename]), city)
    parks = ground_rings(load(snapshots[SOURCES.parks.filename]), city)
    packed_streets = streets(load(snapshots[SOURCES.streets.filename]), city)

    CLEAN_DIR.mkdir(parents=True, exist_ok=True)
    pack_world(packed_buildings, parts, meshes, water, parks, bounds)
    pack_streets(packed_streets, bounds)
    write_metadata(snapshots, bounds, packed_buildings, parts, meshes, water, parks, packed_streets)
    print(
        f"wrote {WORLD_BIN} ({WORLD_BIN.stat().st_size / 1_000_000:.1f} MB) and "
        f"{STREETS_BIN} ({STREETS_BIN.stat().st_size / 1_000_000:.1f} MB)"
    )


def main() -> None:
    asyncio.run(main_async())


if __name__ == "__main__":
    main()
