from __future__ import annotations

import unittest

import pandas as pd

from geo_philly_ingest.config import DEFAULT_HEIGHT_METERS
from geo_philly_ingest.geometry import building_height
from geo_philly_ingest.models import Bounds


class BoundsTests(unittest.TestCase):
    def test_from_rings_uses_every_point(self) -> None:
        bounds = Bounds.from_rings([((1.0, 4.0), (5.0, 2.0), (3.0, 8.0))])

        self.assertEqual(bounds, Bounds(1.0, 2.0, 5.0, 8.0))

    def test_from_rings_rejects_empty_geometry(self) -> None:
        with self.assertRaises(ValueError):
            Bounds.from_rings([])


class HeightTests(unittest.TestCase):
    def test_prefers_city_approximate_height_in_feet(self) -> None:
        result = building_height(pd.Series({"approx_hgt": 30, "max_hgt": 100.0}))

        self.assertAlmostEqual(result, 9.144018288, places=6)

    def test_uses_max_height_when_approximate_height_is_invalid(self) -> None:
        result = building_height(pd.Series({"approx_hgt": 0, "max_hgt": 25.0}))

        self.assertAlmostEqual(result, 7.62001524, places=6)

    def test_replaces_unusable_heights(self) -> None:
        result = building_height(pd.Series({"approx_hgt": None, "max_hgt": 0.001}))

        self.assertEqual(result, DEFAULT_HEIGHT_METERS)


if __name__ == "__main__":
    unittest.main()
