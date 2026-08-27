# Data pipeline

`uv run poe ingest` downloads content-addressed snapshots of five City of
Philadelphia ArcGIS layers and one OpenStreetMap query. It processes them
offline and writes:

- `data/clean/philly.bin`: the version 2 building, building part, water, and park
  world read by the Rust server.
- `data/clean/streets.bin`: a separate optional street-centerline artifact.
- `data/clean/meta.json`: source URLs, request times, HTTP validators, SHA-256
  checksums, CRS, bounds, counts, and output checksums.

Generated data is ignored by Git. Keep `data/raw/` and `meta.json` together when
pinning a release: the raw filenames include the first 12 characters of their
full SHA-256 digest, so a live source refresh never silently overwrites a prior
snapshot.

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
- streets include City classes 1–5, 9, and 10 (expressways through local roads
  and ramps). Class 6 driveways and non-traversable/walking/boundary lines are
  excluded from the clean render input.
- the Center City query covers longitude `-75.19042` to `-75.13356` and latitude
  `39.94018` to `39.96987`. It selects OpenStreetMap ways tagged
  `building:part`. An explicit height takes priority over a level count, and
  one level is estimated as 3.2 metres. Parts without either value are skipped.
  The renderer retains the City footprint under incomplete part sets.

## World binary format

All integers and floats are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOPHILY"
u32      version (2)
u32      EPSG (32129)
u32      building count
u32      building part count
u32      water ring count
u32      park ring count
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
  ring
repeat water and park counts:
  ring

ring:
  u32    point count
  repeat point count times: f32 x, f32 y
```

Roof shape values are 0 flat, 1 gabled, 2 hipped, 3 pyramidal, 4 dome, 5 cone,
and 6 mansard. The clean metadata stores the Overpass generator, data timestamp,
query URL, response checksum, and part count.

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

Textured rendering requests 2025 Philadelphia orthophotography from the
[PASDA ArcGIS image service](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer).
The source advertises three inch imagery. The renderer asks for a 1024 by 1024
JPEG in EPSG:32129 over the exact source footprint of each isometric tile. The
crop includes a two pixel overlap. The renderer never stretches a whole city
preview.

The original photograph is cached once and shared by the two deterministic
render modes. `full` uses bilinear color sampling. `pixel` snaps sampling to a
global grid, averages a 3 by 3 source neighborhood, and posterizes each channel.
Neither path generates imagery. Source crops persist under `data/aerial/` until
the fixed 1 GiB cache ceiling is reached. Final PNG tiles through z8 have their
own bounded cache. Deeper final tiles stay volatile.

## Provenance

The sources and licensing caveat are documented in [RESEARCH.md](RESEARCH.md).
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
