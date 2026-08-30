from __future__ import annotations

import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

from isophilly_ingest.models import RoofShape, Snapshot
from isophilly_ingest.osm import building_parts, parse_length


class OpenStreetMapTests(unittest.TestCase):
    def test_parses_metric_and_imperial_lengths(self) -> None:
        self.assertEqual(parse_length("12 m"), 12.0)
        self.assertAlmostEqual(parse_length("100 ft") or 0.0, 30.48)
        self.assertIsNone(parse_length("12 yards"))
        self.assertIsNone(parse_length("-2 m"))

    def test_parses_height_backed_building_part_at_the_boundary(self) -> None:
        payload = {
            "version": 0.6,
            "generator": "test",
            "elements": [
                {
                    "type": "way",
                    "id": 42,
                    "tags": {
                        "building:part": "yes",
                        "height": "30 m",
                        "min_height": "3 m",
                        "roof:shape": "pyramidal",
                        "roof:height": "4 m",
                    },
                    "geometry": [
                        {"lat": 39.95, "lon": -75.16},
                        {"lat": 39.95, "lon": -75.1598},
                        {"lat": 39.9502, "lon": -75.1598},
                        {"lat": 39.9502, "lon": -75.16},
                        {"lat": 39.95, "lon": -75.16},
                    ],
                }
            ],
        }
        with TemporaryDirectory() as directory:
            path = Path(directory) / "parts.json"
            path.write_text(json.dumps(payload))
            snapshot = Snapshot(
                "parts", "https://example.test", path, "0" * 64, 1, "now", None, None
            )
            parts = building_parts(snapshot)

        self.assertEqual(len(parts), 1)
        self.assertEqual(parts[0].osm_id, 42)
        self.assertEqual(parts[0].height, 30.0)
        self.assertEqual(parts[0].min_height, 3.0)
        self.assertEqual(parts[0].roof_height, 4.0)
        self.assertIs(parts[0].roof_shape, RoofShape.PYRAMIDAL)


if __name__ == "__main__":
    unittest.main()
