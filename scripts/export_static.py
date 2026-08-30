from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import tempfile
from dataclasses import dataclass
from pathlib import Path

CLOUDFLARE_FILE_LIMIT = 20_000
CLOUDFLARE_ASSET_SIZE_LIMIT = 25 * 1024 * 1024
TILE_DIGEST = re.compile(r"[0-9a-f]{64}")
TILE_VERSION = re.compile(r"[0-9A-Za-z-]+")
STATIC_FILES = ("index.html", "app.js", "city-overlay.js", "neighborhoods.json", "_headers")


@dataclass(frozen=True, slots=True)
class Scene:
    tile_version: str


@dataclass(frozen=True, slots=True)
class Tile:
    z: int
    x: int
    y: int
    size: int
    sha256: str

    @property
    def key(self) -> str:
        return f"{self.z}/{self.x}/{self.y}"

    @property
    def relative_path(self) -> Path:
        return Path(str(self.z), str(self.x), f"{self.y}.webp")


def parse_scene(path: Path) -> Scene:
    raw: object = json.loads(path.read_text())
    if not isinstance(raw, dict):
        raise ValueError("scene manifest must be an object")
    tile_version = raw.get("tile_version")
    if not isinstance(tile_version, str) or TILE_VERSION.fullmatch(tile_version) is None:
        raise ValueError("scene manifest has an invalid tile version")
    return Scene(tile_version)


def parse_inventory(path: Path) -> tuple[Tile, ...]:
    tiles: list[Tile] = []
    keys: set[tuple[int, int, int]] = set()
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        fields = line.split("/")
        if len(fields) != 5:
            raise ValueError(f"invalid tile inventory line {line_number}")
        try:
            z, x, y, size = (int(value) for value in fields[:4])
        except ValueError as error:
            raise ValueError(f"invalid number on tile inventory line {line_number}") from error
        digest = fields[4]
        if (
            z < 0
            or z > 8
            or x < 0
            or x >= 2**z
            or y < 0
            or y >= 2**z
            or size <= 0
            or TILE_DIGEST.fullmatch(digest) is None
        ):
            raise ValueError(f"out-of-range tile inventory line {line_number}")
        key = (z, x, y)
        if key in keys:
            raise ValueError(f"duplicate tile inventory line {line_number}")
        keys.add(key)
        tiles.append(Tile(z, x, y, size, digest))
    if not tiles:
        raise ValueError("tile inventory is empty")
    return tuple(sorted(tiles, key=lambda tile: (tile.z, tile.x, tile.y)))


def verify_tile(path: Path, tile: Tile) -> None:
    if path.stat().st_size != tile.size:
        raise ValueError(f"tile size does not match inventory: {tile.key}")
    with path.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    if digest != tile.sha256:
        raise ValueError(f"tile digest does not match inventory: {tile.key}")


def link_or_copy(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    try:
        os.link(source, destination)
    except OSError:
        shutil.copy2(source, destination)


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, separators=(",", ":")) + "\n")


def export_site(project_root: Path, output: Path) -> tuple[int, int]:
    scene_path = project_root / "data/tiles/current.json"
    scene = parse_scene(scene_path)
    tile_root = project_root / "data/tiles" / scene.tile_version
    if not (tile_root / ".complete").is_file():
        raise ValueError("tile pyramid is incomplete")
    tiles = parse_inventory(tile_root / ".inventory")

    output_parent = output.parent
    output_parent.mkdir(parents=True, exist_ok=True)
    temporary = Path(tempfile.mkdtemp(prefix=f".{output.name}-", dir=output_parent))
    try:
        for name in STATIC_FILES:
            shutil.copy2(project_root / "static" / name, temporary / name)
        texture_coverage = project_root / "data/clean/texture-coverage.json"
        if texture_coverage.is_file():
            shutil.copy2(texture_coverage, temporary / "texture-coverage.json")
        shutil.copy2(scene_path, temporary / "meta")
        write_json(
            temporary / "coverage.json",
            {
                "schema_version": 1,
                "tile_version": scene.tile_version,
                "tiles": [tile.key for tile in tiles],
            },
        )
        for tile in tiles:
            source = tile_root / tile.relative_path
            verify_tile(source, tile)
            link_or_copy(source, temporary / "tiles" / tile.relative_path)

        assets = [path for path in temporary.rglob("*") if path.is_file()]
        largest = max(path.stat().st_size for path in assets)
        if len(assets) > CLOUDFLARE_FILE_LIMIT:
            raise ValueError(
                f"static export has {len(assets)} files; Cloudflare allows {CLOUDFLARE_FILE_LIMIT}"
            )
        if largest > CLOUDFLARE_ASSET_SIZE_LIMIT:
            raise ValueError(
                f"largest static asset is {largest} bytes; Cloudflare allows "
                f"{CLOUDFLARE_ASSET_SIZE_LIMIT}"
            )
        total_size = sum(path.stat().st_size for path in assets)
        if output.exists():
            shutil.rmtree(output)
        temporary.replace(output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return len(assets), total_size


def main() -> None:
    project_root = Path(__file__).resolve().parent.parent
    files, total_size = export_site(project_root, project_root / "dist")
    print(f"exported {files:,} static files ({total_size / 1024 / 1024:.1f} MiB) to dist/")


if __name__ == "__main__":
    main()
