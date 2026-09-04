# Release checklist

## Build and evidence

- Start from a clean commit and run `uv sync --locked --all-groups` and
  `npm ci`.
- Run `uv run --locked poe check`; do not waive formatter, type, lint, test,
  release-build, or dependency-audit failures.
- Run a fresh `uv run --locked poe ingest` and archive `data/clean/meta.json`
  with the exact source commit and every raw snapshot. Confirm that every source
  has a URL, retrieval time, byte count, and SHA-256 checksum.
- Run `uv run --locked poe land-cover-audit` before prebuild. Archive the mask
  header and whole artifact SHA-256. Confirm that the scene uses the
  `v50-citywide-polish` identity and the reviewed OSGeo image digest.
- A release does not use LiDAR heights until all 664 selected sources are
  accounted for and `poe lidar-merge` publishes the canonical schema-3 Parquet
  and JSON pair. Do not publish from a diagnostic partial merge. Archive the
  inventory, canonical pair, rejected-source and gap records, and the nonzero
  applied-building count from the release ingest. The verified 2026-08-31
  inputs account for 653 evidence tiles, three outside-City tiles, and eight
  rejected PASDA sources; the schema-9 ingest applies LiDAR heights to 292,048
  buildings.
- Prebuild every zoom level intended for the public overview. Increment the
  tile/cache version whenever data, projection, colors, or rendering rules
  change.
- Confirm that the full static export contains the citywide pyramid and stays
  below the host's 20,000-file limit. The current export contains 18,009 files
  and uses 1,245.0 MiB.

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
- Inspect Rocky on the PMA steps and the Reading Terminal and Convention Center
  vicinity in the citywide captures. Check the boundary of the 2015 mesh and
  confirm that footprint and OpenStreetMap fallback buildings fill surrounding
  gaps, share correct depth with textured buildings, and keep their procedural
  walls visually distinct from photographed facades.

## Publication

- Keep the City, OpenDataPhilly, and PASDA attribution visible.
- Preserve the source names, capture dates, links, and other provenance in the
  public project documentation.
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
- Run `uv run --locked poe deploy-check`, then `uv run --locked poe deploy`.
  Confirm <https://isophilly.horv.co> from a signed-out browser.
