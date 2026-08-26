from __future__ import annotations

import struct
from concurrent.futures import ThreadPoolExecutor
from io import BytesIO
from pathlib import Path
from urllib.request import urlopen

from PIL import Image
from pyproj import Transformer

ROOT = Path(__file__).resolve().parent.parent.parent
DATA = ROOT / "data" / "clean" / "philly.bin"
OUT = ROOT / "data" / "clean" / "aerial.png"
FEET_TO_METERS = 0.3048006096012192
SIZE = 1024
TILE_ZOOM = 11
TILE_SIZE = 256
WEB_MERCATOR_HALF = 20_037_508.342789244
SERVICE = "https://tiles.arcgis.com/tiles/fLeGjb7u4uXqeF9q/arcgis/rest/services/CityImagery_2023/MapServer/tile"


def source_bounds() -> tuple[float, float, float, float]:
    header = DATA.read_bytes()[:60]
    magic, version, epsg, *_counts, min_x, min_y, max_x, max_y = struct.unpack("<8sIIIII4d", header)
    if magic != b"GEOPHILY" or version != 1 or epsg != 2272:
        raise ValueError("unsupported geo-philly data")
    return min_x, min_y, max_x, max_y


def web_mercator_bounds(
    bounds: tuple[float, float, float, float],
) -> tuple[float, float, float, float]:
    transform = Transformer.from_crs(2272, 3857, always_xy=True)
    min_x, min_y, max_x, max_y = (value / FEET_TO_METERS for value in bounds)
    corners = [transform.transform(x, y) for x in (min_x, max_x) for y in (min_y, max_y)]
    xs, ys = zip(*corners, strict=True)
    return min(xs), min(ys), max(xs), max(ys)


def tile_image(row: int, column: int) -> Image.Image:
    with urlopen(f"{SERVICE}/{TILE_ZOOM}/{row}/{column}", timeout=60) as response:
        return Image.open(BytesIO(response.read())).convert("RGB")


def download() -> Image.Image:
    min_x, min_y, max_x, max_y = web_mercator_bounds(source_bounds())
    world_size = WEB_MERCATOR_HALF * 2 / 2**TILE_ZOOM
    min_column = int((min_x + WEB_MERCATOR_HALF) // world_size)
    max_column = int((max_x + WEB_MERCATOR_HALF) // world_size)
    min_row = int((WEB_MERCATOR_HALF - max_y) // world_size)
    max_row = int((WEB_MERCATOR_HALF - min_y) // world_size)
    columns = range(min_column, max_column + 1)
    rows = range(min_row, max_row + 1)
    mosaic = Image.new("RGB", (len(columns) * TILE_SIZE, len(rows) * TILE_SIZE))
    requests = [(row, column) for row in rows for column in columns]
    with ThreadPoolExecutor(max_workers=8) as pool:
        images = dict(
            zip(requests, pool.map(lambda tile: tile_image(*tile), requests), strict=True)
        )
    for row, column in requests:
        mosaic.paste(
            images[row, column], ((column - min_column) * TILE_SIZE, (row - min_row) * TILE_SIZE)
        )
    west = min_column * world_size - WEB_MERCATOR_HALF
    north = WEB_MERCATOR_HALF - min_row * world_size
    crop = (
        round((min_x - west) / world_size * TILE_SIZE),
        round((north - max_y) / world_size * TILE_SIZE),
        round((max_x - west) / world_size * TILE_SIZE),
        round((north - min_y) / world_size * TILE_SIZE),
    )
    return mosaic.crop(crop).resize((SIZE, SIZE), Image.Resampling.BILINEAR)


def main() -> None:
    image = download()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    image.save(OUT, format="PNG", optimize=True)
    print(f"wrote {OUT} ({OUT.stat().st_size / 1_000_000:.1f} MB)")


if __name__ == "__main__":
    main()
