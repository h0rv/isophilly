from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from scripts.export_static import export_site, parse_inventory


class StaticExportTests(unittest.TestCase):
    def test_exports_only_inventory_tiles_and_coverage(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            static = root / "static"
            static.mkdir()
            for name in (
                "index.html",
                "app.js",
                "city-overlay.js",
                "neighborhoods.json",
                "_headers",
            ):
                (static / name).write_text(name)

            version = "v1-test"
            tile_root = root / "data/tiles" / version
            tile_path = tile_root / "0/0/0.webp"
            tile_path.parent.mkdir(parents=True)
            tile_bytes = b"small-webp-fixture"
            tile_path.write_bytes(tile_bytes)
            (tile_root / ".complete").write_text("complete\n")
            digest = hashlib.sha256(tile_bytes).hexdigest()
            (tile_root / ".inventory").write_text(f"0/0/0/{len(tile_bytes)}/{digest}\n")
            current = root / "data/tiles/current.json"
            rich_views = []
            for view in ("se", "sw", "nw", "ne"):
                rich_version = f"{version}-rich-{view}"
                rich_root = root / "data/tiles" / rich_version
                rich_tile = rich_root / "0/0/0.webp"
                rich_tile.parent.mkdir(parents=True)
                rich_tile.write_bytes(tile_bytes)
                (rich_root / ".complete").write_text("complete\n")
                (rich_root / ".inventory").write_text(f"0/0/0/{len(tile_bytes)}/{digest}\n")
                rich_views.append({"id": view, "tile_version": rich_version})
            current.write_text(json.dumps({"tile_version": version, "rich": {"views": rich_views}}))

            output = root / "dist"
            files, _ = export_site(root, output)

            self.assertEqual(files, 16)
            self.assertEqual((output / "tiles/0/0/0.webp").read_bytes(), tile_bytes)
            coverage = json.loads((output / "coverage.json").read_text())
            self.assertEqual(coverage["tile_version"], version)
            self.assertEqual(coverage["tiles"], ["0/0/0"])
            self.assertEqual((output / "rich/se/tiles/0/0/0.webp").read_bytes(), tile_bytes)

    def test_rejects_inventory_coordinates_outside_zoom(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / ".inventory"
            inventory.write_text(f"1/2/0/1/{'0' * 64}\n")

            with self.assertRaisesRegex(ValueError, "out-of-range"):
                parse_inventory(inventory)

    def test_rejects_duplicate_inventory_tiles(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            inventory = Path(directory) / ".inventory"
            line = f"0/0/0/1/{'0' * 64}\n"
            inventory.write_text(line + line)

            with self.assertRaisesRegex(ValueError, "duplicate"):
                parse_inventory(inventory)


if __name__ == "__main__":
    unittest.main()
