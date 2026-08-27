# Visual QA

`uv run poe visual` builds and boots the real release server, opens fixed City
Hall views in system Chromium, and writes screenshots plus a JSON report to
`artifacts/visual/`.

The check covers z3, z4, z5, z7, and z9. It fails on browser errors, HTTP tile
errors, blank canvases, uncovered parent-fallback gaps, or tiles that do not
settle. The report also times one uncached-or-disk z8 tile and its immediate
warm repeat. Generated artifacts stay local because they depend on the current
ignored data snapshot. Z8 is the deepest level persisted by the local disk
cache.

The server namespaces disk tiles by both renderer revision and a fingerprint of
the clean data. Empty tiles never reach the render queue or disk, and z9+ tiles
stay browser/edge-only so arbitrary public URLs cannot grow the local cache
without bound.

Use `?z=7` on the viewer URL to open any deterministic QA zoom. Tiles display
near their native 256-pixel size, keeping visible requests bounded. The
renderer preserves roof positions across LOD changes: city fabric through z3,
rooftops at z4, and consistently lit extrusions from z5 onward.
