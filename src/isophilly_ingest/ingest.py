from __future__ import annotations

import argparse
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
    ROOT,
    SOURCES,
    STADIUM_ARCHIVE,
    STREET_TREE_SOURCE_SHA256,
    TEXTURE_COVERAGE_JSON,
    WORLD_BIN,
)
from .download import download_all, local_snapshot
from .frontage_audit import RULES as FRONTAGE_RULES
from .frontage_audit import UNKNOWN_FRONTAGE_EDGE, build_street_index
from .geometry import (
    buildings,
    city_rings,
    ground_rings,
    projected,
    street_trees,
    transport_lines,
    validate_street_tree_output,
)
from .lidar import MERGED_EVIDENCE_PATH, PASDA_LAS_URL, load_height_evidence, preflight_merge_read
from .mesh import building_meshes, merge_mesh_sources, prune_mesh_textures, texture_digest
from .models import (
    Bounds,
    Building,
    BuildingMesh,
    BuildingPart,
    MeshFace,
    Ring,
    Snapshot,
    StreetTree,
    TransportLine,
)
from .osm import building_parts, source_metadata
from .quality import texture_coverage_report, write_texture_coverage
from .terrain import ARTIFACT_NAME as TERRAIN_ARTIFACT_NAME
from .terrain import TerrainBuild, build_if_evidence_present

WORLD_MAGIC = b"GEOPHILY"
VERSION = 12


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


def write_building_part(file: BinaryIO, part: BuildingPart) -> None:
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
    write_ring(file, part.ring)


def write_transport_line(file: BinaryIO, line: TransportLine) -> None:
    file.write(struct.pack("<BI", int(line.kind), len(line.points)))
    for x, y in line.points:
        file.write(struct.pack("<ff", x, y))


def write_world(
    file: BinaryIO,
    packed_buildings: list[Building],
    parts: list[BuildingPart],
    meshes: list[BuildingMesh],
    city: list[Ring],
    water: list[Ring],
    parks: list[Ring],
    trees: list[StreetTree],
    bounds: Bounds,
    texture_sha256: bytes,
    transport: list[TransportLine] | None = None,
) -> None:
    if len(texture_sha256) != 32:
        raise ValueError("texture digest must be SHA-256")
    file.write(WORLD_MAGIC)
    # Keeping the no-transport form byte-compatible makes the golden v9
    # fixture an explicit compatibility check. Production ingest always passes
    # an actual list and emits v12.
    if transport is None:
        file.write(
            struct.pack(
                "<IIIIIIIII",
                9,
                EPSG,
                len(packed_buildings),
                len(parts),
                len(meshes),
                len(city),
                len(water),
                len(parks),
                len(trees),
            )
        )
    else:
        file.write(
            struct.pack(
                "<IIIIIIIIII",
                VERSION,
                EPSG,
                len(packed_buildings),
                len(parts),
                len(meshes),
                len(city),
                len(water),
                len(parks),
                len(trees),
                len(transport),
            )
        )
    file.write(texture_sha256)
    file.write(struct.pack("<dddd", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y))
    for building in packed_buildings:
        file.write(struct.pack("<f", building.height))
        write_ring(file, building.ring)
        if transport is not None:
            frontage_edge = (
                UNKNOWN_FRONTAGE_EDGE if building.frontage_edge is None else building.frontage_edge
            )
            if building.frontage_edge is not None and not 0 <= frontage_edge < min(
                len(building.ring), UNKNOWN_FRONTAGE_EDGE
            ):
                raise ValueError(
                    "building frontage edge must be an existing non-sentinel ring edge"
                )
            file.write(struct.pack("<B", frontage_edge))
    for part in parts:
        write_building_part(file, part)
    for mesh in meshes:
        file.write(struct.pack("<IfI", mesh.texture_id, mesh.height, len(mesh.faces)))
        write_ring(file, mesh.footprint)
        for face in mesh.faces:
            write_face(file, face)
    for outline in city:
        write_ring(file, outline)
    for outline in water:
        write_ring(file, outline)
    for outline in parks:
        write_ring(file, outline)
    for tree in trees:
        file.write(struct.pack("<fff", tree.point[0], tree.point[1], tree.diameter_m))
        if transport is not None:
            file.write(struct.pack("<B", int(tree.form)))
    for line in transport or []:
        write_transport_line(file, line)


def pack_world(
    path: Path,
    packed_buildings: list[Building],
    parts: list[BuildingPart],
    meshes: list[BuildingMesh],
    city: list[Ring],
    water: list[Ring],
    parks: list[Ring],
    trees: list[StreetTree],
    transport: list[TransportLine],
    bounds: Bounds,
    texture_sha256: bytes,
) -> None:
    temporary = path.with_suffix(".bin.part")
    with temporary.open("wb") as file:
        write_world(
            file,
            packed_buildings,
            parts,
            meshes,
            city,
            water,
            parks,
            trees,
            bounds,
            texture_sha256,
            transport,
        )
    temporary.replace(path)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_tree_snapshot(snapshot: Snapshot) -> None:
    if snapshot.sha256 != STREET_TREE_SOURCE_SHA256:
        raise ValueError(
            "2025 tree inventory bytes changed; review the official replacement, schema, "
            "count, and license before updating STREET_TREE_SOURCE_SHA256"
        )


def write_metadata(
    path: Path,
    world_path: Path,
    snapshots: dict[str, Snapshot],
    bounds: Bounds,
    packed_buildings: list[Building],
    parts: list[BuildingPart],
    meshes: list[BuildingMesh],
    city: list[Ring],
    water: list[Ring],
    parks: list[Ring],
    trees: list[StreetTree],
    transport: list[TransportLine],
    texture_sha256: bytes,
    texture_bytes: int,
    texture_coverage: dict[str, object],
    lidar_height_count: int,
    terrain: TerrainBuild | None,
) -> None:
    heights = [building.height for building in packed_buildings]
    mesh_heights = [mesh.height for mesh in meshes]
    sources = []
    for source in SOURCES.all():
        snapshot = snapshots[source.filename]
        metadata = snapshot.metadata()
        metadata.update(source.provenance())
        if source is SOURCES.building_parts:
            metadata.update(source_metadata(snapshot))
        sources.append(metadata)
    preflight_merge_read(MERGED_EVIDENCE_PATH)
    if MERGED_EVIDENCE_PATH.exists():
        sources.append(
            {
                "name": "PASDA 2025 LiDAR building evidence",
                "url": PASDA_LAS_URL,
                "file": MERGED_EVIDENCE_PATH.name,
                "sha256": sha256(MERGED_EVIDENCE_PATH),
                "bytes": MERGED_EVIDENCE_PATH.stat().st_size,
            }
        )
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
            "lidar_height_buildings": lidar_height_count,
            "building_parts": len(parts),
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
            "water": len(water),
            "parks": len(parks),
            "street_trees": len(trees),
            "transport_lines": len(transport),
            "frontage_known_buildings": sum(
                building.frontage_edge is not None for building in packed_buildings
            ),
            "frontage_unknown_buildings": sum(
                building.frontage_edge is None for building in packed_buildings
            ),
        },
        "height_m": {
            "buildings": {"min": min(heights), "max": max(heights)},
            "building_parts": {
                "min": min(part.height for part in parts),
                "max": max(part.height for part in parts),
            },
            "building_meshes": {"min": min(mesh_heights), "max": max(mesh_heights)},
        },
        "texture_coverage": texture_coverage["citywide"],
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
        "frontage": {
            "unknown_sentinel": UNKNOWN_FRONTAGE_EDGE,
            "rules_sha256": hashlib.sha256(
                json.dumps(FRONTAGE_RULES, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest(),
            "packed_edge_digest_sha256": hashlib.sha256(
                bytes(
                    UNKNOWN_FRONTAGE_EDGE
                    if building.frontage_edge is None
                    else building.frontage_edge
                    for building in packed_buildings
                )
            ).hexdigest(),
            "selection": (
                "exact street name/range and final projected clipped repaired simplified polygon"
            ),
        },
        "sources": sources,
    }
    if terrain is not None:
        metadata["artifacts"][TERRAIN_ARTIFACT_NAME] = {
            "bytes": terrain.path.stat().st_size,
            "sha256": terrain.sha256,
        }
        metadata["terrain_relief"] = {
            "grid": {
                "cell_size_m": 256.0,
                "width": terrain.grid.width,
                "height": terrain.grid.height,
            },
            "coverage": {
                "direct_cells": terrain.direct_cells,
                "interpolated_cells": terrain.interpolated_cells,
                "rejected_gap_cells": terrain.rejected_gap_cells,
                "unsupported_cells": terrain.unsupported_cells,
            },
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


async def main_async(*, refresh: bool = False) -> None:
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
        tuple(source for source in SOURCES.all() if source.filename not in local_archives),
        refresh=refresh,
    )
    for filename, (source, path) in local_archives.items():
        print(f"verifying {source.name}", flush=True)
        snapshots[filename] = local_snapshot(source, path)
    print(f"loaded {len(snapshots)} source snapshots", flush=True)
    tree_snapshot = snapshots[SOURCES.street_trees.filename]
    validate_tree_snapshot(tree_snapshot)
    print("projecting and clipping citywide footprints", flush=True)
    city, packed_city, bounds = city_geometry(snapshots[SOURCES.city.filename])
    height_evidence: dict[str, float] | None = None
    preflight_merge_read(MERGED_EVIDENCE_PATH)
    if MERGED_EVIDENCE_PATH.exists():
        building_snapshot = snapshots[SOURCES.buildings.filename]
        height_evidence = load_height_evidence(MERGED_EVIDENCE_PATH, building_snapshot.sha256)
        print(
            f"loaded trustworthy LiDAR heights for {len(height_evidence):,} buildings",
            flush=True,
        )
    street_snapshot = snapshots[SOURCES.streets.filename]
    print("indexing full street centerlines for final-polygon frontage candidates", flush=True)
    frontage_streets = build_street_index(street_snapshot.path)
    packed_buildings = buildings(
        load(snapshots[SOURCES.buildings.filename]),
        city,
        height_evidence,
        frontage_streets,
    )
    parts = building_parts(snapshots[SOURCES.building_parts.filename])
    water = ground_rings(load(snapshots[SOURCES.water.filename]), city)
    parks = ground_rings(load(snapshots[SOURCES.parks.filename]), city)
    trees = street_trees(load(tree_snapshot), city)
    transport = transport_lines(load(street_snapshot), city)
    validate_street_tree_output(trees)
    if len(packed_buildings) < MIN_BUILDING_COUNT:
        raise ValueError(
            f"building source produced only {len(packed_buildings):,} usable footprints; "
            f"expected at least {MIN_BUILDING_COUNT:,}"
        )
    print(f"packed {len(packed_buildings):,} building footprints", flush=True)
    print(f"packed {len(parts):,} height-backed building parts", flush=True)
    print(f"packed {len(trees):,} inventoried street trees", flush=True)
    print(f"packed {len(transport):,} major transport centerlines", flush=True)
    staging = prepare_clean_staging()
    texture_dir = staging / MESH_TEXTURE_DIR.name
    print("importing textured mesh sources", flush=True)
    center_city = await building_meshes(snapshots[SOURCES.downtown_meshes.filename], texture_dir)
    legacy_downtown = legacy_downtown_meshes(
        snapshots[SOURCES.legacy_downtown_meshes.filename], texture_dir
    )
    stadium = stadium_meshes(snapshots[SOURCES.stadium_meshes.filename], texture_dir)
    meshes = merge_mesh_sources(
        center_city,
        legacy_downtown,
        stadium,
    )
    print(f"accepted {len(meshes):,} textured building meshes", flush=True)
    prune_mesh_textures(meshes, texture_dir)
    texture_sha256, texture_bytes = texture_digest(meshes, texture_dir)
    print(f"verified {texture_bytes / 1_000_000:.1f} MB of texture atlases", flush=True)
    print("measuring photographed building coverage", flush=True)
    texture_coverage = texture_coverage_report(
        packed_buildings, meshes, ROOT / "static" / "neighborhoods.json"
    )
    write_texture_coverage(staging / TEXTURE_COVERAGE_JSON.name, texture_coverage)
    citywide_coverage = texture_coverage["citywide"]
    if not isinstance(citywide_coverage, dict):
        raise ValueError("citywide texture coverage is invalid")
    print(
        f"photographed facade coverage: "
        f"{citywide_coverage['photographed_building_percent']}% of buildings",
        flush=True,
    )

    staged_world = staging / WORLD_BIN.name
    staged_metadata = staging / METADATA_JSON.name
    terrain = build_if_evidence_present(staging / TERRAIN_ARTIFACT_NAME, bounds)
    if terrain is not None:
        print(
            f"built terrain relief {terrain.grid.width}x{terrain.grid.height} "
            f"({terrain.direct_cells:,} direct cells)",
            flush=True,
        )
    pack_world(
        staged_world,
        packed_buildings,
        parts,
        meshes,
        packed_city,
        water,
        parks,
        trees,
        transport,
        bounds,
        texture_sha256,
    )
    write_metadata(
        staged_metadata,
        staged_world,
        snapshots,
        bounds,
        packed_buildings,
        parts,
        meshes,
        packed_city,
        water,
        parks,
        trees,
        transport,
        texture_sha256,
        texture_bytes,
        texture_coverage,
        len(height_evidence) if height_evidence is not None else 0,
        terrain,
    )
    (staging / "streets.bin").unlink(missing_ok=True)
    publish_clean(staging)
    print(f"wrote {WORLD_BIN} ({WORLD_BIN.stat().st_size / 1_000_000:.1f} MB)")


def main() -> None:
    parser = argparse.ArgumentParser(description="Build IsoPhilly's compact world data")
    parser.add_argument(
        "--refresh",
        action="store_true",
        help="refresh mutable public sources instead of using verified cached snapshots",
    )
    arguments = parser.parse_args()
    asyncio.run(main_async(refresh=arguments.refresh))


if __name__ == "__main__":
    main()
