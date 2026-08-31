from __future__ import annotations

import hashlib
import json
import struct
import unittest
from io import BytesIO
from pathlib import Path
from tempfile import TemporaryDirectory
from typing import Any
from unittest.mock import patch

import geopandas as gpd
import httpx
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
from shapely.geometry import Polygon, box

from isophilly_ingest.lidar import (
    EVIDENCE_SCHEMA,
    PASDA_LAS_URL,
    Inventory,
    LidarError,
    Tile,
    _publish_staged_merge,
    _recover_merge_publication,
    _summary,
    derive_evidence,
    download_tile,
    inventory_dict,
    iter_las_points,
    load_height_evidence,
    load_las_header,
    merge_evidence,
    parse_inventory,
    parse_listing,
    pending_tiles,
    preflight_merge_read,
    process_tile,
    read_las_header,
    recheck_rejected_sources,
    select_city_tiles,
    semantic_inventory_sha256,
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

    def _valid_value(self) -> dict[str, Any]:
        tiles = parse_listing(
            b'1/12/2026  100 <a href="26452E204072N.las">26452E204072N.las</a>\n'
            b'1/12/2026  200 <a href="26479E201432N.las">26479E201432N.las</a>'
        )
        inventory = Inventory(2, PASDA_LAS_URL, "a" * 64, "now", "b" * 64, "c" * 64, tiles)
        return json.loads(json.dumps(inventory_dict(inventory)))

    def test_inventory_rejects_url_outside_exact_pasda_path(self) -> None:
        value = self._valid_value()
        value["tiles"][0]["url"] = "https://evil.test/26452E204072N.las"
        with self.assertRaisesRegex(LidarError, "outside the audited"):
            parse_inventory(value, require_audited=False)

    def test_inventory_rejects_duplicates_and_reordering(self) -> None:
        duplicate = self._valid_value()
        duplicate["tiles"][1] = dict(duplicate["tiles"][0])
        duplicate["counts"] = {"listed": 2, "selected": 0}
        duplicate["bytes"] = {"listed": 200, "selected": 0}
        with self.assertRaisesRegex(LidarError, "duplicate"):
            parse_inventory(duplicate, require_audited=False)

        reordered = self._valid_value()
        reordered["tiles"].reverse()
        with self.assertRaisesRegex(LidarError, "sorted"):
            parse_inventory(reordered, require_audited=False)

    def test_inventory_rejects_bad_hash_and_checked_in_pin_mismatch(self) -> None:
        bad_hash = self._valid_value()
        bad_hash["listing_sha256"] = "A" * 64
        with self.assertRaisesRegex(LidarError, "lowercase SHA-256"):
            parse_inventory(bad_hash, require_audited=False)

        value = self._valid_value()
        candidate = parse_inventory(value, require_audited=False)
        self.assertEqual(len(semantic_inventory_sha256(candidate)), 64)
        with self.assertRaisesRegex(LidarError, "checked-in"):
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
        roof = np.asarray([40 + x % 3 for x in range(2, 18)], dtype=np.float64)
        expected_roof = np.quantile(roof, (0.1, 0.5, 0.9)) * 0.3048006096012192
        expected_ground = 10 * 0.3048006096012192
        self.assertAlmostEqual(evidence["roof_p10_m"], expected_roof[0], places=5)
        self.assertAlmostEqual(evidence["roof_p50_m"], expected_roof[1], places=5)
        self.assertAlmostEqual(evidence["roof_p90_m"], expected_roof[2], places=5)
        self.assertAlmostEqual(
            evidence["height_p90_m"], expected_roof[2] - expected_ground, places=5
        )
        self.assertEqual(evidence["quality"], "usable")

    def test_dense_footprint_is_not_rejected_by_an_in_memory_match_limit(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        points = [(x, 5, 10, 2) for x in range(5)]
        points += [(x, 5, 40, 6) for x in range(10)]
        points += [(100 + x, 5, 20, 2) for x in range(5)]
        points += [(100 + x, 5, 60, 6) for x in range(10)]
        las = root / "dense.las"
        las.write_bytes(las_bytes(points))
        buildings = gpd.GeoDataFrame(
            {"building_id": ["one", "two"], "source_sha256": ["a" * 64, "a" * 64]},
            geometry=[box(-1, -1, 11, 11), box(99, -1, 111, 11)],
            crs=6565,
        )
        output = root / "evidence.parquet"
        # Thirty total samples exceeded the former patched 20-value guard.
        self.assertEqual(derive_evidence(las, buildings, output), 2)
        rows = pq.read_table(output).to_pylist()
        self.assertEqual({row["building_point_count"] for row in rows}, {10})

    def test_many_buildings_scan_the_las_point_stream_once(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        points = [(x, 5, 10, 2) for x in range(5)]
        points += [(x, 5, 40, 6) for x in range(10)]
        points += [(100 + x, 5, 20, 2) for x in range(5)]
        points += [(100 + x, 5, 60, 6) for x in range(10)]
        las = root / "many.las"
        las.write_bytes(las_bytes(points))
        buildings = gpd.GeoDataFrame(
            {"building_id": ["one", "two"], "source_sha256": ["a" * 64, "a" * 64]},
            geometry=[box(-1, -1, 11, 11), box(99, -1, 111, 11)],
            crs=6565,
        )
        with patch("isophilly_ingest.lidar.iter_las_points", wraps=iter_las_points) as iterator:
            self.assertEqual(derive_evidence(las, buildings, root / "evidence.parquet"), 2)
        self.assertEqual(iterator.call_count, 1)

    def test_interrupted_spill_is_cleaned_and_restart_succeeds(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        points = [(x, 5, 10, 2) for x in range(5)]
        points += [(x, 5, 40, 6) for x in range(10)]
        las = root / "restart.las"
        las.write_bytes(las_bytes(points))
        buildings = gpd.GeoDataFrame(
            {"building_id": ["one"], "source_sha256": ["a" * 64]},
            geometry=[box(-1, -1, 11, 11)],
            crs=6565,
        )
        output = root / "evidence.parquet"
        stale = root / ".lidar-work" / las.name
        stale.mkdir(parents=True)
        (stale / "stale").write_text("old")

        def interrupted(*args: object) -> object:
            del args
            raise RuntimeError("interrupted")

        with (
            patch("isophilly_ingest.lidar.iter_las_points", side_effect=interrupted),
            self.assertRaisesRegex(RuntimeError, "interrupted"),
        ):
            derive_evidence(las, buildings, output)
        self.assertFalse(stale.exists())

        self.assertEqual(derive_evidence(las, buildings, output), 1)
        self.assertFalse(stale.exists())
        metadata = pq.read_metadata(output).metadata
        assert metadata is not None
        self.assertEqual(metadata[b"lidar_point_passes"], b"1")
        self.assertGreater(int(metadata[b"lidar_spill_bytes"]), 0)


class DownloadTests(unittest.TestCase):
    def test_rejects_redirected_tile_response(self) -> None:
        payload = las_bytes([(10, 20, 30, 2)])
        tile = Tile(
            "26452E204072N.las", "https://example.test/tile.las", len(payload), (0, 0, 1, 1), True
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)

        def handler(request: httpx.Request) -> httpx.Response:
            if request.url.host == "example.test":
                return httpx.Response(302, headers={"Location": "https://evil.test/tile.las"})
            return httpx.Response(200, content=payload, request=request)

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", root / "raw"),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler), follow_redirects=True) as client,
            self.assertRaisesRegex(LidarError, "redirected outside"),
        ):
            download_tile(tile, inventory_for(tile), client)

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

    def test_rejects_short_200_response_before_writing_source(self) -> None:
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

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, content=payload[:-1], request=request)

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
            self.assertRaisesRegex(LidarError, "Content-Length"),
        ):
            download_tile(tile, inventory_for(tile), client)

        self.assertFalse((raw / tile.name).exists())
        self.assertFalse((raw / f"{tile.name}.part").exists())
        progress = json.loads((root / "progress.json").read_text())
        self.assertEqual(progress["tiles"][tile.name]["status"], "downloading")

    def test_exact_pinned_response_is_not_downloaded_when_las_header_requires_more(self) -> None:
        payload = bytearray(las_bytes([(10, 20, 30, 2)]))
        struct.pack_into("<Q", payload, 247, 100)
        source = bytes(payload)
        tile = Tile(
            "26452E204072N.las",
            "https://example.test/tile.las",
            len(source),
            (0, 0, 1, 1),
            True,
        )
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        raw = root / "raw"

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, content=source, request=request)

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
            self.assertRaisesRegex(LidarError, "point data is truncated"),
        ):
            download_tile(tile, inventory_for(tile), client)

        self.assertFalse((raw / tile.name).exists())
        self.assertEqual((raw / f"{tile.name}.part").read_bytes(), source)
        progress = json.loads((root / "progress.json").read_text())
        self.assertEqual(progress["tiles"][tile.name]["status"], "downloading")

    def test_cached_exact_but_truncated_source_becomes_terminal_rejection(self) -> None:
        payload = bytearray(las_bytes([(10, 20, 30, 2)]))
        struct.pack_into("<Q", payload, 247, 100)
        source = bytes(payload)
        tile = Tile(
            "26452E204072N.las",
            "https://example.test/tile.las",
            len(source),
            (0, 0, 1, 1),
            True,
        )
        inventory = inventory_for(tile)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        raw = root / "raw"
        derived = root / "derived"
        raw.mkdir()
        destination = raw / tile.name
        destination.write_bytes(source)
        digest = hashlib.sha256(source).hexdigest()
        (root / "progress.json").write_text(
            json.dumps(
                {
                    "schema_version": 2,
                    "inventory_listing_sha256": "a" * 64,
                    "inventory_city_sha256": "b" * 64,
                    "inventory_building_sha256": "c" * 64,
                    "tiles": {
                        tile.name: {
                            "status": "downloaded",
                            "expected_bytes": len(source),
                            "bytes": len(source),
                            "sha256": digest,
                        }
                    },
                }
            )
        )

        def handler(request: httpx.Request) -> httpx.Response:
            self.fail(f"cached pinned source should not be fetched again: {request.url}")

        with (
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.PROGRESS_PATH", root / "progress.json"),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            process_tile(tile, inventory, client, discard_raw=True)
            self.assertEqual(pending_tiles(inventory), ())

        self.assertFalse(destination.exists())
        metadata = json.loads((derived / f"{tile.name}.json").read_text())
        self.assertEqual(metadata["result"], "rejected_source")
        self.assertEqual(metadata["actual_bytes"], len(source))
        self.assertEqual(metadata["expected_minimum_bytes"], 375 + 100 * 30)
        self.assertEqual(metadata["source_sha256"], digest)
        progress = json.loads((root / "progress.json").read_text())
        self.assertEqual(progress["tiles"][tile.name]["status"], "rejected_source")


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
            "las": {
                "version": "1.4",
                "point_format": 6,
                "point_record_bytes": 30,
                "point_count": 1,
                "point_data_offset": 375,
                "scales": [0.01, 0.01, 0.01],
                "offsets": [0.0, 0.0, 0.0],
                "bounds_ft": [0.0, 0.0, 10.0, 10.0],
            },
        }
        (root / f"{tile.name}.json").write_text(json.dumps(metadata))
        return output

    def _write_rejection(self, root: Path, tile: Tile, inventory: Inventory) -> None:
        metadata = {
            "result": "rejected_source",
            "inventory_listing_sha256": inventory.listing_sha256,
            "inventory_city_sha256": inventory.city_sha256,
            "inventory_building_sha256": inventory.building_sha256,
            "source_url": tile.url,
            "source_bytes": tile.bytes,
            "source_sha256": "e" * 64,
            "actual_bytes": tile.bytes,
            "expected_minimum_bytes": max(tile.bytes, 227) + 100,
            "error": "LAS point data is truncated",
            "las": {
                "version": "1.4",
                "point_format": 6,
                "point_record_bytes": 20,
                "point_count": 5,
                "point_data_offset": max(tile.bytes, 227),
                "scales": [0.01, 0.01, 0.01],
                "offsets": [0.0, 0.0, 0.0],
                "bounds_ft": [0.0, 0.0, 10.0, 10.0],
            },
        }
        (root / f"{tile.name}.json").write_text(json.dumps(metadata))

    def test_recheck_keeps_rejection_until_replacement_is_structurally_valid(self) -> None:
        payload = bytearray(las_bytes([(10, 20, 30, 2)]))
        struct.pack_into("<Q", payload, 247, 100)
        source = bytes(payload)
        tile = Tile(
            "26452E204072N.las", "https://example.test/tile.las", len(source), (0, 0, 1, 1), True
        )
        inventory = inventory_for(tile)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        derived = root / "derived"
        raw = root / "raw"
        derived.mkdir()
        self._write_rejection(derived, tile, inventory)
        rejection_path = derived / f"{tile.name}.json"
        rejection = json.loads(rejection_path.read_text())
        rejection["expected_minimum_bytes"] = 375 + 100 * 30
        rejection["las"]["point_record_bytes"] = 30
        rejection["las"]["point_count"] = 100
        rejection["las"]["point_data_offset"] = 375
        rejection_path.write_text(json.dumps(rejection))

        def handler(request: httpx.Request) -> httpx.Response:
            return httpx.Response(200, content=source, request=request)

        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.RAW_LAS_DIR", raw),
            httpx.Client(transport=httpx.MockTransport(handler)) as client,
        ):
            self.assertEqual(recheck_rejected_sources(inventory, client, discard_raw=True), (1, 0))
        self.assertTrue((derived / f"{tile.name}.json").exists())
        self.assertFalse((raw / tile.name).exists())

    def test_rejected_header_minimum_is_recomputed(self) -> None:
        tile = Tile("26452E204072N.las", "https://example.test/tile.las", 500, (0, 0, 1, 1), True)
        inventory = inventory_for(tile)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        self._write_rejection(root, tile, inventory)
        path = root / f"{tile.name}.json"
        metadata = json.loads(path.read_text())
        metadata["expected_minimum_bytes"] += 1
        path.write_text(json.dumps(metadata))
        with patch("isophilly_ingest.lidar.DERIVED_DIR", root):
            self.assertEqual(pending_tiles(inventory), (tile,))

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
            partial_manifest = json.loads(partial.with_suffix(".json").read_text())
            self.assertTrue(partial_manifest["partial"])
            self.assertFalse(partial_manifest["source_coverage_complete"])
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

    def test_merge_accounts_for_rejected_source_without_becoming_locally_partial(self) -> None:
        usable = Tile(
            "26452E204072N.las", "https://example.test/usable.las", 100, (0, 0, 1, 1), True
        )
        rejected = Tile(
            "26479E201432N.las", "https://example.test/rejected.las", 200, (0, 0, 1, 1), True
        )
        rejected_second = Tile(
            "26505E201432N.las", "https://example.test/rejected-two.las", 300, (0, 0, 1, 1), True
        )
        inventory = inventory_for(usable, rejected, rejected_second)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        derived = root / "derived"
        derived.mkdir()
        merged = root / "building-evidence.parquet"
        footprints = root / "footprints.parquet"
        self._write_artifact(
            derived,
            usable,
            inventory,
            [self._row(usable, building_points=200, ground_points=100, height=10, spread=1)],
        )
        self._write_rejection(derived, rejected, inventory)
        self._write_rejection(derived, rejected_second, inventory)
        gpd.GeoDataFrame(
            {"building_id": ["gap-building"], "source_sha256": [inventory.building_sha256]},
            geometry=[box(1, 1, 2, 2)],
            crs=6565,
        ).to_parquet(footprints, write_covering_bbox=True)

        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.FOOTPRINTS_PATH", footprints),
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", merged),
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", root / "partial.parquet"),
        ):
            self.assertEqual(merge_evidence(inventory), 1)

        manifest = json.loads(merged.with_suffix(".json").read_text())
        self.assertFalse(manifest["partial"])
        self.assertFalse(manifest["source_coverage_complete"])
        self.assertEqual(manifest["accounted_tiles"], 3)
        self.assertEqual(manifest["rejected_source_tiles"], [rejected.name, rejected_second.name])
        self.assertEqual(
            [gap["intersecting_footprints"] for gap in manifest["rejected_source_gaps"]],
            [1, 1],
        )
        self.assertEqual(manifest["rejected_source_intersecting_footprints_unique"], 1)
        provenance = manifest["rejected_source_provenance"]
        self.assertEqual(
            [item["tile"] for item in provenance], [rejected.name, rejected_second.name]
        )
        self.assertEqual(provenance[0]["source_sha256"], "e" * 64)
        self.assertGreater(provenance[0]["expected_minimum_bytes"], provenance[0]["actual_bytes"])
        with patch("isophilly_ingest.lidar.INVENTORY_PATH", root / "missing-inventory.json"):
            self.assertEqual(
                load_height_evidence(merged, inventory.building_sha256), {"same-building": 10}
            )

    def test_gap_failure_preserves_existing_merge_pair(self) -> None:
        usable = Tile(
            "26452E204072N.las", "https://example.test/usable.las", 100, (0, 0, 1, 1), True
        )
        rejected = Tile(
            "26479E201432N.las", "https://example.test/rejected.las", 200, (0, 0, 1, 1), True
        )
        inventory = inventory_for(usable, rejected)
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        derived = root / "derived"
        derived.mkdir()
        merged = root / "building-evidence.parquet"
        merged_metadata = merged.with_suffix(".json")
        merged.write_bytes(b"existing parquet")
        merged_metadata.write_text("existing metadata")
        self._write_artifact(
            derived,
            usable,
            inventory,
            [self._row(usable, building_points=200, ground_points=100, height=10, spread=1)],
        )
        self._write_rejection(derived, rejected, inventory)
        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", merged),
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", root / "partial.parquet"),
            patch(
                "isophilly_ingest.lidar._rejected_source_gap",
                side_effect=LidarError("gap audit failed"),
            ),
            self.assertRaisesRegex(LidarError, "gap audit failed"),
        ):
            merge_evidence(inventory)
        self.assertEqual(merged.read_bytes(), b"existing parquet")
        self.assertEqual(merged_metadata.read_text(), "existing metadata")
        self.assertFalse(merged.with_suffix(".parquet.part").exists())
        self.assertFalse(merged_metadata.with_suffix(".json.part").exists())

        with (
            patch("isophilly_ingest.lidar.DERIVED_DIR", derived),
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", merged),
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", root / "partial.parquet"),
            patch(
                "isophilly_ingest.lidar._rejected_source_gap",
                return_value=(
                    {
                        "tile": rejected.name,
                        "bounds_ft": [0.0, 0.0, 10.0, 10.0],
                        "intersecting_footprints": 0,
                    },
                    set(),
                ),
            ),
            patch(
                "isophilly_ingest.lidar._validate_staged_merge",
                side_effect=LidarError("stage audit failed"),
            ),
            self.assertRaisesRegex(LidarError, "stage audit failed"),
        ):
            merge_evidence(inventory)
        self.assertEqual(merged.read_bytes(), b"existing parquet")
        self.assertEqual(merged_metadata.read_text(), "existing metadata")
        self.assertFalse(merged.with_suffix(".parquet.part").exists())
        self.assertFalse(merged_metadata.with_suffix(".json.part").exists())

    def test_second_replace_failure_restores_prior_merge_pair(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        destination = root / "building-evidence.parquet"
        metadata_destination = destination.with_suffix(".json")
        temporary = destination.with_suffix(".parquet.part")
        metadata_temporary = metadata_destination.with_suffix(".json.part")
        destination.write_bytes(b"old parquet")
        metadata_destination.write_text("old metadata")
        temporary.write_bytes(b"new parquet")
        metadata_temporary.write_text("new metadata")
        original_replace = Path.replace

        def fail_metadata_replace(source: Path, target: Path) -> Path:
            if source == metadata_temporary and target == metadata_destination:
                raise OSError("forced metadata replace failure")
            return original_replace(source, target)

        with (
            patch.object(Path, "replace", autospec=True, side_effect=fail_metadata_replace),
            self.assertRaisesRegex(OSError, "forced metadata replace failure"),
        ):
            _publish_staged_merge(destination, temporary, metadata_temporary)
        self.assertEqual(destination.read_bytes(), b"old parquet")
        self.assertEqual(metadata_destination.read_text(), "old metadata")
        self.assertFalse(destination.with_suffix(".parquet.backup").exists())
        self.assertFalse(destination.with_suffix(".json.backup").exists())
        self.assertFalse(destination.with_suffix(".publish.json").exists())

    def test_crash_recovery_restores_prior_pair_before_metadata_publish(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        destination = root / "building-evidence.parquet"
        metadata_destination = destination.with_suffix(".json")
        destination.write_bytes(b"new parquet installed before crash")
        destination.with_suffix(".parquet.backup").write_bytes(b"old parquet")
        destination.with_suffix(".json.backup").write_text("old metadata")
        destination.with_suffix(".publish.json").write_text(
            json.dumps({"schema_version": 1, "had_parquet": True, "had_metadata": True})
        )

        _recover_merge_publication(destination)

        self.assertEqual(destination.read_bytes(), b"old parquet")
        self.assertEqual(metadata_destination.read_text(), "old metadata")
        self.assertFalse(destination.with_suffix(".parquet.backup").exists())
        self.assertFalse(destination.with_suffix(".json.backup").exists())
        self.assertFalse(destination.with_suffix(".publish.json").exists())

    def test_merge_recovers_both_destinations_before_early_no_path_exit(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        canonical = root / "canonical.parquet"
        partial = root / "partial.parquet"
        for destination in (canonical, partial):
            destination.write_bytes(b"interrupted new parquet")
            destination.with_suffix(".parquet.backup").write_bytes(b"prior parquet")
            destination.with_suffix(".json.backup").write_text("prior metadata")
            destination.with_suffix(".publish.json").write_text(
                json.dumps({"schema_version": 1, "had_parquet": True, "had_metadata": True})
            )

        with (
            patch("isophilly_ingest.lidar.MERGED_EVIDENCE_PATH", canonical),
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", partial),
            self.assertRaisesRegex(LidarError, "no validated derived"),
        ):
            merge_evidence(inventory_for())

        for destination in (canonical, partial):
            self.assertEqual(destination.read_bytes(), b"prior parquet")
            self.assertEqual(destination.with_suffix(".json").read_text(), "prior metadata")
            self.assertFalse(destination.with_suffix(".publish.json").exists())

    def test_reader_marker_fails_closed_even_when_active_parquet_is_missing(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "building-evidence.parquet"
        path.with_suffix(".publish.json").write_text(
            json.dumps({"schema_version": 1, "had_parquet": True, "had_metadata": True})
        )
        with self.assertRaisesRegex(LidarError, "interrupted.*poe lidar-merge"):
            preflight_merge_read(path)
        with self.assertRaisesRegex(LidarError, "interrupted.*poe lidar-merge"):
            load_height_evidence(path, "c" * 64)

    def test_crash_recovery_accepts_fully_valid_new_pair_and_cleans_backups(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        destination = root / "building-evidence.parquet"
        pq.write_table(pa.table({"value": [1]}), destination)
        metadata_destination = destination.with_suffix(".json")
        metadata_destination.write_text(
            json.dumps(
                {
                    "schema_version": 3,
                    "output_file": destination.name,
                    "output_bytes": destination.stat().st_size,
                    "output_sha256": hashlib.sha256(destination.read_bytes()).hexdigest(),
                    "rows": 1,
                }
            )
        )
        destination.with_suffix(".parquet.backup").write_bytes(b"old parquet")
        destination.with_suffix(".json.backup").write_text("old metadata")
        destination.with_suffix(".publish.json").write_text(
            json.dumps({"schema_version": 1, "had_parquet": True, "had_metadata": True})
        )

        _recover_merge_publication(destination)

        self.assertEqual(pq.read_table(destination).to_pydict(), {"value": [1]})
        self.assertEqual(json.loads(metadata_destination.read_text())["schema_version"], 3)
        self.assertFalse(destination.with_suffix(".parquet.backup").exists())
        self.assertFalse(destination.with_suffix(".json.backup").exists())
        self.assertFalse(destination.with_suffix(".publish.json").exists())

    def test_schema_two_merge_requires_regeneration(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        merged = root / "building-evidence.parquet"
        merged.write_bytes(b"legacy")
        merged.with_suffix(".json").write_text(json.dumps({"schema_version": 2}))
        with self.assertRaisesRegex(LidarError, "schema 2 is legacy.*lidar-merge"):
            load_height_evidence(merged, "c" * 64)

    def test_summary_lists_rejected_tiles_deterministically(self) -> None:
        first = Tile("26452E204072N.las", "https://example.test/one.las", 100, (0, 0, 1, 1), True)
        second = Tile("26479E201432N.las", "https://example.test/two.las", 200, (0, 0, 1, 1), True)
        inventory = inventory_for(second, first)

        def metadata(tile: Tile, _: Inventory) -> dict[str, object]:
            return {"result": "rejected_source", "tile": tile.name}

        with (
            patch("isophilly_ingest.lidar.pending_tiles", return_value=()),
            patch("isophilly_ingest.lidar.validate_tile_artifact", side_effect=metadata),
        ):
            summary = _summary(inventory)
        self.assertIn(f"rejected tiles {first.name}, {second.name}", summary)

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
            patch("isophilly_ingest.lidar.PARTIAL_EVIDENCE_PATH", root / "partial.parquet"),
        ):
            merge_evidence(inventory, allow_partial=True)
        manifest_path = merged.with_suffix(".json")
        manifest = json.loads(manifest_path.read_text())
        manifest["partial"] = True
        manifest["source_coverage_complete"] = False
        manifest_path.write_text(json.dumps(manifest))
        with patch("isophilly_ingest.lidar.INVENTORY_PATH", root / "missing-inventory.json"):
            with self.assertRaisesRegex(LidarError, "complete LiDAR merge"):
                load_height_evidence(merged, "c" * 64)
            self.assertEqual(load_height_evidence(merged, "c" * 64, allow_partial=True), {})


if __name__ == "__main__":
    unittest.main()
