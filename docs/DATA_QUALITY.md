# Data quality audit

Measured 2026-08-28 against the raw snapshots and version 5 clean binaries.
Counts describe this snapshot, not permanent properties of the live services.
Source URLs and full SHA-256 checksums are in
`data/clean/meta.json`; the source rationale is in [RESEARCH.md](RESEARCH.md).

## Delivered coverage

| Layer | Raw City records | Clean artifact | Notes |
| --- | ---: | ---: | --- |
| Building footprints | 546,084 | 545,672 polygons | Output is polygon parts after clipping, repair, multipart expansion, and the 10 m² cutoff—not a record join count. |
| OSM Center City building parts | 1,039 ways | 827 polygons | Output keeps parts with a valid height or level count and rejects bad geometry. Of these, 275 have a sourced facade color or material. |
| Official 2015 Center City 3D scene | 367 leaf chunks | 294,443 textured triangles and 367 JPEG atlases | Output keeps the I3S triangles, UV coordinates, atlas regions, and textures. |
| Legacy 2008/09 downtown 3D models | 2,689 highest-detail models | Accepted model count is recorded in `meta.json` after overlap suppression | Output keeps r0 photographically textured geometry outside the newer I3S coverage. |
| Official 2008 stadium-area 3D models | 814 highest-detail models | 126,181 textured triangles and 808 JPEG textures | Output keeps the textured KML/COLLADA geometry and excludes six components belonging to the demolished Spectrum. |
| Hydrology polygons | 2,000 | 69 polygons | Exactly the 69 source records that intersect the official City Limits mask. |
| PPR properties | 506 | 659 polygons | 505 records intersect; repair and multipart expansion produce more output polygons. |
| Street centerlines | 41,271 | 40,418 lines | Selected classes are clipped and multipart lines can split at the boundary. |

The official City Limits extent is 27.29 × 30.52 km in EPSG:32129. It replaces
the former layer-derived 31.91 × 41.93 km extent, so off-city hydrology no longer
creates a large empty map.

The five raw GeoJSON snapshots total about 516 MB. The OSM JSON adds about 0.8
MB. Building footprints alone are 476.4 MB in this capture. The I3S cache adds
about 38 MB of geometry and 146 MB of JPEG atlases. The runtime world is about
54.5 MB before the stadium models; the stadium archive is 647 MB and contributes
about 84 MB of source JPEG textures. The street file is about 1.0 MB.

## Building heights

The raw footprint table supplies much more height coverage than a default-only
v1 plan would imply:

| Selected height source | Raw footprints | Share |
| --- | ---: | ---: |
| Valid City `approx_hgt` | 540,569 | 98.99% |
| `max_hgt` fallback | 444 | 0.08% |
| Fixed 8 m fallback | 5,071 | 0.93% |

Both City fields are interpreted as feet. Values below 2.4 m or above 400 m
are rejected before fallback. Of the raw `approx_hgt` values, 5,279 are below
that floor and none exceed the ceiling.

The packed height distribution is strongly rowhouse-shaped:

| Height | Buildings | Share |
| --- | ---: | ---: |
| 2.4–5 m | 34,019 | 6.23% |
| 5–10 m | 430,817 | 78.95% |
| 10–20 m | 78,982 | 14.47% |
| 20–40 m | 1,335 | 0.25% |
| 40–80 m | 380 | 0.07% |
| 80 m+ | 139 | 0.03% |

Median height is 7.92 m; p90 is 10.67 m, p99 is 15.54 m, and the maximum is
297.49 m. Outside the detailed Center City mesh, the renderer extrudes these
footprints across Philadelphia. Roofs sample the aligned 2025 aerial crop,
while walls use a desaturated local palette derived from robust samples around
the footprint.

The OSM query returned 446 parts with an explicit height and 400 parts with only
a level count. Another 193 parts had neither value and were skipped. One level
is estimated as 3.2 metres. The packed part heights range from 3.2 to 341 metres.
The raw roof tags include 888 flat, 56 gabled, 35 hipped, 28 pyramidal, and 9
dome parts. The current renderer does not draw these untextured parts.

## Street classification

The clean artifact keeps City classes 1–5, 9, and 10:

| Class | Meaning | Lines | Share |
| ---: | --- | ---: | ---: |
| 1 | Expressway | 416 | 1.03% |
| 2 | Major arterial | 4,439 | 10.98% |
| 3 | Minor arterial | 4,505 | 11.15% |
| 4 | Collector | 15,492 | 38.33% |
| 5 | Local | 15,287 | 37.82% |
| 9 | Low-speed ramp | 117 | 0.29% |
| 10 | High-speed ramp | 162 | 0.40% |

The raw layer has 40,490 records in those classes; 40,413 intersect City Limits,
and boundary splitting yields 40,418 packed lines. The 781 class-filtered raw
records are class 6 driveways (94), class 12 non-traversable segments (421),
class 14 boundary lines (167), class 15 walking connectors (11), and 88 records
using undocumented classes 13 or 18. The current renderer uses the aerial
image for roads instead of drawing these centerlines.

## Known limitations

- Binary version 5 stores only exterior rings for the citywide footprint index.
  The City source contains 609 footprint features with 1,006 interior rings, so
  courtyards and atria are filled in the citywide extrusion.
- The 2015 I3S scene covers Center City. Outside it, walls are an
  aerial-derived illustration, not observed facade photography. Roof alignment
  can also expose shadows or small date differences between the continuously
  updated footprints and the 2025 image capture.
- The stadium scene was captured in 2008. The importer excludes the demolished
  Spectrum, but other structures and surface details can still differ from the
  current aerial photography.
- The legacy downtown scene was produced in 2008 and 2009. It extends the
  textured area beyond the 2015 scene but can show buildings that later changed.
- The 0.35 m footprint and 1 m ground/street simplification tolerances are fixed,
  not zoom-specific. Coordinates become `f32` in the binaries.
- City Limits is an official generalized cartographic mask, not a surveyed
  shoreline. Clipping water to it is correct for a Philly-only composition but
  deliberately removes surrounding river context.
- Snapshots are content-addressed and checksummed, but `poe ingest` still fetches
  the current live services. There is no pinned-manifest/offline rebuild command
  yet, so a historical artifact cannot be reproduced from metadata alone unless
  its raw snapshot directory was retained.
- City centerlines describe street topology, not road surfaces. Driveways,
  walking connectors, and undocumented classes are intentionally absent.

## Next visible upgrades

1. Get written permission to redistribute tiles that include the City texture
   atlases.
2. Validate that the canonical z8 pixels stay aligned during every zoom step.
3. Find a newer official textured 3D scene or a practical, openly licensed
   street-level source that can add observed facades without losing citywide
   coverage.
