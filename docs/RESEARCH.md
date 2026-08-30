# Research: deterministic isometric Philadelphia

Research updated: 2026-08-29. This is deliberately a **data-and-rendering**
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
| 1. Water and parks | [Hydrology catalog](https://opendataphilly.org/datasets/hydrology/) and City PPR properties | Small official masks that identify which aerial pixels represent water or vegetation. | Small vector layers. | Apply restrained color grading to the aerial image. Never replace it with flat polygons. |
| Removed. Roads | [Street Centerlines catalog](https://opendataphilly.org/datasets/street-centerlines/) | Citywide reference linework, not exact road surfaces. | Manageable but visually noisy. | Do not ingest while the aerial image is the road surface. |
| 1. Center City parts | [OpenStreetMap Simple 3D Buildings](https://wiki.openstreetmap.org/wiki/Simple_3D_Buildings) | The current snapshot provides 827 height-backed parts, including the Comcast Technology Center shaft and wings. | Small cached snapshot; live Overpass refresh is optional. | Use as fallback geometry only where no photographed mesh exists. Suppress the parent footprint when parts cover most of it. |
| 1. Center City scene | [Philadelphia Buildings I3S service](https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/Philadelphia_Buildings/SceneServer) | 367 official detailed chunks with roofs, facades, setbacks, landmarks, UV coordinates, and JPEG atlases. | About 38 MB of binary geometry and 146 MB of atlases in the current cache. | Render the textured triangles once into the canonical z8 artwork. Get written City permission before redistributing tiles that include the atlases. |
| 1. Legacy downtown scene | [PASDA 2008 and 2009 downtown KML archive](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2010/kml00.zip) | 2,689 highest-detail models with photographed roofs and facades. It extends farther east, west, and south than the 2015 scene. | The smaller download is about 886 MB. Existing checkouts can reuse the retained 2.4 GB outer archive. | Import only `r0`, suppress overlap under the 2015 scene, and record the real 2008 and 2009 date. |
| 1. Stadium scene | [PASDA 2008 stadium-area KML archive](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2008/Stadium%20Area%20Processed%20w%20LiDAR-KML.zip) | 814 highest-detail KML/COLLADA components with measured geometry and JPEG material textures. The 2008 source includes the since-demolished Spectrum. | 647 MB nested archive; output keeps 808 current components, 126,181 textured triangles, and about 84 MB of JPEGs. | Render through the same textured-mesh path as Center City; exclude the six Spectrum components and record the historical capture date. |
| 1. Aerial color/reference | [Aerial imagery catalog](https://opendataphilly.org/datasets/aerial-photography/); [2025 PASDA image service](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer) | 2025 three-inch orthophotography exposed through an export API. | Fixed 1,536 metre exports preserve the 0.75 metre working grid while reducing first-build requests. A bounded shared disk cache makes repeats local. | Use one deterministic pixel treatment for ground, real roof pixels, and local wall color in the canonical z8 scene. Geometry remains authoritative City vectors. |
| 2. Terrain/height | [2022 LiDAR/LAS catalog](https://opendataphilly.org/datasets/lidar-las-data/); [2022 DEM catalog/PASDA](https://www.pasda.psu.edu/uci/DataSummary.aspx?dataset=7152); [NOAA LAZ archive](https://noaa-nos-coastal-lidar-pds.s3.amazonaws.com/laz/geoid18/9848/index.html) | Citywide capture from Apr. 2022. Classified points can refine roof form and height; the DEM alone is only ground elevation. | The NOAA archive is about 93 GB across 752 LAZ files. A full default import would dwarf the current pipeline. | Keep optional and targeted. Use LAZ surface minus ground only where it materially improves a landmark or roof shape; do not impose a 93 GB first run. |
| 3. Street facade reference | [KartaView photo API](https://kartaview.org/doc/photos), [license FAQ](https://kartaview.org/doc/faq) | Public crowdsourced photos expose position, heading, time, and image URLs under CC BY-SA 4.0. A Rittenhouse test found only three images within 500 m, from different years. | Coverage and camera pose are uneven. Correctly projecting a photo onto a visible wall also requires occlusion and attribution handling. | Useful future opt-in source, not a citywide default. Audit coverage before downloading, and never smear a nearby photo across an unmatched facade. |

### Citywide texture expansion

The simplest credible next source is not satellite or random plane photography.
It is a registered ensemble of the City's annual orthophotos. The catalog calls
2022 a two-inch capture, but the service metadata and raster spacing consistently
report three-inch pixels for both 2022 and 2023. The
[2025 metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=PhiladelphiaImagery2025.xml)
lists no use constraints. Keep 2025 as the canonical surface, align the older
captures to the same State Plane grid, and select pixels from another year only
where the current image has a shadow, glare, cloud, or occlusion. A median is
useful inside a detected defect. Averaging every pixel would ghost cars, trees,
and changed buildings.

Both older ArcGIS services accept the same EPSG:32129 fixed-grid export scheme:

- [2022 export](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2022/MapServer/export)
  needs `layers=show:3` because its parent layer is hidden.
- [2023 export](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2023/MapServer/export)
  works without a layer override.

For a deterministic repair, normalize each older cell to 2025 using robust
channel percentiles from its neighbors. Replace a 2025 pixel only when the 2022
and 2023 colors agree, both are at least 20 luminance steps brighter, and 2025
luminance is below 70 percent of both. Remove tiny mask regions and feather the
boundary by two to four output pixels. Keep 2025 whenever the older years
disagree. This guards against construction, trees, moving cars, and different
building lean. It repairs some shadows and transient occlusion but cannot invent
a facade missing from every nadir capture.

True citywide photographed facades need calibrated oblique or stereo frames.
The City's [2024 imagery procurement answers](https://www.phila.gov/media/20230927120650/RFI-PWD-planimetric-data-20230915-q-and-a.pdf)
describe controlled EagleView imagery, aerial triangulation, and 2024 LiDAR.
Ask `maps@phila.gov` for those source frames and written permission to publish
irreversible rasterized texture tiles. The public
[Pictometry viewer](https://pictometry.phila.gov/) is evidence that the imagery
exists, not permission to scrape or redistribute it.

#### EagleView/Pictometry access

The viewer can be driven deterministically, but it is not a public image API.
Philadelphia's Atlas and Pictometry front ends mount EagleView's credentialed
Embedded Explorer. Its documented API can set an exact longitude, latitude,
zoom, pitch, and rotation and reports the resulting view through
`onViewUpdate`. It does not document a method for downloading the underlying
image, its camera model, or a citywide archive. Reusing a browser token or a
credential embedded in the City's deployment would be brittle and outside the
intended access path.

EagleView's separate Imagery API is the correct deterministic source. Its
[official demo](https://www.eagleview.com/blog/developer-demo-see-the-imagery-api-in-action/)
documents discovery, 256 by 256 ortho and oblique tiles, images up to 4096 by
4096 pixels, and an orthomosaic endpoint. The
[official API reference](https://developer.eagleview.com/docs/imagery/api-documentation.md)
documents the exact operations needed for a reproducible import:

- `POST /imagery/v3/discovery/rank/location` ranks north, east, south, west,
  and orthogonal captures for a point or polygon;
- `POST /imagery/v3/discovery/orthomosaics/search` finds stitched top-down imagery;
- `GET /imagery/v3/images/{image_urn}/location` returns a geospatial crop; and
- `GET /imagery/v3/images/{image_urn}/tiles/{z}/{x}/{y}` returns 256-pixel tiles.

The default documented limits are five discovery or image requests per second
and 300 tile requests per second. The normal citywide workflow would:

1. submit the Philadelphia boundary as an authorized area of interest;
2. discover captures once and pin capture IDs, dates, orientations, and checksums;
3. enumerate our canonical z8 cells, request every intersecting oblique image,
   and retain the best two opposing views where the license permits it;
4. project visible facade samples onto the existing textured meshes or City
   footprints, with depth and occlusion checks; and
5. pre-render the static pyramid so no EagleView credentials or source pixels
   reach the browser.

The free developer sandbox uses a vendor-selected sample area. EagleView's
[current trial](https://www.eagleview.com/blog/eagleview/eagleview-launches-an-early-access-free-imagery-api-trial-to-power-the-next-generation-of-geospatial-applications/)
allows a developer-selected two-square-mile area for 30 days, which is enough
for a Center City proof but not Philadelphia. Before implementing an importer,
obtain either City-approved bulk delivery or production Imagery API access with
explicit derivative-tile publication rights. Then implement against the
official OpenAPI specification and recorded credentials rather than reverse
engineering the public viewer.

Discovery metadata includes capture date, ground sample distance, image ground
footprint, tile bounds, an estimated requested pixel, and a `look_at` estimate
with camera center, azimuth, and elevation. The public specification does not
expose focal length, principal point, distortion, or roll. It is enough to pull
deterministic crops and derive honest local facade colors, but not enough by
itself for exact photographic UV projection. A production agreement should
also include full exterior/interior camera calibration or a prepared textured
3-D deliverable, plus explicit rights for bulk retrieval, persistent caching,
derivative texture atlases, public web tiles, attribution, and post-contract
retention.

[WorldView 3D](https://developers.maxar.com/docs/ordering/guides/worldview-3d-ordering)
can provide a commercial 0.5 metre textured surface from stereo satellite
imagery. It is quote-priced and public redistribution is not automatic. It is
unlikely to recover rowhouse facades well enough to justify the cost. Do not
order it without a sample scene and explicit web-tile rights.

Uncalibrated amateur or government oblique photos can help a hand-registered
landmark. They cannot be averaged into the city map. Without camera pose,
control points, overlap, and visibility data, automated projection produces
misregistration and repeated-building artifacts. KartaView is the only useful
open pilot found near Center City, but its sparse coverage and CC BY-SA terms
make it an optional, separately attributed source rather than the default.

### Rejected PASDA raw frames

PASDA exposes about 19,208 RGB TIFF frames in a separate `philly_nadir`
directory. The archive is roughly 1 TB. A sampled 4,872 by 3,248 TIFF had no
GeoTIFF tags, world file, camera pose, or exterior orientation. It was near
vertical and showed little useful facade. A coarse feature match against the
official 2010 orthophoto produced no reliable registration.

The directory exposes filenames but no frame footprint catalog. The imagery
metadata says there are no use constraints, while related Pictometry material
has license restrictions. Do not ingest this archive. A future evaluation first
needs image footprints, camera orientation and calibration, and written
permission to publish derivatives.

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
| [OpenSeadragon](https://github.com/openseadragon/openseadragon) (BSD-3-Clause) + [DZI](https://openseadragon.github.io/examples/tilesource-dzi/) | Mature static deep-zoom viewer. Maps precisely to the desired “one enormous artwork, explore by zooming” interaction. | Keep as a fallback. The current small canvas viewer already provides bounded pan, zoom, overlays, and prefetch without another dependency. |
| [libvips](https://www.libvips.org/) (LGPL-2.1+) | Fast, memory-efficient image pyramid creation; Isometric NYC uses it to produce its DZI/WebP export. | Consider only if the current parallel Rust parent-tile builder becomes a measured bottleneck. |
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
