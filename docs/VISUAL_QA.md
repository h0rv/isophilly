# Visual QA

Run `uv run --locked poe prebuild` before the visual check. For an ordinary
local development run:

```sh
uv run --locked poe visual
```

For release evidence, use the explicit strict mode:

```sh
ISOPHILLY_VISUAL_RELEASE=1 uv run --locked poe visual
```

Strict mode rejects a dirty worktree, noncanonical zooms, a disabled secondary
city audit, and an active tile manifest whose world SHA-256 differs from
`data/clean/philly.bin`. The release and secondary flags accept only `0` or
`1`; misspellings fail instead of silently changing the audit scope.
Local mode keeps those checks advisory so unfinished development can still be
inspected. Both modes build the release server, require that the spawned child
prints its own ready identity, and compare the served tile version with
`data/tiles/current.json`. `ISOPHILLY_VISUAL_PORT` selects a validated local
port and defaults to 3107. An occupied port fails instead of attaching to an
older server. Shutdown waits for `SIGTERM` and escalates to `SIGKILL` after a
bounded timeout, including on an interrupt. Either an exit code or a recorded
terminating signal counts as a completed child shutdown. The run fails if the
child remains alive after the final bounded `SIGKILL` wait.

## Current automated coverage

The default city zooms are z3, z4, z5, z7, z9, and z10. Set
`ISOPHILLY_VISUAL_ZOOMS` to a nonempty comma-separated list of unique decimal
integers for a development-only variant; whitespace, signs, fractions, empty
entries, suffixes, duplicates, and values outside z0 through z10 are rejected.
Strict release evidence always uses the canonical default zoom matrix.
The check rejects browser errors, failed or uncovered tiles, blank
canvases, invalid response policy, unsettled views, and regressions in the
fixed z8 tile-detail metrics.

The secondary matrix derives its centers from the checked-in neighborhood data.
It covers representative Center City, River Wards, Northwest, West, far
Northeast, North, Southwest, lower South, and stadium views. Cultural-area
checks include Italian Market, East Passyunk, Manayunk, Africatown, Reading
Terminal, and Fishtown. Planning-neighborhood and cultural-area controls are
tested independently, with a fixed cap on duplicate or excessive labels.

All four Center City orientations are captured twice: once at their normal
focus and once centered on that orientation's Rocky coordinate. The canvas
records which landmarks it actually painted, so a Rocky capture fails if the
figure is missing or outside the viewport. This verifies rendering of the
configured PMA-steps landmark; changes to the landmark's underlying geographic
coordinate still require review.

The check also records photographed-building coverage and fixed city-tile seam,
blankness, and detail metrics. Center City mesh-boundary quality is still a
human screenshot review; there is not yet a dedicated mesh-edge pixel baseline.

## Evidence layout

Every invocation writes to a new directory:

```text
artifacts/visual/runs/<timestamp>-<commit>-<pid>/
```

Screenshots are never overwritten. Each screenshot record in `report.json`
contains its relative filename, SHA-256, byte count, width, and height. The
report also records success, commit and dirty state, strict-mode state, browser
executable and version, viewports, fixed lighting time, world hash, tile
version, timings, tile-response totals, pyramid metrics, and coverage metrics.
After tile settlement, the response listener is detached, every queued header
task is drained to a stable boundary, and validation, byte totals, and cache
counts are derived from one frozen snapshot. That snapshot must contain at
least one successful tile response, and every response must have a valid
nonnegative integer content length.

On success only, the runner atomically replaces
`artifacts/visual/current.json`. That small manifest points to the successful
run report and records its SHA-256 and input identity. A failed run writes its
own `success:false` report once its run directory has been initialized and
leaves the previous current-success pointer unchanged, so stale screenshots
cannot masquerade as the latest result. Configuration or filesystem failures
that occur before run initialization are reported on stderr and cannot create
either kind of manifest.

The browser and spawned server must both finish bounded teardown before the
runner publishes the success report or current pointer. A teardown failure
therefore leaves the previous successful pointer unchanged.

Failed atomic report or pointer writes remove their temporary `.part` file
before returning the original error when cleanup is possible.

`artifacts/visual/` is ignored by Git. Archive the complete run directory plus
`current.json` when evidence must be retained. A report without its matching
hashed PNGs is incomplete evidence.

## Browser and baseline limits

`playwright-core` is locked, but the runner currently uses system Chromium at
`/usr/bin/chromium`, or `CHROMIUM_PATH` when provided. The exact executable path
and reported browser version are recorded. Use the same Chromium package,
fonts, and host environment when comparing runs.

There is not yet a checked-in perceptual screenshot baseline. The present gate
provides hashed, attributable screenshots plus deterministic structural
metrics; visual sign-off still requires reviewing those screenshots. Baseline
updates must eventually be a separate explicit workflow and must never happen
as a side effect of the normal check.

Timing fields are single-run local diagnostics. Settled views have a default
5-second budget, configurable with `ISOPHILLY_SETTLE_BUDGET_MS`, and tile waits
use `ISOPHILLY_VISUAL_TIMEOUT`. They are not cross-machine benchmark claims;
repeat runs under the same recorded environment before treating timing changes
as performance regressions.

Browser-free tests cover strict numeric settings, response-queue draining and
snapshot freezing, child exit detection, atomic JSON cleanup, the required
outer-city sector matrix, PNG evidence hashes and dimensions, and safe
run-directory components. `npm run check` runs those tests and statically
checks the runner.
