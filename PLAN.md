# geo-philly

Build one detailed isometric scene of Philadelphia from public data. Present it
as deterministic pixel art without generated imagery.

## Pipeline

```text
City snapshots + official Center City I3S scene
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
uv run poe ingest
uv run poe prebuild
uv run poe serve
uv run poe visual
uv run poe check
```

Generated data stays under the ignored `data/` directory. `prebuild` is
restartable and uses all available processors. The server requires a completed
z0 through z8 pyramid. The two closer view levels magnify the canonical z8 pixels and
never invoke a second renderer.

## Rendering rules

- Use one pixel treatment at every zoom.
- Draw only the official textured Center City triangles as 3D geometry.
- Use 2025 PASDA aerial crops for the ground across the rest of the city.
- Do not draw untextured buildings, streets, parks, or water over the imagery.
- Show a lower resolution parent while a canonical pyramid tile loads.
- Never substitute a plain geometry tile for a missing texture.

There is no 3D engine, runtime Python, or generated imagery.
