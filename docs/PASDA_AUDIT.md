# PASDA Philadelphia imagery and 3-D audit

Audit date: 2026-08-30

This record answers one narrow question: what does PASDA actually publish that
can improve photographed building sides in IsoPhilly? The audit inspected the
official catalog metadata, directory listings, ArcGIS services, and ZIP central
directories. It used small range requests and sample images; it did not bulk
download the candidate collections.

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

Do not begin with the 10.51 GiB TIFF delivery. Pin the 191-frame JPEG listing,
URLs, sizes, and SHA-256 checksums first, then create a contact sheet and mark
frames that visibly cover Manayunk, Waterworks, the Museum of Art, and usable
river-facing buildings. If that review is positive:

1. Run sequential-image structure from motion with shared-camera
   self-calibration. Preserve per-frame residuals and reject weak solutions.
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

