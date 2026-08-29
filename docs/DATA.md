# Data pipeline

`uv run --locked poe ingest` downloads five source datasets and writes two
clean artifacts:

- `data/clean/philly.bin` contains fallback buildings, accepted textured
  meshes, texture references, and the City boundary.
- `data/clean/meta.json` records source URLs, request times, HTTP validators,
  SHA-256 checksums, bounds, counts, and output checksums.

The texture JPEGs used by accepted meshes live in
`data/clean/mesh-textures/`. Generated files are ignored by Git. Keep the raw
snapshots and `meta.json` when pinning a release.

## Sources

The active pipeline has one source for each job:

1. City Limits defines the render boundary.
2. Building Footprints supplies citywide outlines and heights.
3. The 2015 I3S scene supplies the newest detailed Center City geometry and
   textures.
4. The 2008 and 2009 legacy downtown archive fills gaps outside that scene.
5. The 2008 stadium archive supplies detailed geometry and textures for the
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
  Fallback walls use a stable, muted color derived from the same local image.
  They are not described as facade textures.
- Accepted I3S and COLLADA faces keep their real UV coordinates and JPEG
  textures. Missing or invalid textures are errors, not a reason to draw a
  plain replacement polygon.

Water, parks, street centerlines, and the former OpenStreetMap building-part
query were removed from the clean format. They did not draw pixels. The aerial
image already supplies those surface details, so retaining the extra downloads,
parsers, indexes, and `streets.bin` file only created failure modes.

## World format

All values are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOPHILY"
u32      version (6)
u32      EPSG (32129)
u32      building count
u32      building mesh count
u32      city ring count
u8 x 32  SHA-256 digest of all retained texture atlases
f64 x 4  official city bounds: min_x, min_y, max_x, max_y
repeat building count times:
  f32    height
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

ring:
  u32    point count
  repeat point count times: f32 x, f32 y
```

## Aerial image and tile pyramid

The ground image comes from the 2024 Philadelphia service at
[PASDA](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2024/MapServer).
The source advertises one inch imagery. Source crops use EPSG:32129 and persist
under `data/aerial/` with an 8 GiB limit. Each fixed 1,536 metre cell contains
2,048 by 2,048 pixels, for a 0.75 metre render sample interval. The 3 by 3 filter can
cross cell boundaries instead of clamping to an edge pixel. Starting a build
removes obsolete aerial cache namespaces, so the 8 GiB limit applies across
runs instead of accumulating once per renderer revision. A corrupt cached JPEG
is deleted and fetched again once.

The prebuilder renders one detailed z8 scene as lossless WebP. It derives z0
through z7 from those exact pixels. The browser magnifies z8 with nearest
neighbor sampling for closer views. It does not switch to another geometry or
style at a different zoom.

Every render path shares one depth buffer. The draw order is aerial ground,
fallback buildings, then accepted textured mesh faces. Texture sampling uses a
stable world grid so adjacent output tiles do not request the same image on
different pixel phases.

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
