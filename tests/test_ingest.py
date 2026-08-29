from __future__ import annotations

import unittest
from io import BytesIO
from pathlib import Path

from geo_philly_ingest.config import DEFAULT_HEIGHT_METERS
from geo_philly_ingest.geometry import height_from_values
from geo_philly_ingest.ingest import write_world
from geo_philly_ingest.models import Bounds, Building, BuildingMesh, MeshFace


class BoundsTests(unittest.TestCase):
    def test_from_rings_uses_every_point(self) -> None:
        bounds = Bounds.from_rings([((1.0, 4.0), (5.0, 2.0), (3.0, 8.0))])

        self.assertEqual(bounds, Bounds(1.0, 2.0, 5.0, 8.0))

    def test_from_rings_rejects_empty_geometry(self) -> None:
        with self.assertRaises(ValueError):
            Bounds.from_rings([])


class HeightTests(unittest.TestCase):
    def test_prefers_city_approximate_height_in_feet(self) -> None:
        result = height_from_values(iter((30, 100.0)))

        self.assertAlmostEqual(result, 9.144018288, places=6)

    def test_uses_max_height_when_approximate_height_is_invalid(self) -> None:
        result = height_from_values(iter((0, 25.0)))

        self.assertAlmostEqual(result, 7.62001524, places=6)

    def test_replaces_unusable_heights(self) -> None:
        result = height_from_values(iter((None, 0.001)))

        self.assertEqual(result, DEFAULT_HEIGHT_METERS)


class WorldFormatTests(unittest.TestCase):
    def test_python_writer_matches_rust_v6_golden_world(self) -> None:
        output = BytesIO()
        face = MeshFace(
            ((2.0, 3.0, 0.0), (4.0, 3.0, 0.0), (2.0, 5.0, 12.0)),
            ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0)),
        )
        mesh = BuildingMesh(
            texture_id=7,
            height=12.0,
            footprint=((2.0, 3.0), (4.0, 3.0), (2.0, 5.0)),
            faces=(face,),
        )

        write_world(
            output,
            [Building(8.0, ((1.0, 2.0), (3.0, 2.0), (1.0, 4.0)))],
            [mesh],
            [((0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0))],
            Bounds(0.0, 0.0, 10.0, 10.0),
            bytes(range(32)),
        )

        fixture = Path(__file__).with_name("fixtures").joinpath("world-v6.hex")
        self.assertEqual(output.getvalue().hex(), fixture.read_text().strip())


if __name__ == "__main__":
    unittest.main()
