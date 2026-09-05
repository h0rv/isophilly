# Data quality

Counts are properties of one source snapshot, not permanent properties of live
services. The exact URLs, checksums, and clean counts are in
`data/clean/meta.json`.

## Coverage

| Source | Retained detail | Notes |
| --- | ---: | --- |
| Building Footprints | 545,672 polygons in the last complete snapshot | Repair, clipping, multipart expansion, and the 10 square metre cutoff can change the raw record count. |
| 2015 Center City I3S | 367 leaf chunks and 294,443 textured triangles | This is the newest and highest priority detailed source. |
| 2008 and 2009 legacy downtown | 2,689 `r0` models and 668,082 valid textured triangles before overlap suppression | The clean count is lower because the 2015 scene wins where they overlap. |
| 2008 stadium area | 808 retained `r0` models and 126,181 textured triangles | Six obsolete Spectrum components are excluded. |
| OpenStreetMap Center City building parts | 827 height-backed parts in the current snapshot | Parts improve setbacks and roof forms only where photographed meshes are absent. |

The City Limits extent is about 27.29 by 30.52 kilometres in EPSG:32129. The
boundary controls tile presence so the viewer does not create an unbounded
empty plane.

## Building heights

The footprint source provides a usable approximate height for almost every
building:

| Selected height source | Raw footprints | Share |
| --- | ---: | ---: |
| Valid City `approx_hgt` | 540,569 | 98.99% |
| `max_hgt` fallback | 444 | 0.08% |
| Fixed 8 metre fallback | 5,071 | 0.93% |

Both City fields are interpreted as feet. Values below 2.4 metres or above 400
metres are rejected before fallback. The last packed snapshot had a 7.92 metre
median, a 10.67 metre ninetieth percentile, and a 297.49 metre maximum.

## What the image means

The detailed I3S and COLLADA areas use observed source geometry, UV coordinates,
and photographs. Outside those areas, the renderer uses the City footprint and
height, refined by height-backed OpenStreetMap parts where available. It samples
the 2025 aerial image for the roof and derives a restrained pixel wall palette
from nearby pixels. Floor lines and window bands are procedural illustrations,
not observed facades.

The 2025 orthophoto is suitable for the ground and roofs. It is not a facade
source. Orthorectification moves roof pixels back onto their mapped footprint
and removes most building lean. Stretching roof or shadow pixels down a wall
would create false detail, so the renderer does not do that.

## Plausible facades without facade imagery

Most buildings outside the photographed mesh areas have observed footprints and
approximate heights, but they do not have observed facade images. OpenStreetMap
parts can add measured setbacks, minimum heights, and roof shapes where those
tags are present. The 2025 aerial image supplies ground and roof color where it
has a usable sample. None of these sources shows the building side that the
isometric view needs.

The renderer fills this gap with a plausible illustration based on building
shape and its immediate neighbors. It first distinguishes high-confidence
attached rowhouse runs, lower-confidence rowhouse-shaped footprints, twins,
detached buildings, warehouses, and generic buildings. A generic building is
then drawn as a low rise, mid rise, or tower according to its height and
proportions. The classifier uses height, footprint area, side lengths, and
parallel overlapping edges within 0.8 metres. At least three compatible
attached footprints are required for the high-confidence run class. Narrow
footprints that fit Philadelphia rowhouse proportions but lack reliable
neighbor continuity use the lower-confidence class. Both receive rowhouse
drawing grammar, but only detected attached runs share a run-wide material
seed. Each class gets a limited material palette and a different pattern of
wall and roof detail. Rowhouses can get a cornice, floor courses, doors, and
aligned window bays on exposed short walls. Detected party walls remain solid.
Warehouses receive wider structural bays and restrained ground-level openings.
Some compatible flat roofs receive deterministic, footprint-contained chimneys
or mechanical units to avoid featureless roof slabs.

The added facade and roof detail is synthesis. It does not report the real
number of floors, window positions, door positions, wall material, building
use, or condition. It also does not report real chimney or mechanical-unit locations.
The exposed rowhouse frontage is inferred from footprint proportions and
attached edges rather than an address, entrance record, or street survey. The
class names describe drawing rules and should not be used as property data.

The synthesis is deterministic. Every detected attached run shares one material
family derived from the sorted geometry of its members. Other buildings use a
120 metre source grid to keep nearby materials related. A stable seed based on
the building position or OpenStreetMap identifier adds limited variation. Wall
patterns use source coordinates, so they do not restart at a tile boundary. Fixed
building and tree shadows are also projected from world coordinates. The same source
snapshot, renderer version, land cover file, and view therefore produce the
same tile pixels. The tile identity includes the source hashes and renderer
version, so a changed input cannot reuse an older cache as the current build.

Observed textured meshes still take priority where their coverage is adequate.
The renderer suppresses a fallback extrusion only when the mesh covers enough
of the footprint and height. OpenStreetMap parts replace a parent footprint only
when the parts cover most of it. These checks keep a small or old mesh fragment
from erasing a larger current building, including the Convention Center.

The method has limits. A building with an unusual footprint or height can get
the wrong drawing class. Small footprint gaps can cause a real attached run to
be missed, while unusually close parallel buildings can be treated as attached.
The 120 metre fallback material grid does not follow parcel, street, or
neighborhood boundaries. Aerial roof color can also reflect a roof
coating, shadow, tree canopy, or a building from a different date. Buildings
without a usable aerial sample receive a class palette rather than observed
color. Flat City footprints still fill courtyards, and they cannot show roof
equipment or shapes that are absent from the source data.

An audit also found a roughly 1 TB PASDA directory of 2010 raw flight frames. A
sample frame has no embedded georeference, world file, or camera pose. It is
near vertical and showed little useful facade. The directory has no public
frame footprint catalog, and redistribution terms are unclear. It is not an
active source. A future test requires exterior orientation data and explicit
permission from the City or PASDA.

## Known limits

- World format version 9 stores exterior footprint rings. Courtyards and atria
  in the original footprint source are filled by the fallback extrusion.
- The 2015 scene covers only Center City. The legacy downtown source is older
  and can show buildings that changed after 2009.
- The stadium source is also from 2008. Surface details can differ from the
  current aerial image.
- The fixed footprint simplification is not zoom specific. Coordinates become
  `f32` in the clean binary.
- City Limits is a generalized cartographic boundary, not a surveyed shoreline.
- Content-addressed snapshots are checksummed, but I3S child resources do not
  yet have a complete pinned manifest.
## Release checks

1. Run the full ingest and record `meta.json` with the source commit.
2. Build the complete pyramid from a clean namespace.
3. Check seams and alignment at City Hall, Rittenhouse, the east and south
   legacy edges, the stadium area, Manayunk, Northeast Philadelphia, and West
   Philadelphia.
4. Confirm that no fallback wall changes color at a tile boundary.
