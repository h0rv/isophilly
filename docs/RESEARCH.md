# Research: deterministic isometric Philadelphia

Research date: 2026-08-28. This is deliberately a **data-and-rendering**
recommendation, not a proposal to train or ship a generative-image product.

## 80/20 recommendation

Use the official 2015 Center City I3S scene for detailed architecture and
facade textures. Across the rest of Philadelphia, extrude official City
footprints at their supplied heights, sample the 2025 PASDA aerial imagery on
the roofs, and derive a restrained wall palette from the same local pixels.
This gives every neighborhood real geometry, height, roof detail, and local
color while remaining honest that unseen walls are illustrative.

Defer citywide point-cloud reconstruction, vegetation geometry, and an
interactive 3-D engine. The City footprint service has **546,084** features
(count queried
2026-08-26); it is large but tractable offline. The captured GeoJSON is
**476.4 MB** on disk. The earlier 110 MB estimate incorrectly inferred size
from its 22-part multipart ETag; the part count does not establish part size. A citywide
point-cloud workflow is materially larger and adds roof-cleanup without being
needed for the intended illustrated isometric look. The current citywide pass
uses the footprint service's height fields and keeps the source limitations
visible in its documentation.

The data we publish should record source URL, request timestamp, and checksum.
Do not silently refresh an artwork from a live endpoint.

## What Isometric NYC teaches us

[Isometric NYC](https://isometric.nyc/) is an AI-styled map, but its strongest
reusable ideas are architectural:

* Its [source (MIT)](https://github.com/cannoneyed/isometric-nyc) separates the
  city-generation workflow, a 3-D renderer, and the viewer. Copy that separation,
  not its model dependency.
* The author first rendered CityGML geometry plus satellite imagery, then found
  their misalignment caused image errors. The [project write-up](https://cannoneyed.com/projects/isometric-nyc)
  explains why it switched to aligned Google 3D Tiles. Philadelphia's I3S scene
  already provides aligned triangles, UV coordinates, and texture atlases, so
  the renderer can use the official mapping without generating textures.
* Their AI path required an estimated 40k tiles, manual review of seams/trees,
  and an approximately 50/50 high-quality generation rate. That is a poor
  trade for a map whose geometry must be trusted. Deterministic rendering makes
  diffs, regression checks, and attribution practical.
* Their [data notes](https://github.com/cannoneyed/isometric-nyc/blob/main/docs/data.md)
  describe about 32k generated quadrants; the raw hosted data is about
  [22.9 GB](https://www.oxen.ai/cannoneyed/isometric-nyc-tiles). Do **not** copy
  that storage budget for v1.
* The best delivery pattern is their [DZI/WebP pyramid](https://github.com/cannoneyed/isometric-nyc/blob/main/docs/app.md):
  OpenSeadragon, static CDN files, browser caching/prefetching, and 25–35% smaller
  WebP tiles than PNG. Keep a small metadata file mapping the pixel space to the
  city projection/bounds for callouts and search.

## Philadelphia source stack

All City catalog entries below say “City of Philadelphia License” and public
use/free. That is **not** a standard permissive software/data license: the
[City terms](https://www.phila.gov/terms-of-use/) say commercial distribution,
republication, and modification need prior written permission. Confirm the
current dataset-specific terms/attribution with the City before commercial use
or redistributing source/derived imagery; preserve source attribution in the
artifact meanwhile. PASDA/NOAA-hosted copies may have additional terms.

| Priority / layer | Authoritative source and endpoint | Quality / recency | Approx. ingestion cost | Decision |
| --- | --- | --- | --- | --- |
| 1. City clip | [City Limits catalog](https://opendataphilly.org/datasets/city-limits/) and [FeatureServer](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/City_Limits/FeatureServer/0) | Official generalized standard boundary; catalog says updated 2012/as needed. | One small polygon; trivial. | Use as outer mask only; do not infer shoreline precision. |
| 1. Buildings | [Building Footprints catalog](https://opendataphilly.org/datasets/building-footprints/), [GeoJSON download](https://hub.arcgis.com/api/v3/datasets/ab9e89e1273f445bb265846c90b38a96_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1), [FeatureServer](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/LI_BUILDING_FOOTPRINTS/FeatureServer/0) | Official planimetric outlines; City service describes early-2015 imagery plus continuous updates, catalog says weekly. It includes houses, commercial/industrial buildings, sheds, garages, etc. | 546,084 polygons; 476.4 MB GeoJSON captured on 2026-08-26; expect several hundred MB RAM in Python/GDAL. Batch/page or download snapshot once—never render straight from HTTP. | Core layer. Dissolve/clip/simplify only after retaining an immutable raw snapshot. |
| 1. Water | [Hydrology catalog](https://opendataphilly.org/datasets/hydrology/) (polygon GeoJSON/SHP/API links) | Philadelphia Water Department polygons for rivers, creeks, ponds, reservoirs, water under bridges and edge water. PASDA lists the current hydrographic polygon/arc data as 2025. | Small vector layer; trivial. | Use polygons, not a filled city-boundary void: it preserves river shape and islands. |
| 1. Roads | [Street Centerlines catalog](https://opendataphilly.org/datasets/street-centerlines/), [GeoJSON download](https://hub.arcgis.com/api/v3/datasets/c36d828494cd44b5bd8b038be696c839_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1), [FeatureServer](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/Street_Centerline/FeatureServer/0) | Citywide reference base layer. The City explicitly says it is not exact engineering geometry. | Manageable linework, but visually noisy at full city scale. | Filter by class/name/length; render arterials and a low-opacity local grid rather than every segment. |
| 1. Center City parts | [OpenStreetMap Simple 3D Buildings](https://wiki.openstreetmap.org/wiki/Simple_3D_Buildings) and the [ODbL terms](https://www.openstreetmap.org/copyright) | Current way geometry and tags. The 2026-08-27 snapshot produced 826 usable parts after height checks. | The Overpass JSON is about 0.8 MB. | Use explicit height first, then levels. Skip missing heights. Keep visible contributor attribution. |
| 1. Center City scene | [Philadelphia Buildings I3S service](https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/Philadelphia_Buildings/SceneServer) | 367 official detailed chunks with roofs, facades, setbacks, landmarks, UV coordinates, and JPEG atlases. | About 38 MB of binary geometry and 146 MB of atlases in the current cache. | Render the textured triangles once into the canonical z8 artwork. Get written City permission before redistributing tiles that include the atlases. |
| 1. Aerial color/reference | [Aerial imagery catalog](https://opendataphilly.org/datasets/aerial-photography/); [2025 PASDA image service](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer) | 2025, three-inch orthophotography exposed through an export API and downloadable source TIFF tiles. | A 512-pixel crop per isometric tile avoids a citywide mosaic. First builds pay network latency; a bounded shared disk cache makes repeats local. | Use one deterministic pixel treatment for ground, real roof pixels, and local wall color in the canonical z8 scene. Geometry remains authoritative City vectors. |
| 2. Terrain/height | [2022 LiDAR/LAS catalog](https://opendataphilly.org/datasets/lidar-las-data/); [2022 DEM catalog/PASDA](https://www.pasda.psu.edu/uci/DataSummary.aspx?dataset=7152); [NOAA LAZ archive](https://noaa-nos-coastal-lidar-pds.s3.amazonaws.com/laz/geoid18/9848/index.html) | Citywide capture from Apr. 2022. Classified points can refine roof form and height; the DEM alone is only ground elevation. | The NOAA archive is about 93 GB across 752 LAZ files. A full default import would dwarf the current pipeline. | Keep optional and targeted. Use LAZ surface minus ground only where it materially improves a landmark or roof shape; do not impose a 93 GB first run. |
| 3. Street facade reference | [KartaView photo API](https://kartaview.org/doc/photos), [license FAQ](https://kartaview.org/doc/faq) | Public crowdsourced photos expose position, heading, time, and image URLs under CC BY-SA 4.0. A Rittenhouse test found only three images within 500 m, from different years. | Coverage and camera pose are uneven. Correctly projecting a photo onto a visible wall also requires occlusion and attribution handling. | Useful future opt-in source, not a citywide default. Audit coverage before downloading, and never smear a nearby photo across an unmatched facade. |

### Deterministic ingestion rules

1. Request/download raw data once, with an explicit CRS and a recorded UTC
   snapshot date. ArcGIS REST query endpoints normally page results; ask for
   only needed fields and geometry, and page by object ID/bounding box rather
   than relying on a single all-record response.
2. Reproject vectors into a local projected CRS before buffering, simplifying,
   or generating building height. Choose the renderer’s orthographic transform
   once and snapshot its parameters.
3. Apply a scale-aware simplification after clipping to the chosen extent:
   rowhouse detail should read as grain, not create hairline moiré. Preserve a
   full-resolution source for deep zoom.
4. Use the official 2015 textured mesh where it exists. Outside it, use the
   City footprint and source height with aerial-sampled roofs and restrained
   aerial-derived walls. Do not add hand-built landmark shapes when reusable
   source geometry exists.

## Open-source components worth copying

| Component | Why it is useful | Fit |
| --- | --- | --- |
| [cannoneyed/isometric-nyc](https://github.com/cannoneyed/isometric-nyc) (MIT) | Reference implementation for an isometric-city pipeline, orthographic 3-D tile renderer, DZI export, bounds clipping, and a React viewer. | Read/copy small pipeline ideas; do not inherit its AI generation, Google 3-D Tiles dependency, 22.9-GB tile corpus, or unmaintained “agent-built” complexity. |
| [OpenSeadragon](https://github.com/openseadragon/openseadragon) (BSD-3-Clause) + [DZI](https://openseadragon.github.io/examples/tilesource-dzi/) | Mature static deep-zoom viewer. Maps precisely to the desired “one enormous artwork, explore by zooming” interaction. | **Recommended viewer.** No GIS runtime or API key required after publish. |
| [libvips](https://www.libvips.org/) (LGPL-2.1+) | Fast, memory-efficient image pyramid creation; Isometric NYC uses it to produce its DZI/WebP export. | **Recommended offline tiler**, subject to normal LGPL distribution compliance. |
| [MapLibre GL JS](https://maplibre.org/maplibre-gl-js/docs/) (BSD-3-Clause) | Open WebGL map renderer; supports [3-D building/fill-extrusion examples](https://maplibre.org/maplibre-gl-js/docs/examples/). | Use only if v2 truly needs free camera/live GIS layers. It is unnecessary complexity for the artwork-first v1. |
| [streets-gl](https://github.com/StrandedKitty/streets-gl) | WebGL2 OSM 3-D renderer that generates geometry on the fly (buildings/roads/trees). | Study its batching/render-graph ideas only; its OSM schema and free-camera product are not the Philly source-of-truth path. |

## Explicit non-recommendations for v1

* Do not train image models or use satellite-to-pixel generation. It compromises
  geographic consistency, creates seam QA, and makes licensing/provenance harder.
* Do not use Google 3-D Tiles as a production geometry/texture source; it is
  neither the City’s authoritative geometry nor compatible with a simple,
  independently redistributable deterministic pipeline.
* Do not make a browser fetch 546k footprints or citywide LiDAR at runtime.
  Pre-render the pyramid; retain the original vectors privately for rebuilds.
