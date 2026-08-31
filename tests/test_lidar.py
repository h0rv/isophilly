from __future__ import annotations

import hashlib
import json
import struct
import unittest
from io import BytesIO
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

import geopandas as gpd
import httpx
import pyarrow as pa
import pyarrow.parquet as pq
from shapely.geometry import Polygon, box

from isophilly_ingest.lidar import (
    EVIDENCE_SCHEMA,
    Inventory,
    LidarError,
    Tile,
    derive_evidence,
    download_tile,
    inventory_dict,
    load_height_evidence,
    load_las_header,
    merge_evidence,
    parse_inventory,
    parse_listing,
    pending_tiles,
    read_las_header,
    select_city_tiles,
)


def las_bytes(points: list[tuple[int, int, int, int]]) -> bytes:
    header = bytearray(375)
    header[:4] = b"LASF"
    struct.pack_into("<BB", header, 24, 1, 4)
    struct.pack_into("<H", header, 94, 375)
    struct.pack_into("<I", header, 96, 375)
    header[104] = 6
    struct.pack_into("<H", header, 105, 30)
    struct.pack_into("<ddd", header, 131, 1.0, 1.0, 1.0)
    struct.pack_into("<ddd", header, 155, 0.0, 0.0, 0.0)
    xs = [point[0] for point in points]
    ys = [point[1] for point in points]
    zs = [point[2] for point in points]
    struct.pack_into("<dddddd", header, 179, max(xs), min(xs), max(ys), min(ys), max(zs), min(zs))
    struct.pack_into("<Q", header, 247, len(points))
    records = bytearray()
    for x, y, z, classification in points:
        record = bytearray(30)
        struct.pack_into("<iii", record, 0, x, y, z)
        record[16] = classification
        records.extend(record)
    return bytes(header + records)


def inventory_for(*tiles: Tile) -> Inventory:
    return Inventory(
        2,
        "https://example.test/",
        "a" * 64,
        "now",
        "b" * 64,
        "c" * 64,
        tiles,
    )


class InventoryTests(unittest.TestCase):
    def test_listing_is_parsed_sorted_and_city_selected(self) -> None:
        listing = b"""
        1/12/2026  250 <a href="26479E201432N.las">26479E201432N.las</a>
        1/12/2026  100 <a href="26452E204072N.las">26452E204072N.las</a>
        """
        tiles = parse_listing(listing)

        self.assertEqual([tile.name for tile in tiles], ["26452E204072N.las", "26479E201432N.las"])
        selected = select_city_tiles(tiles, box(2_645_100, 204_000, 2_646_000, 205_000))
        self.assertTrue(selected[0].selected)
        self.assertFalse(selected[1].selected)

    def test_inventory_parser_rejects_tampered_derived_bounds(self) -> None:
        tile = parse_listing(b'1/12/2026  100 <a href="26452E204072N.las">26452E204072N.las</a>')[0]
        inventory = inventory_for(tile)
        value = inventory_dict(inventory)
        raw_tiles = value["tiles"]
        self.assertIsInstance(raw_tiles, list)
        assert isinstance(raw_tiles, list)
        raw_tile = raw_tiles[0]
        self.assertIsInstance(raw_tile, dict)
        assert isinstance(raw_tile, dict)
        raw_tile["approximate_bounds_ft"] = [0, 0, 1, 1]

        with self.assertRaises(LidarError):
            parse_inventory(value)


class LasTests(unittest.TestCase):
    def test_reads_las_14_format_6_header(self) -> None:
        payload = las_bytes([(10, 20, 30, 2), (50, 60, 70, 6)])

        header = read_las_header(BytesIO(payload))

        self.assertEqual(header.version, "1.4")
        self.assertEqual(header.point_format, 6)
        self.assertEqual(header.point_count, 2)
        self.assertEqual(header.bounds_ft, (10.0, 20.0, 50.0, 60.0))

    def test_rejects_truncated_point_records(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "tile.las"
        path.write_bytes(las_bytes([(10, 20, 30, 2), (50, 60, 70, 6)])[:-1])

        with self.assertRaises(LidarError):
            load_las_header(path)

    def test_derives_building_height_evidence(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        points = [(x, 5, 10, 2) for x in range(0, 21)]
        points += [(x, 5, 40 + x % 3, 6) for x in range(2, 18)]
        las = root / "test.las"
        las.write_bytes(las_bytes(points))
        buildings = gpd.GeoDataFrame(
            {"building_id": ["building-1"], "source_sha256": ["a" * 64]},
            geometry=[Polygon(((1, 1), (19, 1), (19, 9), (1, 9)))],
            crs=6565,
        )
        output = root / "evidence.parquet"

        rows = derive_evidence(las, buildings, output)

        self.assertEqual(rows, 1)
        evidence = pq.read_table(output).to_pylist()[0]
        self.assertEqual(evidence["building_id"], "building-1")
        self.assertEqual(evidence["source_footprints_sha256"], "a" * 64)
        self.assertGreater(evidence["height_p90_m"], 8.0)
        self.assertEqual(evidence["quality"], "usable")


class DownloadTests(unittest.TestCase):
    def test_resumes_partial_download_and_records_checksum(self) -> None:
        payload = las_bytes([(10, 20, 30, 2)])
        tile = Tile(
            "26452E204072N.las", "https://example.test/tile.las", len(payload), (0, 0, 1, 1), True
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        raw = root / "raw"
        raw.mkdir()
        partial = raw / "26452E204072N.las.part"
        partial.write_bytes(payload[:100])
        (root / "progress.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "inventory_listing_sha256": "a" * 64,
                    "inventory_city_sha256": "b" * 64,
                    "inventory_building_sha256": "c" * 64,
                    "tiles": {tile.name: {"status": "downloading", "downloaded_bytes": 100}},
                }
            )
        )

        def handler(request: httpx.Request) -> httpx.Response:
            self.assertEqual(request.headers["range"], "bytes=100-")
            return httpx.Response(
                206,
                content=payload[100:],
                headers={"Content-Range": f"bytes 100-{len(payload) - 1}/{len(payload)}"},
                request=request,
            )

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            path, digest = download_tile(tile, inventory_for(tile), client)

        self.assertEqual(path.read_bytes(), payload)
        self.assertEqual(digest, hashlib.sha256(payload).hexdigest())
        progress = json.loads((root / "progress.json").read_text())
        self.assertEqual(progress["tiles"][tile.name]["status"], "downloaded")
        self.assertEqual(progress["inventory_listing_sha256"], "a" * 64)

    def test_inventory_change_discards_unversioned_partial(self) -> None:
        payload = las_bytes([(10, 20, 30, 2)])
        tile = Tile(
            "26452E204072N.las",
            "https://example.test/tile.las",
            len(payload),
            (0, 0, 1, 1),
            True,
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        raw = root / "raw"
        raw.mkdir()
        partial = raw / "26452E204072N.las.part"
        partial.write_bytes(payload[:100])
        (root / "progress.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "inventory_listing_sha256": "e" * 64,
                    "inventory_city_sha256": "b" * 64,
                    "inventory_building_sha256": "c" * 64,
                    "tiles": {tile.name: {"status": "downloading", "downloaded_bytes": 100}},
                }
            )
        )

        def handler(request: httpx.Request) -> httpx.Response:
            self.assertNotIn("range", request.headers)
            return httpx.Response(200, content=payload, request=request)

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            path, _ = download_tile(tile, inventory_for(tile), client)

        self.assertEqual(path.read_bytes(), payload)

    def test_rejects_mismatched_content_range_without_appending(self) -> None:
        payload = las_bytes([(10, 20, 30, 2)])
        tile = Tile(
            "26452E204072N.las",
            "https://example.test/tile.las",
            len(payload),
            (0, 0, 1, 1),
            True,
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        raw = root / "raw"
        raw.mkdir()
        partial = raw / "26452E204072N.las.part"
        partial.write_bytes(payload[:100])
        (root / "progress.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "inventory_listing_sha256": "a" * 64,
                    "inventory_city_sha256": "b" * 64,
                    "inventory_building_sha256": "c" * 64,
                    "tiles": {tile.name: {"status": "downloading", "downloaded_bytes": 100}},
                }
            )
        )

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(
                206,
                content=payload[100:],
                headers={"Content-Range": f"bytes 99-{len(payload) - 1}/{len(payload)}"},
                request=request,
            )

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
            self.assertRaises(LidarError),
        ):
            download_tile(tile, inventory_for(tile), client)

        self.assertEqual(partial.read_bytes(), payload[:100])


class ArtifactTests(unittest.TestCase):
    def _row(
        self,
        tile: Tile,
        *,
        building_points: int,
        ground_points: int,
        height: float,
        spread: float,
    ) -> dict[str, object]:
        return {
            "building_id": "same-building",
            "source_footprints_sha256": "c" * 64,
            "tile": tile.name,
            "building_point_count": building_points,
            "ground_point_count": ground_points,
            "ground_elevation_m": 1.0,
            "roof_p10_m": height - spread,
            "roof_p50_m": height - spread / 2,
            "roof_p90_m": height,
            "height_p90_m": height,
            "roof_spread_m": spread,
            "quality": "high" if building_points >= 100 and ground_points >= 20 else "usable",
        }

    def _write_artifact(
        self,
        root: Path,
        tile: Tile,
        inventory: Inventory,
        rows: list[dict[str, object]],
    ) -> Path:
        output = root / f"{tile.name}.parquet"
        table = pa.Table.from_pylist(rows, schema=EVIDENCE_SCHEMA)
        pq.write_table(table, output)
        metadata = {
            "result": "derived",
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "source_url": tile.url,
            "source_bytes": tile.bytes,
            "source_sha256": "d" * 64,
            "source_footprints_sha256": inventory.building_sha256,
            "output_file": output.name,
            "output_bytes": output.stat().st_size,
            "output_sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
            "rows": len(rows),
        }
        (root / f"{tile.name}.json").write_text(json.dumps(metadata))
        return output

    def test_pending_audits_derived_checksum(self) -> None:
        tile = Tile("26452E204072N.las", "https://example.test/one.las", 100, (0, 0, 1, 1), True)
        inventory = inventory_for(tile)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        output = self._write_artifact(
            root,
            tile,
            inventory,
            [self._row(tile, building_points=200, ground_points=100, height=10, spread=1)],
        )

        with patch("isophilly_ingest.lidar.DERIVED_DIR", root):
            self.assertEqual(pending_tiles(inventory), ())
            refreshed = Inventory(
                2,
                inventory.source_url,
                "e" * 64,
                inventory.fetched_at,
                inventory.city_sha256,
                inventory.building_sha256,
                inventory.tiles,
            )
            self.assertEqual(pending_tiles(refreshed), (tile,))
            output.write_bytes(output.read_bytes() + b"corrupt")
            self.assertEqual(pending_tiles(inventory), (tile,))

    def test_merge_requires_complete_selection_and_prefers_accepted_ground_support(self) -> None:
        first = Tile("26452E204072N.las", "https://example.test/one.las", 100, (0, 0, 1, 1), True)
        second = Tile("26479E201432N.las", "https://example.test/two.las", 200, (0, 0, 1, 1), True)
        inventory = inventory_for(first, second)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        merged = root / "building-evidence.parquet"
        partial = root / "building-evidence.partial.parquet"
        merged.write_bytes(b"existing complete artifact")
        self._write_artifact(
            root,
            first,
            inventory,
            [self._row(first, building_points=1000, ground_points=25, height=10, spread=5)],
        )

        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", root),
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", merged),
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", partial),
        ):
            with self.assertRaises(LidarError):
                merge_evidence(inventory)
            self.assertEqual(merge_evidence(inventory, allow_partial=True), 1)
            self.assertTrue(partial.is_file())
            self.assertEqual(merged.read_bytes(), b"existing complete artifact")
            self.assertTrue(json.loads(partial.with_suffix(".json").read_text())["partial"])
            self._write_artifact(
                root,
                second,
                inventory,
                [self._row(second, building_points=200, ground_points=500, height=10, spread=1)],
            )
            self.assertEqual(merge_evidence(inventory), 1)

        row = pq.read_table(merged).to_pylist()[0]
        self.assertEqual(row["tile"], second.name)
        self.assertFalse(json.loads(merged.with_suffix(".json").read_text())["partial"])

    def test_height_loader_rejects_complex_roof_spread(self) -> None:
        tile = Tile("26452E204072N.las", "https://example.test/one.las", 100, (0, 0, 1, 1), True)
        inventory = inventory_for(tile)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        merged = root / "building-evidence.parquet"
        derived = root / "derived"
        derived.mkdir()
        self._write_artifact(
            derived,
            tile,
            inventory,
            [self._row(tile, building_points=1000, ground_points=500, height=10, spread=5)],
        )

        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", merged),
        ):
            merge_evidence(inventory, allow_partial=True)
        manifest_path = merged.with_suffix(".json")
        manifest = json.loads(manifest_path.read_text())
        manifest["partial"] = True
        manifest_path.write_text(json.dumps(manifest))
        with patch("isophilly_ingest.lidar.INVENTORY_PATH", root / "missing-inventory.json"):
            with self.assertRaisesRegex(LidarError, "complete LiDAR merge"):
                load_height_evidence(merged, "c" * 64)
            self.assertEqual(load_height_evidence(merged, "c" * 64, allow_partial=True), {})


if __name__ == "__main__":
    unittest.main()
