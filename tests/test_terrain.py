from __future__ import annotations

import hashlib
import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import geopandas as gpd
import pyarrow as pa
import pyarrow.parquet as pq
from pyproj import Transformer
from shapely.geometry import box

from isophilly_ingest.models import Bounds
from isophilly_ingest.terrain import (
    ARTIFACT_NAME,
    COVERAGE_DIRECT,
    COVERAGE_REJECTED_GAP,
    EXPECTED_REJECTED_TILES,
    MAGIC,
    PREFIX,
    TerrainError,
    build_terrain,
    load_terrain,
    parse_evidence_manifest,
)


class TerrainBuilderTests(unittest.TestCase):
    def _paths(self, root: Path) -> tuple[Path, Path, Path]:
        evidence = root / "building-evidence.parquet"
        footprints = root / "footprints.parquet"
        destination = root / ARTIFACT_NAME
        transformer = Transformer.from_crs(32129, 6565, always_xy=True)
        source_points = [
            (811_040.0, 62_720.0),
            (811_060.0, 62_740.0),
            (811_080.0, 62_760.0),
            (811_300.0, 62_730.0),
        ]
        xs, ys = transformer.transform(
            [point[0] for point in source_points], [point[1] for point in source_points]
        )
        identifiers = ["one", "two", "three", "gap"]
        source_sha256 = "a" * 64
        gpd.GeoDataFrame(
            {"building_id": identifiers, "source_sha256": [source_sha256] * len(identifiers)},
            geometry=[box(x - 4, y - 4, x + 4, y + 4) for x, y in zip(xs, ys, strict=True)],
            crs=6565,
        ).to_parquet(footprints)
        rows = [
            {
                "building_id": identifier,
                "source_footprints_sha256": source_sha256,
                "ground_point_count": 30,
                "ground_elevation_m": elevation,
            }
            for identifier, elevation in zip(identifiers, [10.0, 12.0, 14.0, 20.0], strict=True)
        ]
        pq.write_table(pa.Table.from_pylist(rows), evidence)
        gap_x, gap_y = xs[-1], ys[-1]
        gap_bounds = [gap_x - 20, gap_y - 20, gap_x + 20, gap_y + 20]
        rejected_gaps = [
            {
                "tile": tile,
                "bounds_ft": gap_bounds if tile == "27086E256872N.las" else [1, 1, 2, 2],
                "intersecting_footprints": 1 if tile == "27086E256872N.las" else 0,
            }
            for tile in EXPECTED_REJECTED_TILES
        ]
        manifest = {
            "output_file": evidence.name,
            "output_sha256": hashlib.sha256(evidence.read_bytes()).hexdigest(),
            "source_footprints_sha256": source_sha256,
            "source_coverage_complete": False,
            "rejected_source_count": len(rejected_gaps),
            "rejected_source_gaps": rejected_gaps,
        }
        evidence.with_suffix(".json").write_text(json.dumps(manifest))
        return evidence, footprints, destination

    def test_builder_is_deterministic_and_marks_rejected_gap_cells(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        evidence, footprints, destination = self._paths(root)
        bounds = Bounds(810_944.0, 62_428.0, 811_600.0, 63_100.0)

        first = build_terrain(
            destination, bounds, evidence_path=evidence, footprints_path=footprints
        )
        first_bytes = destination.read_bytes()
        second = build_terrain(
            destination, bounds, evidence_path=evidence, footprints_path=footprints
        )
        artifact = load_terrain(destination)

        self.assertEqual(first.sha256, second.sha256)
        self.assertEqual(first_bytes, destination.read_bytes())
        self.assertEqual(artifact.sha256, first.sha256)
        self.assertGreater(first.direct_cells, 0)
        self.assertGreater(first.rejected_gap_cells, 0)
        self.assertIn(COVERAGE_DIRECT, artifact.coverage)
        self.assertIn(COVERAGE_REJECTED_GAP, artifact.coverage)
        self.assertEqual(destination.read_bytes()[:8], MAGIC)

    def test_loader_rejects_tampered_payload(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        evidence, footprints, destination = self._paths(root)
        build_terrain(
            destination,
            Bounds(810_944.0, 62_428.0, 811_600.0, 63_100.0),
            evidence_path=evidence,
            footprints_path=footprints,
        )
        data = bytearray(destination.read_bytes())
        data[-1] ^= 1
        destination.write_bytes(data)

        with self.assertRaisesRegex(TerrainError, "checksum"):
            load_terrain(destination)

    def test_manifest_requires_canonical_gap_order(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        evidence, _, _ = self._paths(root)
        raw = json.loads(evidence.with_suffix(".json").read_text())
        raw["rejected_source_gaps"] = [
            raw["rejected_source_gaps"][0],
            {
                "tile": "26452E204072N.las",
                "bounds_ft": [1, 1, 2, 2],
                "intersecting_footprints": 0,
            },
        ]
        raw["rejected_source_count"] = 9
        evidence.with_suffix(".json").write_text(json.dumps(raw))

        with self.assertRaisesRegex(TerrainError, "not canonical"):
            parse_evidence_manifest(evidence.with_suffix(".json"), evidence)

    def test_real_manifest_exposes_all_eight_terminal_gaps(self) -> None:
        manifest = parse_evidence_manifest(
            Path("data/lidar-2025/building-evidence.json"),
            Path("data/lidar-2025/building-evidence.parquet"),
        )

        self.assertFalse(manifest.source_coverage_complete)
        self.assertEqual(len(manifest.gaps), 8)
        self.assertEqual(
            [gap.tile for gap in manifest.gaps],
            [
                "26822E227832N.las",
                "26848E238392N.las",
                "26954E227832N.las",
                "27086E256872N.las",
                "27086E259512N.las",
                "27086E262152N.las",
                "27086E264792N.las",
                "27086E267432N.las",
            ],
        )

    def test_prefix_has_the_fixed_binary_contract(self) -> None:
        self.assertEqual(PREFIX.size, 16)
        self.assertEqual(PREFIX.pack(MAGIC, 1, 0), b"ISOTERN1\x01\x00\x00\x00\x00\x00\x00\x00")


if __name__ == "__main__":
    unittest.main()
