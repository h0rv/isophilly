# geo-philly

Make a small, flat, isometric map of Philadelphia from real public footprints.

## Shape

```
official Philadelphia ArcGIS snapshots
  -> Python ingest (GeoPandas/Shapely; one offline binary)
  -> Rust runtime (rstar index + tiny-skia PNG tiles + Axum)
  -> static canvas viewer
```

Python is deliberately not in the request-time rendering loop. It is good at the
one-off geospatial import; Rust owns the hot path over ~545k footprints. The
official City Limits polygon defines the extent and clips all edge geometry.

## Commands

```sh
uv run poe ingest     # snapshot sources; refresh clean binaries + provenance
uv run poe prebuild   # materialize z0-z5 to data/tiles
uv run poe serve      # http://127.0.0.1:3000
uv run poe check      # Python + Rust checks
```

`data/` is generated and ignored. Tiles through z5 are pre-rendered. Above that,
the server rasterizes only requested tiles in a blocking worker; cache headers let
a browser or production edge cache them normally. A production deploy that wants
long immutable caching should put a data-version prefix in the tile URL.

## Rendering rules

- z0-z4: water, parks, and a sampled city fabric.
- z5: pre-rendered flat/extruded building forms.
- z6+: full 2.5D footprints, painter-sorted and capped at 6,000 buildings per tile.
- Palette: warm ground, brick rowhouses, cool towers, blue water, green parks.

No 3D engine, textures, satellite imagery, AI rendering, or duplicated data loader.
