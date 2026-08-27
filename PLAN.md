# geo-philly

Make a small, flat, isometric map of Philadelphia from real public footprints.

## Shape

```
official Philadelphia snapshots + Center City 3D meshes + OpenStreetMap parts
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
uv run poe prebuild    # materialize pixel-textured z0-z5 to data/tiles
uv run poe serve       # pixel texture, http://127.0.0.1:3000
uv run poe serve-full  # photographic texture
uv run poe serve-plain # geometry only
uv run poe visual      # pixel screenshot + tile-gap/performance QA
uv run poe visual-full # full-texture QA
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
- z5+: full 2.5D footprints plus street centerlines.
- z5+: height-backed Center City parts replace a base footprint only when their
  combined area covers most of it.
- z7+: official Center City multipatch geometry replaces simple extrusions. A
  tile depth buffer preserves setbacks, roof forms, and landmark silhouettes.
- z7+: height-corrected roof photography. z8+ adds restrained floor bands.
- Texture modes: native detail 2025 PASDA aerial crops, deterministic pixel
  processing, or plain geometry.
- Palette overlays keep water, parks, streets, and building massing readable over imagery.

No 3D engine or AI rendering. Aerial source crops and final tiles persist across
runs within fixed cache limits.
