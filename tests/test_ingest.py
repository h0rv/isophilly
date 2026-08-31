from __future__ import annotations

import unittest
from io import BytesIO
from pathlib import Path

from isophilly_ingest.config import (
    DEFAULT_HEIGHT_METERS,
    SOURCES,
    STREET_TREE_SOURCE_ITEM_ID,
    STREET_TREE_SOURCE_RECORD_COUNT,
    STREET_TREE_SOURCE_SHA256,
)
from isophilly_ingest.geometry import (
    DEFAULT_TREE_DIAMETER_METERS,
    buildings,
    footprint_id,
    height_from_values,
    projected,
    street_trees,
    tree_diameter,
    validate_street_tree_output,
)
from isophilly_ingest.ingest import validate_tree_snapshot, write_world
from isophilly_ingest.models import (
    Bounds,
    Building,
    BuildingMesh,
    BuildingPart,
    MeshFace,
    RoofShape,
    Snapshot,
    StreetTree,
)


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

    def test_lidar_evidence_overrides_footprint_height(self) -> None:
        import geopandas as gpd
        from shapely.geometry import Polygon

        polygon = Polygon(((0, 0), (10, 0), (10, 10), (0, 10)))
        frame = gpd.GeoDataFrame(
            {"approx_hgt": [30.0], "max_hgt": [30.0]}, geometry=[polygon], crs=32129
        )

        result = buildings(frame, polygon, {footprint_id(polygon): 21.5})

        self.assertEqual(len(result), 1)
        self.assertEqual(result[0].height, 21.5)


class StreetTreeTests(unittest.TestCase):
    def test_tree_diameter_parses_inches_and_defaults_invalid_values(self) -> None:
        self.assertAlmostEqual(tree_diameter(10), 0.254)
        self.assertEqual(tree_diameter(None), DEFAULT_TREE_DIAMETER_METERS)
        self.assertEqual(tree_diameter(float("nan")), DEFAULT_TREE_DIAMETER_METERS)
        self.assertEqual(tree_diameter(500), DEFAULT_TREE_DIAMETER_METERS)

    def test_trees_are_projected_clipped_and_sorted_by_stable_id(self) -> None:
        from unittest.mock import patch

        import geopandas as gpd
        from shapely.geometry import Point

        frame = gpd.GeoDataFrame(
            {
                "objectid": [2, 1, 3],
                "tree_name": ["B", "A", "OUTSIDE"],
                "tree_dbh": [20.0, None, 12.0],
                "year": ["2025", "2025", "2025"],
                "loc_y": [40.0, 40.0001, 40.5],
                "loc_x": [-75.1, -75.1001, -76.0],
            },
            geometry=[Point(-75.1, 40.0), Point(-75.1001, 40.0001), Point(-76.0, 40.5)],
            crs=4326,
        )
        projected_frame = projected(frame)
        city = projected_frame.geometry.iloc[:2].union_all().convex_hull.buffer(10)

        with patch("isophilly_ingest.geometry.STREET_TREE_SOURCE_RECORD_COUNT", 3):
            result = street_trees(frame, city)

        self.assertEqual(
            [tree.point for tree in result],
            [
                (projected_frame.geometry.iloc[1].x, projected_frame.geometry.iloc[1].y),
                (projected_frame.geometry.iloc[0].x, projected_frame.geometry.iloc[0].y),
            ],
        )
        self.assertEqual(result[0].diameter_m, DEFAULT_TREE_DIAMETER_METERS)
        self.assertAlmostEqual(result[1].diameter_m, 0.508)

    def test_tree_schema_drift_fails_closed(self) -> None:
        from unittest.mock import patch

        import geopandas as gpd
        from shapely.geometry import Point, Polygon

        frame = gpd.GeoDataFrame(
            {"objectid": [1], "tree_dbh": [4.0], "year": ["2025"]},
            geometry=[Point(1, 1)],
            crs=32129,
        )
        city = Polygon(((0, 0), (10, 0), (10, 10), (0, 10)))

        with (
            patch("isophilly_ingest.geometry.STREET_TREE_SOURCE_RECORD_COUNT", 1),
            self.assertRaisesRegex(ValueError, "schema changed"),
        ):
            street_trees(frame, city)

    def test_tree_geometry_must_agree_with_location_fields(self) -> None:
        from unittest.mock import patch

        import geopandas as gpd
        from shapely.geometry import Point

        frame = gpd.GeoDataFrame(
            {
                "objectid": [1],
                "tree_name": ["A"],
                "tree_dbh": [4.0],
                "year": ["2025"],
                "loc_y": [40.0],
                "loc_x": [-75.2],
            },
            geometry=[Point(-75.1, 40.0)],
            crs=4326,
        )
        city = projected(frame).geometry.iloc[0].buffer(10)

        with (
            patch("isophilly_ingest.geometry.STREET_TREE_SOURCE_RECORD_COUNT", 1),
            self.assertRaisesRegex(ValueError, "disagrees"),
        ):
            street_trees(frame, city)

    def test_retained_tree_payload_is_exactly_pinned(self) -> None:
        import hashlib
        import struct
        from unittest.mock import patch

        trees = [StreetTree((1.0, 2.0), 0.25), StreetTree((3.0, 4.0), 0.5)]
        digest = hashlib.sha256(
            b"".join(struct.pack("<fff", *tree.point, tree.diameter_m) for tree in trees)
        ).hexdigest()
        with (
            patch("isophilly_ingest.geometry.STREET_TREE_ACCEPTED_COUNT", 2),
            patch("isophilly_ingest.geometry.STREET_TREE_PAYLOAD_SHA256", digest),
        ):
            validate_street_tree_output(trees)
        with (
            patch("isophilly_ingest.geometry.STREET_TREE_ACCEPTED_COUNT", 1),
            patch("isophilly_ingest.geometry.STREET_TREE_PAYLOAD_SHA256", digest),
            self.assertRaisesRegex(ValueError, "coordinates changed"),
        ):
            validate_street_tree_output(trees)

    def test_tree_source_route_and_snapshot_are_pinned(self) -> None:
        source = SOURCES.street_trees
        self.assertIn(STREET_TREE_SOURCE_ITEM_ID, source.url)
        self.assertTrue(source.immutable)
        self.assertEqual(STREET_TREE_SOURCE_RECORD_COUNT, 151_726)
        snapshot = Snapshot(
            source.name,
            source.url,
            Path("tree.geojson"),
            STREET_TREE_SOURCE_SHA256,
            42_795_780,
            "2026-08-31T00:00:00Z",
            None,
            None,
        )
        validate_tree_snapshot(snapshot)
        invalid = Snapshot(
            snapshot.name,
            snapshot.url,
            snapshot.path,
            "0" * 64,
            snapshot.size,
            snapshot.fetched_at,
            snapshot.etag,
            snapshot.last_modified,
        )
        with self.assertRaisesRegex(ValueError, "bytes changed"):
            validate_tree_snapshot(invalid)


class WorldFormatTests(unittest.TestCase):
    def test_python_writer_matches_rust_v9_golden_world(self) -> None:
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
            [
                BuildingPart(
                    osm_id=42,
                    height=18.0,
                    min_height=3.0,
                    roof_height=2.0,
                    roof_shape=RoofShape.PYRAMIDAL,
                    ring=((4.0, 4.0), (7.0, 4.0), (7.0, 7.0), (4.0, 7.0)),
                )
            ],
            [mesh],
            [((0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0))],
            [((1.0, 1.0), (4.0, 1.0), (4.0, 4.0), (1.0, 4.0))],
            [((6.0, 6.0), (9.0, 6.0), (9.0, 9.0), (6.0, 9.0))],
            [StreetTree((5.0, 5.0), 0.25)],
            Bounds(0.0, 0.0, 10.0, 10.0),
            bytes(range(32)),
        )

        fixture = Path(__file__).with_name("fixtures").joinpath("world-v9.hex")
        self.assertEqual(output.getvalue().hex(), fixture.read_text().strip())


if __name__ == "__main__":
    unittest.main()
