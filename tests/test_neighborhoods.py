from __future__ import annotations

import unittest
from copy import deepcopy
from json import loads
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from shapely.geometry import Polygon

from scripts.build_neighborhoods import (
    LOCAL_AREAS,
    REVIEWED_PAIR_POLICIES,
    _write_json_atomic,
    audit,
    build,
    publish_build,
)

ROOT = Path(__file__).parent.parent


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

        self.assertEqual(len(local_areas), 61)
        self.assertEqual(
            len({area["name"].casefold() for area in result["features"]}),
            len(result["features"]),
        )
        self.assertEqual(len({area["name"].casefold() for area in local_areas}), len(local_areas))
        for area in local_areas:
            self.assertTrue(area.get("source", "").startswith("https://"), area["name"])
            self.assertIn("non-official", area.get("note", ""), area["name"])
            self.assertIs(area.get("display"), True, area["name"])
            self.assertIn(area.get("display_tier"), {1, 2, 3}, area["name"])
            self.assertIsInstance(area.get("draw_geometry"), bool, area["name"])
            self.assertTrue(area.get("display_label"), area["name"])
            self.assertTrue(area.get("relevance"), area["name"])
            self.assertTrue(area.get("rationale"), area["name"])
            self.assertTrue(area.get("associations"), area["name"])
            self.assertTrue(area.get("planning_parents"), area["name"])
            self.assertIsInstance(area.get("suppresses"), list, area["name"])
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

    def test_dense_overlap_groups_have_explicit_winners_and_suppression(self) -> None:
        result = build(
            {
                "features": [
                    feature(name, index)
                    for index, name in enumerate(
                        [
                            "Bella Vista",
                            "Washington Square West",
                            "Rittenhouse Square",
                            *[f"Area {index}" for index in range(137)],
                        ]
                    )
                ]
            }
        )
        local = {area["name"]: area for area in result["features"] if area["kind"] == "local_area"}

        self.assertEqual(local["Italian Market"].get("overlap_group"), "italian-market")
        self.assertGreater(
            local["Italian Market"].get("priority", 0),
            local["Little Saigon"].get("priority", 0),
        )
        self.assertEqual(local["Gayborhood"].get("overlap_group"), "washington-square-west")
        self.assertIn("Washington Square West", local["Gayborhood"].get("suppresses", []))
        self.assertEqual(local["Africatown"].get("overlap_group"), "africatown")
        self.assertIs(local["Africatown"].get("draw_geometry"), False)
        self.assertGreater(
            local["Africatown"].get("priority", 0),
            local["Woodland Avenue Africatown"].get("priority", 0),
        )

    def test_checked_in_overlay_passes_deterministic_offline_audit(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())

        report = audit(collection)

        self.assertIs(report["ok"], True, report["failures"])
        self.assertEqual(report["counts"]["planning_neighborhoods"], 148)
        self.assertEqual(report["counts"]["displayed_local_areas"], 61)
        self.assertGreaterEqual(len(report["overlaps_at_least_25_percent"]), 1)
        self.assertGreaterEqual(len(report["label_anchors_within_150m"]), 1)
        for finding in (
            *report["overlaps_at_least_25_percent"],
            *report["label_anchors_within_150m"],
        ):
            self.assertIsNotNone(finding["reviewed_policy"], finding)

    def test_audit_rejects_registry_display_drift_and_source_name_drift(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        drifted = deepcopy(collection)
        drifted["features"] = [
            feature for feature in drifted["features"] if feature["name"] != "Italian Market"
        ]
        planning = next(
            feature for feature in drifted["features"] if feature["kind"] == "planning_neighborhood"
        )
        planning["name"] = f"{planning['name']} drift"

        report = audit(drifted)

        self.assertIs(report["ok"], False)
        self.assertIn(
            "generated local areas and Python registry are not a bijection", report["failures"]
        )
        self.assertIn(
            "planning name set does not match the pinned local source snapshot",
            report["failures"],
        )
        self.assertIn(
            "planning geometry payload does not match the pinned local source snapshot",
            report["failures"],
        )

    def test_audit_rejects_local_display_policy_drift(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        italian_market = next(
            feature for feature in collection["features"] if feature["name"] == "Italian Market"
        )
        italian_market["source"] = "https://example.invalid/drift"

        report = audit(collection)

        self.assertIn(
            "Italian Market full generated record differs from the Python registry",
            report["failures"],
        )

    def test_audit_rejects_unreviewed_overlap_and_near_anchor_pairs(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())

        with patch.dict(REVIEWED_PAIR_POLICIES, {}, clear=True):
            report = audit(collection)

        self.assertTrue(
            any(failure.startswith("unreviewed >=25% overlap") for failure in report["failures"])
        )
        self.assertIn(
            "unreviewed <=150m label anchors for Fabric Row and South Street Headhouse",
            report["failures"],
        )

    def test_overlap_group_alone_never_authorizes_a_pair(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        pair = frozenset(("Africatown", "Woodland Avenue Africatown"))
        policies_without_pair = {
            candidate: policy
            for candidate, policy in REVIEWED_PAIR_POLICIES.items()
            if candidate != pair
        }

        with patch.dict(REVIEWED_PAIR_POLICIES, policies_without_pair, clear=True):
            report = audit(collection)

        self.assertIn(
            "unreviewed >=25% overlap between Africatown and Woodland Avenue Africatown",
            report["failures"],
        )
        self.assertIn(
            "unreviewed <=150m label anchors for Africatown and Woodland Avenue Africatown",
            report["failures"],
        )

    def test_reviewed_pair_requires_nonblank_rationale(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        pair = frozenset(("Fabric Row", "South Street Headhouse"))
        invalid_policy = {"winner": "South Street Headhouse", "rationale": " \t "}

        with patch.dict(REVIEWED_PAIR_POLICIES, {pair: invalid_policy}, clear=True):
            report = audit(collection)

        self.assertIn(
            "invalid reviewed overlap policy for ['Fabric Row', 'South Street Headhouse']",
            report["failures"],
        )

    def test_every_canonical_parent_is_a_pinned_planning_name(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        planning_names = {
            feature["name"]
            for feature in collection["features"]
            if feature["kind"] == "planning_neighborhood"
        }

        for feature in collection["features"]:
            if feature["kind"] == "local_area":
                self.assertLessEqual(
                    set(feature["planning_parents"]), planning_names, feature["name"]
                )

    def test_publish_build_audits_before_atomic_replace(self) -> None:
        collection = loads((ROOT / "static/neighborhoods.json").read_text())
        planning = next(
            feature
            for feature in collection["features"]
            if feature["kind"] == "planning_neighborhood"
        )
        planning["label"][0] += 0.000001
        with TemporaryDirectory() as directory:
            output = Path(directory) / "neighborhoods.json"
            output.write_text("existing")

            with self.assertRaisesRegex(ValueError, "planning geometry payload"):
                publish_build(collection, output)

            self.assertEqual(output.read_text(), "existing")

    def test_web_renderer_consumes_generated_presentation_metadata(self) -> None:
        source = (ROOT / "static/app.js").read_text()

        self.assertNotIn("LOCAL_AREA_PRESENTATION", source)
        self.assertNotIn("LOCAL_PARENT_LABELS", source)
        self.assertIn("area.display_tier", source)
        self.assertIn("area.display_label", source)
        self.assertIn("area.draw_geometry", source)
        self.assertIn("area.associations", source)
        self.assertIn("area.planning_parents", source)
        self.assertIn("area.suppresses", source)

    def test_audit_report_writer_atomically_replaces_existing_json(self) -> None:
        with TemporaryDirectory() as directory:
            output = Path(directory) / "audit.json"
            output.write_text("old")

            _write_json_atomic(output, {"ok": True}, indent=2)

            self.assertEqual(loads(output.read_text()), {"ok": True})
            self.assertEqual(list(output.parent.iterdir()), [output])

    def test_rejects_partial_source_response(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing neighborhood features"):
            build({"features": []})


if __name__ == "__main__":
    unittest.main()
