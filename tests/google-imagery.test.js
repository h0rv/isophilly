import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import {
  selectTiles,
  sha256,
  tileBounds,
  validateCaptureResult,
  validateManifest,
  validatePlan,
} from "../scripts/google-imagery-lib.mjs";
import { isometricLonLat } from "../static/city-overlay.js";
import {
  ecefToGeodetic,
  geodeticToEcef,
  inverseIsometric,
  projectEcef,
  projectStatePlane,
} from "../tools/google-imagery/projection.js";

function plan() {
  const center = isometricLonLat(-75.23, 39.96);
  return {
    schema_version: 1,
    world_sha256: "a".repeat(64),
    style: "isophilly-google-v1",
    zoom: 8,
    tile_size: 256,
    iso_bounds: [center[0] - 256, center[1] - 256, center[0] + 256, center[1] + 256],
    tiles: [
      { x: 127, y: 127 },
      { x: 128, y: 127 },
      { x: 127, y: 128 },
      { x: 128, y: 128 },
    ],
    capture: { height_origin: 0, renderer: "state-plane-unlit-v1" },
  };
}

test("plans reject stale schemas, invalid bounds, duplicate or noncanonical tiles", () => {
  assert.equal(validatePlan(plan()).zoom, 8);
  for (const edit of [
    { zoom: 9 },
    { tile_size: 512 },
    { style: "sf-pixel-art" },
    { world_sha256: "oops" },
    { iso_bounds: [0, 1, 0, 2] },
    { iso_bounds: [0, 0, Number.NaN, 1] },
    { tiles: [] },
    {
      tiles: [
        { x: 1, y: 2 },
        { x: 1, y: 2 },
      ],
    },
    { tiles: [{ x: 256, y: 0 }] },
  ])
    assert.throws(() => validatePlan({ ...plan(), ...edit }));
});

test("capture selection is bounded and reports every omitted tile", () => {
  const selected = selectTiles(plan(), [-75.23, 39.96], 20, 2);
  assert.equal(selected.tiles.length, 2);
  assert.equal(selected.omitted, 2);
  assert.deepEqual(
    tileBounds(plan(), { x: 127, y: 127 }).slice(2),
    tileBounds(plan(), { x: 128, y: 128 }).slice(0, 2),
  );
  assert.throws(() => selectTiles(plan(), [-122.4, 37.8], 20, 2));
  assert.throws(() => selectTiles(plan(), [-75.23, 39.96], 20, 0));
  assert.throws(() => selectTiles(plan(), [-75.15, 40.1], 1, 2), /No eligible/);
});

test("resumed captures cannot mix grids, source worlds, camera settings, or missing provenance", () => {
  const source = plan();
  const manifest = {
    ...source,
    tiles: [
      {
        x: 127,
        y: 127,
        sha256: sha256("image"),
        attributions: ["Google Maps", "Fixture provider"],
        captured_at: "2026-11-13T00:00:00Z",
      },
    ],
  };
  assert.equal(validateManifest(manifest, source, source.capture), manifest);
  assert.throws(() =>
    validateManifest(manifest, { ...source, world_sha256: "b".repeat(64) }, source.capture),
  );
  assert.throws(() => validateManifest(manifest, source, { ...source.capture, height_origin: 20 }));
  assert.throws(() =>
    validateManifest(
      { ...manifest, tiles: [{ ...manifest.tiles[0], attributions: [] }] },
      source,
      source.capture,
    ),
  );
  assert.throws(() =>
    validateManifest(
      { ...manifest, tiles: [{ ...manifest.tiles[0], captured_at: "" }] },
      source,
      source.capture,
    ),
  );
});

test("holey, coarse, and unattributed renders never pass the publication gate", () => {
  const result = {
    opaquePixels: 65536,
    visibleTiles: 2,
    maxError: 1,
    attributions: ["Google Maps", "Fixture provider"],
  };
  validateCaptureResult(result);
  for (const edit of [
    { opaquePixels: 65535 },
    { visibleTiles: 0 },
    { maxError: 5 },
    { maxError: Number.NaN },
    { attributions: ["Google Maps"] },
    { attributions: ["Google Maps", ""] },
  ]) {
    assert.throws(() => validateCaptureResult({ ...result, ...edit }));
  }
});

test("ECEF conversion and isometric vertices agree with the existing map", () => {
  for (const [lon, lat, height] of [
    [-75.23, 39.96, -20],
    [-75.08, 40.07, 80],
    [-75.16, 39.95, 350],
  ]) {
    const ecef = geodeticToEcef(lon, lat, height);
    const roundTrip = ecefToGeodetic(...ecef);
    assert.ok(Math.abs(roundTrip[0] - lon) < 1e-9);
    assert.ok(Math.abs(roundTrip[1] - lat) < 1e-9);
    assert.ok(Math.abs(roundTrip[2] - height) < 1e-6);
    const output = projectEcef(...ecef, 10);
    const expected = isometricLonLat(lon, lat, height - 10);
    assert.ok(Math.abs(output[0] - expected[0]) < 1e-5);
    assert.ok(Math.abs(output[1] - expected[1]) < 1e-5);
    const [x, y] = inverseIsometric(output[0], output[1] + height - 10);
    const projected = projectStatePlane(x, y, height - 10);
    assert.ok(Math.abs(projected[0] - output[0]) < 1e-6);
    assert.ok(Math.abs(projected[1] - output[1]) < 1e-6);
  }
});

test("capture command fails without a key before opening a browser or making paid requests", () => {
  const result = spawnSync(
    process.execPath,
    ["scripts/google-imagery.mjs", "capture", "--output=/nonexistent/isophilly-test"],
    {
      encoding: "utf8",
      env: { ...process.env, GOOGLE_MAPS_API_KEY: "" },
    },
  );
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Set GOOGLE_MAPS_API_KEY/);
  assert.doesNotMatch(result.stderr, /ENOENT|Playwright|browserType/);
});
