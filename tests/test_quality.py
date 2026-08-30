from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from pyproj import Transformer

from geo_philly_ingest.models import Building, BuildingMesh, MeshFace, Ring
from geo_philly_ingest.quality import photographed_buildings, texture_coverage_report


def mesh(
    left: float, bottom: float, right: float, top: float, height: float = 10.0
) -> BuildingMesh:
    face = MeshFace(
        (
            (left, bottom, 0.0),
            (right, bottom, 0.0),
            (left, top, height),
        ),
        ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0)),
    )
    return BuildingMesh(1, height, rectangle(left, bottom, right, top), (face,))


def rectangle(left: float, bottom: float, right: float, top: float) -> Ring:
    return ((left, bottom), (right, bottom), (right, top), (left, top))


class TextureCoverageTests(unittest.TestCase):
    def test_matches_renderer_twelve_meter_mesh_buffer(self) -> None:
        buildings = [
            Building(10.0, rectangle(0.0, 0.0, 2.0, 2.0)),
            Building(10.0, rectangle(30, 0, 32, 2)),
        ]

        covered = photographed_buildings(buildings, [mesh(13.0, 0.0, 15.0, 2.0)])

        self.assertEqual(covered, {0})

    def test_stale_short_mesh_does_not_hide_current_tower(self) -> None:
        building = Building(100.0, rectangle(0.0, 0.0, 10.0, 10.0))

        covered = photographed_buildings([building], [mesh(0.0, 0.0, 10.0, 10.0, height=49.0)])

        self.assertEqual(covered, set())

    def test_reports_citywide_and_area_coverage(self) -> None:
        buildings = [
            Building(8.0, rectangle(820_000.0, 70_000.0, 820_010.0, 70_010.0)),
            Building(8.0, rectangle(821_000.0, 70_000.0, 821_010.0, 70_010.0)),
        ]
        to_wgs84 = Transformer.from_crs(32129, 4326, always_xy=True)
        transformer_ring = [
            list(to_wgs84.transform(x, y))
            for x, y in rectangle(819_900.0, 69_900.0, 821_100.0, 70_100.0)
        ]
        transformer_ring.append(transformer_ring[0])
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "neighborhoods.json"
            path.write_text(
                json.dumps(
                    {
                        "features": [
                            {
                                "name": "Test",
                                "kind": "planning_neighborhood",
                                "rings": [transformer_ring],
                            }
                        ]
                    }
                )
            )

            result = texture_coverage_report(
                buildings, [mesh(820_000, 70_000, 820_010, 70_010)], path
            )

        citywide = result["citywide"]
        self.assertIsInstance(citywide, dict)
        assert isinstance(citywide, dict)
        self.assertEqual(citywide["buildings"], 2)
        self.assertEqual(citywide["photographed_buildings"], 1)
        self.assertEqual(citywide["photographed_building_percent"], 50.0)
        areas = result["areas"]
        self.assertIsInstance(areas, list)
        assert isinstance(areas, list)
        self.assertEqual(len(areas), 1)


if __name__ == "__main__":
    unittest.main()
