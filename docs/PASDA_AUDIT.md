# PASDA Philadelphia imagery and 3-D audit

Audit date: 2026-08-30

This record answers one narrow question: what does PASDA actually publish that
can improve photographed building sides in IsoPhilly? The audit inspected the
official catalog metadata, directory listings, ArcGIS services, and ZIP central
directories. It used small range requests and sample images; it did not bulk
download the candidate collections.

The record is exhaustive only for photographed facade sources. It is not a
catalog of every PASDA terrain, elevation, contour, hillshade, or orthophoto
product. The current orthophoto and LiDAR geometry decisions are in
[`DATA.md`](DATA.md) and [`RESEARCH.md`](RESEARCH.md).

## Decision

PASDA does not publish a modern, calibrated, citywide oblique-image collection
or citywide photogrammetric mesh for Philadelphia. The active ingest already
uses every public PASDA textured model that adds substantial facade coverage:
the 2015 Center City I3S scene, the highest-detail 2008/09 downtown KML models,
and the 2008 stadium models.

One previously missed source is actionable: PA DEP publishes unreferenced
coastal oblique photographs, including a 191-frame 2014 flight along the
Schuylkill. Those pixels can improve river-facing areas such as Manayunk,
Waterworks, and the Museum of Art vicinity if a bounded photogrammetry pilot can
recover camera geometry and register it to LiDAR. They do not solve citywide
facades.

The current clean snapshot contains 545,672 City buildings, 827 OSM building
parts, and 3,498 accepted textured mesh components: 367 from the 2015 scene,
2,323 from the legacy downtown source after overlap filtering, and 808 from the
stadium source. It identifies 11,909 buildings with photographed coverage,
which is 2.18% of buildings and 4,324,180 of 70,607,523 square metres, or 6.12%
of building-footprint area. These figures come from `data/clean/meta.json` and
must be regenerated rather than copied if the clean snapshot changes.

## Source matrix

| Rank | Official source | Published pixels or geometry | Coverage and decision |
| --- | --- | --- | --- |
| 1, active | [2015 Philadelphia Buildings SceneServer](https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/Philadelphia_Buildings/SceneServer) and [metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Building_3DModels.xml) | Textured I3S multipatches. The source extent is approximately -75.18465 to -75.14692 longitude and 39.94779 to 39.96987 latitude. | Best public facade source, but Center City only. Active ingest accepts 367 components. The separate 2015 globe and scene geodatabases are alternate packages of this scene. |
| 2, active | [2008/09 downtown `kml00.zip`](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2010/kml00.zip) | 2,689 highest-detail `r0` textured buildings in an approximately 886 MB repackaging. | Roughly Schuylkill to Delaware and South Street to Fairmount. Active ingest accepts 2,323 after newer-scene overlap filtering. |
| 3, candidate | [2014 Schuylkill obliques](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/) | 191 downloadable JPEGs and 191 full-resolution TIFFs with visibly useful angled building pixels. | A continuous shoreline strip, not citywide coverage. Run the registration pilot below before adding it to ingest. |
| 4, active | [2008 stadium model](https://www.pasda.psu.edu/download/philacity/data/3D_Models/2008/Stadium%20Area%20Processed%20w%20LiDAR-KML.zip) | Highest-detail textured KML model hierarchy in a 647 MB outer archive. | Active ingest retains 808 of 814 components after excluding six components for the demolished Spectrum. |
| 5, candidate | [PA DEP Delaware Estuary obliques](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/) and [collection metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=DEP_HistoricalCoastalZoneImagery.xml) | Downloadable angled JPEG and TIFF frames from several years. | Potentially useful along the Delaware waterfront and port. The photographs are unreferenced and do not cover inland Philadelphia. |
| 6, geometry only | [2025 LiDAR metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=Philadelphia_Lidar_2025.xml) and [LAS directory](https://www.pasda.psu.edu/download/phillyLiDAR/2025/LAS/) | 963 classified LAS 1.4 point-format 6 files totaling 362.82 GiB; intensity, but no RGB or NIR. | Citywide roof, terrain, tree, height, and landmark geometry. It cannot supply photographic walls. |
| Rejected for facades | [2010 raw nadir frames](https://www.pasda.psu.edu/download/philly_nadir/) | 19,208 ZIP files totaling 1,012,760,810,768 bytes, or 943.21 GiB. A sampled ZIP contained one TIFF and no sidecar. | Nadir pixels with no published frame coordinates, exterior orientation, or camera calibration. Do not ingest for walls. |
| Ground and roofs only | [Philadelphia imagery archive](https://www.pasda.psu.edu/download/philacity/data/) | Orthophotography for 1996 through 2025; current years are citywide tiled nadir mosaics. | Useful for ground, roofs, temporal repair, and audit. Orthorectification removes rather than preserves a controlled facade view. |

The 2015 orthomosaic metadata mentions that Pictometry captured an eight-way
Center City shoot, and the impervious-surface metadata says oblique imagery was
used during production. PASDA does not publish those raw oblique frames,
interior or exterior orientation, or camera calibration. Their public result is
the bounded 2015 textured scene already in ingest.

The Pictometry access path is also blocked as of 2026-08-30. The project has no
EagleView production credentials, and EagleView requires a sales contact. The
user emailed the City/Pictometry contact to request Philadelphia data and
publication rights, and is waiting for a reply. Do not retry browser cookie
extraction, reuse Embedded Explorer tokens, or scrape the public viewer. Reopen
this path only after the City or EagleView supplies written terms and authorized
API credentials or a bulk delivery.

## Active 2025 LiDAR pin

The ignored live inventory at `data/lidar-2025/inventory.json` is not available
in a fresh clone, so the active 2026-08-30 pin is recorded here:

| Item | SHA-256 |
| --- | --- |
| PASDA LAS directory response | `cbc710dacbf13902a168c6af262734e8a07d79565d15463faf4edf4d7a5f31b5` |
| City Limits snapshot | `b12d1e6e62ce72b5c409792e2535a3b90c6bcfa2d2d6c28455cd750f7db8c942` |
| Building Footprints snapshot | `9e1a96e6287d1253a0f4d92d6f8fb83931776a0c8c43df4525b46a3b1ceef352` |
| Complete ignored inventory JSON | `c4d1857986fc25cc820b9da42b0358795a2ac38e74e8a975d841aa08409e86c1` |
| Stable semantic inventory | `0a04f12d90a4393c09152d2655947456c7b531b5c67c62b51c4b92bf5d9cec96` |

The pin lists 963 files and selects 664 files that intersect the City. Verify
the ignored copy and its two retained source snapshots with:

```sh
sha256sum data/lidar-2025/inventory.json \
  data/raw/city-limits-b12d1e6e62ce.geojson \
  data/raw/building-footprints-9e1a96e6287d.geojson
python -m json.tool data/lidar-2025/inventory.json | sed -n '1,24p'
```

`uv run --locked poe lidar-plan` reuses the active pin. Normal planning and
downloading fail closed unless the exact official HTTPS directory, every tile
URL and basename, lowercase source hashes, deterministic tile order, summaries,
and the full selected inventory match checked-in constants. The semantic hash
covers all authority fields and ordered tiles but deliberately excludes
`fetched_at`, whitespace, and redundant summaries.

To investigate a changed listing, run `python -m isophilly_ingest.lidar
audit-candidate`. It writes a non-active ignored candidate and prints its
semantic hash. Audit the official listing, City snapshot, footprint snapshot,
counts, sizes, bounds, and selection before changing the constants and this
table in one reviewed commit. `plan --refresh` can only re-fetch an inventory
that still matches the existing audit pin; it cannot accept drift.

## Coastal oblique inventory

The [PA DEP collection metadata](https://www.pasda.psu.edu/uci/FullMetadataDisplay.aspx?file=DEP_HistoricalCoastalZoneImagery.xml)
describes oblique and vertical coastal photography acquired from 1984 through
2017. The current Delaware Estuary directory exposes obliques for 2008, 2011,
2012, and 2014. Counts below exclude `Thumbs.db` and packaging ZIPs.

| Flight | JPEG delivery | TIFF delivery | Geographic value |
| --- | ---: | ---: | --- |
| [2008 Delaware shoreline](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2008/DECZ/Oblique/) | 459 frames, 392.8 MiB | 459 frames, 8.05 GiB | Delaware waterfront only. |
| [2011 Delaware shoreline](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2011/DECZ/Obliques/) | 452 frames, 180.0 MiB | 452 frames, 24.98 GiB | Delaware waterfront only. |
| [2012 Delaware shoreline](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2012/DECZ/Oblique/) | 619 frames, 701.9 MiB | 619 frames, 34.83 GiB | Delaware waterfront only. |
| [2014 Delaware shoreline](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP%20-%20DECZ/) | 502 frames, 485.3 MiB | 502 frames, 28.23 GiB | Delaware shoreline. Official notes date the flight to 2014-07-02. |
| [2014 Schuylkill shoreline](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP%20-%20Schuylkill/) | 191 frames, 252.5 MiB | 191 frames, 10.51 GiB | Best new Philadelphia candidate. Official notes date the flight to 2014-07-02. |
| [2014 Little Tinicum Island](https://www.pasda.psu.edu/download/dep/CoastalZoneImageryInventory/DelEstCZ/2014/DECZ/Obliques/DEP-LTI/) | 51 frames, 31.7 MiB | 51 frames, 2.97 GiB | Airport and lower-river context, with little city facade value. |

The 2014 parent listing contains a misleading second `DEP-LTI` directory nested
inside `DEP - Schuylkill`; it contains ten DNG files, not the 51-frame JPEG
delivery. The reproducible acquisition code pins only the top-level
`Obliques/DEP-LTI/JPEG/` directory shown above. It never follows or discovers
TIFF or DNG links.

The planner revalidated the three official JPEG listings on 2026-08-30 without
downloading a photograph. These are the local inventory pins; a changed count,
byte total, ordered frame manifest, or raw-listing SHA-256 is a hard error. The
audited hashes are also encoded in `AUDITED_LISTING_SHA256` and
`AUDITED_FRAME_MANIFEST_SHA256`; even `--refresh` cannot silently accept a new listing.
Acceptance requires re-auditing the official directory, rights, counts, and
sizes, followed by a reviewed code and documentation change. If
`inventory.json` is missing or corrupt, any remaining pixel, partial, progress,
temporary, or review artifact also blocks planning until its provenance is
restored or the collection is archived.

| Collection | Exact JPEG bytes | Raw directory-listing SHA-256 | Ordered frame-manifest SHA-256 |
| --- | ---: | --- | --- |
| 2014 Schuylkill | 264,790,713 | `ca3fe773fcc25077e2b5fd2d8a00d11b95ac9db5b363d6c81dedba24caac5b5c` | `df0a3d4d45f184c19bc87cf50854718179a96235d3a5bb8e6f35f375d807a605` |
| 2014 Delaware | 508,827,973 | `3f984c0f886765148991a436b54eb7590a3329c40e730009332654cec49b4c59` | `78037be7c94377a65c752865468cc4d4618cf281bc124a4dc2d38739be14f5d2` |
| 2014 Little Tinicum | 33,224,886 | `24e73e46e58f7b052eaba7d12cd18af07fd2b160e48f8942ed8bada2e72d7632` | `1f9139a9cb2f60cb3c5e0e00f0b6da8e8f17b1ebd13ae356bee93235e71662a2` |

The semantic digest hashes canonical JSON for the ordered `(name, URL, bytes)`
frame records. It excludes the retrieval timestamp and inventory formatting, so
it binds every downloadable object without making the pin depend on when or how
the ignored JSON file was written.

Three sampled 2014 Schuylkill JPEGs visually confirmed clear angled sides and a
strip extending from upper-river terrain through the airport and industrial
river corridor. The samples identify a Canon EOS 5D Mark II and retain image
dimensions, but contain no GPS, focal length, or pose. Collection metadata says
“Referencing: None”; its exception for georeferencing concerns the 2006 and
2010 Delaware Estuary vertical imagery, not these obliques. No EOP, calibration,
flight-position, stereo-index, or camera-model files are published beside the
frames.

## Legacy 3-D duplicate evidence

The [legacy 3-D root](https://www.pasda.psu.edu/download/philacity/data/3D_Models/)
contains 2008, 2010, and 2015 directories. The 2010 directory totals
8,492,917,031 bytes, but it does not add another photographed neighborhood.

The 1,611,993,906-byte `ph_downtown_kml.zip` central directory contains 2,689
building origins and, across its LOD hierarchy, 9,687 JPEG, 6,998 DAE, and
9,689 KML entries. Its `r0` files are the highest-detail material already
available through `kml00.zip`; `kml01.zip` contains lower LODs. The 2008
“Downtown Area Processed w LiDAR-KML” outer archive contains that same downtown
KML package. Current checkouts can reuse the retained outer archive, while new
checkouts fetch the smaller `kml00.zip` repackaging.

The retained outer archive is
`data/raw/Philadelphia2008_downtown_kml.zip`, with 2,408,076,761 bytes and
SHA-256 `06c42d5b49401bad68db61afe5cfb8f4e1ac0efce2923594b309ae9dce6e1c49`.
The active `data/clean/meta.json` source entry records the same filename, byte
count, and SHA-256, with retrieval time `2026-08-29T01:24:05.553709+00:00`.
Its exact inner `ph_downtown_kml.zip` has SHA-256
`ed8100ba2e83851721166ea764a027f76a171251d2ac1371b9531457ed393e08`.
The retained source and central-directory counts can be checked without
extracting the model files:

```sh
sha256sum data/raw/Philadelphia2008_downtown_kml.zip
unzip -p data/raw/Philadelphia2008_downtown_kml.zip \
  'Downtown Area Processed w LiDAR-KML/ph_downtown_kml.zip' | sha256sum
bash -lc 'bsdtar -tf <(unzip -p data/raw/Philadelphia2008_downtown_kml.zip \
  "Downtown Area Processed w LiDAR-KML/ph_downtown_kml.zip") | awk '\''BEGIN{IGNORECASE=1} \
  /\/r0\/[^/]+\.kml$/{r0++} /\.jpg$/{jpg++} /\.dae$/{dae++} /\.kml$/{kml++} \
  END{printf "r0_kml=%d jpg=%d dae=%d kml=%d\\n",r0,jpg,dae,kml}'\'''
```

The expected final line is `r0_kml=2689 jpg=9687 dae=6998 kml=9689`.
The rejected alternate-format archives were inspected through their official
listings and ZIP central directories but were not retained, so this audit does
not claim local SHA-256 pins for them. Do not repeat those downloads unless an
official listing, file size, coverage statement, or audit reopening trigger
changes.

The other 2010 downloads are alternate deliveries of the same model:

- `ph_downtown_3ds.zip` and its split `part1`/`part2` files are 3DS encodings;
- `ph_downtown_flt_geo.zip` and `ph_downtown_flt_sp.zip` are OpenFlight
  encodings;
- `ph_downtown_dxf.zip` and `ph_downtown_shp.zip` are geometry encodings;
- `tmap01.zip` republishes texture maps; and
- `ph_downtown_gnd_kml.zip` is the ground mesh.

Their manifests repeat the same 2,689 model names, origins, and texture naming
scheme. Lower LODs reduce geometry and the other encodings change containers;
none expands the published bounds or supplies a new photograph. Downloading all
7.91 GiB therefore adds zero facade coverage. The two 2015 geodatabase ZIPs,
`Philadelphia2015_globe.gdb.zip` (155,892,712 bytes) and
`Philadelphia2015_scene.gdb.zip` (182,923,049 bytes), likewise package the
already-used Center City scene rather than a citywide extension.

## Schuylkill registration pilot

Do not begin with the 10.51 GiB TIFF delivery. The repository now pins the
191-frame JPEG listing, every URL and size, and the raw listing SHA-256; it adds
each JPEG SHA-256 to an atomic progress record only after download and structural
validation. Create a contact sheet and mark
frames that visibly cover Manayunk, Waterworks, the Museum of Art, and usable
river-facing buildings. If that review is positive:

```sh
uv run --locked poe oblique-plan
uv run --locked poe oblique-next
uv run --locked poe oblique-status
uv run --locked poe oblique-review
uv run --locked poe oblique-sfm
```

`oblique-next` downloads exactly one pending JPEG by default. Downloads resume
through validated byte ranges, run sequentially, require an exact final EOI and
a successful full ImageMagick decode, reject size or JPEG-structure mismatches,
and publish a final filename and checksum atomically. A valid exact-size final
JPEG or `.part` file is revalidated and recovered when progress is missing or
corrupt, without another request. A changed listing cannot replace its pin while
any JPEG, partial, progress, contact sheet, metadata, or SfM artifact remains.
`oblique-review` writes each frame's dimensions, checksum, and available camera
and EXIF fields. It then creates a reproducible contact sheet with the same
ImageMagick and codec tools plus the exact pinned Noto Sans label font. The
font is resolved with fontconfig, must have SHA-256
`478c558ea716033cd60c03438f628dfa75694dcf6b5f6d505a2f05fd2b4f3823`,
and is passed to montage by file path. The command decodes and resizes exactly one
full size source at a time. ImageMagick montage receives only the cached 320 by
240 thumbnails, so adding frames does not increase the number of full size
images held during a decode. Every thumbnail is labeled with its source
filename. Its cache record includes the source hash, tool key, transform,
thumbnail hash, and exact command. The command rejects stale or damaged cache
records, resumes after an interruption, and removes unused cache files after a
successful sheet. An ordered JSON sidecar records every source SHA-256, the
completed sheet SHA-256, the ImageMagick version, font path and hash, and all
commands that ran. Output is auditable, but different ImageMagick or codec
versions may produce different bytes.
`oblique-sfm` requires at least 20 contiguous frame numbers and records listed/downloaded
counts, range, contiguity, collection completeness, and the camera-intrinsic audit. The explicitly
incomplete diagnostic command is `python -m
isophilly_ingest.coastal_obliques sfm-handoff --allow-incomplete`; such a
manifest labels the override and is not registration evidence. The handoff
detects the free COLMAP CLI or `pycolmap`; if neither is installed it records that prerequisite
instead of pretending a reconstruction occurred. It deliberately records pose
and georeferencing as null because PASDA publishes neither.

The complete Schuylkill JPEG set is not a shared-camera flight. EXIF shows 24
focal lengths from 70 to 270 mm and focal length changes in 106 of 190 frame
transitions. Exact focal-length, dimension, and orientation tuples form 43
groups, 16 of them singletons. Frames 40, 190, and 191 are portrait, and frame
191 has a distinct aspect ratio. There is also a 174-second temporal break
between frames 92 and 93. A reconstruction must therefore create one
EXIF-seeded `SIMPLE_RADIAL` camera per image, quarantine frame 191 initially,
and treat the two temporal sections separately before testing explicit bridge
pairs. A shared intrinsic or shared-camera self-calibration is prohibited.

Use `python -m isophilly_ingest.coastal_obliques ... --collection
delaware-2014` or `--collection little-tinicum-2014` for the other audited 2014
JPEG deliveries. An explicit `fetch` command without `--max-frames` can finish
a pinned collection, but it is opt-in and is not part of normal ingest,
prebuild, tests, or release. Inventory and imagery stay under ignored
`data/coastal-obliques/`; no source pixels are committed.

1. Run CPU-bounded sequential structure from motion with per-image,
   EXIF-seeded variable-zoom intrinsics. Preserve per-frame residuals and reject
   weak solutions. Promote only a dominant model registering at least 153 of
   191 images, with median track length at least 3, median reprojection error at
   most 2 pixels, p95 at most 4 pixels, and no focal drift above 10 percent.
2. Register recovered camera positions and sparse geometry to the 2025 LiDAR,
   2025 orthophoto, and stable bridge/shoreline control points.
3. Validate independent checkpoints before projecting a single facade pixel.
4. Produce a polygon coverage mask and per-building source-frame manifest.
5. Texture only faces with verified visibility, resolution, and occlusion.
6. Render four headings for PMA/Waterworks, Manayunk, and a southern industrial
   control area. Compare seams and at least 30 buildings per area.
7. Fetch the full TIFFs only if JPEG registration succeeds and the additional
   resolution materially improves those comparisons.

Failure to recover stable cameras means the collection remains visual reference
only. A nearby oblique must never be stretched across an unmatched wall.

## Rights and release gate

The PA DEP collection metadata lists access constraints as “None,” but its use
terms are not a conventional open-data license. They grant permission to
translate and add value for use on the user's computer hardware provided the
user annually notifies the University of customizing or value-adding work.
They do not clearly grant public redistribution of source photographs or
derived facade textures.

Local registration experiments may proceed with provenance intact. Do not ship
source images, UV atlases derived from them, or raster tiles containing them in
an HN release until Penn State/PA DEP confirms the required notification and
public derivative-distribution rights in writing. Record that confirmation in
the release evidence.

The existing City/PASDA textured scenes have similarly nonstandard or unclear
redistribution language. Their current use remains subject to the release gate
in `docs/RELEASE.md`.

## When to reopen this audit

Do not repeat this catalog audit merely because the fallback renderer still
looks procedural. Reopen it only when at least one of these triggers occurs:

- PASDA changes the Philadelphia or PA DEP coastal directory inventory or
  updates the cited metadata after 2026-08-30;
- PASDA publishes a new oblique, stereo, camera-pose/EOP, calibration, textured
  I3S/3D Tiles, RGB point-cloud, or photogrammetric-mesh dataset for
  Philadelphia;
- the City, EagleView, Penn State, or PA DEP supplies an authorized bulk
  delivery or written derivative-publication terms;
- a successful Schuylkill pilot justifies auditing adjacent PA DEP flight
  documents at frame level; or
- a current source disappears or changes checksum, bounds, or licensing.

A new nadir orthomosaic or intensity-only LiDAR release does not reopen the
facade conclusion. Update its geometry/ground role in `docs/DATA.md` instead.
