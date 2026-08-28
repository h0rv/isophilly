# Visual QA

Run `uv run poe prebuild` before the visual check. Then run:

```sh
uv run poe visual
```

The check builds and starts the release server, opens fixed City Hall views in
system Chromium, and writes screenshots plus a JSON report under
`artifacts/visual/`.

The check covers z3, z4, z5, z7, and z9. It fails on browser errors, failed tile
requests, blank canvases, uncovered parent gaps, or tiles that do not settle.
It also checks an alternate Center City view and times repeated z8 requests.

The server namespaces tiles by the renderer revision and a fingerprint of the
clean data. Z0 through z8 are one image pyramid made from the textured z8
scene. Z9 through z12 use the same renderer, load on demand, and remain on disk
for later runs. The shared aerial source cache has a separate 1 GiB limit.

Use `?z=7` on the viewer URL to open a fixed QA zoom. The browser shows a lower
resolution parent until a requested deep tile is ready. This avoids blank gaps
without changing the drawing style between zoom levels.
