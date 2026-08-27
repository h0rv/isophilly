from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

type Point = tuple[float, float]
type Ring = tuple[Point, ...]


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


@dataclass(frozen=True, slots=True)
class Street:
    street_class: int
    points: Ring


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
