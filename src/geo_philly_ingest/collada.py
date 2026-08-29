from __future__ import annotations

import math
import os
import posixpath
import re
import tempfile
import xml.etree.ElementTree as ET
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from zipfile import BadZipFile, ZipFile

from pyproj import Transformer
from shapely.geometry import MultiPoint, Polygon

from .config import MESH_TEXTURE_DIR
from .models import BuildingMesh, MeshFace, Point, Point3D, Ring, Snapshot

LEGACY_DOWNTOWN_TEXTURE_ID_OFFSET = 1_000_000
STADIUM_TEXTURE_ID_OFFSET = 2_000_000
EARTH_RADIUS_METERS = 6_378_137.0
REGION_TOLERANCE_DEGREES = 0.000_05
MINIMUM_FOOTPRINT_BUFFER_METERS = 0.25
# Six source components form the Spectrum, demolished after this 2008 survey.
# The current aerial layer correctly shows its replacement on this site.
EXCLUDED_MODEL_NAMES = frozenset(f"ph_stadium{number:04d}" for number in range(773, 779))


class ColladaParseError(ValueError):
    pass


@dataclass(frozen=True, slots=True)
class ColladaDataset:
    label: str
    archive_member: str
    model_prefix: str
    expected_model_count: int
    texture_id_offset: int
    excluded_model_names: frozenset[str] = frozenset()
    published_bounds: tuple[float, float, float, float] | None = None

    def model_members(self, members: list[str]) -> tuple[str, ...]:
        pattern = re.compile(rf"kml/r0/{re.escape(self.model_prefix)}\d+\.kml")
        return tuple(sorted(member for member in members if pattern.fullmatch(member)))

    def identifier(self, model_name: str) -> int:
        suffix = model_name.removeprefix(self.model_prefix)
        if not suffix.isdecimal() or f"{self.model_prefix}{suffix}" != model_name:
            raise ColladaParseError(f"{self.label} model has an invalid name: {model_name!r}")
        return self.texture_id_offset + int(suffix)


LEGACY_DOWNTOWN = ColladaDataset(
    label="legacy downtown",
    archive_member="ph_downtown_kml.zip",
    model_prefix="philly_",
    expected_model_count=2_689,
    texture_id_offset=LEGACY_DOWNTOWN_TEXTURE_ID_OFFSET,
    published_bounds=(-75.1904191883, 39.9401820483, -75.1335632290, 39.9672535290),
)
STADIUM = ColladaDataset(
    label="stadium",
    archive_member="ph_stadium_kml.zip",
    model_prefix="ph_stadium",
    expected_model_count=814,
    texture_id_offset=STADIUM_TEXTURE_ID_OFFSET,
    excluded_model_names=EXCLUDED_MODEL_NAMES,
)


@dataclass(frozen=True, slots=True)
class Placement:
    name: str
    longitude: float
    latitude: float
    altitude: float
    dae_member: str


@dataclass(frozen=True, slots=True)
class Region:
    west: float
    south: float
    east: float
    north: float

    def contains(self, points: tuple[Point3D, ...]) -> bool:
        longitudes = [point[0] for point in points]
        latitudes = [point[1] for point in points]
        return (
            min(longitudes) >= self.west - REGION_TOLERANCE_DEGREES
            and max(longitudes) <= self.east + REGION_TOLERANCE_DEGREES
            and min(latitudes) >= self.south - REGION_TOLERANCE_DEGREES
            and max(latitudes) <= self.north + REGION_TOLERANCE_DEGREES
        )


@dataclass(frozen=True, slots=True)
class FloatSource:
    values: tuple[float, ...]
    stride: int

    def value(self, index: int) -> tuple[float, ...]:
        start = index * self.stride
        result = self.values[start : start + self.stride]
        if len(result) != self.stride:
            raise ColladaParseError(f"COLLADA source index {index} is out of range")
        return result


@dataclass(frozen=True, slots=True)
class LocalFace:
    points: tuple[Point3D, Point3D, Point3D]
    uvs: tuple[Point, Point, Point]


@dataclass(frozen=True, slots=True)
class ParsedModel:
    placement: Placement
    region: Region
    texture_member: str
    faces: tuple[LocalFace, ...]


def _local_name(element: ET.Element) -> str:
    return element.tag.rsplit("}", 1)[-1]


def _children(element: ET.Element, name: str) -> tuple[ET.Element, ...]:
    return tuple(child for child in element if _local_name(child) == name)


def _descendants(element: ET.Element, name: str) -> tuple[ET.Element, ...]:
    return tuple(child for child in element.iter() if _local_name(child) == name)


def _one(items: tuple[ET.Element, ...], label: str) -> ET.Element:
    if len(items) != 1:
        raise ColladaParseError(f"expected one {label}; found {len(items)}")
    return items[0]


def _text(element: ET.Element, label: str) -> str:
    if element.text is None or not element.text.strip():
        raise ColladaParseError(f"{label} is empty")
    return element.text.strip()


def _child_text(element: ET.Element, name: str) -> str:
    return _text(_one(_children(element, name), name), name)


def _finite_float(element: ET.Element, name: str) -> float:
    try:
        value = float(_child_text(element, name))
    except ValueError as error:
        raise ColladaParseError(f"{name} must be numeric") from error
    if not math.isfinite(value):
        raise ColladaParseError(f"{name} must be finite")
    return value


def _member_relative_to(member: str, reference: str) -> str:
    if not reference or reference.startswith(("/", "\\")):
        raise ColladaParseError(f"invalid archive reference: {reference!r}")
    normalized = posixpath.normpath(posixpath.join(posixpath.dirname(member), reference))
    if normalized == ".." or normalized.startswith("../"):
        raise ColladaParseError(f"archive reference escapes its directory: {reference!r}")
    return PurePosixPath(normalized).as_posix()


def _xml(archive: ZipFile, member: str) -> ET.Element:
    try:
        with archive.open(member) as source:
            return ET.parse(source).getroot()
    except KeyError as error:
        raise ColladaParseError(f"archive member is missing: {member}") from error
    except ET.ParseError as error:
        raise ColladaParseError(f"invalid XML in {member}: {error}") from error


@contextmanager
def _model_archive(source: Path, dataset: ColladaDataset) -> Iterator[ZipFile]:
    try:
        outer = ZipFile(source)
    except (BadZipFile, OSError) as error:
        raise ColladaParseError(f"cannot open {dataset.label} archive: {source}") from error
    with outer:
        if dataset.model_members(outer.namelist()):
            yield outer
            return
        inner_members = tuple(
            member
            for member in outer.namelist()
            if PurePosixPath(member).name == dataset.archive_member
        )
        inner_member = _text_member(inner_members, dataset.archive_member)
        with outer.open(inner_member) as inner:
            try:
                with ZipFile(inner) as models:
                    yield models
            except BadZipFile as error:
                raise ColladaParseError(
                    f"nested {dataset.label} model archive is invalid"
                ) from error


def _text_member(members: tuple[str, ...], label: str) -> str:
    if len(members) != 1:
        raise ColladaParseError(f"expected one {label}; found {len(members)}")
    return members[0]


def _parse_region(archive: ZipFile, name: str, dataset: ColladaDataset) -> Region:
    if dataset.published_bounds is not None:
        return Region(*dataset.published_bounds)
    root = _xml(archive, f"kml/{name}.kml")
    boxes = _descendants(root, "LatLonAltBox")
    if not boxes:
        raise ColladaParseError(f"{name} has no published region")
    regions = tuple(
        Region(
            west=_finite_float(box, "west"),
            south=_finite_float(box, "south"),
            east=_finite_float(box, "east"),
            north=_finite_float(box, "north"),
        )
        for box in boxes
    )
    if any(candidate != regions[0] for candidate in regions[1:]):
        raise ColladaParseError(f"{name} has inconsistent LOD regions")
    return regions[0]


def _parse_placement(archive: ZipFile, kml_member: str) -> Placement:
    root = _xml(archive, kml_member)
    placemark = _one(_descendants(root, "Placemark"), "Placemark")
    model = _one(_children(placemark, "Model"), "Model")
    name = _child_text(placemark, "name")
    if _child_text(model, "altitudeMode") != "clampToGround":
        raise ColladaParseError(f"{name} does not clamp to ground")
    orientation = _one(_children(model, "Orientation"), "Orientation")
    if any(_finite_float(orientation, axis) != 0.0 for axis in ("heading", "tilt", "roll")):
        raise ColladaParseError(f"{name} has an unsupported orientation")
    scale = _one(_children(model, "Scale"), "Scale")
    if any(_finite_float(scale, axis) != 1.0 for axis in ("x", "y", "z")):
        raise ColladaParseError(f"{name} has an unsupported scale")
    location = _one(_children(model, "Location"), "Location")
    link = _one(_children(model, "Link"), "Link")
    dae_member = _member_relative_to(kml_member, _child_text(link, "href"))
    if PurePosixPath(dae_member).stem != name:
        raise ColladaParseError(f"{name} points to unexpected mesh {dae_member}")
    return Placement(
        name=name,
        longitude=_finite_float(location, "longitude"),
        latitude=_finite_float(location, "latitude"),
        altitude=_finite_float(location, "altitude"),
        dae_member=dae_member,
    )


def _parse_sources(mesh: ET.Element) -> dict[str, FloatSource]:
    result: dict[str, FloatSource] = {}
    for source in _children(mesh, "source"):
        source_id = source.get("id")
        if not source_id:
            raise ColladaParseError("COLLADA source has no id")
        array = _one(_children(source, "float_array"), f"float array for {source_id}")
        try:
            values = tuple(float(value) for value in _text(array, source_id).split())
        except ValueError as error:
            raise ColladaParseError(f"{source_id} contains invalid floats") from error
        accessor = _one(_descendants(source, "accessor"), f"accessor for {source_id}")
        count = int(accessor.get("count", "-1"))
        stride = int(accessor.get("stride", "-1"))
        if (
            int(array.get("count", "-1")) != len(values)
            or count * stride != len(values)
            or stride <= 0
            or not all(math.isfinite(value) for value in values)
        ):
            raise ColladaParseError(f"{source_id} has invalid dimensions")
        result[source_id] = FloatSource(values, stride)
    return result


def _source_id(reference: str | None, label: str) -> str:
    if not reference or not reference.startswith("#") or len(reference) == 1:
        raise ColladaParseError(f"{label} has an invalid source")
    return reference[1:]


def _vertices_sources(mesh: ET.Element) -> dict[str, str]:
    result: dict[str, str] = {}
    for vertices in _children(mesh, "vertices"):
        vertices_id = vertices.get("id")
        if not vertices_id:
            raise ColladaParseError("COLLADA vertices has no id")
        positions = tuple(
            item for item in _children(vertices, "input") if item.get("semantic") == "POSITION"
        )
        position = _one(positions, f"POSITION input for {vertices_id}")
        result[vertices_id] = _source_id(position.get("source"), "POSITION input")
    return result


def _appearance(root: ET.Element, dae_member: str) -> tuple[str, frozenset[str]]:
    images = _descendants(root, "image")
    image = _one(images, "COLLADA image")
    texture_member = _member_relative_to(dae_member, _child_text(image, "init_from"))
    textured_effects = frozenset(
        effect.get("id", "")
        for effect in _descendants(root, "effect")
        if _descendants(effect, "texture")
    )
    textured_materials = frozenset(
        material.get("id", "")
        for material in _descendants(root, "material")
        if _source_id(
            _one(_children(material, "instance_effect"), "instance_effect").get("url"),
            "instance_effect",
        )
        in textured_effects
    )
    bindings = {
        instance.get("symbol", ""): _source_id(instance.get("target"), "instance_material")
        for instance in _descendants(root, "instance_material")
    }
    symbols = frozenset(
        symbol for symbol, material_id in bindings.items() if material_id in textured_materials
    )
    if not symbols:
        raise ColladaParseError("COLLADA model has no textured material binding")
    return texture_member, symbols


def _parse_triangle_faces(
    triangles: ET.Element,
    sources: dict[str, FloatSource],
    vertices_sources: dict[str, str],
) -> tuple[LocalFace, ...]:
    inputs = _children(triangles, "input")
    offsets = tuple(int(item.get("offset", "-1")) for item in inputs)
    if not offsets or min(offsets) < 0:
        raise ColladaParseError("triangle input has an invalid offset")
    input_stride = max(offsets) + 1
    vertex_input = _one(
        tuple(item for item in inputs if item.get("semantic") == "VERTEX"), "VERTEX input"
    )
    uv_input = _one(
        tuple(item for item in inputs if item.get("semantic") == "TEXCOORD"),
        "TEXCOORD input",
    )
    vertices_id = _source_id(vertex_input.get("source"), "VERTEX input")
    try:
        positions = sources[vertices_sources[vertices_id]]
        uvs = sources[_source_id(uv_input.get("source"), "TEXCOORD input")]
    except KeyError as error:
        raise ColladaParseError("triangle input refers to an unknown source") from error
    if positions.stride != 3 or uvs.stride != 2:
        raise ColladaParseError("COLLADA position or UV source has an invalid stride")
    vertex_offset = int(vertex_input.get("offset", "-1"))
    uv_offset = int(uv_input.get("offset", "-1"))
    indices_element = _one(_children(triangles, "p"), "triangle indices")
    try:
        indices = tuple(int(value) for value in _text(indices_element, "indices").split())
    except ValueError as error:
        raise ColladaParseError("triangle indices must be integers") from error
    triangle_count = int(triangles.get("count", "-1"))
    if triangle_count <= 0 or len(indices) != triangle_count * 3 * input_stride:
        raise ColladaParseError("triangle count does not match its indices")

    points: list[Point3D] = []
    texture_points: list[Point] = []
    for start in range(0, len(indices), input_stride):
        position = positions.value(indices[start + vertex_offset])
        uv = uvs.value(indices[start + uv_offset])
        points.append((position[0], position[1], position[2]))
        # COLLADA uses bottom-left UVs; image rows and I3S use top-left UVs.
        texture_points.append((uv[0], 1.0 - uv[1]))
    return tuple(
        LocalFace(
            (points[index], points[index + 1], points[index + 2]),
            (texture_points[index], texture_points[index + 1], texture_points[index + 2]),
        )
        for index in range(0, len(points), 3)
    )


def _parse_model(archive: ZipFile, kml_member: str, dataset: ColladaDataset) -> ParsedModel:
    placement = _parse_placement(archive, kml_member)
    root = _xml(archive, placement.dae_member)
    if _child_text(_one(_descendants(root, "asset"), "asset"), "up_axis") != "Z_UP":
        raise ColladaParseError(f"{placement.name} is not Z-up")
    unit = _one(_descendants(root, "unit"), "unit")
    if float(unit.get("meter", "nan")) != 1.0:
        raise ColladaParseError(f"{placement.name} is not measured in metres")
    geometry = _one(_descendants(root, "geometry"), "geometry")
    mesh = _one(_children(geometry, "mesh"), "mesh")
    sources = _parse_sources(mesh)
    vertices = _vertices_sources(mesh)
    texture_member, textured_symbols = _appearance(root, placement.dae_member)
    faces = tuple(
        face
        for triangles in _children(mesh, "triangles")
        if triangles.get("material") in textured_symbols
        for face in _parse_triangle_faces(triangles, sources, vertices)
    )
    if not faces:
        raise ColladaParseError(f"{placement.name} has no textured faces")
    return ParsedModel(
        placement=placement,
        region=_parse_region(archive, placement.name, dataset),
        texture_member=texture_member,
        faces=faces,
    )


def _geographic_points(model: ParsedModel) -> tuple[Point3D, ...]:
    result: list[Point3D] = []
    cosine = math.cos(math.radians(model.placement.latitude))
    for face in model.faces:
        for x, y, z in face.points:
            longitude = model.placement.longitude + math.degrees(x / (EARTH_RADIUS_METERS * cosine))
            latitude = model.placement.latitude + math.degrees(y / EARTH_RADIUS_METERS)
            result.append((longitude, latitude, model.placement.altitude + z))
    return tuple(result)


def _building_mesh(
    model: ParsedModel, transformer: Transformer, dataset: ColladaDataset
) -> BuildingMesh:
    geographic = _geographic_points(model)
    if not model.region.contains(geographic):
        raise ColladaParseError(f"{model.placement.name} falls outside its KML region")
    xs, ys = transformer.transform(
        [point[0] for point in geographic], [point[1] for point in geographic]
    )
    minimum_z = min(point[2] for point in geographic)
    projected: tuple[Point3D, ...] = tuple(
        (float(x), float(y), geographic[index][2] - minimum_z)
        for index, (x, y) in enumerate(zip(xs, ys, strict=True))
    )
    faces = tuple(
        MeshFace(
            (projected[index], projected[index + 1], projected[index + 2]),
            model.faces[index // 3].uvs,
        )
        for index in range(0, len(projected), 3)
    )
    hull = MultiPoint([(x, y) for x, y, _ in projected]).convex_hull
    if hull.is_empty:
        raise ColladaParseError(f"{model.placement.name} has no polygon footprint")
    if not isinstance(hull, Polygon):
        hull = hull.buffer(MINIMUM_FOOTPRINT_BUFFER_METERS, cap_style="square")
    if not isinstance(hull, Polygon) or hull.is_empty:
        raise ColladaParseError(f"{model.placement.name} has no polygon footprint")
    footprint: Ring = tuple((float(x), float(y)) for x, y in hull.exterior.coords[:-1])
    height = max(point[2] for point in projected)
    if not 0.0 < height <= 400.0:
        raise ColladaParseError(f"{model.placement.name} has invalid height {height}")
    identifier = dataset.identifier(model.placement.name)
    return BuildingMesh(identifier, height, footprint, faces)


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


def load_collada_meshes(
    source: Path, dataset: ColladaDataset, texture_dir: Path = MESH_TEXTURE_DIR
) -> tuple[BuildingMesh, ...]:
    transformer = Transformer.from_crs(4326, 32129, always_xy=True)
    meshes: list[BuildingMesh] = []
    with _model_archive(source, dataset) as archive:
        members = dataset.model_members(archive.namelist())
        if len(members) != dataset.expected_model_count:
            raise ColladaParseError(
                f"{dataset.label} archive has {len(members)} models; "
                f"expected {dataset.expected_model_count}"
            )
        for member in members:
            if PurePosixPath(member).stem in dataset.excluded_model_names:
                continue
            model = _parse_model(archive, member, dataset)
            mesh = _building_mesh(model, transformer, dataset)
            try:
                texture = archive.read(model.texture_member)
            except KeyError as error:
                raise ColladaParseError(f"missing texture {model.texture_member}") from error
            if not texture.startswith(b"\xff\xd8") or not texture.endswith(b"\xff\xd9"):
                raise ColladaParseError(f"{model.texture_member} is not a complete JPEG")
            _write_atomic(texture_dir / f"{mesh.texture_id}.jpg", texture)
            meshes.append(mesh)
    meshes.sort(key=lambda mesh: mesh.texture_id)
    return tuple(meshes)


def stadium_meshes(
    snapshot: Snapshot, texture_dir: Path = MESH_TEXTURE_DIR
) -> tuple[BuildingMesh, ...]:
    return load_collada_meshes(snapshot.path, STADIUM, texture_dir)


def legacy_downtown_meshes(
    snapshot: Snapshot, texture_dir: Path = MESH_TEXTURE_DIR
) -> tuple[BuildingMesh, ...]:
    return load_collada_meshes(snapshot.path, LEGACY_DOWNTOWN, texture_dir)
