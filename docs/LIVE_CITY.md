# Live city overlays

The map can show live SEPTA vehicles, planning neighborhood outlines, a separately marked
cultural area, and deterministic local lighting. These layers stay independent of the textured
tile pyramid. A failed live request cannot stop the map from loading.

## SEPTA vehicles

`/api/vehicles` combines two official SEPTA JSON feeds:

- [TransitViewAll](https://www3.septa.org/api/TransitViewAll/index.php) for buses and surface
  transit
- [TrainView](https://www3.septa.org/api/TrainView/index.php) for Regional Rail

The server fetches both concurrently, rejects invalid coordinates, and coalesces concurrent map
requests behind one lock. A successful result is held for 15 seconds. If both upstream feeds fail,
the last result is returned with `stale: true`; without a previous result, the route returns 502.
The browser polls every 15 seconds and interpolates from the previous position. SEPTA does not
publish real-time Broad Street Line or Market-Frankford Line vehicle positions in these feeds.

SEPTA's [developer terms](https://www3.septa.org/developer/) grant a limited, revocable right to
use, reproduce, and redistribute the trip-planning data. SEPTA keeps ownership and may change,
revoke, or charge for access. The terms do not grant commercial use of SEPTA trademarks or other
copyrighted material. Review those terms before a public or commercial deployment.

## Neighborhood and cultural names

The generated overlay comes from the Philadelphia City Planning Commission's
[Philly Planning Neighborhoods layer](https://services1.arcgis.com/CtMjdUqInecbPao9/arcgis/rest/services/Philly_Planning_Neighborhoods/FeatureServer/11).
The source description explicitly says these are general historic and development boundaries and
must not be interpreted as official boundaries. The interface repeats that warning.

The Gayborhood is a separate approximate cultural area. It does not replace Washington Square
West. Its extent follows Visit Philadelphia's description of roughly 11th Street to Broad Street
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
