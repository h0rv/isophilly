# geo-philly

See [city overlays](docs/LIVE_CITY.md) for neighborhood and day/night data provenance.

A small, deterministic isometric map of Philadelphia built from official City
building footprints and heights, the official 2015 Center City textured 3D
scene, the legacy 2008/09 downtown and stadium-area textured models, and 2025
City aerial photography. Explore the whole city in a browser, from the regional
silhouette to individual buildings.

The expensive geospatial import happens once in Python. A Rust prebuilder loads
that result and renders lossless WebP tiles in parallel. The small HTTP service
reads only a scene manifest and the finished tiles. The viewer is typed
JavaScript and a canvas. No AI-generated imagery, database, or browser framework
is involved.

## Run it

Install [uv](https://docs.astral.sh/uv/) and Rust 1.95. Then:

```sh
uv sync --locked --all-groups
uv run --locked poe ingest
uv run --locked poe prebuild
uv run --locked poe serve
```

Open <http://127.0.0.1:3000>. The renderer uses deterministic pixel processing.
`ingest` downloads official City Limits, Building Footprints, and the 2015
Center City 3D model. It also imports the highest-detail legacy downtown and
stadium KML/COLLADA models.
For the downtown source, new checkouts download PASDA's smaller `kml00.zip`.
Existing checkouts reuse `data/raw/Philadelphia2008_downtown_kml.zip` when it is
present. It writes one `philly.bin` input plus a `meta.json` provenance record.
The download and conversion are the slow first run step.
The ingest rejects short building exports and uses the newest verified complete
snapshot instead of replacing a full city artifact with partial live data.
Existing checkouts must rerun `ingest` because world format version 6 removes
unused layers and adds the City boundary to the single render input.
`prebuild` renders the detailed z8 scene and creates z0 through z7 by resizing
those tiles. This gives every overview the same textured scene instead of a
different drawing style. The command uses the available logical CPU count,
capped at 16 workers. PASDA downloads have a separate limit of eight. It
resumes an interrupted staging build and publishes a compact inventory only
after the whole pyramid is ready. A completed scene is immutable. If validation
fails, including a content-hash mismatch, prebuild creates and publishes a
replacement namespace instead of changing files beneath a running server. Old namespaces remain valid for
servers that were already running when the new scene was published. Run
`uv run --locked poe prebuild --jobs N` to choose from 1 through 16 workers.

The server only reads z0 through z8 from `data/tiles/`. At closer view levels,
the browser magnifies the canonical z8 pixels with nearest-neighbor sampling.
It does not switch to another renderer or replace textures with plain geometry.

Aerial crops come from the native three-inch 2025 PASDA service. The renderer
divides EPSG:32129 into fixed 1,536 metre cells, and each cell contains 2,048 by 2,048
pixels. Every output tile samples the same source pixel grid, so tile borders
cannot change the sampling phase. At most eight requests reach PASDA at once.
Source cells are stored under `data/aerial/` with a hard 8 GiB limit.
Each build removes obsolete cache formats, and an invalid cached image is
fetched again once.

After the first ingest, the usual development loop is only:

```sh
uv run --locked poe serve
```

Optional EagleView access is configured through one immutable Pydantic Settings
model. Copy `.example.env` to `.env`, add credentials issued through the official
developer API, and run `uv run --locked poe eagleview-smoke`. The smoke test is
bounded to one City Hall cell and downloads at most one image. The normal ingest
and prebuild commands do not read these credentials.

## Static hosting

The finished map can run as plain files on Cloudflare Workers Static Assets.
The export includes the viewer, metadata, tile coverage, and every tile listed
in the SHA-256 inventory. It does not include a Worker script or any paid
Cloudflare service.

```sh
npm ci
uv run --locked poe static-export
uv run --locked poe static-dry-run
uv run --locked poe static-preview
```

The current export has 12,344 files and uses 615.5 MiB. Its largest file is
about 126 KiB. The exporter rejects builds that exceed the Cloudflare Free plan
limits of 20,000 files or 25 MiB per file. Static asset requests and storage do
not incur a charge. See the official [Static Assets limits and
billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)
documentation. Run `npx wrangler deploy` when the dry run and local preview
pass and you are ready to publish.

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
2015 I3S + 2008/09 downtown/stadium COLLADA + City footprints + PASDA aerial
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

Python is not in the request path. The Rust server starts from a small generated
manifest and serves the completed image pyramid. It does not load geometry,
aerial sources, or texture atlases, and it does not render during requests.
Source imagery and rendered tiles persist across runs. The service binds to
localhost by default. A production deployment can serve the pyramid as static
files from an edge cache.

## Project status

This is an early public prototype, not an authoritative map or surveying tool.
The current City footprint layer contains 545,672 usable structure polygons.
Outside the detailed mesh, roofs sample the matching aerial pixels and walls
use a muted palette derived from those pixels. The source height fields drive
extrusion, so this citywide treatment preserves real outlines and local color
without claiming to reconstruct unseen facades. The detailed Center City scene
contains 367 atlas chunks and 294,443 textured triangles. Those triangles
preserve real facades, setbacks, roof equipment, sloped roofs, and landmark
silhouettes. The stadium district adds 808 textured models and 126,181
triangles. Six obsolete Spectrum components are excluded because that arena was
demolished after the 2008 source capture. The legacy downtown r0 source adds
2,689 detailed textured components around the 2015 scene. Models covered by the
newer scene are suppressed before packing, so the same building is not drawn
twice.

The source code is [MIT licensed](LICENSE). That license does not grant rights
to the source datasets or generated map tiles; their provenance and publication
notes are in [docs/DATA.md](docs/DATA.md).
