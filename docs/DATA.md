# Data pipeline

`uv run --locked poe ingest` loads eight source datasets and writes two
clean artifacts:

- `data/clean/philly.bin` contains fallback buildings, building parts,
  accepted textured meshes, surface masks, texture references, and the City
  boundary.
- `data/clean/meta.json` records source URLs, request times, HTTP validators,
  SHA-256 checksums, bounds, counts, and output checksums.

The texture JPEGs used by accepted meshes live in
`data/clean/mesh-textures/`. Generated files are ignored by Git. Keep the raw
snapshots and `meta.json` when pinning a release.

## Sources

The active pipeline has one source for each job:

1. City Limits defines the render boundary.
2. Building Footprints supplies citywide outlines and heights.
3. Hydrology and park polygons provide restrained color grading masks.
4. OpenStreetMap building parts supply documented Center City setbacks and
   roof forms where photographed meshes are unavailable.
5. The 2015 I3S scene supplies the newest detailed Center City geometry and
   textures.
6. The 2008 and 2009 legacy downtown archive fills gaps outside that scene.
7. The 2008 stadium archive supplies detailed geometry and textures for the
   sports complex.

The importer resolves the mesh sources in this order: 2015 I3S, legacy
downtown, stadium, then City footprint fallback. A lower priority mesh is
removed when a newer footprint covers its representative point or at least one
quarter of its area. This decision happens during ingest. The renderer receives
one mesh collection and does not know which source format produced each mesh.

PASDA publishes the legacy downtown files under a 2010 download path, but the
files are byte identical to the retained 2008 package and their metadata says
they were authored in 2009. The importer therefore records them as 2008 and
2009 data. It imports only the highest detail `r0` models. Existing checkouts
reuse `data/raw/Philadelphia2008_downtown_kml.zip`. New checkouts download the
smaller PASDA `kml00.zip` package.

The stadium importer also keeps only `r0`. It excludes six components that
represent the demolished Spectrum. Both legacy archives use the same COLLADA
parser, validation rules, texture store, and output types.

## Geometry rules

- Coordinates use EPSG:32129, NAD83 Pennsylvania South in metres.
- City Limits defines the world bounds and tile presence.
- Building `approx_hgt` is the primary height in US survey feet. `max_hgt` is
  the fallback. Values below 2.4 metres or above 400 metres receive the 8 metre
  default.
- Footprints retain 0.35 metre detail. City Limits uses 1 metre
  topology-preserving simplification.
- Citywide roofs sample aligned aerial pixels at their real coordinates.
  Fallback walls use a stable color derived from the same local image plus
  deterministic pixel floor and window bands. They are illustrations, not
  facade textures.
- The four Center City views use the same fallback rules outside the 2015 mesh
  coverage. Only a 2015 mesh suppresses a fallback in these views. The older
  downtown and stadium meshes remain outside the Center City render policy.
- Height-backed building parts replace the coarse parent footprint only when
  they cover at least 65 percent of it. Photographed meshes remain the highest
  priority.
- Accepted I3S and COLLADA faces keep their real UV coordinates and JPEG
  textures. Missing or invalid textures are errors, not a reason to draw a
  plain replacement polygon.

Street centerlines and `streets.bin` remain removed. Roads come directly from
the aerial image. Water and park polygons now grade only matching aerial pixels
instead of drawing flat replacement polygons.

## World format

All values are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOPHILY"
u32      version (8)
u32      EPSG (32129)
u32      building count
u32      building part count
u32      building mesh count
u32      city ring count
u32      water ring count
u32      park ring count
u8 x 32  SHA-256 digest of all retained texture atlases
f64 x 4  official city bounds: min_x, min_y, max_x, max_y
repeat building count times:
  f32    height
  ring
repeat building part count times:
  u64    OpenStreetMap ID
  f32    height
  f32    minimum height
  f32    roof height
  u8     roof shape
  ring
repeat building mesh count times:
  u32    texture atlas ID
  f32    height
  u32    face count
  ring   footprint used for indexing and fallback suppression
  repeat face count times:
    repeat 3 times: f32 x, f32 y, f32 z, f32 u, f32 v
repeat city ring count times:
  ring
repeat water ring count times:
  ring
repeat park ring count times:
  ring

ring:
  u32    point count
  repeat point count times: f32 x, f32 y
```

## Aerial image and tile pyramid

The ground image comes from the 2025 Philadelphia service at
[PASDA](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer).
The source metadata reports three-inch native imagery. Source crops use
EPSG:32129 and persist under `data/aerial/` with an 8 GiB limit. Each fixed
1,536 metre cell contains
2,048 by 2,048 pixels, for a 0.75 metre render sample interval. The 3 by 3 filter can
cross cell boundaries instead of clamping to an edge pixel. Starting a build
removes obsolete aerial cache namespaces, so the 8 GiB limit applies across
runs instead of accumulating once per renderer revision. A corrupt cached JPEG
is deleted and fetched again once.

The prebuilder renders the citywide z8 scene as lossless WebP and derives z0
through z7 from those exact pixels. It also renders four Center City z5 scenes
and derives z0 through z4 for each orientation. A Center City z5 output pixel
covers about 0.7 metre, so its ground renderer takes one sample per output
pixel from the 0.75 metre PASDA working grid. The browser uses nearest-neighbor
sampling only when it magnifies past a pyramid's canonical level.

Every render path shares one depth buffer. The draw order is aerial ground,
fallback buildings, then accepted textured mesh faces. In a Center City view,
the fallback pass uses City footprints and OpenStreetMap parts wherever the
2015 mesh does not cover a building. Roofs use aerial samples. Walls use a
procedural pattern with colors derived from nearby aerial pixels, so they are
not photographed facades. Texture sampling uses a stable world grid so
adjacent output tiles do not request the same image on different pixel phases.

The four z5 Center City pyramids add 4,096 files compared with z4. The current
static export contains 18,009 files and uses 1,120.0 MiB, which remains below
the 20,000 file limit checked by the exporter. The extra leaf tiles and finer
ground sampling make prebuild slower. The server still reads immutable files,
so request handling does not become more expensive.

After prebuild, `data/tiles/current.json` records the active tile namespace,
scene bounds, counts, landmarks, and clean input digest. The HTTP server reads
that small manifest, a compact tile inventory, and the immutable WebP files. A
new ingest does not interrupt the last completed scene. Prebuild checks the new
world digest, resumes an interrupted private staging build, and switches
`current.json` only after the new pyramid is complete. It never changes a
published namespace. The inventory records every tile size and SHA-256 digest;
failed validation produces a new replacement namespace.
The server never loads `philly.bin` or renders a tile during a request.
Prebuild does not delete old namespaces. A server that started before
publication can continue serving its immutable old scene until it is restarted.

## Audited future sources

PASDA's [full April 2025 LiDAR metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Lidar_2025.xml)
and [LAS directory](https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/)
confirm a citywide classified point cloud that is not yet part of ingest. The
short PASDA catalog abstract incorrectly describes it as 2022. The actual
collection is 963 LAS files totaling 362.82 GiB, in LAS 1.4 point format 6.
It records intensity but no RGB or NIR; a sampled central tile measured about
62 returns/m². The full metadata lists access and use constraints as “None.”

This is a candidate geometry source for terrain, roofs, heights, trees, and
landmark detail such as the Philadelphia Museum of Art steps. It is not a
photographic facade source. No current public PASDA collection supplies
citywide calibrated multi-angle facade imagery. Before any bulk download,
evaluate only three pinned areas: Center City, the Museum of Art/Waterworks,
and Port Richmond. Record source tiles and checksums, derive ground/surface and
normalized-height products, render four headings, and compare at least 30
buildings per area with the current scene.

The remaining 2010 textured KML/COLLADA archives may support a bounded downtown
experiment but are too old and have insufficiently clear redistribution terms
for a default expansion. The roughly 1 TB raw nadir frame archive has no
published positions, camera orientation, or calibration and is not an ingest
candidate.

Google's 3-D map products are viable only as live hosted views. The Maps
JavaScript API 3-D map uses the Immersive Maps SKU: 5,000 free map loads per
month, then $7 per 1,000 through 100,000. Direct Map Tiles API Photorealistic
3D Tiles instead have 1,000 free root-tile requests, then cost $6 per 1,000
through 100,000
([pricing](https://developers.google.com/maps/billing-and-pricing/pricing),
[SKU details](https://developers.google.com/maps/billing-and-pricing/sku-details)).
Its [Map Tiles policies](https://developers.google.com/maps/documentation/tile/policies)
prohibit the persistent/offline cache, prefetching, image or machine analysis,
and extracted or derived imagery needed to save this project's source pixels.
Neither route is therefore an approved input. Reconsider only if live-only
delivery, billing, attribution, and no retained pixels become acceptable
requirements.

## Reproducibility and publication

The building export must contain at least 300 MB and produce at least 500,000
usable footprints. If the live export returns a short response, ingest uses the
newest complete content-addressed snapshot. It stops when neither source is
complete.

Ingest builds the world, metadata, and texture directory under a staging
directory. It replaces the prior clean directory only after all three pass.
An interrupted import therefore leaves the last complete dataset available.

The repository's MIT license covers only project code. It does not relicense
City data, PASDA data, texture atlases, or generated tiles. Public access to the
I3S and COLLADA files does not clearly grant the right to republish their JPEG
textures. Get written permission from the City before publishing generated
tiles that contain them. Record the permission with the release provenance.

The I3S child cache does not yet have a complete pinned resource manifest. A
build is not fully reproducible unless every raw snapshot and I3S child resource
used by that build is retained.
