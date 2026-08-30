from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

from pyproj import Transformer
from shapely import union_all
from shapely.geometry import Polygon
from shapely.geometry.base import BaseGeometry
from shapely.ops import transform
from shapely.strtree import STRtree

from .config import EPSG
from .models import Building, BuildingMesh

MESH_COVERAGE_BUFFER_METERS = 12.0


@dataclass(frozen=True, slots=True)
class Coverage:
    buildings: int
    footprint_m2: float
    photographed_buildings: int
    photographed_footprint_m2: float

    def metadata(self) -> dict[str, int | float]:
        building_percent = (
            100.0 * self.photographed_buildings / self.buildings if self.buildings else 0
        )
        footprint_percent = (
            100.0 * self.photographed_footprint_m2 / self.footprint_m2 if self.footprint_m2 else 0
        )
        return {
            "buildings": self.buildings,
            "footprint_m2": round(self.footprint_m2),
            "photographed_buildings": self.photographed_buildings,
            "photographed_building_percent": round(building_percent, 2),
            "photographed_footprint_m2": round(self.photographed_footprint_m2),
            "photographed_footprint_percent": round(footprint_percent, 2),
        }


def _building_polygons(buildings: list[Building]) -> list[Polygon]:
    return [Polygon(building.ring) for building in buildings]


def photographed_buildings(buildings: list[Building], meshes: list[BuildingMesh]) -> set[int]:
    if not buildings or not meshes:
        return set()
    building_polygons = _building_polygons(buildings)
    mesh_tree = STRtree([Polygon(mesh.footprint) for mesh in meshes])
    matches = mesh_tree.query(
        building_polygons,
        predicate="dwithin",
        distance=MESH_COVERAGE_BUFFER_METERS,
    )
    return {
        int(building_index)
        for building_index, mesh_index in zip(matches[0], matches[1], strict=True)
        if meshes[int(mesh_index)].height * 2.0 >= buildings[int(building_index)].height
    }


def _coverage(
    footprint_areas: list[float], photographed: set[int], indices: set[int] | None = None
) -> Coverage:
    selected = range(len(footprint_areas)) if indices is None else indices
    photographed_indices = photographed if indices is None else indices & photographed
    return Coverage(
        buildings=len(footprint_areas) if indices is None else len(indices),
        footprint_m2=sum(footprint_areas[index] for index in selected),
        photographed_buildings=len(photographed_indices),
        photographed_footprint_m2=sum(footprint_areas[index] for index in photographed_indices),
    )


def _area_geometry(rings: object, projector: Transformer) -> BaseGeometry:
    if not isinstance(rings, list):
        raise ValueError("neighborhood rings must be an array")
    polygons: list[Polygon] = []
    for raw_ring in rings:
        if not isinstance(raw_ring, list):
            raise ValueError("neighborhood ring must be an array")
        points: list[tuple[float, float]] = []
        for raw_point in raw_ring:
            if (
                not isinstance(raw_point, list)
                or len(raw_point) != 2
                or not all(isinstance(value, (int, float)) for value in raw_point)
            ):
                raise ValueError("neighborhood point must contain two numbers")
            points.append((float(raw_point[0]), float(raw_point[1])))
        polygon = Polygon(points)
        if not polygon.is_valid or polygon.is_empty:
            raise ValueError("neighborhood polygon is invalid")
        polygons.append(polygon)
    if not polygons:
        raise ValueError("neighborhood must contain a polygon")
    return transform(projector.transform, union_all(polygons))


def texture_coverage_report(
    buildings: list[Building],
    meshes: list[BuildingMesh],
    neighborhoods_path: Path,
) -> dict[str, object]:
    building_polygons = _building_polygons(buildings)
    footprint_areas = [polygon.area for polygon in building_polygons]
    photographed = photographed_buildings(buildings, meshes)
    report: dict[str, object] = {
        "schema_version": 1,
        "definition": (
            "Photographed means an official footprint is within 12 metres of a textured "
            "3D mesh at least half its current height. All other buildings use citywide "
            "aerial-derived roofs and walls."
        ),
        "citywide": _coverage(footprint_areas, photographed).metadata(),
    }

    raw: object = json.loads(neighborhoods_path.read_text())
    if not isinstance(raw, dict) or not isinstance(raw.get("features"), list):
        raise ValueError("neighborhood collection is invalid")
    building_tree = STRtree(building_polygons)
    projector = Transformer.from_crs(4326, EPSG, always_xy=True)
    areas: list[dict[str, object]] = []
    for raw_feature in raw["features"]:
        if not isinstance(raw_feature, dict):
            raise ValueError("neighborhood feature must be an object")
        name = raw_feature.get("name")
        kind = raw_feature.get("kind")
        if not isinstance(name, str) or not isinstance(kind, str):
            raise ValueError("neighborhood name and kind must be strings")
        geometry = _area_geometry(raw_feature.get("rings"), projector)
        indices = {int(index) for index in building_tree.query(geometry, predicate="intersects")}
        areas.append(
            {
                "name": name,
                "kind": kind,
                **_coverage(footprint_areas, photographed, indices).metadata(),
            }
        )
    report["areas"] = sorted(areas, key=lambda area: (str(area["kind"]), str(area["name"])))
    return report


def write_texture_coverage(path: Path, report: dict[str, object]) -> None:
    temporary = path.with_suffix(".json.part")
    temporary.write_text(json.dumps(report, indent=2) + "\n")
    temporary.replace(path)
