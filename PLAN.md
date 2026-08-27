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
uv run poe visual     # screenshot + tile-gap/performance QA
uv run poe check      # Python + Rust checks
```

`data/` is generated and ignored. Tiles through z5 are pre-rendered. Above that,
the server rasterizes only requested tiles in a blocking worker. Cache paths and
browser URLs include both the renderer revision and a clean-data fingerprint.
Empty tiles are shared rather than persisted; z9+ stays browser/edge-only.

## Rendering rules

- z0-z2: water, parks, and sampled city fabric.
- z3: sampled fabric plus expressways and ramps.
- z4: height-aligned roofs plus major streets.
- z5+: full painter-sorted 2.5D footprints plus street centerlines.
- Palette: warm ground, varied brick rowhouses, cool towers, blue water, green parks.

No 3D engine, textures, satellite imagery, or AI rendering.
