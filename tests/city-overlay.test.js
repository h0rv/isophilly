// @ts-check

import assert from "node:assert/strict";
import test from "node:test";

import {
  isVehicleSnapshot,
  lightingState,
  projectLonLat,
  solarPosition,
} from "../static/city-overlay.js";

test("projects City Hall to EPSG:32129 within half a metre", () => {
  const [x, y] = projectLonLat(-75.1652, 39.9526);
  assert.ok(Math.abs(x - 820_846.3957) < 0.5, `${x}`);
  assert.ok(Math.abs(y - 71_992.3545) < 0.5, `${y}`);
});

test("solar position distinguishes Philadelphia noon and midnight", () => {
  const noon = solarPosition(new Date("2026-06-21T17:00:00Z"), 39.9526, -75.1652);
  const midnight = solarPosition(new Date("2026-06-21T05:00:00Z"), 39.9526, -75.1652);
  assert.ok(noon.altitude > 65, `${noon.altitude}`);
  assert.ok(midnight.altitude < -20, `${midnight.altitude}`);
  assert.equal(lightingState(noon.altitude).phase, "day");
  assert.equal(lightingState(midnight.altitude).phase, "night");
});

test("lighting states meet at explicit horizon thresholds", () => {
  assert.equal(lightingState(10).phase, "day");
  assert.equal(lightingState(0).phase, "golden");
  assert.equal(lightingState(-6).phase, "twilight");
  assert.equal(lightingState(-6.01).phase, "night");
});

test("vehicle snapshot validator rejects malformed feed data", () => {
  assert.equal(
    isVehicleSnapshot({
      updated_at: 1,
      stale: false,
      vehicles: [
        {
          id: "surface:1",
          mode: "surface",
          route: "17",
          label: "17",
          destination: "Penn's Landing",
          latitude: 39.95,
          longitude: -75.16,
          heading: 90,
        },
      ],
    }),
    true,
  );
  assert.equal(isVehicleSnapshot({ updated_at: 1, stale: false, vehicles: [{ id: 1 }] }), false);
});
