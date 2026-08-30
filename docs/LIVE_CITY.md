# Live city overlays

The map can show planning neighborhood outlines, separately styled local names, and
deterministic local lighting. These overlays stay independent of the textured tile pyramid.
Live transit is intentionally disabled while the visual map is being finished.

## Neighborhood names

The generated overlay comes from the Philadelphia City Planning Commission's
[Philly Planning Neighborhoods layer](https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/Philly_Planning_Neighborhoods/FeatureServer/11).
The source description explicitly says these are general historic and development boundaries and
must not be interpreted as official boundaries. The interface repeats that warning.

The Gayborhood is a separate approximate local boundary. Its label is simply “Gayborhood,” and it
does not replace Washington Square West. Its extent follows Visit Philadelphia's description of roughly 11th Street to Broad Street
and Pine Street to Chestnut Street. Visit Philadelphia also describes it as nested within Midtown
Village and Washington Square West:

- [Washington Square West neighborhood guide](https://www.visitphilly.com/media-center/press-releases/neighborhood-guide-washington-square-west/)
- [Washington Square West](https://www.visitphilly.com/areas/philadelphia-neighborhoods/washington-square-west/)

Run `uv run --locked poe neighborhoods` to refresh `static/neighborhoods.json`. The
builder requires at least 140 features and checks for Bella Vista, Washington Square West, and
Rittenhouse Square before replacing the generated file.

## Sun and lighting

`city-overlay.js` calculates the sun position from the current time with a deterministic NOAA-style
solar equation. It applies a light color wash at golden hour, twilight, and night. Use
`?time=2026-06-21T17:00:00Z` to pin the visual state in tests or screenshots. This is a presentation
effect, not a weather model or a physical shadow simulation.
