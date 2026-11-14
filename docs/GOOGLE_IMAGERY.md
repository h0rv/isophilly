# Map Tiles API imagery outside existing photographed coverage

For the default citywide workflow without a key, use
[Google Earth capture](GOOGLE_EARTH.md). This document describes the separate
optional Map Tiles API route.

The optional capture workflow uses Google's Photorealistic 3D Tiles, NASA JPL's
3D Tiles Renderer, three.js, and headless Chromium. This is the source and capture
approach described in [Isometric SF](https://www.justinchavez.com/blog/isometric-sf-how/).
It does not use SF's paintings, prompts, or trained models.

IsoPhilly keeps its existing projection and color treatment. The capture tool
projects every photographed vertex into the same Pennsylvania State Plane and
Broad Street isometric coordinates as the Rust renderer. It renders textures
without adding lighting. Prebuild applies the existing display finish once and
writes the usual lossless WebP tiles. There is no AI image generation.

The project owner authorized this integration. Operators must have permission
under their applicable imagery agreement to retain and publish the captures.
An API key alone does not establish those rights. The previous standard-terms
research remains relevant to that check. This workflow is separate from normal
ingest and is never enabled automatically.

## Make a pilot

Use a checkout that already contains `data/clean/philly.bin`. Run normal ingest
first if the world is missing. Planning does not download Google imagery.

```sh
npm ci
npm run google-api:plan -- --center=-75.23,39.96 --radius=350 --limit=4 --height-origin=0
```

The example selects up to four tiles near West Philadelphia. The planner excludes
any tile that touches an existing photographed mesh, including the legacy
downtown and stadium sources. It adds a small safety margin around those meshes.
The command reports candidates omitted by the limit. It writes
`data/google-imagery/plan.json`.

`height-origin` is the fixed WGS84 ellipsoid elevation to subtract from source
vertices. Zero preserves the source ellipsoid heights. It is not an assertion
that local ground is at zero, and it is not a conversion to the City's vertical
datum. Inspect the pilot against surrounding streets and set a reviewed ground
reference before expanding coverage. Keep one origin for an entire capture set.
Changing it requires a new output directory so neighboring captures cannot use
different height references.

Enable the Map Tiles API on a billed Google Cloud project. Set
`GOOGLE_MAPS_API_KEY` in the local environment or ignored `.env` file. Restrict
that key for the capture machine and API. Do not put the key in a command line,
plan, committed file, or public viewer.

```sh
npm run google-api:capture
uv run --locked poe prebuild --google-imagery data/google-imagery
uv run --locked poe serve
```

Capture defaults to four new images per invocation. It captures sequentially and
retries a failed tile once. Each attempt starts a new root request and can incur
Google charges. Review quotas and billing before raising `--limit`. A valid
capture is recorded immediately, so an interrupted run resumes without paying
to capture completed tiles again.

On macOS the tool uses installed Google Chrome. Elsewhere, install Chromium for
Playwright or pass `--browser=/path/to/chromium`. You can also set
`ISOPHILLY_CHROMIUM_PATH`.

## Inspect before expanding

Open the pilot PNGs under `data/google-imagery/8/x/y.png`. These are raw renders.
Inspect the rebuilt map to see the existing IsoPhilly finish. Check street
alignment and building heights at the patch edges. Inspect color differences
against neighboring City imagery. Neither capture nor prebuild claims to remove
source acquisition differences automatically.

A capture is rejected if it has transparent holes, missing provider attribution,
or insufficient visible detail. The renderer waits for loading to finish and
for the visible tile set to settle. A second failed attempt stops the run and
leaves the rejected tile out of the manifest.

Before using captures, Rust checks the world hash, exact grid, protected mesh
coverage, PNG hashes, dimensions, RGBA format, opacity, timestamps, and credits.
Each image is checked again when used. It does not silently substitute a
procedural tile for a broken capture.

The complete capture manifest hash is part of the citywide tile identity. A
changed image or manifest cannot reuse an older completed build. Center City
rich-view pyramids stay unchanged. Uncaptured citywide tiles retain the existing
City imagery and illustrated buildings.

To expand the same source world and camera settings, rerun `google-api:plan` with a
larger radius and limit, then capture more tiles. Existing completed captures
remain in the manifest. Use `--output` on both commands for a separate experiment.
To return to the ordinary map, run prebuild without `--google-imagery`.

## Attribution and publication

The capture tool records Google Maps and the original visible provider credits.
Prebuild puts their sorted union in the scene metadata, and the viewer displays
those credits whenever the scene includes Google captures. Static export copies
that metadata along with the tiles. No Google key or source 3D tiles are included
in the public site.

## Checks

```sh
npm run check
npm run google-api:smoke
cargo test --locked
```

The smoke command runs the actual browser renderer against a synthetic local
ECEF glTF surface. It checks projection, opaque RGBA output, provider credits,
and repeatable pixels after renderer disposal. It makes no Google requests.
It does not verify Philadelphia coverage, real source quality, or pilot alignment.
Those checks require a key, the generated world, and a reviewed capture.
