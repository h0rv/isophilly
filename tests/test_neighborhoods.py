from __future__ import annotations

import unittest

from scripts.build_neighborhoods import build


def feature(name: str, index: int) -> dict[str, object]:
    west = -75.20 + index * 0.0001
    south = 39.90 + index * 0.0001
    return {
        "type": "Feature",
        "properties": {"NAME": name},
        "geometry": {
            "type": "Polygon",
            "coordinates": [
                [
                    [west, south],
                    [west + 0.00005, south],
                    [west + 0.00005, south + 0.00005],
                    [west, south + 0.00005],
                    [west, south],
                ]
            ],
        },
    }


class NeighborhoodTests(unittest.TestCase):
    def test_local_area_stays_separate_from_planning_neighborhoods(self) -> None:
        names = ["Bella Vista", "Washington Square West", "Rittenhouse Square"]
        names.extend(f"Area {index}" for index in range(137))

        result = build({"features": [feature(name, index) for index, name in enumerate(names)]})

        gayborhood = next(area for area in result["features"] if area["name"] == "Gayborhood")
        washington_square_west = next(
            area for area in result["features"] if area["name"] == "Washington Square West"
        )
        self.assertEqual(gayborhood["kind"], "local_area")
        self.assertEqual(washington_square_west["kind"], "planning_neighborhood")
        self.assertEqual(len(result["features"]), 141)

    def test_rejects_partial_source_response(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing neighborhood features"):
            build({"features": []})


if __name__ == "__main__":
    unittest.main()
