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
| 2025 PPR street-tree inventory | 151,371 retained point records | The packed record preserves projected location, DBH-derived diameter, and one conservative visual-form byte; it is not a botanical or canopy survey. |
| Terrain relief | 108 by 121 cells | 4,364 direct cells, 3,172 interpolated cells, 133 rejected-gap cells, and 5,399 unsupported cells. The eight rejected PASDA gaps stay neutral, and the artifact is tonal only. |

The City Limits extent is about 27.29 by 30.52 kilometres in EPSG:32129. The
boundary controls tile presence so the viewer does not create an unbounded
empty plane.

## Street-tree forms

The inventory's 2025 tree-name text is normalized only enough to make spacing
deterministic, then must retain the exact ASCII ` - ` scientific/common-name
delimiter. The renderer uses a round fallback for 146,245 records and assigns
the other 5,126 records only from explicit reviewed labels: 3,989 conifers, 251
columnar trees, 644 weeping trees, and 242 shrubs. These are bounded drawing
forms, not a statement of species, crown measurement, health, or pruning.
Malformed, missing, generic, palm, and unsupported values remain the fallback.

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

## Terrain relief

The current terrain artifact is `data/clean/terrain-v1.isoterrain`. It uses 256
metre cells in EPSG:32129 and a fixed sun tone. It does not displace buildings,
roads, or water. The renderer multiplies the slope used for lighting by three
so Philadelphia's gentle relief remains visible at this grid size. It does not
change the recorded elevations, and the final tone stays within 92 to 108
percent of the source ground color.

Direct cells come from at least three accepted ground observations inside a
cell. Interpolated cells use nearby direct cells and then a 3 by 3 median pass.
Rejected-gap cells are the interpolated cells that fall inside one of the eight
rejected PASDA gap tiles. Unsupported cells have no accepted ground support and
remain neutral. The hillshade is a tone multiplier only, so the renderer leaves
water alone and the rich Center City mesh tiles do not use the artifact. The
terrain digest is part of the cache identity, so a changed artifact cannot reuse
an older tile cache.

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
drawing grammar, and broader rowhouse-shaped clusters keep a shared family
seed when their footprint adjacency supports it. Exact attached runs still get
the strongest shared seed, while rowhouse-like buildings can inherit a smaller
family so nearby blocks read as a run instead of isolated boxes. Each class
gets a limited material palette and a different pattern of wall and roof
detail. Rowhouses can get a cornice, floor courses, doors, and aligned window
bays on exposed short walls. High-confidence attached runs also receive a
shallow cornice ledge with real depth on exposed edges between 3.048 and 9.144
metres long. The ledge projects 0.24 metres and occupies the top 0.42 metres of
the recorded building height, so it does not make the building taller. Detected
party walls remain solid. Warehouses
receive wider structural bays and restrained ground-level openings. Some
compatible flat roofs receive deterministic, footprint-contained chimneys or
mechanical units to avoid featureless roof slabs. A separate conservative roof
pass gives only simple, unattached detached-house footprints a low pitched
silhouette. Compact near-square rectangles get a hipped apex; elongated
rectangles get a ridge along their long axis. The recorded City height remains
the roof top, with the wall top lowered by a bounded 1.0 to 2.8 metre rise.
Rowhouse, rowhouse-like, twin, warehouse, generic, and non-rectangular
footprints remain flat. Pitched roofs use the same aerial-derived roof color
and depth buffer as flat roofs, and never receive synthetic roof furniture.

The morphology calculations use EPSG:32129 metres and square metres. Earlier
renderer revisions incorrectly applied feet and square-foot thresholds directly
to those packed coordinates, and also accumulated small footprint areas from
large absolute `f32` products. The current implementation converts the intended
physical thresholds explicitly and computes area relative to the first vertex
with an `f64` sum. On the pinned production world this recovers 286,338
high-confidence attached rowhouses, 51,849 rowhouse-like footprints, and
139,080 two-building/twin cases. These are rendering classes, not land-use or
architectural survey labels.

The added facade, cornice, and roof detail is synthesis. It does not report the
real number of floors, window positions, door positions, wall material,
building use, or condition. It also does not report real chimney or
mechanical-unit locations. The exposed rowhouse frontage and cornice are
inferred from footprint proportions and attached edges rather than an address,
entrance record, or street survey. The class names describe drawing rules and
should not be used as property data.

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
