from __future__ import annotations

import re
import struct
from collections.abc import Iterable
from math import isfinite
from pathlib import Path, PurePosixPath
from tempfile import TemporaryDirectory
from zipfile import ZipFile

import pyogrio
from shapely.geometry import MultiPoint, Polygon

from .models import BuildingMesh, MeshFace, Point3D, Ring, Snapshot

_MODEL_NAME = re.compile(r"PHIL(\d+)\.flt", re.IGNORECASE)
_COLLECTION_TYPES = frozenset({4, 5, 6, 7, 15, 16})
_SURFACE_TYPES = frozenset({3, 17})
_SOURCE_X_RANGE = (750_000.0, 900_000.0)
_SOURCE_Y_RANGE = (0.0, 200_000.0)


class MeshParseError(ValueError):
    pass


class _Cursor:
    __slots__ = ("data", "offset")

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.offset = 0

    def take(self, size: int) -> bytes:
        end = self.offset + size
        if size < 0 or end > len(self.data):
            raise MeshParseError("multipatch WKB ended unexpectedly")
        value = self.data[self.offset : end]
        self.offset = end
        return value

    def byte(self) -> int:
        return self.take(1)[0]

    def uint32(self, endian: str) -> int:
        return struct.unpack(f"{endian}I", self.take(4))[0]

    def point3d(self, endian: str) -> Point3D:
        return struct.unpack(f"{endian}ddd", self.take(24))


def _normalize_face(points: Iterable[Point3D], z_min: float) -> MeshFace | None:
    normalized: list[Point3D] = []
    for x, y, z in points:
        if not all(isfinite(value) for value in (x, y, z)):
            raise MeshParseError("multipatch contains a non-finite coordinate")
        point = (x, y, max(0.0, z - z_min))
        if not normalized or point != normalized[-1]:
            normalized.append(point)
    if len(normalized) > 1 and normalized[0] == normalized[-1]:
        normalized.pop()
    if len(normalized) < 3:
        return None

    origin = normalized[0]
    area_twice = 0.0
    for left, right in zip(normalized[1:-1], normalized[2:], strict=True):
        ux, uy, uz = (left[index] - origin[index] for index in range(3))
        vx, vy, vz = (right[index] - origin[index] for index in range(3))
        cross_x = uy * vz - uz * vy
        cross_y = uz * vx - ux * vz
        cross_z = ux * vy - uy * vx
        area_twice += (cross_x * cross_x + cross_y * cross_y + cross_z * cross_z) ** 0.5
    if area_twice < 0.002:
        return None
    return MeshFace(tuple(normalized))


def _parse_geometry(cursor: _Cursor, z_min: float, faces: list[MeshFace]) -> None:
    byte_order = cursor.byte()
    if byte_order not in {0, 1}:
        raise MeshParseError("multipatch WKB has an invalid byte order")
    endian = "<" if byte_order == 1 else ">"
    geometry_type = cursor.uint32(endian)
    if not 1000 <= geometry_type < 2000:
        raise MeshParseError(f"multipatch geometry is not three-dimensional: {geometry_type}")
    base_type = geometry_type - 1000

    if base_type in _COLLECTION_TYPES:
        for _ in range(cursor.uint32(endian)):
            _parse_geometry(cursor, z_min, faces)
        return
    if base_type not in _SURFACE_TYPES:
        raise MeshParseError(f"unsupported multipatch geometry type: {base_type}")

    ring_count = cursor.uint32(endian)
    for ring_index in range(ring_count):
        points = tuple(cursor.point3d(endian) for _ in range(cursor.uint32(endian)))
        if ring_index == 0:
            face = _normalize_face(points, z_min)
            if face is not None:
                faces.append(face)


def parse_multipatch(data: bytes, z_min: float) -> tuple[MeshFace, ...]:
    if not isfinite(z_min):
        raise MeshParseError("multipatch minimum elevation must be finite")
    cursor = _Cursor(data)
    faces: list[MeshFace] = []
    _parse_geometry(cursor, z_min, faces)
    if cursor.offset != len(data):
        raise MeshParseError("multipatch WKB contains trailing bytes")
    if not faces:
        raise MeshParseError("multipatch contains no usable faces")
    return tuple(faces)


def _source_id(value: object) -> int:
    if not isinstance(value, str):
        raise MeshParseError("multipatch model name must be text")
    match = _MODEL_NAME.fullmatch(value)
    if match is None:
        raise MeshParseError(f"unexpected multipatch model name: {value}")
    return int(match.group(1))


def _elevation(value: object, label: str) -> float:
    if not isinstance(value, int | float) or isinstance(value, bool):
        raise MeshParseError(f"multipatch {label} must be numeric")
    result = float(value)
    if not isfinite(result):
        raise MeshParseError(f"multipatch {label} must be finite")
    return result


def _wkb(value: object) -> bytes:
    if not isinstance(value, bytes):
        raise MeshParseError("multipatch geometry must be WKB bytes")
    return value


def _footprint(faces: tuple[MeshFace, ...]) -> Ring:
    hull = MultiPoint([(x, y) for face in faces for x, y, _ in face.points]).convex_hull
    if not isinstance(hull, Polygon) or hull.is_empty:
        raise MeshParseError("multipatch footprint is not a polygon")
    points = tuple((float(x), float(y)) for x, y in hull.exterior.coords[:-1])
    if len(points) < 3:
        raise MeshParseError("multipatch footprint has fewer than three points")
    return points


def validate_source_space(source_id: int, faces: tuple[MeshFace, ...]) -> None:
    for face in faces:
        for x, y, _ in face.points:
            if not _SOURCE_X_RANGE[0] <= x <= _SOURCE_X_RANGE[1]:
                raise MeshParseError(f"multipatch {source_id} X coordinate is not in metres")
            if not _SOURCE_Y_RANGE[0] <= y <= _SOURCE_Y_RANGE[1]:
                raise MeshParseError(f"multipatch {source_id} Y coordinate is not in metres")


def _extract(snapshot: Snapshot, destination: Path) -> Path:
    with ZipFile(snapshot.path) as archive:
        for member in archive.infolist():
            path = PurePosixPath(member.filename)
            if path.is_absolute() or ".." in path.parts:
                raise MeshParseError("multipatch archive contains an unsafe path")
        archive.extractall(destination)
    path = destination / "Philadelphia2015_scene.gdb" / "Philadelphia2015_scene.gdb"
    if not path.is_dir():
        raise MeshParseError("multipatch archive does not contain the expected FileGDB")
    return path


def building_meshes(snapshot: Snapshot) -> list[BuildingMesh]:
    with TemporaryDirectory(prefix="geo-philly-mesh-") as directory:
        geodatabase = _extract(snapshot, Path(directory))
        _, table = pyogrio.read_arrow(geodatabase, layer="Buildings")

    names: list[object] = table.column("Name").to_pylist()
    minimums: list[object] = table.column("Z_Min").to_pylist()
    maximums: list[object] = table.column("Z_Max").to_pylist()
    geometries: list[object] = table.column("Shape").to_pylist()
    meshes: list[BuildingMesh] = []
    for raw_name, raw_minimum, raw_maximum, raw_geometry in zip(
        names, minimums, maximums, geometries, strict=True
    ):
        source_id = _source_id(raw_name)
        z_min = _elevation(raw_minimum, "minimum elevation")
        z_max = _elevation(raw_maximum, "maximum elevation")
        if z_max <= z_min:
            raise MeshParseError(f"multipatch {source_id} has an invalid elevation range")
        faces = parse_multipatch(_wkb(raw_geometry), z_min)
        validate_source_space(source_id, faces)
        height = max(z for face in faces for _, _, z in face.points)
        if not 0.0 < height <= 400.0:
            raise MeshParseError(f"multipatch {source_id} has an invalid height: {height}")
        meshes.append(
            BuildingMesh(
                source_id=source_id,
                height=height,
                footprint=_footprint(faces),
                faces=faces,
            )
        )
    meshes.sort(key=lambda mesh: mesh.source_id)
    if not meshes or len({mesh.source_id for mesh in meshes}) != len(meshes):
        raise MeshParseError("multipatch source IDs must be nonempty and unique")
    return meshes
