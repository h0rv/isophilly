# Data quality audit

Measured 2026-08-27 against the raw snapshots and version 2 clean binaries.
Counts describe this snapshot, not permanent properties of the live services.
Source URLs and full SHA-256 checksums are in
`data/clean/meta.json`; the source rationale is in [RESEARCH.md](RESEARCH.md).

## Delivered coverage

| Layer | Raw City records | Clean artifact | Notes |
| --- | ---: | ---: | --- |
| Building footprints | 546,084 | 545,672 polygons | Output is polygon parts after clipping, repair, multipart expansion, and the 10 m² cutoff—not a record join count. |
| OSM Center City building parts | 1,039 ways | 826 polygons | Output keeps parts with a valid height or level count. It also rejects bad geometry and the replaced William Penn placeholder. |
| Hydrology polygons | 2,000 | 69 polygons | Exactly the 69 source records that intersect the official City Limits mask. |
| PPR properties | 506 | 659 polygons | 505 records intersect; repair and multipart expansion produce more output polygons. |
| Street centerlines | 41,271 | 40,418 lines | Selected classes are clipped and multipart lines can split at the boundary. |

The official City Limits extent is 27.29 × 30.52 km in EPSG:32129. It replaces
the former layer-derived 31.91 × 41.93 km extent, so off-city hydrology no longer
creates a large empty map.

The five raw GeoJSON snapshots total about 516 MB. The OSM JSON adds about 0.8
MB. Building footprints alone are 476.4 MB in this capture. Runtime binaries
remain small at about 36.8 MB for the world and 1.0 MB for streets.

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
297.49 m. These are useful City-provided approximations, not architectural
solids: every footprint receives one flat extrusion, vertical renovations may
not be captured, and the sparse skyline tail has not been reconciled against a
landmark list or 2022 LiDAR.

The OSM query returned 446 parts with an explicit height and 400 parts with only
a level count. Another 193 parts had neither value and were skipped. One level
is estimated as 3.2 metres. The packed part heights range from 3.2 to 341 metres.
The raw roof tags include 888 flat, 56 gabled, 35 hipped, 28 pyramidal, and 9
dome parts. Some rare values map to the closest supported roof family, while
unsupported values remain flat.

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
using undocumented classes 13 or 18. Street widths in the renderer are visual
rules by class; the source does not provide measured curb-to-curb width.

## Known limitations

- Binary version 2 still stores only exterior rings. The City source contains
  609 footprint features with 1,006 interior rings, so courtyards and atria are
  filled. It reduces each City footprint to one height. OSM parts can add roof
  geometry in Center City when their tags provide enough data.
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

## Next three visible, non-AI upgrades

1. **Correct the skyline first.** Add a reviewed BIN-keyed landmark-height table
   for the small set of visually dominant buildings, then validate those values
   against robust 2022 LiDAR roof-minus-ground samples. This fixes recognizable
   Center City errors without committing to a multi-gigabyte citywide point-cloud
   pipeline.
2. **Preserve courtyard geometry.** Evolve the world format and rasterizer to
   retain polygon interior rings and multipart identity. Only 0.11% of source
   footprint features have holes, but they are disproportionately large campuses,
   blocks, and civic buildings where the filled courtyard is obvious at deep zoom.
3. **Replace symbolic lines with street surfaces.** Derive scale-aware road
   polygons from the City's curb/cartway geometry where available, falling back
   to class-width buffers for missing segments. Intersections, medians, and the
   diagonal/grid character of Philadelphia would read more accurately than
   fixed-pixel centerline strokes.

Tree canopy is the next layer after these three: it would add substantial visual
texture, but geometry and skyline correctness should come first.
