# Live city overlays

The map can show planning neighborhood outlines, separately styled local names, and
deterministic local lighting. These overlays stay independent of the textured tile pyramid.
Live transit is intentionally disabled while the visual map is being finished.

## Neighborhood names

The generated overlay comes from the Philadelphia City Planning Commission's
[Philly Planning Neighborhoods layer](https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/Philly_Planning_Neighborhoods/FeatureServer/11).
The source description explicitly says these are general historic and development boundaries and
must not be interpreted as official boundaries. The interface repeats that warning.

About 60 cultural and commercial areas are generated as separate approximate local boundaries.
They do not replace the PCPC planning neighborhoods. Examples include the Gayborhood, Italian
Market, East Passyunk, Little Saigon, El Centro de Oro, Africatown, Manayunk Main Street, and the
Castor Avenue multicultural corridor. Each generated feature carries its own source URL and a note
identifying the associated planning neighborhood or neighborhoods. District bounds and buffered
street centerlines are intentionally simple and reviewable; they should not be used for legal,
postal, or property decisions.

The registry is kept in `scripts/build_neighborhoods.py`. Its principal sources are:

- [City of Philadelphia eligible commercial corridors](https://www.phila.gov/programs/instore-forgivable-loan-program/eligible-commercial-corridors/)
- [Visit Philadelphia retail corridors](https://www.visitphilly.com/media-center/press-releases/a-guide-to-philadelphias-retail-corridors-where-to-go-and-who-to-contact/)
- [Visit Philadelphia food corridors](https://www.visitphilly.com/media-center/press-releases/philadelphias-food-corridors-offer-neighborhood-dining-at-its-best-2/)
- [Africatown neighborhood guide](https://www.visitphilly.com/areas/philadelphia-neighborhoods/africatown/)

Run `uv run --locked poe neighborhoods` to refresh `static/neighborhoods.json`. The
builder requires at least 140 features and checks for Bella Vista, Washington Square West, and
Rittenhouse Square before replacing the generated file.

## Sun and lighting

`city-overlay.js` calculates the sun position from the current time with a deterministic NOAA-style
solar equation. It applies a light color wash at golden hour, twilight, and night. Use
`?time=2026-06-21T17:00:00Z` to pin the visual state in tests or screenshots. This is a presentation
effect, not a weather model or a physical shadow simulation.
