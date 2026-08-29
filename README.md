# geo-philly

See [live city overlays](docs/LIVE_CITY.md) for SEPTA, neighborhood, and day/night data provenance.

A small, deterministic isometric map of Philadelphia built from official City
building footprints and heights, the official 2015 Center City textured 3D
scene, and 2024 City aerial photography. Explore the whole city in a browser,
from the regional silhouette to individual buildings.

The expensive geospatial import happens once in Python. A compact Rust service
loads that result, renders lossless WebP tiles in parallel, and caches them on disk. The
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

Open <http://127.0.0.1:3000>. The renderer uses deterministic pixel processing.
`ingest` downloads official City Limits, Building Footprints, Hydrology, PPR
Properties, and Street Centerline snapshots. It also downloads height-backed
OpenStreetMap building parts and the official 2015 Center City 3D model. It
writes the compact `philly.bin` and `streets.bin` inputs plus a `meta.json`
provenance record. The download and conversion are the slow first run step.
The ingest rejects short building exports and uses the newest verified complete
snapshot instead of replacing a full city artifact with partial live data.
Existing checkouts must rerun `ingest` because world format version 5 adds the
texture digest, atlas IDs, triangle UV coordinates, and texture atlases.
`prebuild` renders the detailed z8 scene and creates z0 through z7 by resizing
those tiles. This gives every overview the same textured scene instead of a
different drawing style. The command uses four times the available logical CPU
count, capped at 32 workers. The extra workers keep downloads and image writes
moving while CPU work runs. It resumes from existing z8 tiles and writes a
completion marker only after the whole pyramid is ready. Run
`uv run --locked poe prebuild --jobs N` to choose a different worker count.

The server only reads z0 through z8 from `data/tiles/`. At closer view levels,
the browser magnifies the canonical z8 pixels with nearest-neighbor sampling.
It does not switch to another renderer or replace textures with plain geometry.

Aerial crops come from the native one inch 2024 PASDA service. Each z8 request
uses a 512 pixel crop of the source area for one isometric tile. Up to 32
prebuild jobs can fetch source requests at once. Source crops are stored under
`data/aerial/` with a hard 8 GiB limit.

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
Official Center City I3S scene + City footprints and limits + PASDA aerial
             |
             v
Python + GeoPandas/Shapely  ->  data/clean/philly.bin
                                      |
                                      v
Rust + rstar/tiny-skia      ->  one textured citywide z8 scene
                                      |
                                      v
Rust + image               ->  z0 through z7 image pyramid
                                      |
                                      v
                              canvas deep-zoom viewer
```

Python is not in the request path. The Rust server starts from one compact
geometry file and serves the completed image pyramid. It does not load aerial
sources or texture atlases and does not render during requests. Source imagery
and rendered tiles persist across runs. The service binds to localhost by
default. A production deployment can serve the pyramid as static files from an
edge cache.

## Project status

This is an early public prototype, not an authoritative map or surveying tool.
The current City footprint layer contains 545,672 usable structure polygons.
Outside the detailed mesh, roofs sample the matching aerial pixels and walls
use a muted palette derived from those pixels. The source height fields drive
extrusion, so this citywide treatment preserves real outlines and local color
without claiming to reconstruct unseen facades. The detailed Center City scene
contains 367 atlas chunks and 294,443 textured triangles. Those triangles
preserve real facades, setbacks, roof equipment, sloped roofs, and landmark
silhouettes.

The source code is [MIT licensed](LICENSE). That license does not grant rights
to the source datasets or generated map tiles; their provenance and publication
notes are in [docs/DATA.md](docs/DATA.md).
