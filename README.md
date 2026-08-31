# IsoPhilly

See [city overlays](docs/LIVE_CITY.md) for neighborhood and day/night data provenance.

A small, deterministic isometric map of Philadelphia built from official City
building footprints and heights, the official 2015 Center City textured 3D
scene, the legacy 2008/09 downtown and stadium-area textured models, and 2025
City aerial photography. The launch view stays inside the strongest Center City
source and offers four prebuilt 90-degree orientations. A toggle opens the
citywide illustrated overview without implying that its aerial-derived walls
are photographed facades.

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
`ingest` downloads official City Limits, Building Footprints, hydrology, park
boundaries, OpenStreetMap building parts, and the 2015 Center City 3D model. It
also imports the highest-detail legacy downtown and stadium KML/COLLADA models.
For the downtown source, new checkouts download PASDA's smaller `kml00.zip`.
Existing checkouts reuse `data/raw/Philadelphia2008_downtown_kml.zip` when it is
present. It writes one `philly.bin` input plus a `meta.json` provenance record.
The download and conversion are the slow first run step.
The ingest rejects short building exports and uses the newest verified complete
snapshot instead of replacing a full city artifact with partial live data.
Existing checkouts must rerun `ingest` because world format version 8 adds
surface masks and height-backed building parts to the single render input.
`prebuild` renders the citywide z8 scene and creates z0 through z7 by resizing
those tiles. It also renders four Center City views at z5 and derives z0
through z4 for each view. The Center City work makes a new prebuild heavier,
but it does not add work to the HTTP server. The command uses the available
logical CPU count, capped at 16 workers. PASDA downloads have a separate limit
of eight. It resumes an interrupted staging build and publishes a compact
inventory only after the whole pyramid is ready. A completed scene is
immutable. If validation fails, including a content-hash mismatch, prebuild
creates and publishes a replacement namespace instead of changing files
beneath a running server. Old
namespaces remain valid for servers that were already running when the new
scene was published. Run
`uv run --locked poe prebuild --jobs N` to choose from 1 through 16 workers.

The server reads the finished citywide and Center City pyramids from
`data/tiles/`. It does not load geometry or render during a request. At closer
view levels, the browser magnifies the canonical pixels with nearest-neighbor
sampling.

Center City has four separate z0 through z5 pyramids. At z5, one output pixel
covers about 0.7 metre and uses one aerial sample. The 2015 textured meshes
remain the first choice. City footprints and OpenStreetMap parts fill the space
around them. Their roofs sample aerial imagery, and their walls are procedural.
The browser permits one additional nearest-neighbor zoom level and caps there.
Rotation switches immutable pyramids. It does not reproject raster tiles in the
browser or claim continuous 360-degree motion.

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

The current z5 Center City build exports 18,009 files and uses 1,245.0 MiB.
The exporter checks the completed inventories and rejects builds that exceed
the Cloudflare Free plan limits of 20,000 files or 25 MiB per file. Static
asset requests and storage do not incur a charge. See the official
[Static Assets limits and billing](https://developers.cloudflare.com/workers/static-assets/billing-and-limitations/)
documentation. Run `npx wrangler deploy` when the dry run and local preview
pass and you are ready to publish.

Generated data and tiles are intentionally gitignored. See the [data pipeline
and attribution notes](docs/DATA.md) before publishing a build.
The [research record](docs/RESEARCH.md) also documents the audited April 2025
PASDA LiDAR candidate and why Google's 3-D map products are viable only as a
live, billed mode rather than as saved source pixels for this project.
The dated [PASDA facade audit](docs/PASDA_AUDIT.md) is the canonical inventory
of photographed-side sources, duplicate archives, and conditions for reopening
that research.

## Quality checks

```sh
npm ci
uv run --locked poe check
uv run --locked poe visual
```

The visual task is the repeatable browser release gate. It starts the local
server, captures the citywide audit views plus all four Center City
orientations, verifies Rocky in every orientation, exercises rotation and
keyboard navigation at a 390-pixel mobile viewport, checks tile settlement and
response policy, and writes screenshots plus a JSON timing report under
`artifacts/visual/`.

Install a current Node.js LTS release and
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) (`cargo install
cargo-deny --locked`) before running the quality suite. The single Poe task
runs Ruff and ty for Python, Biome and TypeScript for the web viewer, and
rustfmt, Clippy, tests, a locked release build, and cargo-deny for Rust. CI runs
the same checks on Linux. Dependency updates are managed for all three
ecosystems plus GitHub Actions.

## How it fits together

```text
2015 I3S + 2008/09 COLLADA + City footprints + OSM parts + PASDA aerial
             |
             v
Python + GeoPandas/Shapely  ->  data/clean/philly.bin
                                      |
                                      v
Rust + rstar/tiny-skia      ->  citywide z8 plus four Center City z5 scenes
                                      |
                                      v
Rust + image               ->  lower zoom levels for all five pyramids
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
Outside the detailed mesh, including gaps inside the Center City view, roofs
sample the matching aerial pixels. Walls use a procedural pixel pattern and a
palette derived from nearby aerial pixels. Height-backed OpenStreetMap parts
replace a parent footprint when they cover most of it, preserving documented
setbacks and towers without claiming to reconstruct unseen facades. The detailed Center City scene
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
