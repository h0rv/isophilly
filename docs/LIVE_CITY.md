# Live city overlays

The map can show planning neighborhood outlines, separately styled local names, and
deterministic local lighting. These overlays stay independent of the textured tile pyramid.
Live transit is intentionally disabled while the visual map is being finished.

## Water, parks, and land cover

The ingest builds water and park masks from the City hydrology and PPR property sources. The tile
builder combines them with the audited 2018 Philadelphia land cover mask. The raster classes come
from 2018 LiDAR and 2017 NAIP imagery, while the displayed ground remains the 2025 aerial image.
Tree canopy and grass or shrub classes receive different restrained green grading. Building, road,
railroad, paved, and bare earth classes keep their aerial color. The tree-canopy class also draws a
single low, depth-tested foliage surface in the renderer. This makes parks and woodlands read as
continuous leafy mass without turning raster cells into invented street-tree points.

All tile types now share one baked color contract. The restrained ground, vegetation, water,
building, and tree anchors live in the renderer, and the final continuous saturation and contrast
finish is applied before PNG encoding. The browser does not recolor the canvas. This keeps exported
tiles, derived zoom levels, deterministic screenshots, and the live display on the same pixels while
preserving the continuous detail in the photographed Center City mesh.

Outside photographed mesh coverage, a conservative citywide roof pass can add a low hipped or
gabled silhouette to simple detached-house rectangles. It is derived at render time from the existing
footprint, context, and metre height: the City height stays the roof top and the walls stop below the
bounded roof rise. Attached rows, rowhouse-like footprints, twins, warehouses, and irregular shapes
stay flat. The roof color remains sampled from the aerial imagery, with no invented chimneys or other
roof furniture on pitched roofs.

High-confidence attached rowhouse runs receive a shallow depth-tested cornice ledge on exposed
frontage-width edges. It projects 0.24 metres and stays within the top 0.42 metres of the recorded
height. Party walls and lower-confidence rowhouse-like footprints do not receive the added volume.
When the v12 world stores an exact address and street range match, every attached drawing family uses
only that nonparty edge for painted openings. Its component seed shares the floor rhythm while each
wall keeps local material and glass variation. Cornices and stoops remain limited to high-confidence
rowhouses. A selected high-confidence edge can receive a two tier stoop aligned with the drawn door.
The stoop projects no more than 0.70 metres and reaches 0.36 metres above the ground. Unknown
frontages keep the earlier exposed short edge rule and do not receive a stoop. The first two upper
floors on a selected high-confidence frontage can receive painted stone toned surrounds, and the
same frontage can receive a painted stone base no taller than 0.56 metres outside the door. These
features add no geometry. The result is illustrative geometry derived from the footprint, street
record, and neighbor context. It is not a facade or entrance survey.

Water now uses a shoreline-aware deterministic treatment: official City hydrology still has
priority, but the renderer also consults the 3 m land-cover mask to extend water only when nearby
water pixels and hydrology agree. The result keeps a narrow quantized shoreline, then falls back to
the same fixed blue bands for open water. The bands use source
coordinates, so they stay aligned across tile boundaries and camera views. Park grading applies
only where the land cover class is tree canopy or grass and shrub. Courts, paths, and plazas
therefore keep their aerial color. If the optional mask is absent, the renderer keeps the earlier
aerial-color vegetation test instead of failing a normal prebuild.

These effects are baked into the tile images by `poe prebuild`. They add no browser timer, network
request, or repeated repaint. There is no continuous motion, so people who prefer reduced motion see
the same stable scene. The citywide tile identity includes the exact mask digest. Rebuild the tiles
after changing the mask or renderer to view the result.

The canopy surface samples the PASDA class and its broad foliage tone from source coordinates for
each output pixel. It blends that tone with the corresponding sampled aerial color, preserving local
canopy detail instead of replacing it with a flat green fill. It is bounded to the 256 by 256 output
tile, creates no retained canopy geometry, and cannot reset at a tile edge or when the city changes
orientation. It shares the scene depth buffer: buildings and mesh faces occlude it where closer,
while the separate official street-tree inventory is drawn afterward and remains the explicit
individual-tree layer.

The separate 2025 street-tree inventory retains its measured location and DBH-derived size. A
strict subset also uses an explicit source label to select a bounded conifer, columnar, weeping, or
shrub crown. All other records keep the established varied round fallback. The packed form is a
drawing category, not a claim about measured crown dimensions, health, or exact species.

The terrain relief adds a deterministic tonal hillshade to the ground pass only. It does not move
geometry or change water color, and the rich Center City mesh views do not use the terrain artifact.
The eight rejected PASDA gaps stay neutral, so the relief does not add a visible seam where the
LiDAR evidence stops.

## Neighborhood names

The generated overlay comes from the Philadelphia City Planning Commission's
[Philly Planning Neighborhoods layer](https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/Philly_Planning_Neighborhoods/FeatureServer/11).
The source description explicitly says these are general historic and development boundaries and
must not be interpreted as official boundaries. The interface repeats that warning.

Exactly 61 cultural, commercial, civic, and arts areas are generated as separate approximate local
boundaries. All 61 are eligible for display and have an explicit display setting in the Python
registry. The browser reads each display label, zoom tier, geometry setting, relevance class,
priority, contextual association, canonical planning parent, planning label suppression, overlap
group, and inclusion reason from the generated JSON. There is no second browser list. Africatown's
broad sourced extent uses a label without a filled shape, so smaller West and Southwest Philadelphia
corridors remain clear.
They do not replace the PCPC planning neighborhoods. Examples include the Gayborhood, Italian
Market, East Passyunk, Little Saigon, El Centro de Oro, Africatown, Manayunk Main Street, and the
Castor Avenue multicultural corridor. Each generated feature carries its own source URL and a note.
The `associations` field keeps local context such as Passyunk Square or Elmwood, even when the PCPC
layer does not use that name. The separate `planning_parents` field contains only names found in the
pinned PCPC layer. District bounds and buffered street centerlines are intentionally simple and
reviewable. They should not be used for legal, postal, or property decisions.

Eligible does not mean that every label appears at once. The renderer waits until the configured zoom
tier, applies the viewport label limit, and removes labels whose measured boxes collide. Higher
priority labels win before lower priority labels. At equal priority, local labels win over planning
labels, and then labels use name order. Filled shapes can overlap, but Africatown and the closest tier
destinations use labels without filled shapes where a broad fill would be noisy.

The registry is kept in `scripts/build_neighborhoods.py`. Its principal sources are:

- [City of Philadelphia eligible commercial corridors](https://www.phila.gov/programs/instore-forgivable-loan-program/eligible-commercial-corridors/)
- [Visit Philadelphia retail corridors](https://www.visitphilly.com/media-center/press-releases/a-guide-to-philadelphias-retail-corridors-where-to-go-and-who-to-contact/)
- [Visit Philadelphia food corridors](https://www.visitphilly.com/media-center/press-releases/philadelphias-food-corridors-offer-neighborhood-dining-at-its-best-2/)
- [Africatown neighborhood guide](https://www.visitphilly.com/areas/philadelphia-neighborhoods/africatown/)

Run `uv run --locked poe neighborhoods` to refresh `static/neighborhoods.json` from the live source,
or pass `--input` to `scripts/build_neighborhoods.py` to rebuild from a reviewed saved response. The
builder first runs the complete offline audit. It replaces the file atomically only when the audit
passes. The audit requires the pinned 148 name planning payload, including its geometry, and the full
generated record for every local area.

Run `uv run --locked poe neighborhood-audit` for the normal offline review. The command makes no
network request and does not rebuild data. It pins the checked-in planning layer at 148 names and an
audited SHA-256 of those names and the full planning geometry payload. It compares every generated
local record with the registry, including the source, note, label, shape, and display policy. It also
checks duplicate names, canonical planning parents, suppression targets, containment within the
planning layer bounds, and minimum eligible counts in six city regions.

The report lists shape overlap ratios of at least 25 percent and label anchors within 150 metres in
`artifacts/neighborhood-audit.json`. Every listed pair must have an exact reviewed policy that names
the winning label and explains the decision. Overlap groups are descriptive only and never approve a
pair. The audit fails on any new pair, including a new pair inside an existing group, and it fails when
a winner's priority is too low. The current pair policies give priority to Italian Market, the
Gayborhood, and Africatown in their dense groups. The audit checks data and deterministic collision
policy, but it does not claim that a person has viewed every one of the 61 areas. A changed PCPC name
set or geometry payload requires a source review and pin update.

## Sun and lighting

`city-overlay.js` calculates the sun position from the current time with a deterministic NOAA-style
solar equation. It applies a light color wash at golden hour, twilight, and night. Use
`?time=2026-06-21T17:00:00Z` to pin the visual state in tests or screenshots. This is a presentation
effect, not a weather model or a physical shadow simulation.
