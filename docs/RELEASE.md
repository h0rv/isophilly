# Release checklist

## Build and evidence

- Start from a clean commit and run `uv sync --locked --all-groups` and
  `npm ci`.
- Run `uv run --locked poe check`; do not waive formatter, type, lint, test,
  release-build, or dependency-audit failures.
- Run a fresh `uv run --locked poe ingest` and archive `data/clean/meta.json`
  with the exact source commit and every raw snapshot. Confirm that every source
  has a URL, retrieval time, byte count, and SHA-256 checksum.
- If LiDAR heights are enabled, require a complete 664-tile evidence merge for
  the pinned inventory. Do not publish from a diagnostic partial merge. Archive
  the inventory, merged-evidence metadata, and applied-building count.
- Prebuild every zoom level intended for the public overview. Increment the
  tile/cache version whenever data, projection, colors, or rendering rules
  change.
- Confirm that all four Center City pyramids include z0 through z5. The full
  current static export contains 18,009 files and uses 1,245.0 MiB. The
  exporter must keep the actual count below 20,000.

## Visual smoke test

- Test a cold cache and a warm cache at desktop and narrow/mobile sizes.
- Pan to every official city edge and confirm the camera cannot get lost beyond
  the map.
- Zoom continuously through every level; watch for jumps, blank quadrants,
  stale parent tiles, seams, and abrupt changes of visual language.
- Check City Hall recentering, mouse wheel, trackpad, pointer drag, touch, and
  keyboard navigation.
- Save representative whole-city, neighborhood, and street-level screenshots
  from the exact release build.
- Run the visual check at City Hall and across the permanent neighborhood views
  so both official mesh and citywide footprint paths are tested. Confirm that
  the City and PASDA attribution remains present when the info panel is closed.
- Rotate through all four Center City views and inspect the boundary of the
  2015 mesh. Confirm that footprint and OpenStreetMap fallback buildings fill
  surrounding gaps, share correct depth with textured buildings, and keep
  their procedural walls visually distinct from photographed facades.

## Publication

- Keep the City, OpenDataPhilly, and PASDA attribution visible.
  Re-check the current source terms because the code's MIT license does not
  cover source data or tiles.
- Treat [`PASDA_AUDIT.md`](PASDA_AUDIT.md) as the facade-source decision record.
  Do not publish PA DEP coastal photographs or derived facade textures until
  Penn State/PA DEP confirms derivative-distribution rights and the annual
  notification requirement has been satisfied and recorded.
- Publish behind TLS and a caching proxy/CDN. The built-in server binds to
  localhost and is a development origin, not a hardened public edge server.
- Serve the fully prebuilt static pyramid for an untrusted public audience. No
  dynamic tile renderer is needed in production.
- Serve immutable, versioned tile URLs with long cache lifetimes. Serve the HTML
  entry point with revalidation so a deployment cannot strand users on stale
  tile versions.
- Add the live URL and one current screenshot to the README, test the link in a
  signed-out browser, and verify that no secret or generated multi-megabyte data
  file is tracked.
- In launch copy, call height estimates and data recency what they are. Do not
  present this as an authoritative GIS, survey, property, or navigation product.
