# geo-philly

Build one detailed isometric scene of Philadelphia from public data. Present it
as deterministic pixel art without generated imagery.

## Pipeline

```text
City snapshots + Center City meshes + OpenStreetMap parts
  -> Python ingest
  -> one compact binary
  -> Rust renders textured z8 tiles
  -> Rust derives z0 through z7 from the same scene
  -> canvas viewer
```

Python handles the offline geospatial import. Rust owns rendering and the local
HTTP service. The official City Limits polygon defines the extent.

## Commands

```sh
uv run poe ingest
uv run poe prebuild
uv run poe serve
uv run poe visual
uv run poe check
```

Generated data stays under the ignored `data/` directory. `prebuild` is
restartable and uses all available processors. The server requires a completed
z0 through z8 pyramid. Z9 through z12 use the same textured renderer, load only
when requested, and persist across runs.

## Rendering rules

- Use one pixel treatment at every zoom.
- Use the official Center City meshes where available.
- Use OpenStreetMap parts where they cover the source footprint well.
- Use City footprints as the citywide fallback.
- Take ground and roof color from 2025 PASDA aerial crops.
- Keep water, parks, streets, and building mass readable over the photograph.
- Show a lower resolution parent while a deep tile loads.
- Never substitute a plain geometry tile for a missing texture.

There is no 3D engine, runtime Python, or generated imagery.
