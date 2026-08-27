# Data pipeline

`uv run poe ingest` downloads immutable, content-addressed snapshots of five
official City of Philadelphia ArcGIS layers, processes them offline, and writes:

- `data/clean/philly.bin`: the stable version-1 building/water/park world read by
  the Rust server.
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

## Provenance

The sources and licensing caveat are documented in [RESEARCH.md](RESEARCH.md).
The City catalog describes the building footprints as weekly-updated public
data, the hydrology as current surface-water geometry, the street centerline as
a reference base layer, and City Limits as the generalized standard boundary.
Before redistributing a generated dataset or commercial image, verify the
current City of Philadelphia license/terms and preserve attribution.

## Licensing boundary

The repository's MIT license applies only to the project code. It does not
relicense City source data or generated tiles. OpenDataPhilly catalog pages
identify these inputs as City of Philadelphia data and link to the applicable
terms. Review the current [City of Philadelphia terms of
use](https://www.phila.gov/terms-of-use/) before publishing or commercially
using a derived artifact, and display City of Philadelphia/OpenDataPhilly
attribution with the map. This project is an illustration, not an authoritative
boundary, property, elevation, or navigation product.
