# Data pipeline

`uv run poe ingest` downloads content-addressed snapshots of five City of
Philadelphia ArcGIS layers and one OpenStreetMap query. It also downloads the
official 2015 Center City 3D model into a versioned local cache and downloads
the legacy 2008/09 downtown and stadium-area KML/COLLADA archives. It processes
them offline and writes:

- `data/clean/philly.bin`: the version 5 building, building part, building mesh,
  water, and park world read by the Rust server.
- `data/clean/mesh-textures/`: the JPEG atlases used by the accepted I3S,
  legacy downtown, and stadium meshes.
- `data/clean/streets.bin`: a separate optional street-centerline artifact.
- `data/clean/meta.json`: source URLs, request times, HTTP validators, SHA-256
  checksums, CRS, bounds, counts, and output checksums.

Generated data is ignored by Git. Keep `data/raw/` and `meta.json` together when
pinning a release: the raw filenames include the first 12 characters of their
full SHA-256 digest, so a live source refresh never silently overwrites a prior
snapshot.

The I3S child-node and geometry cache is versioned but is not yet represented by
a complete resource manifest. The aggregate JPEG atlas digest is embedded in
`philly.bin` and verified by the Rust process at startup. Do not describe a
build as fully reproducible until every I3S child URL, fetch time, size, and
checksum is recorded and pinned.

## Geometry decisions

- EPSG:32129 is NAD83 / Pennsylvania South in metres. It is the metre-native
  equivalent of the City's EPSG:2272 State Plane coordinates; no manual unit
  scaling or falsely labelled CRS is involved.
- the official City Limits polygon defines the world bounds and clips water,
  parks, buildings at the edge, and streets. Hydrology is not allowed to expand
  the map several kilometres beyond Philadelphia.
- building `approx_hgt` is the primary height in US survey feet, with `max_hgt`
  as a fallback. Values below 2.4 m or above 400 m are treated as unusable and
  receive the 8 m default.
- building footprints retain 0.35 m detail; ground polygons and centerlines use
  1 m topology-preserving simplification.
- citywide footprint roofs reuse the aligned aerial pixels at their footprint
  coordinates. Wall colors come from robust, desaturated samples of the same
  local imagery. They are intentionally not described as facade textures.
- streets include City classes 1–5, 9, and 10 (expressways through local roads
  and ramps). Class 6 driveways and non-traversable/walking/boundary lines are
  excluded from the clean render input.
- the Center City query covers longitude `-75.19042` to `-75.13356` and latitude
  `39.94018` to `39.96987`. It selects OpenStreetMap ways tagged
  `building:part`. An explicit height takes priority over a level count, and
  one level is estimated as 3.2 metres. Parts without either value are skipped.
  The current renderer does not draw these untextured parts.
- The public 2015 Center City I3S service contains 367 detailed leaf chunks.
  The importer reads each binary geometry resource and its matching JPEG atlas.
  It converts longitude and latitude offsets to EPSG:32129, applies each atlas
  region to the UV coordinates, and checks a 400 metre height ceiling.
- PASDA's legacy downtown package contains 2,689 highest-detail (`r0`)
  KML/COLLADA components. The photographs and geometry were produced in 2008
  and 2009 even though PASDA also publishes the files under a 2010 download
  path. New checkouts use `kml00.zip`, which contains r0 and r1, and import r0
  only. The importer can instead read the retained 2.4 GB outer archive without
  downloading a duplicate. It validates every model against the audited r0
  geographic envelope.
- The City of Philadelphia's 2008 stadium-area archive contains 814 highest
  detail (`r0`) KML/COLLADA models around the sports complex. The importer
  validates their published KML bounds, metre scale, Z-up axis, neutral
  orientation, and JPEG texture references. Local east/north offsets are
  anchored by each KML model location and projected to EPSG:32129. COLLADA's
  bottom-left UV origin is converted to the renderer's top-left image origin.
  Six components representing the Spectrum are excluded because the arena was
  [demolished after this survey](https://corporate.comcast.com/comcast-voices/saying-goodbye-to-the-spectrum)
  and the current aerial layer shows the replacement site. The resulting 808
  models have 126,181 textured triangles. Their diffuse-only triangles
  are omitted because they are source solids without photographic material,
  not textured exterior surfaces.
- Mesh sources are merged in this order: 2015 I3S, 2008/09 legacy downtown,
  2008 stadium, then City footprint fallback. A lower-priority mesh is removed
  when a newer footprint covers its representative point or at least one
  quarter of its area. The runtime's existing mesh footprint test suppresses
  the final untextured footprint fallback.

## World binary format

All integers and floats are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOPHILY"
u32      version (5)
u32      EPSG (32129)
u32      building count
u32      building part count
u32      building mesh count
u32      water ring count
u32      park ring count
u8 x 32  SHA-256 digest of all texture atlases
f64 x 4  official city bounds: min_x, min_y, max_x, max_y
repeat building count times:
  f32    height
  ring
repeat building part count times:
  u64    OpenStreetMap way ID
  f32    height
  f32    minimum height
  f32    roof height
  u8     roof shape
  u8 x 4 facade RGBA; alpha 0 means no sourced color
  ring
repeat building mesh count times:
  u32    I3S node ID
  u32    texture atlas ID
  f32    height
  u32    face count
  ring   footprint used for indexing
  repeat face count times:
    repeat 3 times: f32 x, f32 y, f32 z, f32 u, f32 v
repeat water and park counts:
  ring

ring:
  u32    point count
  repeat point count times: f32 x, f32 y
```

Roof shape values are 0 flat, 1 gabled, 2 hipped, 3 pyramidal, 4 dome, 5 cone,
and 6 mansard. The clean metadata stores the Overpass generator, data timestamp,
query URL, response checksum, and part count.

The I3S scene and both COLLADA archives expose triangle positions, UV
coordinates, and JPEG textures. The renderer samples those textures on the
matching triangles. It returns an error when a texture is missing, so it cannot
replace a textured building with a plain polygon.

## Street binary format

All integers and floats are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOSTRPH"
u32      version (1)
u32      EPSG (32129)
u32      line count
f64 x 4  official city bounds: min_x, min_y, max_x, max_y
repeat line count times:
  u8     City street class
  u32    point count
  repeat point count times: f32 x, f32 y
```

The street artifact is deliberately separate so roads can be added or tuned
without changing the stable `philly.bin` contract.

## Aerial imagery

Textured rendering requests 2024 Philadelphia orthophotography from the
[PASDA ArcGIS image service](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2024/MapServer).
The source advertises one inch imagery. The renderer asks for a 512 by 512
JPEG in EPSG:32129 over the source footprint of each z8 isometric tile. The crop
includes a two pixel overlap. The renderer never stretches a whole city
preview.

The renderer chooses one source tile for each world coordinate. The choice does
not depend on the output tile, so a building cannot change color at a tile
edge. The renderer then snaps sampling to a global grid, averages a 3 by 3
source neighborhood, and posterizes each channel. It does not generate imagery.
Source crops persist under `data/aerial/` until the fixed 8 GiB cache ceiling is
reached. The prebuild renders z8 as lossless WebP and derives the lower levels
from it. The browser magnifies z8 for closer views instead of generating more
tiles.

For each footprint outside the official I3S coverage, the renderer reverse-maps
roof pixels into the source plane and samples this same aerial crop. It then
uses a stable median of interior footprint samples for each wall. The fallback
does not claim to reconstruct a facade. A shared
depth buffer preserves overlap among citywide buildings. Official textured
triangles render over the fallback buildings in their coverage area.

## Provenance

The sources and licensing caveat are documented in [RESEARCH.md](RESEARCH.md).
The building source must contain at least 300 MB and produce at least 500,000
usable footprints. If the live Hub export returns a short HTTP 200 response,
ingest uses the newest complete content-addressed snapshot. It stops before
overwriting the clean artifact when neither source is complete.
The City catalog describes the building footprints as weekly-updated public
data, the hydrology as current surface-water geometry, the street centerline as
a reference base layer, and City Limits as the generalized standard boundary.
Before redistributing a generated dataset or commercial image, verify the
current City of Philadelphia terms and preserve attribution. The interface must
also display [OpenStreetMap contributor attribution](https://www.openstreetmap.org/copyright).

## Licensing boundary

The repository's MIT license applies only to the project code. It does not
relicense City source data or generated tiles. OpenDataPhilly catalog pages
identify these inputs as City of Philadelphia data and link to the applicable
terms. Review the current [City of Philadelphia terms of
use](https://www.phila.gov/terms-of-use/) before publishing or commercially
using a derived artifact, and display City of Philadelphia and OpenDataPhilly
attribution with the map. OpenStreetMap data is available under the ODbL, which
has separate attribution and database sharing requirements. This project is an
illustration, not an authoritative boundary, property, elevation, or navigation
product.

Public access to the I3S service and PASDA stadium archive does not state that
their JPEG textures may be redistributed. Get written permission from the City
before publishing generated tiles that contain those textures. Local
development does not resolve that publication requirement. Source attribution
and the PASDA metadata URL are recorded in `meta.json`.
