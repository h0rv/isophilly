from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import pandas as pd

from geo_philly_ingest.config import DEFAULT_HEIGHT_METERS
from geo_philly_ingest.geometry import building_height
from geo_philly_ingest.models import Bounds, RoofShape, Snapshot
from geo_philly_ingest.osm import building_parts, parse_length, parse_levels


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


class OpenStreetMapTests(unittest.TestCase):
    def test_parses_metric_and_imperial_lengths(self) -> None:
        self.assertEqual(parse_length("12.5 m"), 12.5)
        self.assertAlmostEqual(parse_length("40 ft") or 0.0, 12.192)
        self.assertIsNone(parse_length("12 cubits"))
        self.assertEqual(parse_levels("3.5"), 3.5)
        self.assertIsNone(parse_levels("3m"))

    def test_building_part_preserves_height_and_roof_tags(self) -> None:
        payload = {
            "elements": [
                {
                    "type": "way",
                    "id": 42,
                    "tags": {
                        "building:part": "yes",
                        "height": "40 ft",
                        "min_height": "3 m",
                        "roof:shape": "gabled",
                        "roof:height": "4 m",
                    },
                    "geometry": [
                        {"lat": 39.950, "lon": -75.165},
                        {"lat": 39.950, "lon": -75.164},
                        {"lat": 39.951, "lon": -75.164},
                        {"lat": 39.951, "lon": -75.165},
                        {"lat": 39.950, "lon": -75.165},
                    ],
                }
            ]
        }
        with TemporaryDirectory() as directory:
            path = Path(directory) / "parts.json"
            path.write_text(json.dumps(payload))
            snapshot = Snapshot(
                name="parts",
                url="https://example.test/parts",
                path=path,
                sha256="0" * 64,
                size=path.stat().st_size,
                fetched_at="2026-08-27T00:00:00Z",
                etag=None,
                last_modified=None,
            )

            parts = building_parts(snapshot)

        self.assertEqual(len(parts), 1)
        self.assertAlmostEqual(parts[0].height, 12.192)
        self.assertEqual(parts[0].min_height, 3.0)
        self.assertEqual(parts[0].roof_height, 4.0)
        self.assertIs(parts[0].roof_shape, RoofShape.GABLED)

    def test_empty_building_part_response_is_rejected(self) -> None:
        with TemporaryDirectory() as directory:
            path = Path(directory) / "parts.json"
            path.write_text('{"elements": []}')
            snapshot = Snapshot(
                name="parts",
                url="https://example.test/parts",
                path=path,
                sha256="0" * 64,
                size=path.stat().st_size,
                fetched_at="2026-08-27T00:00:00Z",
                etag=None,
                last_modified=None,
            )

            with self.assertRaisesRegex(ValueError, "no building parts"):
                building_parts(snapshot)


if __name__ == "__main__":
    unittest.main()
