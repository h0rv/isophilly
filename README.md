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

Open <http://127.0.0.1:3000>. The renderer uses deterministic pixel processing.
`ingest` downloads official City Limits, Building Footprints, Hydrology, PPR
Properties, and Street Centerline snapshots. It also downloads height-backed
OpenStreetMap building parts and the official 2015 Center City 3D model. It
writes the compact `philly.bin` and `streets.bin` inputs plus a `meta.json`
provenance record. The download and conversion are the slow first run step.
The ingest rejects short building exports and uses the newest verified complete
snapshot instead of replacing a full city artifact with partial live data.
Existing checkouts must rerun `ingest` because world format version 4 adds the
3D mesh records and sourced facade colors.
`prebuild` renders the detailed z8 scene and creates z0 through z7 by resizing
those tiles. This gives every overview the same textured scene instead of a
different drawing style. The command uses all available processors, resumes
from existing z8 tiles, and writes a completion marker only after the whole
pyramid is ready.

The server reads z0 through z8 from `data/tiles/`. It renders z9 through z12 on
demand and saves those tiles for later runs. The browser shows the textured z8
parent while a deeper tile loads. It never replaces a missing texture with a
plain geometry tile.

Aerial crops come from the native three inch 2025 PASDA service. Each request
uses a 512 pixel crop of the exact ground area for one isometric tile. Up to
eight source requests and tile renders can run at once. Source crops are stored
under `data/aerial/` with a hard 1 GiB limit.

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
OpenDataPhilly geometry + official Center City meshes + OSM parts + PASDA aerial
             |
             v
Python + GeoPandas/Shapely  ->  data/clean/philly.bin
                                      |
                                      v
Rust + rstar/tiny-skia      ->  textured pixel z8 tiles
                                      |
                                      v
Rust + image               ->  z0 through z7 image pyramid
                                      |
                                      v
                              canvas deep-zoom viewer
```

Python is not in the request path. The Rust server starts from one compact
geometry file and serves the completed image pyramid. It only renders deeper
tiles when they are requested. Source imagery and rendered tiles persist across
runs. The service binds to localhost by default. A production deployment can
serve the pyramid as static files from an edge cache.

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
