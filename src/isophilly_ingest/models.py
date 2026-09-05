from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from pathlib import Path

type Point = tuple[float, float]
type Ring = tuple[Point, ...]
type Point3D = tuple[float, float, float]


@dataclass(frozen=True, slots=True)
class Bounds:
    min_x: float
    min_y: float
    max_x: float
    max_y: float

    @classmethod
    def from_rings(cls, rings: list[Ring]) -> Bounds:
        if not rings:
            raise ValueError("cannot calculate bounds without geometry")
        xs = [x for ring in rings for x, _ in ring]
        ys = [y for ring in rings for _, y in ring]
        return cls(min(xs), min(ys), max(xs), max(ys))

    @property
    def width(self) -> float:
        return self.max_x - self.min_x

    @property
    def height(self) -> float:
        return self.max_y - self.min_y


@dataclass(frozen=True, slots=True)
class Building:
    height: float
    ring: Ring


class TreeForm(IntEnum):
    """Validated, deliberately conservative visual form for a street tree."""

    DEFAULT = 0
    CONIFER = 1
    COLUMNAR = 2
    WEEPING = 3
    SHRUB = 4


@dataclass(frozen=True, slots=True)
class StreetTree:
    point: Point
    diameter_m: float
    form: TreeForm = TreeForm.DEFAULT

    def __post_init__(self) -> None:
        if not isinstance(self.form, TreeForm):
            raise ValueError("street-tree form must be a validated TreeForm")


class TransportKind(IntEnum):
    EXPRESSWAY = 1
    ARTERIAL = 2
    CONNECTOR = 3


@dataclass(frozen=True, slots=True)
class TransportLine:
    kind: TransportKind
    points: tuple[Point, ...]


class RoofShape(IntEnum):
    FLAT = 0
    GABLED = 1
    HIPPED = 2
    PYRAMIDAL = 3
    DOME = 4
    CONE = 5
    MANSARD = 6


@dataclass(frozen=True, slots=True)
class BuildingPart:
    osm_id: int
    height: float
    min_height: float
    roof_height: float
    roof_shape: RoofShape
    ring: Ring


@dataclass(frozen=True, slots=True)
class MeshFace:
    points: tuple[Point3D, ...]
    uvs: tuple[Point, ...]


@dataclass(frozen=True, slots=True)
class BuildingMesh:
    texture_id: int
    height: float
    footprint: Ring
    faces: tuple[MeshFace, ...]


@dataclass(frozen=True, slots=True)
class Snapshot:
    name: str
    url: str
    path: Path
    sha256: str
    size: int
    fetched_at: str
    etag: str | None
    last_modified: str | None

    def metadata(self) -> dict[str, str | int | None]:
        return {
            "name": self.name,
            "url": self.url,
            "file": self.path.name,
            "sha256": self.sha256,
            "bytes": self.size,
            "fetched_at": self.fetched_at,
            "etag": self.etag,
            "last_modified": self.last_modified,
        }
