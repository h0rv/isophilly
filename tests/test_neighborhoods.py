from __future__ import annotations

import unittest

from shapely.geometry import Polygon

from scripts.build_neighborhoods import LOCAL_AREAS, build


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
        self.assertEqual(len(result["features"]), 140 + len(LOCAL_AREAS))

    def test_local_areas_are_unique_valid_and_sourced(self) -> None:
        names = ["Bella Vista", "Washington Square West", "Rittenhouse Square"]
        names.extend(f"Area {index}" for index in range(137))

        result = build({"features": [feature(name, index) for index, name in enumerate(names)]})
        local_areas = [area for area in result["features"] if area["kind"] == "local_area"]

        self.assertGreaterEqual(len(local_areas), 55)
        self.assertEqual(
            len({area["name"].casefold() for area in result["features"]}),
            len(result["features"]),
        )
        self.assertEqual(len({area["name"].casefold() for area in local_areas}), len(local_areas))
        for area in local_areas:
            self.assertTrue(area.get("source", "").startswith("https://"), area["name"])
            self.assertIn("non-official", area.get("note", ""), area["name"])
            for ring in area["rings"]:
                polygon = Polygon(ring)
                self.assertTrue(polygon.is_valid and not polygon.is_empty, area["name"])

    def test_required_local_areas_and_citywide_spread(self) -> None:
        names = ["Bella Vista", "Washington Square West", "Rittenhouse Square"]
        names.extend(f"Area {index}" for index in range(137))
        result = build({"features": [feature(name, index) for index, name in enumerate(names)]})
        local_areas = {
            area["name"]: area for area in result["features"] if area["kind"] == "local_area"
        }

        required = {"Italian Market", "East Passyunk", "Little Saigon", "Africatown"}
        self.assertLessEqual(required, local_areas.keys())
        for launch_area in (
            "Italian Market",
            "Africatown",
            "Manayunk Main Street",
            "Castor Avenue",
            "Fishtown Frankford Avenue",
        ):
            self.assertEqual(local_areas[launch_area].get("priority"), 100)
        self.assertGreater(
            local_areas["Italian Market"].get("priority", 0),
            local_areas["Mexican Market"].get("priority", 0),
        )
        points = [
            point for area in local_areas.values() for ring in area["rings"] for point in ring
        ]
        longitudes = [point[0] for point in points]
        latitudes = [point[1] for point in points]
        self.assertLess(min(longitudes), -75.24)  # Southwest Philadelphia
        self.assertGreater(max(longitudes), -75.06)  # Northeast Philadelphia
        self.assertLess(min(latitudes), 39.90)  # Navy Yard
        self.assertGreater(max(latitudes), 40.07)  # Northwest / Northeast

    def test_rejects_partial_source_response(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing neighborhood features"):
            build({"features": []})


if __name__ == "__main__":
    unittest.main()
