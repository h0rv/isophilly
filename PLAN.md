# IsoPhilly

Build one detailed isometric scene of Philadelphia from public data. Present it
as deterministic pixel art without generated imagery.

## Pipeline

```text
City footprints and heights + aerial photography + Center City I3S scene
  -> Python ingest
  -> one compact geometry binary + official texture atlases
  -> Rust renders textured z8 tiles
  -> Rust derives z0 through z7 from the same scene
  -> canvas viewer
```

Python handles the offline geospatial import. Rust owns rendering and the local
HTTP service. The official City Limits polygon defines the extent.

## Commands

```sh
uv run --locked poe ingest
uv run --locked poe prebuild
uv run --locked poe serve
uv run --locked poe visual
uv run --locked poe check
```

Generated data stays under the ignored `data/` directory. `prebuild` is
restartable and uses available processors, capped at 16. The server requires a
completed z0 through z8 pyramid. The two closer view levels magnify the canonical
z8 pixels and never invoke a second renderer.

## Rendering rules

- Use one pixel treatment at every zoom.
- Draw the official textured Center City triangles where they exist.
- Elsewhere, extrude official City footprints at their source heights, sample
  real aerial pixels on roofs, and derive restrained wall colors from the same
  local imagery.
- Let aerial photography provide roads, parks, water, and ground detail.
- Show a lower resolution parent while a canonical pyramid tile loads.
- Never substitute a plain geometry tile for a missing texture.

There is no 3D engine, runtime Python, or generated imagery.
