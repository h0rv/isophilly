# Google Earth citywide capture

This workflow downloads the public Google Earth 3D source without an API key,
login, browser cookies, or extracted credentials. It renders the photographed
geometry locally rather than stitching perspective screenshots. The approved
map has all 10,144 planned Southeast leaves with the selected lower, narrower
camera, including Center City and outer neighborhoods. It uses source level 18
and original colors. The optional level 20 citywide refinement was stopped and
is not part of this release. The earlier identity-camera atlas has 9,214 leaves.

The original City artwork and `data/tiles/current.json` are not changed.
Generated Google imagery lives in a separate workspace.

## Full city

```sh
npm ci
ROOT=data/google-earth-current # Use a fresh workspace for this camera.
npm run google:plan -- --root "$ROOT" --projection tools/google-imagery/view.json
npm run google:capture -- --root "$ROOT" --cache data/google-earth/cache
npm run google:status -- --root "$ROOT"
npm run google:build -- --root "$ROOT" --source-colors
npm run google:serve -- --root "$ROOT"
```

Planning pins the checked-in reference metadata and tile coverage. With the
selected camera, conservative coverage includes 10,144 zoom 8 leaves. It covers
the original citywide area after projection, including Center City and stadiums.
The interface and neighborhood data are reused. Each leaf contains 512 by 512
pixels. `--source-colors` skips the older display finish and cannot be combined
with water styling. The original identity camera remains available by omitting
`--projection` in a fresh workspace.

Capture is resumable. Completed PNGs and cached source responses are checked by
SHA-256 before reuse. A process lock protects the capture manifest. New source
requests are limited to ten per second per process, with up to four mesh
requests in flight. HTTP failures stop the run instead of retrying indefinitely.
The run also stops with less than 10 GiB free, or after 20 GiB of new source
responses in one invocation. Free space or review the source failure, then run
the same capture command to resume.

The default source detail is level 18. The renderer loads levels 16 and 17
underneath it and keeps Google's hidden terrain layer below all visible surfaces.
This fills small holes in photogrammetry with genuine source imagery. Fine
geometry is drawn after clearing the background depth buffer, so approximate
background surfaces cannot hide better geometry. Uncovered output
pixels cause that image to be rejected and recorded in `progress.json`. A strict
build will not claim completion while a planned image is missing. The capture
command exits unsuccessfully if any images were rejected, while preserving valid
progress.

Files are under `data/google-earth/`:

- `plan.json` contains the immutable extent, views, resolution, and source settings.
- `reference/` contains the pinned original metadata, coverage, and their hashes.
- `cache/` contains hash-checked source responses and the reviewed decoder.
- `raw/se/8/x/y.png` contains unstyled captures.
- `manifest.json` records completed image hashes, capture times, and source credits.
- `progress.json` records completed and rejected images, timing, and network bytes.
- `source.json` records the source epoch, globe radius, decoder, and copyright table.
- `rejected/` preserves failed images and hole coordinates for diagnosis. Those
  images never enter the published pyramid.
- `site/` is the generated static viewer and lossless WebP pyramid.
- `water/` contains a separate, resumable mask capture bound to the exact photo
  plan and manifest. Original Google photo pixels are never overwritten.

The builder publishes a completed generation through an atomic symlink change.
It refuses to overwrite an unrelated existing output directory. A failed build
does not replace the previous generated site. Prior generations remain available
for readers that already opened them. Close those preview servers before removing
old generation directories.

The default pyramid is exported once, not duplicated under a view folder. Its tile
version includes the captured image hashes, so a completed build cannot reuse a
partial build's immutable image URLs. The metadata does not claim the City's
building or tree counts as measured Google entities.

## Historical optional water style

This is not used in the current map. Keep `--source-colors` for the approved
presentation. The following describes the separate historical experiment.

Google's river photographs can have abrupt gray, blue, or teal joins. The
optional water pass uses the City's hydrographic polygons and Google's visible
fine geometry to identify water. It uses the photographed elevation to exclude
elevated bridge decks. Uncertain elevations are left unchanged. Polygon holes
preserve islands.

The builder applies the existing water palette and continuous diagonal bands
before applying the usual display finish once. It does not flatten the terrain,
replace building photographs, or generate imagery. The synthetic browser smoke
checks raised roads, bridges, and islands. Source seasonal changes remain visible
in the photographs rather than being painted over.

The City query is paginated and pinned. Incomplete pages, repeated pages, and
responses exceeding the explicit 64 MiB limit are rejected. Mask image hashes
and pixel counts are checked during build. Missing or stale masks cannot replace
a previous atlas, and a mask workspace cannot be published as photographs.

Use `npm run google:build -- --source-colors` for unchanged captured colors.
Omitting only `--water` still applies the older display finish. The old generated
site remains available after either build.

## Detail and future angles

For a separate, finer experiment:

```sh
npm run google:plan -- --root=data/google-earth-fine --tile-size=1024 --level=19
npm run google:capture -- --root=data/google-earth-fine --cache=data/google-earth/cache
```

For eight fixed angles:

```sh
npm run google:plan -- --root=data/google-earth-eight --angles=0,45,90,135,180,225,270,315
npm run google:capture -- --root=data/google-earth-eight --cache=data/google-earth/cache
```

Capture reduces block size automatically for finer source levels or larger
output images to bound rendering memory. It also excludes source cells that
cannot reach the current camera image.

The cache can be shared across experiments and angles. A new plan must use a new
workspace; existing captures cannot be silently mixed with a different grid,
detail level, or camera. Angle 0 is the current Southeast view. The other angles
are supported by capture and independent atlas generation at the same map scale. Viewer rotation
controls and correct placement of overlays in those views are separate follow-up
work. Do not publish them as complete interactive modes yet.

The source is 3D geometry, so later views can reuse downloaded responses instead
of downloading a separate source image set eight times. More pixels cannot
recover detail absent from the chosen source level.

## Pilot and visual review

```sh
npm run google:plan -- --root=data/google-earth-preview --center=-75.23,39.961 --radius=250 --limit=16
npm run google:capture -- --root=data/google-earth-preview --block-size=2
npm run google:build -- --root=data/google-earth-preview
node scripts/google-earth-visual.mjs --root=data/google-earth-preview --targets=west --zooms=8,10
```

`npm run google:smoke` checks native projection, fine-pixel counts, and terrain
priority in a synthetic browser scene without contacting Google.
`npm run google:audit` checks every finished leaf and creates geographic contact
sheets. Neither replaces looking at the images.

The visual command captures matching baseline and Google screenshots with fixed
camera coordinates, viewport, zoom, and lighting time. By default it covers City
Hall, Rocky, West Philadelphia, and permanent outer-city sectors at five zoom
levels. It checks that tiles settle without failures, that the image is not
empty, and that Google source credits are present. Review the images under
`ROOT/visual/index.html`; automated checks do not prove that the artwork is good.
Each run is archived with its viewport and device pixel ratio. The current report
pointer changes only after successful capture. Each report also identifies the
exact atlas generation and its water treatment. Add `--width=390 --height=844
--dpr=2` for a phone comparison. Use `--neighborhoods --local-areas` to check
labels and borders. Photography is shown without an artificial lighting overlay.

A pilot plan can be complete for its selected patch without covering the city.
Its omitted city tiles are reported during planning. Never treat that pilot as
a complete citywide release.

## Viewer presentation

The viewer opens at City Hall at zoom 7. Names are on by default and combine
neighborhood names with local district names. Labels are distributed across the
viewport, rather than giving Center City all the available space. Official
neighborhood names are not suppressed by local district names.

Borders is separate and off by default. It adds one set of neighborhood outlines
without colored fills. There is no second set of local-area boundaries. The
Rocky shortcut and custom figure have been removed.

Imagery uses a 1 CSS pixel grid without runtime recoloring or palette reduction.
Shrinking images uses filtered sampling; enlarging them retains native pixels.
Tile detail follows the viewport resolution and actual tile dimensions instead
of forcing the same tile level on every screen. Text and controls stay sharp.
Planned coverage controls a softened outer edge, not the currently loaded tiles.

For a viewer-only change, run `npm run google:refresh`. It copies the existing
atlas into a new generation without reencoding the images, then replaces the
local site pointer. Existing preview servers pin their original generation, so
restart the preview server to show the new viewer.

Build with `--source-colors` to preserve captured colors without the older display
finish, vegetation grading, or water palette. Seasonal joins remain part of the
source photography. No museum-specific patch or invented stadium surface is used.

The selected camera is configured in `tools/google-imagery/view.json`. Pass it to
planning with `--projection tools/google-imagery/view.json`. It lowers the viewing
angle and narrows the horizontal proportions while preserving building heights.
Changing it requires a new capture workspace, not stretching finished images.

## Publication

Publish the verified generated Earth site, not the retained City-source export
from `poe deploy`. Check `build.json` for `complete: true`, compare coverage with
the pinned plan, and verify `meta.raw_colors` and the camera before publishing.
Review the actual map at desktop and phone sizes.

```sh
# ROOT is the workspace containing the approved complete build.
SITE=$(realpath "$ROOT/site")
npx wrangler deploy --assets "$SITE" --dry-run
npx wrangler deploy --assets "$SITE"
```

The approved generation is under `data/google-earth-preview-perspective/site`.
It contains 13,778 files totaling 4,394,513,553 bytes. The largest file is
548,432 bytes. This is within the Workers Free limits of 20,000 files and 25 MiB
per file. The configured custom domain is <https://isophilly.horv.co>.
Authenticate separately. Do not put credentials in the site or repository.
After publication, verify the live metadata and tile version against this
completed generation. Remove temporary deployment credentials after use.

## Known image limitations

Franklin Field has a raised-looking feature over its playing surface. It is
present in matched captures at source levels 18, 19, and 20, including the level
20 capture with essentially complete fine coverage. A separate 2025 City aerial
patch shows an unobstructed field, but the acquisition dates do not establish a
same-date comparison. A temporary structure is possible, not proven. The source
reconstruction versus the shared decoder and renderer is still unresolved.

Other inspected areas have softened stadium seating, smeared thin rail and
bridge details, and visible water texture triangles. These are visible in raw
captures before frontend sampling. No invented sports surface or painted patch
has been substituted. This release is not certified free of image artifacts.

## Source details

The decoder is pinned to `@nakasyou/google-earth-internal-api-client` 0.1.1.
That version rejects valid partial final octant groups present in Philadelphia.
A narrowly checked correction is written under the ignored source cache, without
changing `node_modules`. Offline tests reproduce that failure and verify the
corrected triangle ranges. Any upstream source change requires reviewing the
correction again.

Google Earth RockTree coordinates use a sphere whose radius is supplied by the
source metadata. They are not WGS84 ellipsoid ECEF coordinates. Using the latter
moves Philadelphia geometry by many kilometres. The renderer converts the source
sphere to longitude, latitude, and source height before applying IsoPhilly's
State Plane projection. Tests cover that distinction and all eight camera angles.

Real terrain elevations are retained. Unlike the old height-relative building
extrusions, terrain in the new view is not flattened to zero. Review geographic
overlay placement and landmark framing with this difference in mind.

This is an undocumented public protocol, not an official stable Google API.
Source compatibility can change. The project owner has authorized the imagery
work; operators remain responsible for their applicable source permissions.
No credentials or source meshes are included in the public static viewer.
