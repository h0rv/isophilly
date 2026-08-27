# geo-philly

A small, deterministic isometric map of Philadelphia built from public City
geometry. Explore the whole city in a browser, from the regional silhouette to
individual building footprints.

The expensive geospatial import happens once in Python. A compact Rust service
loads that result, renders PNG tiles in parallel, and caches them on disk. The
viewer is one typed JavaScript file and a canvas. No AI-generated imagery, API
key, database, or browser framework is involved.

## Run it

Install [uv](https://docs.astral.sh/uv/) and Rust 1.95. Then:

```sh
uv sync --locked --all-groups
uv run --locked poe ingest
uv run --locked poe prebuild
uv run --locked poe serve
```

Open <http://127.0.0.1:3000>. `ingest` downloads official City Limits, Building
Footprints, Hydrology, PPR Properties, and Street Centerline snapshots. It
writes the compact `philly.bin` and `streets.bin` inputs plus a `meta.json`
provenance record. The download and conversion are the slow first-run step.
`prebuild` creates the overview tiles; deeper tiles render lazily and are reused
from `data/tiles/`.

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
OpenDataPhilly / official City ArcGIS snapshots
             |
             v
Python + GeoPandas/Shapely  ->  data/clean/philly.bin
                                      |
                                      v
Rust + rstar/tiny-skia      ->  cached 256 px PNG tiles
                                      |
                                      v
                              canvas deep-zoom viewer
```

Python is not in the request path. The Rust server starts from one compact
binary file, bounds concurrent tile renders, warms low zooms in the background,
and serves the dependency-free viewer. The service binds to localhost by
default; production deployment should put it behind a static/edge cache and
version tile URLs when the data or renderer changes.

## Project status

This is an early public prototype, not an authoritative map or surveying tool.
The current City footprint layer has roughly 546,000 structures. Building
height is estimated when the source does not supply it, so the geometry is
recognizable but not yet a complete 3D model.

The source code is [MIT licensed](LICENSE). That license does not grant rights
to the source datasets or generated map tiles; their provenance and publication
notes are in [docs/DATA.md](docs/DATA.md).
