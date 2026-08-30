from __future__ import annotations

import asyncio
import hashlib
import json
import os
import struct
import tempfile
from collections.abc import Mapping
from pathlib import Path
from typing import Any

import httpx
from pyproj import Transformer
from shapely import STRtree
from shapely.geometry import MultiPoint, Polygon

from .config import MESH_TEXTURE_DIR, RAW_DIR
from .download import RETRY_DELAYS_SECONDS, RETRYABLE_HTTP_STATUS, USER_AGENT
from .models import BuildingMesh, MeshFace, Point, Point3D, Ring, Snapshot

_MAX_DOWNLOADS = 12
_EXPECTED_LAYER_VERSION = "1.7"
_EXPECTED_LEAF_COUNT = 367
_GEOMETRY_HEADER_BYTES = 8


class MeshParseError(ValueError):
    pass


def _object(value: object, label: str) -> Mapping[str, Any]:
    if not isinstance(value, dict):
        raise MeshParseError(f"{label} must be an object")
    return value


def _integer(value: object, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise MeshParseError(f"{label} must be an integer")
    return value


def _center(node: Mapping[str, Any]) -> Point3D:
    obb = _object(node.get("obb"), "I3S node OBB")
    center = obb.get("center")
    if not isinstance(center, list) or len(center) != 3:
        raise MeshParseError("I3S node OBB center must have three values")
    if not all(isinstance(value, int | float) and not isinstance(value, bool) for value in center):
        raise MeshParseError("I3S node OBB center must be numeric")
    return float(center[0]), float(center[1]), float(center[2])


def _resource_id(node: Mapping[str, Any]) -> int:
    mesh = _object(node.get("mesh"), "I3S node mesh")
    geometry = _object(mesh.get("geometry"), "I3S node geometry")
    material = _object(mesh.get("material"), "I3S node material")
    geometry_id = _integer(geometry.get("resource"), "I3S geometry resource")
    material_id = _integer(material.get("resource"), "I3S material resource")
    if geometry_id != material_id:
        raise MeshParseError("I3S geometry and material resources must match")
    return geometry_id


def _leaf_nodes(pages: list[Mapping[str, Any]]) -> list[Mapping[str, Any]]:
    nodes: list[Mapping[str, Any]] = []
    for page in pages:
        raw_nodes = page.get("nodes")
        if not isinstance(raw_nodes, list):
            raise MeshParseError("I3S node page has no nodes")
        for raw_node in raw_nodes:
            node = _object(raw_node, "I3S node")
            if node.get("mesh") is not None and node.get("children") is None:
                nodes.append(node)
    nodes.sort(key=lambda node: _integer(node.get("index"), "I3S node index"))
    if len(nodes) != _EXPECTED_LEAF_COUNT:
        raise MeshParseError(
            f"I3S scene has {len(nodes)} detailed nodes; expected {_EXPECTED_LEAF_COUNT}"
        )
    expected = list(range(1, _EXPECTED_LEAF_COUNT + 1))
    actual = [_integer(node.get("index"), "I3S node index") for node in nodes]
    if actual != expected:
        raise MeshParseError("I3S detailed node indexes are not contiguous")
    return nodes


def parse_geometry(data: bytes, node: Mapping[str, Any]) -> BuildingMesh:
    if len(data) < _GEOMETRY_HEADER_BYTES:
        raise MeshParseError("I3S geometry is truncated")
    vertex_count, feature_count = struct.unpack_from("<II", data)
    if feature_count == 0 or vertex_count == 0 or vertex_count % 3 != 0:
        raise MeshParseError("I3S geometry must contain triangulated features")
    mesh = _object(node.get("mesh"), "I3S node mesh")
    geometry = _object(mesh.get("geometry"), "I3S node geometry")
    expected_vertices = _integer(geometry.get("vertexCount"), "I3S vertex count")
    if vertex_count != expected_vertices:
        raise MeshParseError(
            f"I3S geometry has {vertex_count} vertices; expected {expected_vertices}"
        )

    position_bytes = vertex_count * 3 * 4
    normal_bytes = vertex_count * 3 * 4
    uv_bytes = vertex_count * 2 * 4
    color_bytes = vertex_count * 4
    region_bytes = vertex_count * 4 * 2
    feature_bytes = feature_count * (8 + 8)
    expected_bytes = (
        _GEOMETRY_HEADER_BYTES
        + position_bytes
        + normal_bytes
        + uv_bytes
        + color_bytes
        + region_bytes
        + feature_bytes
    )
    if len(data) != expected_bytes:
        raise MeshParseError(f"I3S geometry has {len(data)} bytes; expected {expected_bytes}")

    offset = _GEOMETRY_HEADER_BYTES
    positions = struct.unpack_from(f"<{vertex_count * 3}f", data, offset)
    offset += position_bytes + normal_bytes
    raw_uvs = struct.unpack_from(f"<{vertex_count * 2}f", data, offset)
    offset += uv_bytes + color_bytes
    raw_regions = struct.unpack_from(f"<{vertex_count * 4}H", data, offset)

    center_lon, center_lat, center_z = _center(node)
    longitudes = [center_lon + positions[index] for index in range(0, len(positions), 3)]
    latitudes = [center_lat + positions[index] for index in range(1, len(positions), 3)]
    transformer = Transformer.from_crs(4326, 32129, always_xy=True)
    xs, ys = transformer.transform(longitudes, latitudes)
    absolute_zs = [center_z + positions[index] for index in range(2, len(positions), 3)]
    minimum_z = min(absolute_zs)
    points: list[Point3D] = [
        (float(x), float(y), float(z - minimum_z))
        for x, y, z in zip(xs, ys, absolute_zs, strict=True)
    ]

    uvs: list[Point] = []
    for index in range(vertex_count):
        u = raw_uvs[index * 2]
        v = raw_uvs[index * 2 + 1]
        region = raw_regions[index * 4 : index * 4 + 4]
        u = (float(region[0]) + u * float(region[2] - region[0])) / 65_535.0
        v = (float(region[1]) + v * float(region[3] - region[1])) / 65_535.0
        uvs.append((u, v))

    faces = tuple(
        MeshFace(tuple(points[index : index + 3]), tuple(uvs[index : index + 3]))
        for index in range(0, vertex_count, 3)
    )
    hull = MultiPoint([(x, y) for x, y, _ in points]).convex_hull
    if not isinstance(hull, Polygon) or hull.is_empty:
        raise MeshParseError("I3S mesh footprint is not a polygon")
    footprint: Ring = tuple((float(x), float(y)) for x, y in hull.exterior.coords[:-1])
    height = max(z for _, _, z in points)
    if not 0.0 < height <= 400.0:
        raise MeshParseError(f"I3S mesh has an invalid height: {height}")
    return BuildingMesh(_resource_id(node), height, footprint, faces)


async def _get(client: httpx.AsyncClient, url: str) -> bytes:
    last_error: Exception | None = None
    for attempt in range(len(RETRY_DELAYS_SECONDS) + 1):
        try:
            response = await client.get(url)
            response.raise_for_status()
            if not response.content:
                raise MeshParseError(f"I3S returned an empty resource: {url}")
            return response.content
        except httpx.HTTPStatusError as error:
            if error.response.status_code not in RETRYABLE_HTTP_STATUS:
                raise
            last_error = error
        except httpx.TransportError as error:
            last_error = error
        if attempt < len(RETRY_DELAYS_SECONDS):
            await asyncio.sleep(RETRY_DELAYS_SECONDS[attempt])
    raise MeshParseError(f"I3S request failed: {url}: {last_error}")


async def _cached_resource(client: httpx.AsyncClient, url: str, path: Path) -> bytes:
    try:
        cached = path.read_bytes()
    except FileNotFoundError:
        cached = b""
    if cached:
        return cached
    data = await _get(client, url)
    _write_atomic(path, data)
    return data


def _write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as file:
            file.write(data)
        temporary.replace(path)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise


async def _node_pages(
    client: httpx.AsyncClient, base_url: str, cache_dir: Path
) -> list[Mapping[str, Any]]:
    pages: list[Mapping[str, Any]] = []
    for page_id in range(64):
        url = f"{base_url}/layers/0/nodepages/{page_id}"
        data = await _cached_resource(client, url, cache_dir / "nodepages" / f"{page_id}.json")
        try:
            payload = json.loads(data)
        except json.JSONDecodeError as error:
            raise MeshParseError(f"I3S node page {page_id} is invalid JSON") from error
        if not isinstance(payload, dict) or "nodes" not in payload:
            break
        pages.append(_object(payload, "I3S node page"))
        nodes = payload.get("nodes")
        if isinstance(nodes, list) and len(nodes) < 64:
            break
    if not pages:
        raise MeshParseError("I3S scene has no node pages")
    return pages


async def _load_node(
    client: httpx.AsyncClient,
    semaphore: asyncio.Semaphore,
    base_url: str,
    cache_dir: Path,
    texture_dir: Path,
    node: Mapping[str, Any],
) -> BuildingMesh:
    resource_id = _resource_id(node)
    geometry_path = cache_dir / "geometry" / f"{resource_id}.bin"
    texture_path = texture_dir / f"{resource_id}.jpg"
    async with semaphore:
        async with asyncio.TaskGroup() as group:
            geometry_task = group.create_task(
                _cached_resource(
                    client,
                    f"{base_url}/layers/0/nodes/{resource_id}/geometries/0",
                    geometry_path,
                )
            )
            texture_task = group.create_task(
                _cached_resource(
                    client,
                    f"{base_url}/layers/0/nodes/{resource_id}/textures/0",
                    texture_path,
                )
            )
        geometry = geometry_task.result()
        texture = texture_task.result()
    if not texture.startswith(b"\xff\xd8"):
        texture_path.unlink(missing_ok=True)
        texture = await _cached_resource(
            client,
            f"{base_url}/layers/0/nodes/{resource_id}/textures/0",
            texture_path,
        )
    if not texture.startswith(b"\xff\xd8") or not texture.endswith(b"\xff\xd9"):
        raise MeshParseError(f"I3S texture {resource_id} is not a complete JPEG")
    return parse_geometry(geometry, node)


def texture_digest(
    meshes: list[BuildingMesh], texture_dir: Path = MESH_TEXTURE_DIR
) -> tuple[bytes, int]:
    digest = hashlib.sha256()
    size = 0
    ordered = sorted(meshes, key=lambda mesh: mesh.texture_id)
    texture_ids = [mesh.texture_id for mesh in ordered]
    if len(texture_ids) != len(set(texture_ids)):
        raise MeshParseError("building mesh texture IDs must be unique")
    for mesh in ordered:
        path = texture_dir / f"{mesh.texture_id}.jpg"
        data = path.read_bytes()
        digest.update(struct.pack("<I", mesh.texture_id))
        digest.update(data)
        size += len(data)
    return digest.digest(), size


def merge_mesh_sources(
    *sources: tuple[BuildingMesh, ...],
) -> list[BuildingMesh]:
    """Merge highest-to-lowest priority sources without overlapping meshes."""
    selected: list[BuildingMesh] = []
    coverage: list[Polygon] = []
    tree: STRtree | None = None
    for source in sources:
        accepted: list[BuildingMesh] = []
        accepted_footprints: list[Polygon] = []
        for mesh in source:
            footprint = Polygon(mesh.footprint)
            if footprint.is_empty or not footprint.is_valid or footprint.area <= 0.0:
                raise MeshParseError(f"building mesh {mesh.texture_id} has an invalid footprint")
            covered = False
            if tree is not None:
                for raw_index in tree.query(footprint, predicate="intersects"):
                    higher = coverage[int(raw_index)]
                    overlap = higher.intersection(footprint).area / footprint.area
                    if higher.covers(footprint.representative_point()) or overlap >= 0.25:
                        covered = True
                        break
            if not covered:
                accepted.append(mesh)
                accepted_footprints.append(footprint)
        selected.extend(accepted)
        coverage.extend(accepted_footprints)
        tree = STRtree(coverage)
    return sorted(selected, key=lambda mesh: mesh.texture_id)


def prune_mesh_textures(meshes: list[BuildingMesh], texture_dir: Path = MESH_TEXTURE_DIR) -> None:
    expected = {f"{mesh.texture_id}.jpg" for mesh in meshes}
    for path in texture_dir.glob("*.jpg"):
        if path.name not in expected:
            path.unlink()


async def building_meshes(
    snapshot: Snapshot, texture_dir: Path = MESH_TEXTURE_DIR
) -> tuple[BuildingMesh, ...]:
    metadata = _object(json.loads(snapshot.path.read_text()), "I3S service metadata")
    if metadata.get("serviceVersion") != _EXPECTED_LAYER_VERSION:
        raise MeshParseError(
            f"I3S service version is {metadata.get('serviceVersion')!r}; "
            f"expected {_EXPECTED_LAYER_VERSION!r}"
        )
    layers = metadata.get("layers")
    if not isinstance(layers, list) or len(layers) != 1:
        raise MeshParseError("I3S service must contain one layer")
    layer = _object(layers[0], "I3S layer")
    if layer.get("layerType") != "3DObject":
        raise MeshParseError("I3S layer must contain 3D objects")
    base_url = snapshot.url.partition("?")[0]
    cache_dir = RAW_DIR / f"center-city-i3s-{layer.get('version', 'unknown')}"
    texture_dir.mkdir(parents=True, exist_ok=True)
    timeout = httpx.Timeout(120.0)
    limits = httpx.Limits(max_connections=_MAX_DOWNLOADS, max_keepalive_connections=_MAX_DOWNLOADS)
    async with httpx.AsyncClient(
        headers={"User-Agent": USER_AGENT}, follow_redirects=True, timeout=timeout, limits=limits
    ) as client:
        nodes = _leaf_nodes(await _node_pages(client, base_url, cache_dir))
        semaphore = asyncio.Semaphore(_MAX_DOWNLOADS)
        tasks: list[asyncio.Task[BuildingMesh]] = []
        async with asyncio.TaskGroup() as group:
            for node in nodes:
                tasks.append(
                    group.create_task(
                        _load_node(client, semaphore, base_url, cache_dir, texture_dir, node)
                    )
                )
        meshes = [task.result() for task in tasks]
    meshes.sort(key=lambda mesh: mesh.texture_id)
    return tuple(meshes)
