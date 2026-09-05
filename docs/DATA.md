# Data pipeline

`uv run --locked poe ingest` loads ten source datasets and writes two
clean artifacts:

- `data/clean/philly.bin` contains fallback buildings, building parts,
  accepted textured meshes, surface masks, typed major-transport lines, street-tree points, texture
  references, and the City boundary.
- `data/clean/meta.json` records source URLs, request times, HTTP validators,
  SHA-256 checksums, bounds, counts, and output checksums.

The texture JPEGs used by accepted meshes live in
`data/clean/mesh-textures/`. Generated files are ignored by Git. Keep the raw
snapshots and `meta.json` when pinning a release.

## Sources

The active pipeline has one source for each job:

1. City Limits defines the render boundary.
2. Building Footprints supplies citywide outlines and heights.
3. Hydrology polygons provide restrained water color grading masks.
4. Park polygons provide restrained vegetation color grading masks.
5. Street Centerlines supplies only three City-ranked through-road classes for
   restrained citywide transport linework.
6. OpenStreetMap building parts supply documented Center City setbacks and
   roof forms where photographed meshes are unavailable.
7. The 2015 I3S scene supplies the newest detailed Center City geometry and
   textures.
8. The 2008 and 2009 legacy downtown archive fills gaps outside that scene.
9. The 2008 stadium archive supplies detailed geometry and textures for the
   sports complex.
10. The Philadelphia Parks & Recreation 2025 tree inventory supplies a
   citywide point layer and trunk diameters.

The current retained hydrology snapshot comes from the official
[`Hydrographic_Features_Poly` FeatureServer layer 1](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/Hydrographic_Features_Poly/FeatureServer/1).
It was fetched on 2026-08-27. The GeoJSON is 6,151,395 bytes with SHA-256
`8e5b08218bb956e7ef8f266924a07966f570384ac1c303bd55c8ea68661361e8`,
and the current clean snapshot retains 69 water rings. The current park
snapshot comes from the official
[`PPR_Properties` FeatureServer layer 0](https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/PPR_Properties/FeatureServer/0).
It was fetched on 2026-08-27. The GeoJSON is 2,265,530 bytes with SHA-256
`50764361fbd49473ffdc06cd1443ab733554244edb3cf329773bdb4832fae4c7`,
and the current clean snapshot retains 659 park polygons. The retrieval date is
the snapshot date, not a claim about when either source was surveyed. Both
catalog entries use the City of Philadelphia License and provide no warranty.
Regenerate the counts from `data/clean/meta.json` when the clean snapshot
changes.

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

- Horizontal coordinates use EPSG:32129 as retained by the City pipeline
  (metres). Building and mesh heights remain metres. Do not reuse a
  height threshold as a horizontal distance.
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

The pinned Street Centerlines snapshot is `street-centerline-b9a1466fce07.geojson`
(SHA-256 `b9a1466fce07dd0463198995d6f4fd705c463aa483e58ce594ab9a07de22fd5f`).
Only City classes 1, 2, and 3 are retained as expressway, arterial, and connector
cues (416, 4,439, and 4,505 lines respectively from this snapshot). They are
world-anchored, translucent strokes drawn below shadows and
buildings; their role is navigation hierarchy, not to replace the aerial road
surface. Local classes stay in the aerial to avoid a dense wireframe. This
snapshot has no independently typed rail geometry, so rail remains visible only
through the aerial and PASDA road-or-rail land-cover class until a pinned public
rail centerline source is added.

## Land cover class mask

The active land cover mask uses PASDA dataset 1587, Philadelphia Land Cover Raster 2018. The
source is layer 2 of the `PhillyLULC` MapServer. Its seven classes are tree canopy, grass and shrub,
bare earth, water, building, road and railroad, and other paved surface. The source was made from
2018 LiDAR and 2017 NAIP imagery, so it is a classification aid rather than current photography.
The official archive is 521,373,667 bytes with SHA-256
`555ab81428c239dd4d1a1f162fdd072f4ff1b0b2ab15a2e96a3f241e2823bb3f`.
The audited File Geodatabase is `PPR_LandCover_2018.gdb`, and its exact raster
is `landcover_2018_philadelphia`.

The converter reads one exact File Geodatabase raster inside the audited PASDA ZIP. It builds the
GDAL `OpenFileGDB` and `/vsizip` name itself from the pinned geodatabase root and a simple raster
name. It rejects paths, connection strings, container results, subdatasets, and another GDAL
driver. It also checks the source description and file list against the reviewed evidence.

The reviewed converter uses GDAL 3.12.4, raster OpenFileGDB with virtual file
support, and PROJ 9.8.1. The repository pins the official OSGeo linux/amd64
small image as
`ghcr.io/osgeo/gdal@sha256:d834c2ffb3e7a2f3e35dae2a4cee35108b551db92b8349827f63ceda56979462`.
The scripts in `tools/land-cover/` run only `gdalinfo`, `gdalwarp`, and `proj`
from that image. The wrapper disables networking, sets a read-only container
root, uses the calling user, and limits temporary storage to 256 MB. Conversion
checks complete output digests for both GDAL version and build commands, the
GDAL driver list, both general help commands, and `proj --version`. The
official driver syntax and virtual ZIP behavior are
documented by [GDAL OpenFileGDB](https://gdal.org/en/stable/drivers/vector/openfilegdb.html) and
[GDAL virtual file systems](https://gdal.org/en/stable/user/virtual_file_systems.html).

The target grid is pinned to EPSG:32129 at 3 metre resolution. It has 9,098 columns and 10,174 rows,
so its class payload is 92,563,052 bytes. Rows run from north to south. GDAL uses nearest neighbor
resampling, one thread, and a 64 MB work limit. The converter rejects more than 100 million cells,
non square cells, rotation, another data type, and values above the seven official classes. Source
holes remain NoData class zero.

The converter writes `classes.npy`, `grid.json`, and `conversion.json` in a versioned generation
directory. The manifest records the source evidence, each member SHA-256, the NumPy SHA-256, and a
count for every class. A complete audit runs before one atomic `current.json` pointer change. A
failed or concurrent conversion leaves the prior generation active.

Fetch or verify the reviewed archive with the pinned command:

```text
uv run --locked poe land-cover-fetch
```

The command downloads only the official `ARCHIVE_URL` to the ignored
`data/raw/PhiladelphiaLandCoverRaster2018.zip` path. It requires the exact 521,373,667 byte length
and the pinned strong ETag. It uses 30 second connect, write, and pool timeouts and a 120 second read
timeout. The archive gets four deterministic attempts with 2, 4, and 8 second delays. There is no jitter.
Only transport failures, timeouts, HTTP 408, HTTP 429, and HTTP 500 to 599 responses are retried.

Each flushed chunk updates an atomic checkpoint. A resumed request sends `Range` and `If-Range`, and
the response must have status 206, the exact content range and length, and the same strong ETag. A
legacy partial without that validator is discarded. Redirects, weak or changed ETags, malformed
ranges, and other validation errors fail without a retry. The complete partial is hashed and then
renamed atomically. The command reports the SHA-256 and never changes the
checked-in audit pin.

Both a completed partial and an existing cached archive must match the audited
SHA-256 before reuse or publication. A mismatch leaves an existing destination unchanged. The
fetch rejects symbolic links and nonregular destination, partial, checkpoint, and lock files. It
opens cached archives, partial files, and checkpoints through file descriptors with the operating
system's no-follow flag. It checks the opened device and inode against the pathname again after each
read, and it repeats that check immediately before final rename. A pathname swap therefore fails
without publishing the replacement. Checkpoint and final file changes are followed by a directory
sync, so a reported checkpoint never depends only on an unflushed directory entry.

The fetch lock contains its process ID and host name. A killed fetch can leave the exact
`data/raw/PhiladelphiaLandCoverRaster2018.zip.download.lock` file behind. First, read that file.
Second, confirm that the named process is not running on the named host. Third, remove only that
exact lock file, and rerun the command. The fetch never guesses that a lock is stale and never
removes another process's lock.

The source candidate command below reproduces the reviewed archive, raster, and
tool evidence. Pass the pinned wrappers explicitly so no host GDAL executable
can satisfy the audit by accident.

```text
uv run --locked python -m isophilly_ingest.land_cover source-candidate \
  --source-archive data/raw/PhiladelphiaLandCoverRaster2018.zip \
  --gdb-root PPR_LandCover_2018.gdb \
  --raster-name landcover_2018_philadelphia \
  --gdalinfo tools/land-cover/gdalinfo \
  --gdalwarp tools/land-cover/gdalwarp \
  --proj tools/land-cover/proj
```

The command hashes the archive and each member. It also opens the exact raster and records the full
raster and tool evidence used by the active pins. It prints one canonical JSON object with
sorted keys and no optional formatting. Both names accept only ASCII letters, digits, periods,
underscores, and hyphens, and each name must start with a letter or digit. The geodatabase root must
end in `.gdb`.

The checked-in `RasterEvidence` and `ToolchainEvidence` values must match the
candidate output exactly. The same reviewed archive SHA-256 is pinned in Python
and Rust. A present artifact fails closed if either pin changes.

Convert the File Geodatabase raster with the reviewed pins. The raster name
must be the exact simple name below. The converter constructs the full GDAL
connection string.

```text
uv run --locked python -m isophilly_ingest.land_cover convert \
  --source-archive data/raw/PhiladelphiaLandCoverRaster2018.zip \
  --raster-name landcover_2018_philadelphia \
  --gdalinfo tools/land-cover/gdalinfo \
  --gdalwarp tools/land-cover/gdalwarp \
  --proj tools/land-cover/proj

uv run --locked python -m isophilly_ingest.land_cover build \
  --conversion data/land-cover-2018/converted/generations/REVIEWED_GENERATION \
  --source-archive data/raw/PhiladelphiaLandCoverRaster2018.zip
```

Both conversion and final mask writing take an exclusive lock before they hash or inspect large
inputs. A concurrent process fails without changing the current output. A normal failure removes
its lock and its unique temporary files, and a complete generation becomes active through one
atomic pointer update. A killed process can leave `.convert.lock` or a hidden mask lock behind.
Remove a stale lock only after checking that no converter or mask writer process is running. The
next run then cleans its own temporary files and keeps the prior published output until the new
output passes a complete read and digest check.

The converter parses the ENVI header and requires the exact samples, lines, band count, zero header
offset, unsigned byte data type, band sequential interleave, and little endian byte order. The
stored rows run from north to south. A post-write audit reloads the NumPy grid and the final mask,
checks their hashes and class counts, and only then publishes them.

Run `uv run --locked poe land-cover-audit` to check the completed artifact without a network
request. The artifact contains a 16 byte prefix, a JSON header, and one byte per target cell. The
sampler uses nearest neighbor lookup and clamps exact outer bounds with the next representable
floating point value.

The verified 2026-08-31 conversion generation is
`65148c297d05f70e26246f28312cfbcf24bec9af528cfb41bad3fb7bfeb70918`. Its
92,563,052 cells contain 51,486,312 unknown, 7,820,368 tree canopy, 9,023,799
grass or shrub, 789,446 bare earth, 2,388,870 water, 7,482,856 building,
4,973,515 road or railroad, and 8,597,886 other paved cells. The resulting
mask SHA-256 is
`217fdf2e5aeed51b7bbee3f798b1f136b7c016843385bd3af3202ecc22b35643`.
The complete v48 local rebuild produced 9,214 citywide z8 tiles, every parent
level, and all 1,024 z5 tiles plus parents for each of the four Center City
orientations. A second `poe prebuild` confirmed the pyramid was complete.

The Rust prebuilder reads the optional mask with the same strict schema, source,
grid, payload, and digest checks. A present invalid mask fails closed. Its whole
artifact SHA-256 is part of the `v50-citywide-polish` scene and tile identity, so
adding or changing the mask cannot reuse an earlier tile pyramid. Official City
hydrology takes priority over every raster class. The raster water class uses
the same stable water treatment outside those polygons. Park treatment applies
only to tree canopy and grass or shrub classes. Elsewhere, canopy and grass use
separate restrained grading, while building, road, railroad, paved, and bare
earth pixels keep their aerial color. No mask data is sent to the browser.

The City reserves rights in the dataset and provides it without a warranty. Confirm the current
City and PASDA terms before publishing source pixels or raster tiles derived from them. Preserve the
University of Vermont Spatial Analysis Laboratory and Philadelphia 2018 Tree Canopy Assessment
credit in local provenance and any approved release.

The tree input is the official
[2025 Philadelphia Tree Inventory](https://opendataphilly.org/datasets/philadelphia-tree-inventory/),
ArcGIS item `dc6826e1319c4b35a7b662bc6be68104_0`. The retained GeoJSON is
42,795,780 bytes with SHA-256
`cdec5a2141ef4c754ef714c76ca4a0203356dffb2bd14cde6d362e9353bd5a05`.
It contains 151,726 records; 151,371 point geometries fall within the official
City Limits geometry and are packed. The importer requires the exact fields
`objectid`, `tree_name`, `tree_dbh`, `year`, `loc_y`, `loc_x`, and `geometry`,
unique object IDs, and only year 2025. A changed file, schema, year, or record
count fails closed and requires a reviewed pin update. `loc_x` and `loc_y`
must be finite and agree with the GeoJSON point after projection to within one
metre; the audited source has a consistent approximately 0.91 metre transform
offset and no record exceeds that tolerance.

Only projected x/y, DBH-derived diameter, and one conservative visual-form byte
are retained, in stable object-ID order. The v11 records contribute 1,967,823
bytes: 13 bytes for each of 151,371 records. The ordered retained-record payload
is separately pinned as SHA-256
`846992de1b2289410a714fea86c3e81ce96fd643e4ffda7ba426da1c53333868`.
Both its exact record count and digest are enforced, binding the output to the
reviewed tree inventory and City Limits snapshot even though City Limits is
otherwise a refreshable source. A newer wrong cached tree file cannot shadow the
older pinned snapshot; cache selection and download fallback require the full
expected SHA-256.

The form is not a species assertion. It is `Default` (round fallback) unless a
strict normalized `SCIENTIFIC - COMMON` value provides an exact safe rule.
`SHRUB` records become `Shrub`; exact common-name words `COLUMNAR`,
`FASTIGIATE`, `UPRIGHT`, `NARROW`, or `PYRAMIDAL` become `Columnar`; exact
`WEEPING` or `PENDULA` words become `Weeping`; and only the reviewed conifer
genera become `Conifer`. The precedence is Shrub, Columnar, Weeping, Conifer,
then Default. Nulls, unknowns, palms, malformed delimiters including en dashes,
and other unsupported names remain Default. The pinned output contains 146,245
Default, 3,989 Conifer, 251 Columnar, 644 Weeping, and 242 Shrub records.

The renderer builds spatial indexes once during prebuild, queries
only points intersecting a tile, skips subpixel crowns, and writes trees into
the shared depth buffer. No citywide tree array reaches the browser or request
path. DBH is a trunk measurement, not tree height or crown width; the displayed
height and crown are deliberately clamped visual proxies and must not be read
as measurements. A narrow view-facing trunk reaches the ground. Each crown is
a projected sphere rather than a flat disk: every output pixel solves the same
sphere projection used for its depth, so building edges and overlapping crowns
occlude consistently in all four orientations and across tile seams.

OpenDataPhilly identifies the dataset license as the City of Philadelphia
License and publishes it without a warranty of accuracy. Retain the source
attribution and City terms link in release metadata and the viewer. The annual
snapshot may change, so a future year must be reviewed as a new immutable
source rather than silently replacing the 2025 release input.

## World format

All values are little-endian. Coordinates are EPSG:32129 metres.

```text
8 bytes  magic "GEOPHILY"
u32      version (11)
u32      EPSG (32129)
u32      building count
u32      building part count
u32      building mesh count
u32      city ring count
u32      water ring count
u32      park ring count
u32      street-tree count
u32      transport-line count
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
repeat street-tree count times:
  f32    x
  f32    y
  f32    trunk diameter in metres
  u8     validated tree form: Default=0, Conifer=1, Columnar=2, Weeping=3, Shrub=4
repeat transport-line count times:
  u8     transport kind
  u32    point count
  f32 x, f32 y repeated point count times

ring:
  u32    point count
  repeat point count times: f32 x, f32 y
```

The reader keeps v9 and v10 compatibility: their trees have no form byte and
load as the Default round fallback. v9 also has no transport count. v11 rejects
reserved tree-form byte values rather than guessing their meaning.

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
fallback buildings, accepted textured mesh faces, then depth-tested street
trees. In a Center City view,
the fallback pass uses City footprints and OpenStreetMap parts wherever the
2015 mesh does not cover a building. Roofs use aerial samples. Walls use a
procedural pattern with colors derived from nearby aerial pixels, so they are
not photographed facades. Texture sampling uses a stable world grid so
adjacent output tiles do not request the same image on different pixel phases.

The four z5 Center City pyramids add 4,096 files compared with z4. The current
static export contains 18,009 files and uses 1,245.0 MiB, which remains below
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

The 2026-08-30 PASDA photographed-facade source decision record is
[`PASDA_AUDIT.md`](PASDA_AUDIT.md). The current clean snapshot contains 3,498
accepted textured mesh components and identifies photographed coverage for
11,909 of 545,672 buildings: 2.18% by building count and 6.12% by footprint
area. Recalculate those figures from `data/clean/meta.json` after any ingest.

PA DEP's newly identified
[2014 Schuylkill shoreline obliques](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/)
are the only un-ingested PASDA source likely to add real photographed walls.
They contain 191 JPEGs (252.5 MiB) and 191 TIFFs (10.51 GiB), but no published
camera pose, EOP, calibration, or georeferencing. They are a JPEG-first SfM and
LiDAR-registration candidate, not an active source. Do not fetch the TIFF set
until registration and contact-sheet review succeed. The metadata's annual
Penn State notification term is recorded in the source audit.

The local-only JPEG pilot is reproducible and separate from normal ingest:

```sh
uv run --locked poe oblique-plan       # pin official names, URLs, sizes, listing hash
uv run --locked poe oblique-next       # resume and verify exactly one JPEG
uv run --locked poe oblique-status     # re-hash and audit completed frames
uv run --locked poe oblique-review     # metadata and labeled/hash-pinned contact sheet
uv run --locked poe oblique-sfm        # guarded contiguous-sequence SfM handoff
uv run --locked poe oblique-sfm-plan   # immutable, offline all-191 execution plan
```

The review command has a fixed memory bound for source image decoding. It
decodes and resizes exactly one full size JPEG at a time, then runs montage on
the 320 by 240 thumbnails. Each cached thumbnail records the source hash,
ImageMagick version, transform settings, output hash, and exact command. Labels
use an explicitly resolved Noto Sans file with an audited SHA-256; the final
sidecar records both its path and hash. A
source or tool change causes a fresh thumbnail, and a successful run removes
stale cache files. An interrupted run can reuse every thumbnail that still
passes those checks.

The same module supports `delaware-2014` and `little-tinicum-2014` through
`--collection`. It never downloads TIFFs or DNGs, never invents camera poses or
georeferencing, and does not feed the renderer. See `PASDA_AUDIT.md` for the
exact source directories, the nested Little Tinicum directory trap,
registration acceptance criteria, and source metadata terms.

Acquisition requires a complete JPEG EOI marker and full ImageMagick decode.
Exact complete `.part` or final files survive lost progress and are recovered
after revalidation. Inventory refresh refuses a changed listing whenever cached
pixels or review artifacts remain, and a clean plan also rejects any listing
outside the three audited SHA-256 pins. `--refresh` does not override that gate;
acceptance requires a reviewed code and documentation update. The contact-sheet
sidecar records the exact ordered inputs, hashes, executable, ImageMagick
version, pinned font path and hash, and command. It is reproducible on the same
toolchain, not promised to be byte-identical across toolchain versions. SfM handoff requires at least 20
contiguous frames by default; the manifest records whether the collection is
partial and an explicit `--allow-incomplete` diagnostic never implies
registration success. The handoff also records a per-image camera policy: the
Schuylkill flight used a variable zoom (24 EXIF focal lengths and 43 exact
focal/dimension/orientation groups), so shared-camera self-calibration is
prohibited. Reconstruction must seed one `SIMPLE_RADIAL` camera per image,
respect the frame 92/93 temporal break, and quarantine frame 191's aspect-ratio
outlier for the first diagnostic.

`oblique-sfm-plan` is stricter than the diagnostic handoff. It requires the
complete pinned 191-frame Schuylkill set and locally re-hashes every JPEG while
reading its EXIF, dimensions, and capture time. It performs no network access,
does not import or execute pycolmap, and does not reconstruct anything. Its
atomic artifact set is
`data/coastal-obliques/schuylkill-2014/sfm/plan/{plan.json,pairs.txt,plan.sha256}`.
The plan links the audited listing and ordered image manifest to 1,790 explicit
sanitized-name pairs, split into frames 1 through 92 and 93 through 190; frame
191 is recorded but quarantined. It also records the pinned backend version,
CPU and memory bounds, and promotion and georegistration gates as policy rather
than execution evidence. Publication is forbidden. If any member is missing or
differs, the command changes none of the published plan set. Archive the entire
`sfm/plan/` directory after reviewing the drift, then rerun; never replace one
member in place.

PASDA's [full April 2025 LiDAR metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Lidar_2025.xml)
and [LAS directory](https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/)
confirm the citywide classified point cloud used by the opt-in queue below. The
short PASDA catalog abstract incorrectly describes it as 2022. The actual
collection is 963 LAS files totaling 362.82 GiB, in LAS 1.4 point format 6.
It records intensity but no RGB or NIR; a sampled central tile measured about
62 returns/m². The full metadata lists access and use constraints as “None.”

This is a candidate geometry source for terrain, roofs, heights, trees, and
landmark detail such as the Philadelphia Museum of Art steps. It is not a
photographic facade source. No current public PASDA collection supplies
citywide calibrated multi-angle facade imagery. The original bounded review
areas were Center City, the Museum of Art/Waterworks, and Port Richmond. On
2026-08-30 the user explicitly authorized processing the full 664-tile
City-intersection evidence queue. That authorization does not make LiDAR part of
normal ingest, does not authorize unrelated PASDA archives, and cannot create
photographed facades.

### Resumable 2025 LiDAR ingest

The repository includes an opt-in citywide LiDAR queue. It never makes the
362.82 GiB archive a prerequisite for normal ingest and does not retain every
raw tile:

```sh
uv run --locked poe lidar-plan
uv run --locked poe lidar-next
uv run --locked python -m isophilly_ingest.lidar run --all --discard-raw
uv run --locked poe lidar-recheck-rejected  # optional upstream repair check
uv run --locked poe lidar-status
uv run --locked poe lidar-merge
uv run --locked poe ingest
uv run --locked poe prebuild
uv run --locked poe visual
```

Read the current local queue state from `poe lidar-status`. Do not copy a
mutable progress count into release notes. No build is LiDAR enabled until all
664 selected sources are accounted for and `poe lidar-merge` publishes the
canonical schema-3 Parquet and JSON pair. After the merge, verify the canonical
manifest, its ordered rejected-source and gap records, and the applied-building
count from the next ingest. Archive the pinned inventory, canonical pair,
`data/clean/meta.json`, tile manifest, and visual report with the release. A
diagnostic merge made with `--allow-partial` is never a release input.

The 2026-08-31 completed run accounts for all 664 selected sources: 653
evidence tiles, three outside-City tiles, and eight terminally rejected source
tiles. The canonical schema-3 manifest is not partial, contains 531,149
building-evidence rows, and binds the 17,079,778-byte Parquet file with SHA-256
`ef513ef75a6f41e1c66654bdedc6fd5bf8183f0dd64debadccf6ec9cbdef55b3`.
The eight rejected gaps intersect 10,429 unique footprints; this is a
union-deduplicated intersection count, not a claim that every footprint lacks
valid evidence from an adjacent tile. The subsequent schema-9 ingest applied
trustworthy LiDAR heights to 292,048 of 545,672 packed buildings and retained
City fallback heights elsewhere. The exact rejected filenames, bounds, source
bytes, header requirements, URLs, and payload hashes live in
`data/lidar-2025/building-evidence.json` and the progress ledger.

`lidar-plan` pins the official PASDA directory response, file sizes, City
Limits checksum, Building Footprints checksum, selected tile names, URLs, and
conservative filename-derived bounds in `data/lidar-2025/inventory.json`. It
also creates a content-linked
GeoParquet footprint index in official NAD83(2011) Pennsylvania South US survey
feet (EPSG:6565). Re-running the command uses that pin. `lidar-plan --refresh`
can re-fetch only when the result still matches the checked-in semantic audit
pin. Use `python -m isophilly_ingest.lidar audit-candidate` to write a
non-active candidate and print the proposed semantic digest. Accepting changed
source or selection authority requires a reviewed constants-and-docs update.

`lidar-next` is the smoke-test path. It resumes one `.las.part`, verifies the
pinned byte count, computes SHA-256, validates the LAS header and true bounds,
and emits one atomic Zstandard Parquet file plus a provenance JSON file. It
uses building-class points inside each footprint and nearby ground-class
points to record robust roof and ground quantiles. Raw LAS is deleted only
after the derived Parquet checksum has been written and reverified, or after an
exact pinned payload is structurally rejected and its terminal provenance has
been atomically written and revalidated. The progress manifest is atomic, so
the same commands resume after interruption. PASDA requests use a 30 second
connect timeout, a 120 second read timeout, and 30 second write and connection
pool timeouts. Each tile gets at most four attempts. Retries wait 2, 4, and 8
seconds, with no random delay. The job saves a response ETag with each checkpoint
only when the server supplies a strong ETag. A retry can resume at the size of
the saved partial file only when that strong ETag is present. The request sends
the ETag in `If-Range`, and the response must return the same strong ETag with an
exact matching `Content-Range` and length before the job appends data. A weak
ETag, a missing ETag, or a legacy partial without an ETag causes a restart from
zero. A full HTTP 200 response also rewrites the partial from zero. Each 4 MiB
chunk is flushed to disk before the progress manifest records it. A killed
process can leave a partial file ahead of the last manifest write. The job uses
that actual file size only when the active inventory, tile, and stored strong
ETag still match. The completed file still has to pass the pinned size, SHA-256,
and LAS structure checks before it can replace the partial file.
Each LAS point stream is decoded exactly once. Matching roof and buffered-ground
Z integers are appended to per-building files in a tile-scoped `.lidar-work`
directory through a 64-handle LRU. Quantiles then read one building at a time,
so RAM is bounded by the point chunk plus the largest single-building sample,
without a match-count rejection or approximate statistics. The Parquet schema
metadata records the one-pass invariant, exact spill bytes, and little-endian
int32 encoding. Temporary disk use is four bytes per matched building/sample
association. A stale tile work directory is removed before restart, and the
tile work directory is removed on success or failure.
`lidar-status` audits every result against the active inventory, sibling
provenance JSON, output size, SHA-256, and Parquet source fields. A progress
flag without a valid artifact remains pending. Resumed HTTP responses must
also return a matching `Content-Range` start and total, along with the stored
strong ETag, before bytes are appended.

The sequential `--all --discard-raw` path has peak raw storage near the largest
individual PASDA tile rather than the complete archive. The current directory's
largest files are about 1.2 GiB; allow several additional GiB for a partial
download, the footprint index, per-tile evidence, and filesystem overhead.
Network transfer is still the sum of every City-intersecting source tile and
the exact selected count and bytes are printed by `lidar-plan`.
Timeouts, connection errors, HTTP 408 and 429 responses, and HTTP 500 through
599 responses use the bounded retry policy. Invalid redirects, byte ranges,
lengths, hashes, and LAS structures do not retry. When all attempts fail, the
queue records the saved byte count, leaves the tile pending, and continues with
the remaining tiles. A later run resumes the pending tile when its checkpoint
has a strong ETag. Otherwise, the later run starts that tile from zero.
The 2026-08-30 audit selected 664 of 963 files, totaling 289.51 GiB, and
created a 102.1 MiB footprint index. The queue starts with the smallest file,
so `lidar-next` is a bounded smoke test rather than an arbitrary 1.2 GiB pull.

PASDA serves some selected objects at their exact pinned directory size even
though their LAS headers declare a larger point-record payload. The downloader
requires exact pinned HTTP lengths/ranges and hashes, then the LAS structural
minimum. An exact but structurally truncated object is terminal, excluded from
retry and evidence, and retained through its URL, sizes, hash, parsed header,
expected minimum, and error metadata. Record the final exact rejection list and
coverage impact only after the full queue and canonical merge finish.

Run `uv run --locked poe lidar-recheck-rejected` to test whether PASDA repaired
a terminal source without changing its directory entry. The old rejection stays
authoritative while the replacement downloads separately. Only an exact-size,
exact-URL response with a structurally complete LAS payload replaces it and
resumes derivation. An unchanged truncation is discarded. Recheck downloads use
the same timeouts, byte range validation, retry limit, and saved partial files.
A recheck that exhausts transient retries keeps the old rejection and continues
with the next rejected tile.

`lidar-merge` requires every selected tile to be accounted for by a valid
derived, outside-City, or validated terminal `rejected_source` result. A
deliberate unfinished-local-state diagnostic can use `python -m
isophilly_ingest.lidar merge --allow-partial`; its manifest records that it is
partial. It writes `data/lidar-2025/building-evidence.partial.parquet` and its
sibling JSON instead of the canonical artifact. Partial smoke merges are
diagnostic only, always record `source_coverage_complete:false`, and normal
`poe ingest` never discovers them. Once all 664
sources are accounted for, a canonical merge with upstream rejections is still
locally `partial:false`, but records `source_coverage_complete:false`, the
deterministically ordered rejected-source list, exact source failure provenance,
gap bounds, per-gap intersecting-footprint counts, and a union-deduplicated
intersecting-footprint total. “Intersecting” does not claim that every footprint
lacks evidence from an adjacent valid tile. Normal ingest may consume that
canonical evidence and retains City fallback heights in rejected gaps. Status
prints the rejected tile names as well as their count. Canonical merge remains
blocked by locally pending, missing, or invalid artifacts. Merge
verifies every sibling manifest, source SHA-256 shape, active
inventory membership, source URL and byte count, output checksum, and
footprint provenance. For a boundary-spanning building it prefers an
observation that passes the height acceptance gate, then high-quality
classification and ground support, before raw roof-point count. Normal `poe
ingest` uses only evidence with at least 100 classified building returns, 20
nearby ground returns, no more than 3 metres of roof spread, and roof spread no
greater than the conservative height-relative complexity limit. It rejects
evidence made from a different footprint snapshot or active inventory.
When the artifact is absent, ingest behaves exactly as before. When present,
the evidence refines fallback footprint heights; photographed meshes and
explicit building parts retain their existing render priority. This pipeline
does not invent or provide facade photography.

The canonical Parquet and schema-3 JSON manifest are both staged and checked
before publication. Schema-2 manifests predate the complete rejection and gap
provenance and must be regenerated with `poe lidar-merge`. Rejected-gap
inspection happens before either active destination is replaced, so a
footprint-index or gap-audit failure preserves the prior pair. Publication uses
a transaction marker and renames the prior pair to backups first. A failed
second rename restores both prior files; a later run resolves a crash marker by
either accepting a fully validated new pair or restoring the backups. The JSON
is published last, and readers also verify its recorded Parquet size and
SHA-256. Recovery runs for both canonical and diagnostic-partial destinations
before a merge scans tile state, so even an otherwise-early merge error repairs
an interrupted publication. Ingest checks for a marker before testing whether
the canonical Parquet exists, and the evidence loader repeats that check. A
reader never repairs writer state; it fails closed with instructions to run
`poe lidar-merge`.

### Terrain relief

The LiDAR merge now also feeds `data/clean/terrain-v1.isoterrain`. The file is a
256 metre EPSG:32129 grid, and the renderer uses it only as a deterministic
tonal hillshade on the ground pass. It does not move geometry. The lighting
calculation uses three times the measured slope and clamps the ground tone to
92 through 108 percent, so gentle hills remain visible without replacing the
aerial color.

The current artifact reports a 108 by 121 grid. Its cells break down into 4,364
direct cells, 3,172 interpolated cells, 133 rejected-gap cells, and 5,399
unsupported cells. The eight rejected PASDA gaps remain neutral in the renderer.

Direct cells come from at least three accepted ground observations. Interpolated
cells come from nearby direct cells and then a local median pass. Rejected-gap
cells are the cells that fall inside the eight rejected PASDA gaps, so they stay
neutral and cannot introduce a seam. Unsupported cells also stay neutral.

To reproduce the artifact and the cached tile identity, run these commands in
order:

```sh
uv run --locked poe lidar-merge
uv run --locked poe ingest
uv run --locked poe terrain-audit
uv run --locked poe prebuild
```

`poe ingest` writes the terrain artifact when the merged evidence is present.
`poe terrain-audit` prints the artifact hash and the current counts. `poe
prebuild` then folds the terrain digest into the scene and tile cache identity,
so a changed terrain artifact cannot reuse an older cache.

The remaining 2010 KML, 3DS, OpenFlight, DXF, SHP, ground-mesh, and texture-map
archives are duplicate formats or lower LODs of the already-ingested 2,689
downtown models. Their audited manifests add no photographed bounds or facades.
The 19,208-file raw nadir archive totals 943.21 GiB and has no published
positions, camera orientation, or calibration. Neither group is an ingest
candidate; see `PASDA_AUDIT.md` before reconsidering it.

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

The repository's MIT license covers project code. The rendered map uses public
open-data inputs from the City, OpenDataPhilly, PASDA, OpenStreetMap, and the
other sources recorded here. Preserve their attribution and provenance in a
published build.

The I3S child cache does not yet have a complete pinned resource manifest. A
build is not fully reproducible unless every raw snapshot and I3S child resource
used by that build is retained.
