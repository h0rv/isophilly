# geo-philly

A small, deterministic isometric map of Philadelphia built from City geometry,
the official 2015 Center City 3D model, OpenStreetMap building parts, and 2025
City aerial photography. Explore the whole city in a browser, from the regional
silhouette to individual buildings.

The expensive geospatial import happens once in Python. A compact Rust service
loads that result, renders PNG tiles in parallel, and caches them on disk. The
viewer is one typed JavaScript file and a canvas. No AI-generated imagery,
database, or browser framework is involved.

## Run it

Install [uv](https://docs.astral.sh/uv/) and Rust 1.95. Then:

```sh
uv sync --locked --all-groups
uv run --locked poe ingest
uv run --locked poe prebuild
uv run --locked poe serve
```

Open <http://127.0.0.1:3000>. The default is deterministic pixel processing.
`ingest` downloads official City Limits, Building Footprints, Hydrology, PPR
Properties, and Street Centerline snapshots. It also downloads height-backed
OpenStreetMap building parts and the official 2015 Center City 3D model. It
writes the compact `philly.bin` and `streets.bin` inputs plus a `meta.json`
provenance record. The download and conversion are the slow first run step.
Existing checkouts must rerun `ingest` because world format version 4 adds the
3D mesh records and sourced facade colors.
`prebuild` creates the overview tiles. Requested tiles through z8 are reused
from `data/tiles/`; z9+ render lazily and stay browser/edge-only so local disk
usage remains bounded. Aerial source crops come from the native three-inch 2025
PASDA service as a 1024 pixel crop of the exact ground extent for each requested
isometric tile. The first visit needs network access. Cold source requests are
serialized so the public image service is not overloaded. A failed image request
returns a temporary geometry tile and is retried later. Source crops are reused
by both texture modes under `data/aerial/`. The cache has a hard 1 GiB ceiling.

```sh
uv run --locked poe serve-full   # photographic aerial color
uv run --locked poe serve-plain  # original geometry-only palette
```

The same setting is available directly as `--texture pixel|full|none` on both
the `serve` and `prebuild` commands.

After the first ingest, the usual development loop is only:

```sh
uv run --locked poe serve
```

Generated data and tiles are intentionally gitignored. See the [data pipeline
and attribution notes](docs/DATA.md) before publishing a build.

## Quality checks

```sh
npm ci
uv run --locked poe check
uv run --locked poe visual
uv run --locked poe visual-full
```

Install a current Node.js LTS release and
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) (`cargo install
cargo-deny --locked`) before running the quality suite. The single Poe task
runs Ruff and ty for Python, Biome and TypeScript for the web viewer, and
rustfmt, Clippy, tests, a locked release build, and cargo-deny for Rust. CI runs
the same checks on Linux. Dependency updates are managed for all three
ecosystems plus GitHub Actions.

## How it fits together

```text
OpenDataPhilly geometry + official Center City meshes + OSM parts + PASDA aerial
             |
             v
Python + GeoPandas/Shapely  ->  data/clean/philly.bin
                                      |
                                      v
Rust + rstar/tiny-skia      ->  full or pixel-textured 256 px PNG tiles
                                      |
                                      v
                              canvas deep-zoom viewer
```

Python is not in the request path. The Rust server starts from one compact
geometry file, bounds concurrent source fetches and tile renders, warms low
zooms in the background, and serves the dependency-free viewer. Source imagery
and rendered tiles persist across runs within fixed cache limits. The service
binds to localhost by default; production deployment should prebuild or put it
behind a static/edge cache.

## Project status

This is an early public prototype, not an authoritative map or surveying tool.
The current City footprint layer has roughly 546,000 structures. The detailed
Center City area adds 859 official multipatch buildings and about 800
OpenStreetMap parts. The mesh geometry preserves setbacks, sloped roofs, and
landmark silhouettes. City footprints elsewhere still use one height per
outline, including an 8 metre fallback when the source has no usable value.

The source code is [MIT licensed](LICENSE). That license does not grant rights
to the source datasets or generated map tiles; their provenance and publication
notes are in [docs/DATA.md](docs/DATA.md).
