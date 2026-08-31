# Live city overlays

The map can show planning neighborhood outlines, separately styled local names, and
deterministic local lighting. These overlays stay independent of the textured tile pyramid.
Live transit is intentionally disabled while the visual map is being finished.

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
