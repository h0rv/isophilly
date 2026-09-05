# Research: deterministic isometric Philadelphia

Research updated: 2026-09-05. This is deliberately a **data-and-rendering**
recommendation, not a proposal to train or ship a generative-image product.

## 80/20 recommendation

For the first public release, default to the Center City extent and pre-render
four orientations separated by 90 degrees. The official 2015 mesh supplies
photographed geometry and textures where it has coverage. City footprints and
OpenStreetMap parts fill the surrounding gaps. Keep the citywide scene behind
an explicit toggle and describe walls outside photographed coverage as
procedural. The four view renderer stays inside static hosting limits.
Continuous 360-degree orbit requires live 3-D geometry; nadir orthophotography
has only a top view and cannot reveal missing facades.

Render each Center City orientation through z5. At the current extent, one z5
pixel covers about 0.7 metre and uses one sample from the 0.75 metre PASDA
working grid. Draw City footprints and OpenStreetMap parts around the 2015
mesh, and share the depth buffer with the textured triangles. Their roofs can
use aerial pixels, but their procedural walls must not be described as
photographed facades. Four full z5 pyramids add 4,096 files over z4, for a
current export of 18,009 files and 1,245.0 MiB. The finer build costs more time,
while the static server performs the same file reads.

If continuous orbit becomes a product requirement, the lowest-risk hosted
experiment is an opt-in, lazy-loaded
[Google Maps JavaScript 3D map](https://developers.google.com/maps/documentation/javascript/3d-map-overview),
not an offline export of Google tiles. It is technically viable for a live,
continuously rotatable view and supports independent glTF overlays. Google
currently bills that JavaScript route as an Immersive Maps map load: the first
5,000 monthly loads are free, then the price is $7 per 1,000 through 100,000
loads. Direct Map Tiles API Photorealistic 3D Tiles are a separate SKU with
1,000 free root-tile requests, then $6 per 1,000 through 100,000
([pricing](https://developers.google.com/maps/billing-and-pricing/pricing),
[SKU details](https://developers.google.com/maps/billing-and-pricing/sku-details)).
That is not viable under this project's saved-pixel requirement. Google's
[Map Tiles policies](https://developers.google.com/maps/documentation/tile/policies)
prohibit prefetching, persistent or offline caching, image or machine analysis,
and extraction into derived imagery or overlays. The default decision is no
Google integration. Reconsider only if the product accepts live-only delivery,
usage billing, required attribution, and no retained source pixels.

Use the official 2015 Center City I3S scene for detailed architecture and
facade textures. Across the rest of Philadelphia, extrude official City
footprints at their supplied heights, sample the 2025 PASDA aerial imagery on
the roofs, and derive a restrained wall palette from the same local pixels.
This gives every neighborhood real geometry, height, roof detail, and local
color while remaining honest that unseen walls are illustrative.

Defer citywide point-cloud reconstruction, canopy or LiDAR-derived vegetation
geometry, and an interactive 3-D engine. The active renderer already uses the
official 2025 PPR street-tree inventory as a point-based visual layer. The City
footprint service has **546,084** features
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

## Reference comparison: transferable lessons and rendering order

[Isopolis](https://sf.isopolis.city/) and Isometric NYC are useful visual
references, but not source-pipeline templates. Isopolis documents a Google
Photorealistic 3D Tiles capture followed by curated AI image pairs, LoRA/infill
generation, manual repair, and water stamping; its report also records residual
seams. Isometric NYC documents a similar generated-quadrant workflow. Those
approaches can create a strong, unified illustration, but they make individual
pixels hard to trace, require subjective seam review, and conflict with this
project's saved-pixel, open-data, deterministic-rendering requirements. Google
tile policy also rules out retaining or analysing its 3-D tile imagery for this
offline artifact. Do not import their rasters, training workflow, or Google
capture path.

The transferable lessons are narrower and practical: a fixed citywide
isometric camera; one coherent palette and water treatment; clear road, rail,
vegetation, and landmark cues; static pyramids with nearest-neighbour deep zoom;
and annotations or tours stored as geographic data rather than painted into
imagery. IsoPhilly already has the fixed projection, prebuilt pyramid, and
data-backed overlays. The recommended route is to add deterministic vector and
procedural layers to the existing official-I3S/City-footprint/PASDA-aerial
stack, keeping every source snapshot, rule, and output reproducible.

### Philadelphia rowhouse extrapolation limits

The Philadelphia City Planning Commission's [Rowhouse
Manual](https://www.phila.gov/media/20190521124726/Philadelphia_Rowhouse_Manual.pdf)
describes rowhouses as a building type with many sizes, periods, and exterior
forms. The manual says brick is widespread, while stone often appears at the
base and around windows and doors. It also describes porch and bay window forms
in several parts of the city, but it warns that its neighborhood examples are
not exclusive style rules.

The [Strawberry Mansion neighborhood conservation
rules](https://codelibrary.amlegal.com/codes/philadelphia/latest/philadelphia_pa/0-0-0-290844)
provide a useful evidence standard even though they do not apply citywide. The
rules require new porches and bay windows to follow nearby buildings on the same
block face. IsoPhilly therefore does not add a porch or bay from height,
neighborhood name, or a random seed. The packed world has no observed block
face type, sidewalk setback, or porch dimensions.

The current citywide LiDAR artifact cannot supply the missing facade evidence.
It retains building and ground point counts, roof and ground height quantiles,
and roof spread. The merge rejects height evidence with roof spread above 3
metres or above 35 percent of its candidate height. That is a height-quality
test, not evidence of a roof form or facade. It does not retain the spatial
facade returns needed to detect a bay or porch, and the validated raw LAS files
were deleted after the bounded evidence pass. A future bay or porch rule needs
a reviewed block face source or a new evidence artifact built for this purpose.

The accepted conservative treatment uses the renderer's existing openings.
An exact named nonparty frontage selects painted openings for rowhouses,
rowhouse-like footprints, and twins. Their attached component shares a floor
rhythm while walls keep deterministic local material and glass variation. Only
exact high-confidence rowhouses can add a shallow entrance stoop, painted upper
window surrounds, and a painted stone base. Cornices remain limited to eligible
exposed edges on high-confidence rowhouses, whether or not the frontage is
known. Unknown frontages keep the earlier exposed-short-wall treatment. These
features are illustrative and do not claim an observed facade, entrance, or
historical style.

Priority order for that work:

1. Restore typed street, rail, bridge, and major-infrastructure linework above
   aerial ground. The current renderer intentionally removes street centerlines;
   this is the largest citywide legibility gap. Use City centerlines and, where
   needed, OpenStreetMap tags to derive scale-aware road widths, rails, bridge
   decks, curbs, medians, and restrained markings.
2. Apply one deterministic, material-aware palette/quantization pass across
   ground, aerial roofs, procedural walls, trees, water, and textured meshes.
   This should replace browser-only saturation/contrast styling, while retaining
   enough texture colors for the photographed Center City mesh to remain legible.
3. Upgrade hydrology from blue-tinted aerial pixels to a data-derived water
   surface: shallow/deep bands and a narrow quantized shoreline treatment from
   the existing hydrology and land-cover masks. It must remain static and
   world-anchored so adjacent tiles agree.
4. Add canopy masses beyond inventoried street-tree points. First use
   deterministic, building- and road-rejecting procedural clusters inside the
   reviewed canopy class; consider the audited canopy polygons only after their
   stated validity and height gates pass.
5. Extend the authorized LiDAR-derived geometry cautiously: terrain around
   rivers and slopes, then confidence-gated roof planes/ridges and landmark
   forms. Keep flat/height-only fallbacks in source gaps and do not represent
   this as photographed facade information.
6. Add sparse procedural street furniture and data-backed landmark/tour
   annotations. Reuse existing detailed mesh geometry where available; otherwise
   use clearly illustrative sprites or silhouettes rather than invented building
   reconstructions.

## Philadelphia source stack

The active source stack uses publicly available open data from the City,
OpenDataPhilly, PASDA, OpenStreetMap, and the other sources recorded below.
Preserve source attribution and capture dates in published artifacts.

| Priority / layer | Authoritative source and endpoint | Quality / recency | Approx. ingestion cost | Decision |
| --- | --- | --- | --- | --- |
| 1. City clip | [City Limits catalog](https://opendataphilly.org/datasets/city-limits/) and [FeatureServer](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/City_Limits/FeatureServer/0) | Official generalized standard boundary; catalog says updated 2012/as needed. | One small polygon; trivial. | Use as outer mask only; do not infer shoreline precision. |
| 1. Buildings | [Building Footprints catalog](https://opendataphilly.org/datasets/building-footprints/), [GeoJSON download](https://hub.arcgis.com/api/v3/datasets/ab9e89e1273f445bb265846c90b38a96_0/downloads/data?format=geojson&spatialRefId=4326&where=1%3D1), [FeatureServer](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/LI_BUILDING_FOOTPRINTS/FeatureServer/0) | Official planimetric outlines; City service describes early-2015 imagery plus continuous updates, catalog says weekly. It includes houses, commercial/industrial buildings, sheds, garages, etc. | 546,084 polygons; 476.4 MB GeoJSON captured on 2026-08-26; expect several hundred MB RAM in Python/GDAL. Batch/page or download snapshot once—never render straight from HTTP. | Core layer. Dissolve/clip/simplify only after retaining an immutable raw snapshot. |
| 1. Water | [Hydrology catalog](https://opendataphilly.org/datasets/hydrology/) and [FeatureServer layer 1](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/Hydrographic_Features_Poly/FeatureServer/1) | Official hydrology polygons. The retained 2026-08-27 snapshot is 6,151,395 bytes with SHA-256 `8e5b08218bb956e7ef8f266924a07966f570384ac1c303bd55c8ea68661361e8`. The current clean snapshot retains 69 rings. The retrieval date does not assert survey vintage. | Small vector layer. | Apply restrained color grading to matching aerial pixels. Never replace the aerial image with flat polygons. |
| 1. Parks | [PPR Properties FeatureServer layer 0](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/PPR_Properties/FeatureServer/0) | Official park properties. The retained 2026-08-27 snapshot is 2,265,530 bytes with SHA-256 `50764361fbd49473ffdc06cd1443ab733554244edb3cf329773bdb4832fae4c7`. The current clean snapshot retains 659 polygons. The retrieval date does not assert survey vintage. | Small vector layer. | Grade only aerial pixels that already look like vegetation. Never replace the aerial image with flat polygons. |
| 1. Street trees | [2025 Philadelphia Tree Inventory](https://opendataphilly.org/datasets/philadelphia-tree-inventory/), ArcGIS item `dc6826e1319c4b35a7b662bc6be68104_0` | Official 2025 PPR inventory. The pinned snapshot has 151,726 records, and 151,371 point geometries inside City Limits are retained. OpenDataPhilly identifies the City of Philadelphia License and no warranty. | The retained GeoJSON is 42,795,780 bytes with SHA-256 `cdec5a2141ef4c754ef714c76ca4a0203356dffb2bd14cde6d362e9353bd5a05`. | Render depth-tested tree proxies during prebuild. DBH informs a clamped visual size. The layer is an inventory, not complete vegetation coverage or measured crown and height geometry. |
| 1. Land cover | [PASDA dataset 1587, Philadelphia Land Cover Raster 2018](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhillyLULC/MapServer/2) | Seven official classes made from 2018 LiDAR and 2017 NAIP imagery. The source is a classification aid, not current photography. | The 521,373,667 byte archive has SHA-256 `555ab81428c239dd4d1a1f162fdd072f4ff1b0b2ab15a2e96a3f241e2823bb3f`. A pinned OSGeo image converts `landcover_2018_philadelphia` to a 3 metre local mask. | Grade canopy, grass or shrub, and water while preserving the 2025 aerial pixels. City hydrology has priority. |
| 1. Major roads | [Street Centerlines catalog](https://opendataphilly.org/datasets/street-centerlines/) | Citywide reference linework, not exact road surfaces. | Pinned local snapshot, with the City classes 1–3 retained and all local streets suppressed. | Draw translucent, scale-aware expressway, arterial, and connector cues below buildings. Preserve the aerial as the road surface. The snapshot has no rail geometry. |
| 1. Center City parts | [OpenStreetMap Simple 3D Buildings](https://wiki.openstreetmap.org/wiki/Simple_3D_Buildings) | The current snapshot provides 827 height-backed parts, including the Comcast Technology Center shaft and wings. | Small cached snapshot; live Overpass refresh is optional. | Use as fallback geometry only where no photographed mesh exists. Suppress the parent footprint when parts cover most of it. |
| 1. Center City scene | [Philadelphia Buildings I3S service](https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/Philadelphia_Buildings/SceneServer) | 367 official detailed chunks with roofs, facades, setbacks, landmarks, UV coordinates, and JPEG atlases. | About 38 MB of binary geometry and 146 MB of atlases in the current cache. | Render the textured triangles into four canonical z5 orientations. Fill gaps with City and OpenStreetMap geometry, but let only the 2015 mesh suppress those fallbacks. |
| 1. Legacy downtown scene | [PASDA 2008 and 2009 downtown KML archive](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2010/kml00.zip) | 2,689 highest-detail models with photographed roofs and facades. It extends farther east, west, and south than the 2015 scene. | The smaller download is about 886 MB. Existing checkouts can reuse the retained 2.4 GB outer archive. | Import only `r0`, suppress overlap under the 2015 scene, and record the real 2008 and 2009 date. |
| 1. Stadium scene | [PASDA 2008 stadium-area KML archive](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2008/Stadium%20Area%20Processed%20w%20LiDAR-KML.zip) | 814 highest-detail KML/COLLADA components with measured geometry and JPEG material textures. The 2008 source includes the since-demolished Spectrum. | 647 MB nested archive; output keeps 808 current components, 126,181 textured triangles, and about 84 MB of JPEGs. | Render through the same textured-mesh path as Center City; exclude the six Spectrum components and record the historical capture date. |
| 1. Aerial color/reference | [Aerial imagery catalog](https://opendataphilly.org/datasets/aerial-photography/); [2025 PASDA image service](https://imagery.pasda.psu.edu/arcgis/rest/services/pasda/PhiladelphiaImagery2025/MapServer) | 2025 three-inch orthophotography exposed through an export API. | Fixed 1,536 metre exports preserve the 0.75 metre working grid while reducing first-build requests. A bounded shared disk cache makes repeats local. | Use one deterministic pixel treatment for ground, real roof pixels, and local wall color in the canonical z8 scene. Geometry remains authoritative City vectors. |
| 2. Terrain/height | [2025 LiDAR full metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Lidar_2025.xml); [LAS directory](https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/) | Genuine citywide April 2025 classified LiDAR. LAS 1.4 point format 6 includes intensity but no RGB or NIR; one central sample measured about 62 returns/m². | 963 raw LAS files totaling 362.82 GiB. Full metadata lists access and use constraints as “None.” | The user explicitly authorized the opt-in 664-tile City-intersection evidence queue on 2026-08-30 after the bounded-pilot design review. Process resumably and discard validated raw tiles. It cannot provide photographic facades, and partial evidence never becomes canonical. |
| 3. Street facade reference | [KartaView photo API](https://kartaview.org/doc/photos), [license FAQ](https://kartaview.org/doc/faq) | Public crowdsourced photos expose position, heading, time, and image URLs under CC BY-SA 4.0. A Rittenhouse test found only three images within 500 m, from different years. | Coverage and camera pose are uneven. Correctly projecting a photo onto a visible wall also requires occlusion and attribution handling. | Useful future opt-in source, not a citywide default. Audit coverage before downloading, and never smear a nearby photo across an unmatched facade. |

### PPR 2015 tree canopy source audit

The City catalog describes the [PPR 2015 Tree Canopy
Outlines](https://opendataphilly.org/datasets/ppr-tree-canopy/) as crowns wider
than six feet. The City made the data from 2015 leaf off, three inch imagery,
and it derived heights from 2015 LiDAR. The catalog applies the City of
Philadelphia License. The public map labels `avg_height` in feet.

A metadata and aggregate query audit on 2026-08-31 found 193,418 polygons in
the [City CARTO table](https://cityofphiladelphia.carto.com/u/phl/tables/ppr_tree_canopy_outlines_2015/public).
Its WGS84 bounds are `-75.2802459, 39.8713408` to
`-74.9557098, 40.1378411`. The table has `objectid`, `polyid`, `fcode`,
`avg_height`, `shape_length`, and `shape_area`, plus CARTO geometry fields.
The main geometry uses EPSG:4326, and CARTO also stores a Web Mercator copy.
The schema does not declare units for `shape_length` or `shape_area`, so an
importer must not infer their units from the field names.
Every row has `fcode` 3000 and a nonnull height. Heights range from 4 to 563
feet, with a mean of 25.32 feet. The 50th, 90th, 95th, 99th, and 99.9th
percentiles are 22, 45, 54, 70, and 84 feet. Eighty two rows exceed 100 feet,
32 exceed 150 feet, nine exceed 200 feet, and four exceed 300 feet. Most of the
largest values lie in Center City. Their locations and impossible tree heights
are strong evidence of building or classification artifacts.

The [ArcGIS service with the same
name](https://services.arcgis.com/fLeGjb7u4uXqeF9q/ArcGIS/rest/services/PPR_Tree_Canopy_Outlines_2015/FeatureServer)
is not a usable mirror. Its service JSON lists no layers or tables, and its
advertised layer zero returns `Invalid URL`.
Its stale service extent also covers only about 3.4 by 2.6 kilometres. Use the
CARTO table as the only working official endpoint, and do not treat the ArcGIS
service as a backup.

Do not ingest the canopy outlines as measured tree geometry yet. A bounded
pilot must first check polygon validity, confirm that the published bounds cover
the intended City area, remove overlaps with current building footprints, and
set a documented height rejection rule. Any accepted snapshot must record the
endpoint, request time, byte count, SHA-256, row count, bounds, field schema,
height distribution, and City license. The active 2018 land cover raster is the
reviewed citywide vegetation mask because it has complete study area coverage
and manual review.

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

### PASDA coverage audit

The exhaustive, dated facade-source matrix and reopening criteria live in
[`PASDA_AUDIT.md`](PASDA_AUDIT.md). PASDA has useful photographed models and
geometry, but no current public citywide calibrated multi-angle facade source.
The active ingest already uses the strongest published Center City, legacy
downtown, and stadium textured models. Its clean snapshot photographs 11,909 of
545,672 buildings: 2.18 percent by building count and 6.12 percent by footprint
area.

The audit found one additional angled-pixel candidate: PA DEP's public
[2014 Schuylkill shoreline obliques](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/).
The collection has 191 JPEGs and 191 TIFFs with visibly useful river-facing
building sides, but publishes no camera positions, EOP, calibration, or
georeferencing. Pin and visually index the JPEG set, recover cameras with SfM,
and register them to 2025 LiDAR before considering the 10.51 GiB TIFF delivery.
The collection's unusual annual Penn State notification term is recorded in the
source audit. This experimental collection is not part of the deployed build.

The complete 191-frame Schuylkill JPEG audit rules out a shared-camera model:
EXIF contains 24 focal lengths and 43 exact focal/dimension/orientation groups,
with a major timestamp break after frame 92. Use per-image EXIF-seeded
`SIMPLE_RADIAL` intrinsics and CPU-bounded sequential matching. Do not project
pixels unless the recovered cameras pass the registration and reprojection
gates in `PASDA_AUDIT.md`.

The deterministic local preflight is `uv run --locked poe
oblique-sfm-plan`. It requires and re-hashes all 191 JPEGs and validates their
EXIF and dimensions without network access or any pycolmap import/execution. It
atomically links the pinned source manifest to an immutable plan, checksum, and
1,790-pair list split at 92/93 with frame 191 quarantined. This is planning
evidence only, never reconstruction or georegistration evidence, and none of
the source pixels or derived artifacts may be published. A changed or partial
plan is resolved by archiving the entire local `sfm/plan/` directory after
review, not by replacing individual files.

The decisive geometry find remains the
[April 2025 Philadelphia LiDAR metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Lidar_2025.xml)
and its [public LAS directory](https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/).
The short PASDA catalog abstract is stale and says 2022; the full metadata and
file inventory confirm a 2025 acquisition. The collection contains 963 LAS
files totaling 362.82 GiB. It is LAS 1.4 point format 6, with classified points
and intensity but no RGB or NIR. A sampled central tile contained about 62
returns/m². The full metadata lists both access and use constraints as “None.”

This point cloud can materially improve terrain, building heights, roof planes,
tree masses, and detailed landmark geometry such as the Philadelphia Museum of
Art steps. It cannot create photographic wall textures. The current public
PASDA holdings still do not provide a modern, citywide set of calibrated
multi-angle images or textured meshes for facades.

The initial decision gate proposed three repeatable areas: Center City towers,
the Museum of Art/steps/Waterworks, and Port Richmond rowhouses. That bounded
design remains useful for visual comparisons, but it is no longer an authority
limit. On 2026-08-30 the user explicitly authorized the full opt-in queue of 664
City-intersecting source tiles (289.51 GiB pinned). The queue remains resumable,
checksum-verified, raw-discarding, and separate from normal ingest.

The completed 2026-08-31 queue accounts for all 664 selected sources: 653
evidence tiles, three outside-City tiles, and eight exact but structurally
truncated PASDA objects recorded as terminal `rejected_source` results. The
canonical schema-3 merge contains 531,149 rows, records
`source_coverage_complete:false`, and reports 10,429 union-deduplicated
footprints intersecting rejected gaps. The subsequent ingest applied LiDAR
heights to 292,048 buildings; City heights remain the fallback elsewhere.
Use `uv run --locked poe lidar-status` and the canonical manifest for the exact
filenames, validation records, gap bounds, and provenance. `--allow-partial`
is reserved for unfinished local processing, not an upstream PASDA defect.

The remaining 2010 KML, 3DS, OpenFlight, DXF, SHP, ground-mesh, and texture-map
downloads are alternate formats or lower LODs of the already-ingested 2,689
downtown models. Their manifests add zero photographic bounds or facades. The
19,208-file raw nadir archive is 943.21 GiB and publishes no frame positions,
exterior orientation, or calibration. Do not download either group to look for
new wall pixels; reopen the audit only under the triggers in the decision
record.

#### EagleView/Pictometry access

The operational status as of 2026-08-30 is blocked on authorized access. The
project has no EagleView production credentials, and the available EagleView
path requires a sales contact. The user emailed the City/Pictometry contact to
request the Philadelphia data and publication rights, and is waiting for a
reply. Do not retry browser cookie extraction, reuse Embedded Explorer tokens,
or scrape the public viewer. Reopen the importer work only after the City or
EagleView supplies written access terms and either production API credentials
or an authorized bulk delivery.

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

PASDA exposes 19,208 RGB TIFF frames in a separate `philly_nadir` directory.
The ZIP inventory totals 943.21 GiB. A sampled 4,872 by 3,248 TIFF had no
GeoTIFF tags, world file, camera pose, or exterior orientation. It was near
vertical and showed little useful facade. A coarse feature match against the
official 2010 orthophoto produced no reliable registration.

The directory exposes filenames but no frame footprint catalog. The imagery
metadata says there are no use constraints, while related Pictometry material
has license restrictions. Do not ingest this archive. A future evaluation first
needs image footprints, camera orientation and calibration, and written
permission to publish derivatives.
The definitive inventory and criteria for reopening this conclusion are in
[`PASDA_AUDIT.md`](PASDA_AUDIT.md).

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
* Do not use Google's 3-D map products as a production geometry/texture source
  under the saved-pixel requirement. Live-only Maps JavaScript 3-D and direct
  Photorealistic 3D Tiles are technically viable at different prices, but their
  policies prohibit the local, independently reproducible artifact required
  here.
* Do not make a browser fetch 546k footprints or citywide LiDAR at runtime.
  Pre-render the pyramid; retain the original vectors privately for rebuilds.
