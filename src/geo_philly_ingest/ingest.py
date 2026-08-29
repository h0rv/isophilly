from __future__ import annotations

import asyncio
import hashlib
import json
import os
import shutil
import struct
from pathlib import Path
from typing import BinaryIO

import geopandas as gpd
from shapely import union_all
from shapely.geometry.base import BaseGeometry

from .collada import (
    LEGACY_DOWNTOWN_TEXTURE_ID_OFFSET,
    STADIUM_TEXTURE_ID_OFFSET,
    legacy_downtown_meshes,
    stadium_meshes,
)
from .config import (
    CLEAN_DIR,
    EPSG,
    LEGACY_DOWNTOWN_ARCHIVE,
    MESH_TEXTURE_DIR,
    METADATA_JSON,
    MIN_BUILDING_COUNT,
    SOURCES,
    STADIUM_ARCHIVE,
    WORLD_BIN,
)
from .download import download_all, local_snapshot
from .geometry import buildings, city_rings, projected
from .mesh import building_meshes, merge_mesh_sources, prune_mesh_textures, texture_digest
from .models import Bounds, Building, BuildingMesh, MeshFace, Ring, Snapshot

WORLD_MAGIC = b"GEOPHILY"
VERSION = 6


def load(snapshot: Snapshot) -> gpd.GeoDataFrame:
    frame = gpd.read_file(snapshot.path)
    if frame.empty:
        raise ValueError(f"{snapshot.name} source is empty")
    return frame


def write_ring(file: BinaryIO, outline: Ring) -> None:
    file.write(struct.pack("<I", len(outline)))
    for x, y in outline:
        file.write(struct.pack("<ff", x, y))


def write_face(file: BinaryIO, face: MeshFace) -> None:
    if len(face.points) != 3 or len(face.uvs) != 3:
        raise ValueError("textured mesh faces must be triangles")
    for (x, y, z), (u, v) in zip(face.points, face.uvs, strict=True):
        file.write(struct.pack("<fffff", x, y, z, u, v))


def write_world(
    file: BinaryIO,
    packed_buildings: list[Building],
    meshes: list[BuildingMesh],
    city: list[Ring],
    bounds: Bounds,
    texture_sha256: bytes,
) -> None:
    if len(texture_sha256) != 32:
        raise ValueError("texture digest must be SHA-256")
    file.write(WORLD_MAGIC)
    file.write(
        struct.pack(
            "<IIIII",
            VERSION,
            EPSG,
            len(packed_buildings),
            len(meshes),
            len(city),
        )
    )
    file.write(texture_sha256)
    file.write(struct.pack("<dddd", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y))
    for building in packed_buildings:
        file.write(struct.pack("<f", building.height))
        write_ring(file, building.ring)
    for mesh in meshes:
        file.write(struct.pack("<IfI", mesh.texture_id, mesh.height, len(mesh.faces)))
        write_ring(file, mesh.footprint)
        for face in mesh.faces:
            write_face(file, face)
    for outline in city:
        write_ring(file, outline)


def pack_world(
    path: Path,
    packed_buildings: list[Building],
    meshes: list[BuildingMesh],
    city: list[Ring],
    bounds: Bounds,
    texture_sha256: bytes,
) -> None:
    temporary = path.with_suffix(".bin.part")
    with temporary.open("wb") as file:
        write_world(file, packed_buildings, meshes, city, bounds, texture_sha256)
    temporary.replace(path)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def write_metadata(
    path: Path,
    world_path: Path,
    snapshots: dict[str, Snapshot],
    bounds: Bounds,
    packed_buildings: list[Building],
    meshes: list[BuildingMesh],
    city: list[Ring],
    texture_sha256: bytes,
    texture_bytes: int,
) -> None:
    heights = [building.height for building in packed_buildings]
    mesh_heights = [mesh.height for mesh in meshes]
    sources = []
    for source in SOURCES.all():
        snapshot = snapshots[source.filename]
        metadata = snapshot.metadata()
        metadata.update(source.provenance())
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
            "building_meshes": len(meshes),
            "center_city_building_meshes": sum(
                mesh.texture_id < LEGACY_DOWNTOWN_TEXTURE_ID_OFFSET for mesh in meshes
            ),
            "legacy_downtown_building_meshes": sum(
                LEGACY_DOWNTOWN_TEXTURE_ID_OFFSET <= mesh.texture_id < STADIUM_TEXTURE_ID_OFFSET
                for mesh in meshes
            ),
            "stadium_building_meshes": sum(
                mesh.texture_id >= STADIUM_TEXTURE_ID_OFFSET for mesh in meshes
            ),
            "building_mesh_faces": sum(len(mesh.faces) for mesh in meshes),
            "building_texture_atlases": len({mesh.texture_id for mesh in meshes}),
            "city_rings": len(city),
        },
        "height_m": {
            "buildings": {"min": min(heights), "max": max(heights)},
            "building_meshes": {"min": min(mesh_heights), "max": max(mesh_heights)},
        },
        "artifacts": {
            WORLD_BIN.name: {
                "bytes": world_path.stat().st_size,
                "sha256": sha256(world_path),
            },
            "mesh-textures": {
                "bytes": texture_bytes,
                "sha256": texture_sha256.hex(),
            },
        },
        "sources": sources,
    }
    temporary = path.with_suffix(".json.part")
    temporary.write_text(json.dumps(metadata, indent=2) + "\n")
    temporary.replace(path)


def prepare_clean_staging() -> Path:
    staging = CLEAN_DIR.with_name(".clean-next")
    previous = CLEAN_DIR.with_name(".clean-previous")
    if not CLEAN_DIR.exists() and previous.exists():
        previous.replace(CLEAN_DIR)
    if staging.exists():
        shutil.rmtree(staging)
    if CLEAN_DIR.exists():
        shutil.copytree(CLEAN_DIR, staging, copy_function=os.link)
    else:
        staging.mkdir(parents=True)
    return staging


def publish_clean(staging: Path) -> None:
    previous = CLEAN_DIR.with_name(".clean-previous")
    if previous.exists():
        shutil.rmtree(previous)
    if CLEAN_DIR.exists():
        CLEAN_DIR.replace(previous)
    try:
        staging.replace(CLEAN_DIR)
    except Exception:
        if previous.exists():
            previous.replace(CLEAN_DIR)
        raise
    if previous.exists():
        shutil.rmtree(previous)


def city_geometry(snapshot: Snapshot) -> tuple[BaseGeometry, list[Ring], Bounds]:
    frame = load(snapshot)
    city = union_all(projected(frame).geometry)
    rings = city_rings(frame)
    return city, rings, Bounds.from_rings(rings)


async def main_async() -> None:
    local_archives = {
        source.filename: (source, path)
        for source, path in (
            (SOURCES.legacy_downtown_meshes, LEGACY_DOWNTOWN_ARCHIVE),
            (SOURCES.stadium_meshes, STADIUM_ARCHIVE),
        )
        if path.is_file()
    }
    print("loading verified source snapshots", flush=True)
    snapshots = await download_all(
        tuple(source for source in SOURCES.all() if source.filename not in local_archives)
    )
    for filename, (source, path) in local_archives.items():
        print(f"verifying {source.name}", flush=True)
        snapshots[filename] = await asyncio.to_thread(
            local_snapshot,
            source,
            path,
        )
    print("loaded 5 source snapshots", flush=True)
    print("projecting and clipping citywide footprints", flush=True)
    city, packed_city, bounds = city_geometry(snapshots[SOURCES.city.filename])
    packed_buildings = buildings(load(snapshots[SOURCES.buildings.filename]), city)
    if len(packed_buildings) < MIN_BUILDING_COUNT:
        raise ValueError(
            f"building source produced only {len(packed_buildings):,} usable footprints; "
            f"expected at least {MIN_BUILDING_COUNT:,}"
        )
    print(f"packed {len(packed_buildings):,} building footprints", flush=True)
    staging = await asyncio.to_thread(prepare_clean_staging)
    texture_dir = staging / MESH_TEXTURE_DIR.name
    print("importing textured mesh sources", flush=True)
    async with asyncio.TaskGroup() as group:
        center_city_task = group.create_task(
            building_meshes(snapshots[SOURCES.downtown_meshes.filename], texture_dir)
        )
        stadium_task = group.create_task(
            asyncio.to_thread(
                stadium_meshes,
                snapshots[SOURCES.stadium_meshes.filename],
                texture_dir,
            )
        )
        legacy_downtown_task = group.create_task(
            asyncio.to_thread(
                legacy_downtown_meshes,
                snapshots[SOURCES.legacy_downtown_meshes.filename],
                texture_dir,
            )
        )
    meshes = merge_mesh_sources(
        center_city_task.result(),
        legacy_downtown_task.result(),
        stadium_task.result(),
    )
    print(f"accepted {len(meshes):,} textured building meshes", flush=True)
    await asyncio.to_thread(prune_mesh_textures, meshes, texture_dir)
    texture_sha256, texture_bytes = await asyncio.to_thread(texture_digest, meshes, texture_dir)
    print(f"verified {texture_bytes / 1_000_000:.1f} MB of texture atlases", flush=True)

    staged_world = staging / WORLD_BIN.name
    staged_metadata = staging / METADATA_JSON.name
    pack_world(
        staged_world,
        packed_buildings,
        meshes,
        packed_city,
        bounds,
        texture_sha256,
    )
    write_metadata(
        staged_metadata,
        staged_world,
        snapshots,
        bounds,
        packed_buildings,
        meshes,
        packed_city,
        texture_sha256,
        texture_bytes,
    )
    (staging / "streets.bin").unlink(missing_ok=True)
    await asyncio.to_thread(publish_clean, staging)
    print(f"wrote {WORLD_BIN} ({WORLD_BIN.stat().st_size / 1_000_000:.1f} MB)")


def main() -> None:
    asyncio.run(main_async())


if __name__ == "__main__":
    main()
