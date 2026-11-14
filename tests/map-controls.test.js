import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { runInNewContext } from "node:vm";

const app = await readFile(new URL("../static/app.js", import.meta.url), "utf8");
const index = await readFile(new URL("../static/index.html", import.meta.url), "utf8");
const visual = await readFile(new URL("../scripts/visual-check.mjs", import.meta.url), "utf8");

function functionSource(name) {
  const start = app.indexOf(`function ${name}(`);
  assert.notEqual(start, -1, `missing ${name}`);
  const end = app.indexOf("\n}\n", start);
  assert.notEqual(end, -1, `unterminated ${name}`);
  return app.slice(start, end + 2);
}

test("Names and Borders retain their IDs with independent defaults", () => {
  assert.match(index, /id="neighborhoods-toggle"[^>]*aria-pressed="true"[^>]*>Names</);
  assert.match(index, /id="local-areas-toggle"[^>]*aria-pressed="false"[^>]*>Borders</);
  assert.match(app, /let showNeighborhoods = true;/);
  assert.match(app, /let showLocalAreas = false;/);
  for (const [toggle, state, other] of [
    ["neighborhoodsToggle", "showNeighborhoods", "showLocalAreas"],
    ["localAreasToggle", "showLocalAreas", "showNeighborhoods"],
  ]) {
    const handler = app.slice(app.indexOf(`${toggle}.addEventListener("click"`)).split("\n});")[0];
    assert.match(handler, new RegExp(`${state} = !${state}`));
    assert.doesNotMatch(handler, new RegExp(`${other} =`));
  }
});

test("Names combine planning and local names with a viewport budget above 24", () => {
  const features = Array.from({ length: 48 }, (_, i) => ({
    name: `Neighborhood ${i}`,
    kind: "planning_neighborhood",
    label: [90 + (i % 8) * 170, 110 + Math.floor(i / 8) * 140],
  }));
  features.push({ name: "Local district", kind: "local_area", label: [100, 920] });
  const geometry = [];
  const sandbox = {
    neighborhoodData: { features },
    viewportWidth: 1440,
    viewportHeight: 960,
    showNeighborhoods: true,
    showLocalAreas: false,
    city: () => ({ iso_bounds: [0, 0, 1440, 960] }),
    mapPoint: (x, y) => [x, y],
    areaNearViewport: () => true,
    localAreaVisible: () => true,
    areaLabel: (area) => area.name,
    projectArea: (area) => area.name,
    drawAreaGeometry: (rings) => geometry.push(rings),
    ctx: {
      save() {},
      restore() {},
      strokeText() {},
      fillText() {},
      measureText: (text) => ({ width: text.length * 6 }),
    },
    canvas: { dataset: {} },
  };
  function draw(names, borders) {
    sandbox.showNeighborhoods = names;
    sandbox.showLocalAreas = borders;
    sandbox.canvas.dataset = { areaLabels: "[]", planningAreas: "[]", localAreas: "[]" };
    geometry.length = 0;
    runInNewContext(
      `${functionSource("drawAreaOverlays")}\ndrawAreaOverlays(4, 0, 0, 1);`,
      sandbox,
    );
    return JSON.parse(sandbox.canvas.dataset.areaLabels);
  }
  const namesOnly = draw(true, false);
  assert.ok(namesOnly.includes("Local district"));
  assert.ok(namesOnly.includes("Neighborhood 0"));
  assert.ok(namesOnly.length > 24);
  assert.ok(namesOnly.length <= Math.floor((1440 * 960) / 23_000));
  assert.equal(new Set(namesOnly).size, namesOnly.length);
  assert.equal(geometry.length, 0);
  assert.deepEqual(draw(true, true), namesOnly);
  assert.deepEqual(
    geometry,
    features.filter((area) => area.kind === "planning_neighborhood").map((area) => area.name),
  );
  assert.deepEqual(draw(false, true), []);
  assert.equal(geometry.length, 48);
  assert.deepEqual(draw(false, false), []);
  assert.equal(geometry.length, 0);
});

test("Borders draw thin polygon outlines, never covering fills", () => {
  const strokes = [];
  const context = {
    save() {},
    restore() {},
    setLineDash() {},
    beginPath() {},
    moveTo() {},
    lineTo() {},
    closePath() {},
    stroke() {
      strokes.push(this.lineWidth);
    },
    fill() {
      assert.fail("border polygon fill");
    },
    fillRect() {
      assert.fail("border rectangle fill");
    },
  };
  runInNewContext(`${functionSource("drawAreaGeometry")}\ndrawAreaGeometry(rings);`, {
    ctx: context,
    viewportWidth: 390,
    viewportHeight: 844,
    rings: [
      [
        { x: 50, y: 80 },
        { x: 300, y: 80 },
        { x: 200, y: 400 },
      ],
    ],
  });
  assert.deepEqual(strokes, [1]);
});

test("City Hall starts and resets at z7 without removed landmark UI or shortcuts", () => {
  assert.match(functionSource("initialTileZoom"), /Math\.max\(7, sceneHomeZoom\(\)\)/);
  assert.match(app, /event\.key === "0"\) centerCityHall\(7\)/);
  assert.match(app, /home\.addEventListener\("click", \(\) => centerCityHall\(7\)\)/);
  assert.doesNotMatch(index, /rocky|Press R/i);
  assert.doesNotMatch(
    app,
    /drawLandmark|drawRocky|centerRocky|event\.key\.(?:toLowerCase|toUpperCase)\(\) === "[rR]"|event\.key === "[rR]"/,
  );
  assert.match(app, /canvas\.dataset\.landmarks = "\[\]"/);
});

test("visual proof requests Names and optional Borders without legacy layer or Rocky expectations", () => {
  assert.match(visual, /namesToggle\.getAttribute\("aria-pressed"\)\) !== "true"/);
  assert.match(
    visual,
    /bordersToggle\.getAttribute\("aria-pressed"\)\) !== String\(view\.planningAreas === true\)/,
  );
  assert.match(visual, /canvas\.areaLabels\.includes\(view\.expectedAreaLabel\)/);
  assert.match(visual, /canvas\.areaLabels\.length > canvas\.labelBudget/);
  assert.match(visual, /canvas\.unfittingLabels\.length > 0/);
  assert.match(visual, /borderGeometry\.fills !== 0/);
  assert.match(visual, /map\?\.getAttribute\("data-zoom"\) === "7"/);
  assert.doesNotMatch(
    visual,
    /leaked planning|leaked local|expectedLandmark|rockyShortcut|keyboard\.press\("[rR]"\)|name: "rocky"|landmark\.name === "Rocky"/,
  );
  assert.match(visual, /independentNamesAndBorders: true/);
  assert.match(visual, /removedLandmarkUiAbsent: true/);
});
