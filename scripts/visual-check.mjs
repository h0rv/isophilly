import { execFileSync, spawn } from "node:child_process";
import { mkdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

import { isometricLonLat } from "../static/city-overlay.js";
import {
  binaryFlagSetting,
  CANONICAL_VISUAL_ZOOMS,
  childHasExited,
  drainGrowingTasks,
  freezeRecords,
  integerListSetting,
  integerSetting,
  pngEvidence,
  publishSuccessAfterTeardown,
  safeRunComponent,
  sha256File,
  stopChild,
  validateCitySectorTargets,
  validateReleaseZooms,
  validateTileResponseSnapshot,
  writeJsonAtomic,
} from "./visual-check-lib.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const neighborhoodOverlay = JSON.parse(
  await readFile(fileURLToPath(new URL("../static/neighborhoods.json", import.meta.url)), "utf8"),
);
const zooms = integerListSetting(
  process.env.ISOPHILLY_VISUAL_ZOOMS,
  CANONICAL_VISUAL_ZOOMS,
  "ISOPHILLY_VISUAL_ZOOMS",
  0,
  10,
);
const artifactRoot = fileURLToPath(new URL("../artifacts/visual", import.meta.url));
const VISUAL_TIME = "2026-06-21T16:00:00Z";
const port = integerSetting(
  process.env.ISOPHILLY_VISUAL_PORT,
  3107,
  "ISOPHILLY_VISUAL_PORT",
  1,
  65535,
);
const tileTimeout = integerSetting(
  process.env.ISOPHILLY_VISUAL_TIMEOUT,
  180_000,
  "ISOPHILLY_VISUAL_TIMEOUT",
  1_000,
  900_000,
);
const settleBudget = integerSetting(
  process.env.ISOPHILLY_SETTLE_BUDGET_MS,
  5_000,
  "ISOPHILLY_SETTLE_BUDGET_MS",
  100,
  120_000,
);
const releaseMode = binaryFlagSetting(
  process.env.ISOPHILLY_VISUAL_RELEASE,
  false,
  "ISOPHILLY_VISUAL_RELEASE",
);
const secondaryEnabled = binaryFlagSetting(
  process.env.ISOPHILLY_VISUAL_SECONDARY,
  true,
  "ISOPHILLY_VISUAL_SECONDARY",
);
validateReleaseZooms(zooms, releaseMode);
const origin = `http://127.0.0.1:${port}`;
const gitSha = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: root,
  encoding: "utf8",
}).trim();
const gitStatus = execFileSync("git", ["status", "--porcelain"], {
  cwd: root,
  encoding: "utf8",
}).trim();
if (releaseMode && gitStatus.length > 0) {
  throw new Error("ISOPHILLY_VISUAL_RELEASE=1 requires a clean working tree");
}
const startedAt = new Date().toISOString();
const runId = `${safeRunComponent(startedAt)}-${safeRunComponent(gitSha.slice(0, 12))}-${process.pid}`;
const runDir = `${artifactRoot}/runs/${runId}`;

/** @param {string} areaName @param {string} screenshotName @param {string} [expectedLabel] */
function localAreaCapture(areaName, screenshotName, expectedLabel = areaName) {
  const area = neighborhoodOverlay.features.find(
    (candidate) => candidate.kind === "local_area" && candidate.name === areaName,
  );
  if (!Array.isArray(area?.label) || area.label.length !== 2) {
    throw new Error(`local-area smoke target is missing: ${areaName}`);
  }
  return {
    name: screenshotName,
    center: isometricLonLat(area.label[0], area.label[1]),
    localAreas: true,
    expectedAreaLabel: expectedLabel,
  };
}

/** @param {string} areaName @param {string} screenshotName */
function planningAreaCapture(areaName, screenshotName) {
  const area = neighborhoodOverlay.features.find(
    (candidate) => candidate.kind === "planning_neighborhood" && candidate.name === areaName,
  );
  if (!Array.isArray(area?.label) || area.label.length !== 2) {
    throw new Error(`planning-area smoke target is missing: ${areaName}`);
  }
  return {
    name: screenshotName,
    center: isometricLonLat(area.label[0], area.label[1]),
    planningAreas: true,
    expectedAreaLabel: areaName,
  };
}

/** @param {string} areaName @param {string} screenshotName @param {string} sector */
function citySectorCapture(areaName, screenshotName, sector) {
  return { ...planningAreaCapture(areaName, screenshotName), sector };
}

/** @param {import("playwright-core").Page} page @param {string} name */
async function screenshotEvidence(page, name) {
  if (!/^[a-z0-9][a-z0-9._-]*\.png$/.test(name)) throw new Error(`unsafe screenshot name: ${name}`);
  const bytes = await page.screenshot({ path: `${runDir}/${name}` });
  return { file: name, ...pngEvidence(bytes) };
}

const server = spawn("target/release/isophilly", ["serve", "--port", String(port)], {
  cwd: root,
  env: { ...process.env, RUST_LOG: "isophilly=warn,tower_http=warn" },
  stdio: ["ignore", "pipe", "pipe"],
});
let serverOutput = "";
let serverSpawnError;
let browser;
let tileZoomLimit = 0;
let richTileZoomLimit = 0;
server.stdout.on("data", (chunk) => {
  serverOutput += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  serverOutput += chunk.toString();
});
server.on("error", (error) => {
  serverSpawnError = error;
});

/** @param {number} milliseconds */
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

/** @param {number} timeout */
async function waitForServerExit(timeout) {
  if (childHasExited(server)) return true;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      server.off("exit", onExit);
      resolve(false);
    }, timeout);
    const onExit = () => {
      clearTimeout(timer);
      resolve(true);
    };
    server.once("exit", onExit);
  });
}

async function stopServer() {
  await stopChild(server, waitForServerExit);
}

let handlingSignal = false;
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, async () => {
    if (handlingSignal) return;
    handlingSignal = true;
    await browser?.close().catch(() => {});
    await stopServer().catch(() => {});
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}

const pyramidAudits = [
  {
    name: "rittenhouse",
    x: 79,
    y: 71,
    columns: 6,
    rows: 5,
    limits: { equalAdjacent: 38, uniformTwoByTwo: 20, edgeDensity: 60 },
  },
  {
    name: "point-breeze",
    x: 72,
    y: 75,
    columns: 6,
    rows: 5,
    limits: { equalAdjacent: 55, uniformTwoByTwo: 30, edgeDensity: 45 },
  },
  {
    name: "italian-market",
    x: 83,
    y: 76,
    columns: 6,
    rows: 5,
    limits: { equalAdjacent: 55, uniformTwoByTwo: 30, edgeDensity: 45 },
  },
  {
    name: "east-passyunk",
    x: 76,
    y: 78,
    columns: 6,
    rows: 5,
    limits: { equalAdjacent: 55, uniformTwoByTwo: 30, edgeDensity: 45 },
  },
];

/**
 * Decode fixed z8 mosaics in the browser so the release gate checks the same
 * lossless WebP bytes that production serves. The thresholds are intentionally
 * derived from the v39 audit and require the two-pixel /128 aerial treatment.
 *
 * @param {import("playwright-core").Browser} browser
 * @param {Record<string, unknown>} meta
 */
async function auditPyramid(browser, meta) {
  const page = await browser.newPage();
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  const results = [];
  for (const audit of pyramidAudits) {
    const result = await page.evaluate(
      async ({ audit, origin, tileVersion }) => {
        const size = 256;
        const canvas = document.createElement("canvas");
        canvas.width = audit.columns * size;
        canvas.height = audit.rows * size;
        const context = canvas.getContext("2d", { willReadFrequently: true });
        if (context === null) throw new Error("audit canvas is unavailable");
        const percentile95 = (values) => {
          if (values.length === 0) return 0;
          const ordered = values.toSorted((left, right) => left - right);
          return ordered[Math.floor((ordered.length - 1) * 0.95)];
        };
        const emptyTiles = [];
        for (let row = 0; row < audit.rows; row += 1) {
          for (let column = 0; column < audit.columns; column += 1) {
            const x = audit.x + column;
            const y = audit.y + row;
            const response = await fetch(
              `${origin}/tiles/8/${x}/${y}.webp?v=${encodeURIComponent(tileVersion)}`,
            );
            if (!response.ok) throw new Error(`audit tile ${x}/${y} returned ${response.status}`);
            if (response.headers.get("x-tile-cache") === "empty") emptyTiles.push(`${x}/${y}`);
            const bitmap = await createImageBitmap(await response.blob());
            if (bitmap.width !== size || bitmap.height !== size) {
              throw new Error(`audit tile ${x}/${y} is ${bitmap.width}x${bitmap.height}`);
            }
            context.drawImage(bitmap, column * size, row * size);
            bitmap.close();
          }
        }
        const width = canvas.width;
        const height = canvas.height;
        const pixels = context.getImageData(0, 0, width, height).data;
        const offset = (x, y) => (y * width + x) * 4;
        const difference = (left, right) =>
          (Math.abs(pixels[left] - pixels[right]) +
            Math.abs(pixels[left + 1] - pixels[right + 1]) +
            Math.abs(pixels[left + 2] - pixels[right + 2])) /
          3;
        const same = (left, right) =>
          pixels[left] === pixels[right] &&
          pixels[left + 1] === pixels[right + 1] &&
          pixels[left + 2] === pixels[right + 2];

        let comparisons = 0;
        let equal = 0;
        let edges = 0;
        let transparent = 0;
        const columnDifferences = [];
        const rowDifferences = [];
        for (let x = 1; x < width; x += 1) {
          let total = 0;
          for (let y = 0; y < height; y += 1) {
            const left = offset(x - 1, y);
            const right = offset(x, y);
            const delta = difference(left, right);
            total += delta;
            comparisons += 1;
            if (same(left, right)) equal += 1;
            if (delta > 4) edges += 1;
          }
          columnDifferences.push(total / height);
        }
        for (let y = 1; y < height; y += 1) {
          let total = 0;
          for (let x = 0; x < width; x += 1) {
            const above = offset(x, y - 1);
            const below = offset(x, y);
            const delta = difference(above, below);
            total += delta;
            comparisons += 1;
            if (same(above, below)) equal += 1;
            if (delta > 4) edges += 1;
          }
          rowDifferences.push(total / width);
        }
        for (let index = 3; index < pixels.length; index += 4) {
          if (pixels[index] !== 255) transparent += 1;
        }

        let twoByTwo = 0;
        let uniformTwoByTwo = 0;
        for (let y = 0; y < height - 1; y += 1) {
          for (let x = 0; x < width - 1; x += 1) {
            const topLeft = offset(x, y);
            twoByTwo += 1;
            if (
              same(topLeft, offset(x + 1, y)) &&
              same(topLeft, offset(x, y + 1)) &&
              same(topLeft, offset(x + 1, y + 1))
            ) {
              uniformTwoByTwo += 1;
            }
          }
        }

        const blankTiles = [];
        for (let row = 0; row < audit.rows; row += 1) {
          for (let column = 0; column < audit.columns; column += 1) {
            const colors = new Set();
            for (let y = row * size; y < (row + 1) * size; y += 8) {
              for (let x = column * size; x < (column + 1) * size; x += 8) {
                const index = offset(x, y);
                colors.add(`${pixels[index]},${pixels[index + 1]},${pixels[index + 2]}`);
              }
            }
            if (colors.size < 8) blankTiles.push(`${audit.x + column}/${audit.y + row}`);
          }
        }

        const verticalSeams = columnDifferences.filter((_, index) => (index + 1) % size === 0);
        const horizontalSeams = rowDifferences.filter((_, index) => (index + 1) % size === 0);
        const internalColumns = columnDifferences.filter((_, index) => (index + 1) % size !== 0);
        const internalRows = rowDifferences.filter((_, index) => (index + 1) % size !== 0);
        return {
          name: audit.name,
          equalAdjacentPercent: Number(((100 * equal) / comparisons).toFixed(2)),
          uniformTwoByTwoPercent: Number(((100 * uniformTwoByTwo) / twoByTwo).toFixed(2)),
          edgeDensityPercent: Number(((100 * edges) / comparisons).toFixed(2)),
          transparentPixels: transparent,
          blankTiles: [...emptyTiles, ...blankTiles],
          seamRatio: Number(
            (
              Math.max(0, ...verticalSeams, ...horizontalSeams) /
              Math.max(1, percentile95([...internalColumns, ...internalRows]))
            ).toFixed(2),
          ),
        };
      },
      {
        audit,
        origin,
        tileVersion: String(meta.tile_version),
      },
    );
    results.push({ ...result, limits: audit.limits });
  }
  await page.close();
  const failures = [];
  for (const result of results) {
    if (result.equalAdjacentPercent > result.limits.equalAdjacent) {
      failures.push(`${result.name} remains too blocky (${result.equalAdjacentPercent}% equal)`);
    }
    if (result.uniformTwoByTwoPercent > result.limits.uniformTwoByTwo) {
      failures.push(`${result.name} has ${result.uniformTwoByTwoPercent}% uniform 2x2 blocks`);
    }
    if (result.edgeDensityPercent < result.limits.edgeDensity) {
      failures.push(`${result.name} detail density is only ${result.edgeDensityPercent}%`);
    }
    if (result.transparentPixels !== 0) {
      failures.push(`${result.name} has ${result.transparentPixels} transparent pixels`);
    }
    if (result.blankTiles.length > 0) {
      failures.push(`${result.name} has blank tiles: ${result.blankTiles.join(", ")}`);
    }
    if (result.seamRatio > 2.5) {
      failures.push(`${result.name} tile seam ratio is ${result.seamRatio}`);
    }
  }
  return { results, failures };
}

async function auditTextureCoverage() {
  const path = fileURLToPath(new URL("../data/clean/texture-coverage.json", import.meta.url));
  let raw;
  try {
    raw = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    return {
      available: false,
      failures: [
        `texture coverage report is unavailable; run ingest first: ${
          error instanceof Error ? error.message : String(error)
        }`,
      ],
    };
  }
  const failures = [];
  const citywide = raw.citywide;
  if (
    typeof citywide?.photographed_building_percent !== "number" ||
    citywide.photographed_building_percent < 2.15
  ) {
    failures.push("citywide photographed building coverage fell below 2.15%");
  }
  if (
    typeof citywide?.photographed_footprint_percent !== "number" ||
    citywide.photographed_footprint_percent < 6
  ) {
    failures.push("citywide photographed footprint coverage fell below 6.0%");
  }
  const areas = Array.isArray(raw.areas) ? raw.areas : [];
  const byName = new Map(areas.map((area) => [area.name, area]));
  for (const required of ["Rittenhouse Square", "Point Breeze", "Bella Vista"]) {
    if (!byName.has(required)) failures.push(`texture coverage report is missing ${required}`);
  }
  const rittenhouse = byName.get("Rittenhouse Square");
  if (
    typeof rittenhouse?.photographed_building_percent !== "number" ||
    rittenhouse.photographed_building_percent < 70
  ) {
    failures.push("Rittenhouse photographed building coverage fell below 70%");
  }
  if (
    typeof rittenhouse?.photographed_footprint_percent !== "number" ||
    rittenhouse.photographed_footprint_percent < 83
  ) {
    failures.push("Rittenhouse photographed footprint coverage fell below 83%");
  }
  return { available: true, citywide, areas: Object.fromEntries(byName), failures };
}

/** @param {string} expectedTileVersion */
async function waitForServer(expectedTileVersion) {
  const readyLine = `IsoPhilly ${origin}`;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (serverSpawnError !== undefined) throw new Error(`server spawn failed: ${serverSpawnError}`);
    if (childHasExited(server)) throw new Error(`server exited early:\n${serverOutput}`);
    if (!serverOutput.includes(readyLine)) {
      await delay(100);
      continue;
    }
    let response;
    try {
      response = await fetch(`${origin}/meta`);
    } catch {
      // The listener is still starting.
      await delay(100);
      continue;
    }
    if (response.ok) {
      const meta = await response.json();
      if (meta.tile_version !== expectedTileVersion) {
        throw new Error(
          `server identity mismatch: ${meta.tile_version} != active ${expectedTileVersion}`,
        );
      }
      return meta;
    }
    await delay(100);
  }
  throw new Error(`server did not become ready:\n${serverOutput}`);
}

/**
 * @param {import("playwright-core").Page} page
 * @param {number} zoom
 * @param {{ name: string, center?: [number, number], mode?: "city" | "detailed", orientation?: "se" | "sw" | "nw" | "ne", planningAreas?: boolean, localAreas?: boolean, expectedAreaLabel?: string, expectedLandmark?: string, sector?: string }} view
 */
async function capture(page, zoom, view = { name: "city-hall" }) {
  const started = performance.now();
  const errors = [];
  const tileRequests = [];
  const responseTasks = [];
  page.on("pageerror", (error) => errors.push(`page: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  const onResponse = (response) => {
    const task = (async () => {
      if (response.url().includes("/tiles/")) {
        const headers = await response.allHeaders();
        const contentLength = headers["content-length"];
        tileRequests.push({
          status: response.status(),
          cache: headers["x-tile-cache"] ?? "unknown",
          bytes:
            contentLength !== undefined && /^\d+$/.test(contentLength)
              ? Number(contentLength)
              : Number.NaN,
          contentType: headers["content-type"] ?? "unknown",
          cacheControl: headers["cache-control"] ?? "",
        });
      } else if (response.status() >= 400 && !response.url().endsWith("/favicon.ico")) {
        errors.push(`${response.status()} ${response.url()}`);
      }
    })().catch((error) =>
      errors.push(`response: ${error instanceof Error ? error.message : error}`),
    );
    responseTasks.push(task);
  };
  page.on("response", onResponse);
  const parameters = new URLSearchParams({ z: String(zoom), time: VISUAL_TIME });
  if (view.mode !== "detailed") parameters.set("mode", "city");
  else parameters.set("view", view.orientation ?? "se");
  if (view.center !== undefined) {
    parameters.set("cx", String(view.center[0]));
    parameters.set("cy", String(view.center[1]));
  }
  await page.goto(`${origin}/?${parameters}`, { waitUntil: "domcontentloaded" });
  const domContentLoadedMs = performance.now() - started;
  try {
    await page.waitForSelector(`canvas[data-zoom="${zoom}"]`);
  } catch (error) {
    const status = await page
      .locator("#status")
      .textContent()
      .catch(() => "missing status");
    const canvasState = await page
      .locator("#map")
      .evaluate((element) => ({
        width: element.getAttribute("width"),
        height: element.getAttribute("height"),
        zoom: element.getAttribute("data-zoom"),
      }))
      .catch(() => ({ missing: true }));
    throw new Error(
      `z${zoom} never initialized: ${error instanceof Error ? error.message : String(error)}\nstatus=${status}\ncanvas=${JSON.stringify(canvasState)}\n${errors.join("\n")}`,
    );
  }
  await page.waitForFunction(() => {
    const canvas = document.querySelector("#map");
    return (
      canvas instanceof HTMLCanvasElement &&
      Number(canvas.dataset.requested) > 0 &&
      Number(canvas.dataset.uncovered) < Number(canvas.dataset.requested)
    );
  });
  const firstMapMs = performance.now() - started;
  await page.waitForFunction(
    () => {
      const canvas = document.querySelector("#map");
      return canvas instanceof HTMLCanvasElement && canvas.dataset.pending === "0";
    },
    undefined,
    { timeout: tileTimeout },
  );
  const settledMs = performance.now() - started;
  page.off("response", onResponse);
  await drainGrowingTasks(responseTasks);
  const settledTileRequests = freezeRecords(tileRequests);
  validateTileResponseSnapshot(settledTileRequests);
  if (view.planningAreas === true) {
    await page.locator("#neighborhoods-toggle").click();
  }
  if (view.localAreas === true) await page.locator("#local-areas-toggle").click();
  if (view.expectedAreaLabel !== undefined) {
    await page.waitForFunction(
      (expected) => {
        const raw = document.querySelector("#map")?.getAttribute("data-area-labels") ?? "[]";
        const labels = JSON.parse(raw);
        return Array.isArray(labels) && labels.includes(expected);
      },
      view.expectedAreaLabel,
      { timeout: settleBudget },
    );
  }
  await page.waitForTimeout(100);
  const screenshot = view.name === "city-hall" ? `z${zoom}.png` : `${view.name}-z${zoom}.png`;
  const screenshotRecord = await screenshotEvidence(page, screenshot);
  const canvas = await page.locator("#map").evaluate((element) => {
    if (!(element instanceof HTMLCanvasElement)) throw new Error("map canvas is missing");
    const context = element.getContext("2d", { willReadFrequently: true });
    if (context === null) throw new Error("map canvas is unavailable");
    const image = context.getImageData(0, 0, element.width, element.height).data;
    let samples = 0;
    let ground = 0;
    const colors = new Set();
    for (let y = 0; y < element.height; y += 8) {
      for (let x = 0; x < element.width; x += 8) {
        const offset = (y * element.width + x) * 4;
        const red = image[offset];
        const green = image[offset + 1];
        const blue = image[offset + 2];
        if (red === 217 && green === 209 && blue === 195) ground += 1;
        colors.add(`${red},${green},${blue}`);
        samples += 1;
      }
    }
    return {
      requested: Number(element.dataset.requested),
      loaded: Number(element.dataset.loaded),
      tileZoom: Number(element.dataset.tileZoom),
      pending: Number(element.dataset.pending),
      uncovered: Number(element.dataset.uncovered),
      failed: Number(element.dataset.failed),
      mode: element.dataset.mode,
      view: element.dataset.view,
      areaLabels: JSON.parse(element.dataset.areaLabels ?? "[]"),
      planningAreas: JSON.parse(element.dataset.planningAreas ?? "[]"),
      localAreas: JSON.parse(element.dataset.localAreas ?? "[]"),
      landmarks: JSON.parse(element.dataset.landmarks ?? "[]"),
      nonGroundRatio: Number((1 - ground / samples).toFixed(4)),
      sampledColors: colors.size,
    };
  });
  if (errors.length > 0) throw new Error(`z${zoom} browser errors:\n${errors.join("\n")}`);
  if (canvas.pending !== 0 || canvas.uncovered !== 0 || canvas.failed !== 0) {
    throw new Error(`z${zoom} has unfinished tiles: ${JSON.stringify(canvas)}`);
  }
  if (view.planningAreas === true && canvas.planningAreas.length === 0) {
    throw new Error(`${view.name} did not paint planning boundaries`);
  }
  if (view.planningAreas !== true && canvas.planningAreas.length !== 0) {
    throw new Error(`${view.name} leaked planning boundaries`);
  }
  if (view.localAreas === true && canvas.localAreas.length === 0) {
    throw new Error(`${view.name} did not paint local areas`);
  }
  if (view.localAreas !== true && canvas.localAreas.length !== 0) {
    throw new Error(`${view.name} leaked local areas`);
  }
  if (view.expectedLandmark !== undefined && !canvas.landmarks.includes(view.expectedLandmark)) {
    throw new Error(`${view.name} did not paint landmark ${view.expectedLandmark}`);
  }
  if (
    canvas.areaLabels.length > 24 ||
    new Set(canvas.areaLabels).size !== canvas.areaLabels.length
  ) {
    throw new Error(
      `${view.name} has noisy or duplicate labels: ${JSON.stringify(canvas.areaLabels)}`,
    );
  }
  if (settledMs > settleBudget) {
    throw new Error(
      `z${zoom} exceeded ${settleBudget} ms settle budget: ${settledMs.toFixed(1)} ms`,
    );
  }
  if (
    settledTileRequests.some(
      (request) =>
        request.contentType !== "image/webp" || !request.cacheControl.includes("immutable"),
    )
  ) {
    throw new Error(`z${zoom} has a non-WebP or non-immutable tile response`);
  }
  const expectedTileZoom = Math.min(
    zoom,
    view.mode === "detailed" ? richTileZoomLimit : tileZoomLimit,
  );
  if (canvas.tileZoom !== expectedTileZoom) {
    throw new Error(
      `z${zoom} used tile z${canvas.tileZoom}; expected canonical z${expectedTileZoom}`,
    );
  }
  if (canvas.sampledColors < 8 || canvas.nonGroundRatio < 0.01) {
    throw new Error(`z${zoom} appears blank: ${JSON.stringify(canvas)}`);
  }
  return {
    view: view.name,
    sector: view.sector ?? null,
    zoom,
    screenshot: screenshotRecord,
    performance: {
      domContentLoadedMs: Number(domContentLoadedMs.toFixed(1)),
      firstMapMs: Number(firstMapMs.toFixed(1)),
      settledMs: Number(settledMs.toFixed(1)),
      tileBytes: settledTileRequests.reduce((total, request) => total + request.bytes, 0),
    },
    canvas,
    tileResponses: {
      total: settledTileRequests.length,
      rendered: settledTileRequests.filter((request) => request.cache === "rendered").length,
      disk: settledTileRequests.filter((request) => request.cache === "disk").length,
      empty: settledTileRequests.filter((request) => request.cache === "empty").length,
    },
  };
}

/**
 * @param {import("playwright-core").Browser} browser
 * @param {Record<string, unknown>} meta
 */
async function interactions(browser, meta) {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  await page.goto(`${origin}/?z=5&view=se&time=${encodeURIComponent(VISUAL_TIME)}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(() => document.querySelector("#map")?.dataset.pending === "0", null, {
    timeout: tileTimeout,
  });
  await page.waitForFunction(
    () => document.querySelector("#map")?.dataset.prefetch === "ready",
    null,
    {
      timeout: tileTimeout,
    },
  );
  const panStarted = performance.now();
  await page.locator("#map").focus();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(() => document.querySelector("#map")?.dataset.pending === "0", null, {
    timeout: settleBudget,
  });
  const prefetchedPanMs = performance.now() - panStarted;
  const controls = await page.locator(".controls button").evaluateAll((buttons) =>
    buttons
      .filter((button) => !button.hidden && button.getClientRects().length > 0)
      .map((button) => {
        const bounds = button.getBoundingClientRect();
        return { id: button.id, width: bounds.width, height: bounds.height };
      }),
  );
  if (controls.some((control) => control.width < 44 || control.height < 44)) {
    throw new Error(`mobile controls are too small: ${JSON.stringify(controls)}`);
  }
  const horizontalOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  if (horizontalOverflow) throw new Error("mobile controls overflow the viewport");
  await page.locator("#rotate-right").click();
  await page.waitForSelector('canvas[data-mode="detailed"][data-view="sw"]');
  await page.waitForFunction(() => document.querySelector("#map")?.dataset.pending === "0", null, {
    timeout: tileTimeout,
  });
  const canvas = page.locator("#map");
  await canvas.focus();
  const cameraBefore = Number(await canvas.getAttribute("data-camera-x"));
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction(
    (before) => Number(document.querySelector("#map")?.dataset.cameraX) > before,
    cameraBefore,
  );
  const cameraAfter = Number(await canvas.getAttribute("data-camera-x"));
  if (!(cameraAfter > cameraBefore)) throw new Error("keyboard pan did not move the camera");
  await canvas.focus();
  await page.keyboard.press("0");
  await page.locator("details").evaluate((element) => {
    element.open = true;
  });
  const screenshot = await screenshotEvidence(page, "mobile.png");

  await page.goto(`${origin}/?mode=city&z=8&time=${encodeURIComponent(VISUAL_TIME)}`, {
    waitUntil: "domcontentloaded",
  });
  await page.waitForFunction(() => document.querySelector("#map")?.dataset.pending === "0", null, {
    timeout: tileTimeout,
  });
  const planningToggle = page.locator("#neighborhoods-toggle");
  const localToggle = page.locator("#local-areas-toggle");
  const overlayControls = await page
    .locator("#neighborhoods-toggle, #local-areas-toggle")
    .evaluateAll((buttons) =>
      buttons.map((button) => {
        const bounds = button.getBoundingClientRect();
        return { id: button.id, width: bounds.width, height: bounds.height };
      }),
    );
  if (overlayControls.some((control) => control.width < 44 || control.height < 44)) {
    throw new Error(`mobile overlay controls are too small: ${JSON.stringify(overlayControls)}`);
  }
  const overlayOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth > window.innerWidth,
  );
  if (overlayOverflow) throw new Error("mobile overlay controls overflow the viewport");
  await localToggle.click();
  await page.waitForFunction(
    () => JSON.parse(document.querySelector("#map")?.dataset.localAreas ?? "[]").length > 0,
  );
  const localOnly = await canvas.evaluate((element) => ({
    planning: JSON.parse(element.dataset.planningAreas ?? "[]").length,
    local: JSON.parse(element.dataset.localAreas ?? "[]").length,
  }));
  if (localOnly.planning !== 0 || localOnly.local === 0) {
    throw new Error(`local-area toggle is not independent: ${JSON.stringify(localOnly)}`);
  }
  await planningToggle.click();
  await page.waitForFunction(
    () => JSON.parse(document.querySelector("#map")?.dataset.planningAreas ?? "[]").length > 0,
  );
  await localToggle.click();
  await page.waitForFunction(
    () => JSON.parse(document.querySelector("#map")?.dataset.localAreas ?? "[]").length === 0,
  );
  const planningOnly = await canvas.evaluate((element) => ({
    planning: JSON.parse(element.dataset.planningAreas ?? "[]").length,
    local: JSON.parse(element.dataset.localAreas ?? "[]").length,
  }));
  if (planningOnly.planning === 0 || planningOnly.local !== 0) {
    throw new Error(`planning toggle is not independent: ${JSON.stringify(planningOnly)}`);
  }

  const landmarks = /** @type {{ name: string }[]} */ (meta.landmarks);
  if (!landmarks.some((landmark) => landmark.name === "Rocky")) {
    throw new Error("Rocky landmark is missing");
  }
  return {
    viewport: [390, 844],
    controls,
    keyboardPan: true,
    independentAreaToggles: true,
    defaultMode: "detailed",
    rotatedTo: "sw",
    prefetchedPanMs: Number(prefetchedPanMs.toFixed(1)),
    rocky: true,
    screenshot,
  };
}

/** @param {Record<string, unknown>} meta */
async function profile(meta) {
  const bounds = /** @type {number[]} */ (meta.iso_bounds);
  const hall = /** @type {number[]} */ (meta.city_hall);
  const zoom = /** @type {number} */ (meta.max_tile_zoom);
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  const count = 2 ** zoom;
  const x = Math.floor(((hall[0] - bounds[0]) / side) * count);
  const y = Math.floor(((hall[1] - bounds[1]) / side) * count);
  const url = `${origin}/tiles/${zoom}/${x}/${y}.webp?v=${meta.tile_version}`;
  const timedFetch = async () => {
    const started = performance.now();
    const response = await fetch(url);
    const bytes = (await response.arrayBuffer()).byteLength;
    return {
      milliseconds: Number((performance.now() - started).toFixed(1)),
      source: response.headers.get("x-tile-cache"),
      bytes,
      status: response.status,
    };
  };
  const first = await timedFetch();
  const second = await timedFetch();
  const overzoomResponse = await fetch(
    `${origin}/tiles/${zoom + 1}/${x * 2}/${y * 2}.webp?v=${meta.tile_version}`,
  );
  const emptyResponse = await fetch(`${origin}/tiles/${zoom}/0/0.webp?v=${meta.tile_version}`);
  const policy = {
    canonical: second.source,
    overzoom: overzoomResponse.status,
    empty: emptyResponse.headers.get("x-tile-cache"),
  };
  await Promise.all([overzoomResponse.arrayBuffer(), emptyResponse.arrayBuffer()]);
  if (policy.canonical !== "disk" || policy.overzoom !== 404 || policy.empty !== "empty") {
    throw new Error(`tile cache policy failed: ${JSON.stringify(policy)}`);
  }
  return { zoom, x, y, first, second, policy };
}

let report;
try {
  await mkdir(`${artifactRoot}/runs`, { recursive: true });
  await mkdir(runDir);
  const activeScene = JSON.parse(
    await readFile(fileURLToPath(new URL("../data/tiles/current.json", import.meta.url)), "utf8"),
  );
  const activeScenePath = fileURLToPath(new URL("../data/tiles/current.json", import.meta.url));
  const binaryPath = fileURLToPath(new URL("../target/release/isophilly", import.meta.url));
  const activeSceneSha256 = await sha256File(activeScenePath);
  const binarySha256 = await sha256File(binaryPath);
  if (
    typeof activeScene.tile_version !== "string" ||
    typeof activeScene.world_sha256 !== "string"
  ) {
    throw new Error("active tile manifest is missing its tile or world identity");
  }
  if (releaseMode) {
    if (!secondaryEnabled) {
      throw new Error("release visual QA cannot disable secondary city-sector captures");
    }
    const worldPath = fileURLToPath(new URL("../data/clean/philly.bin", import.meta.url));
    const worldSha256 = await sha256File(worldPath);
    if (worldSha256 !== activeScene.world_sha256) {
      throw new Error(
        `active tiles are stale: world ${worldSha256} != scene ${activeScene.world_sha256}; run prebuild`,
      );
    }
  }
  const meta = await waitForServer(activeScene.tile_version);
  tileZoomLimit = meta.max_tile_zoom;
  richTileZoomLimit = meta.rich?.max_tile_zoom;
  if (!Number.isInteger(tileZoomLimit) || tileZoomLimit < 0 || tileZoomLimit > meta.max_zoom) {
    throw new Error(`server has an invalid tile zoom limit: ${JSON.stringify(meta)}`);
  }
  if (!Number.isInteger(richTileZoomLimit) || richTileZoomLimit < 0) {
    throw new Error(`server has an invalid rich tile zoom limit: ${JSON.stringify(meta.rich)}`);
  }
  if (!Number.isInteger(meta.counts?.buildings) || meta.counts.buildings < 1) {
    throw new Error(`server has no fallback buildings: ${JSON.stringify(meta.counts)}`);
  }
  if (!Number.isInteger(meta.counts?.building_meshes) || meta.counts.building_meshes < 1) {
    throw new Error(`server has no detailed building meshes: ${JSON.stringify(meta.counts)}`);
  }
  if (!Array.isArray(meta.city_hall) || meta.city_hall.length !== 2) {
    throw new Error(`server has no City Hall mesh focus: ${JSON.stringify(meta.city_hall)}`);
  }
  if (!Array.isArray(meta.landmarks)) throw new Error("server has no landmarks");
  if (
    !Array.isArray(meta.rich?.views) ||
    meta.rich.views.length !== 4 ||
    meta.rich.views.some(
      (view) =>
        !Array.isArray(view.landmarks) ||
        !view.landmarks.some((landmark) => landmark.name === "Rocky"),
    )
  ) {
    throw new Error("every detailed orientation must include Rocky");
  }
  const rendering = await profile(meta);
  browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH ?? "/usr/bin/chromium",
    headless: true,
    args: ["--no-sandbox", "--disable-gpu"],
  });
  const browserVersion = browser.version();
  const pyramidAudit = await auditPyramid(browser, meta);
  const textureCoverageAudit = await auditTextureCoverage();
  const results = [];
  for (const zoom of zooms) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(await capture(page, zoom));
    await page.close();
  }
  for (const orientation of ["se", "sw", "nw", "ne"]) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(
      await capture(page, 5, {
        name: `center-city-${orientation}`,
        mode: "detailed",
        orientation,
      }),
    );
    await page.close();
    const richView = meta.rich.views.find((view) => view.id === orientation);
    const richRocky = richView?.landmarks.find((landmark) => landmark.name === "Rocky");
    if (richRocky === undefined) throw new Error(`${orientation} detailed view is missing Rocky`);
    const rockyPage = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(
      await capture(rockyPage, 5, {
        name: `center-city-rocky-${orientation}`,
        mode: "detailed",
        orientation,
        center: richRocky.point,
        expectedLandmark: "Rocky",
      }),
    );
    await rockyPage.close();
  }
  if (secondaryEnabled) {
    const hall = /** @type {number[]} */ (meta.city_hall);
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(
      await capture(page, 8, {
        name: "east-center-city",
        center: [hall[0] + 900, hall[1] + 180],
      }),
    );
    await page.close();
    const rocky = /** @type {{ name: string, point: [number, number] }[]} */ (meta.landmarks).find(
      (landmark) => landmark.name === "Rocky",
    );
    if (rocky === undefined) throw new Error("Rocky landmark is missing");
    const rockyPage = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(await capture(rockyPage, 8, { name: "rocky", center: rocky.point }));
    await rockyPage.close();
    const neighborhoods = [
      { name: "rittenhouse", center: [985167.68, 310418.65] },
      {
        ...localAreaCapture(
          "Reading Terminal & Convention Center",
          "overlay-dense-center-city",
          "Reading Terminal Market / Convention Center",
        ),
        planningAreas: true,
      },
      localAreaCapture("Italian Market", "overlay-italian-market"),
      localAreaCapture("East Passyunk", "overlay-east-passyunk"),
      { name: "stadiums", center: [981156.04, 313684.68] },
      localAreaCapture("Manayunk Main Street", "local-northwest-manayunk", "Main Street Manayunk"),
      planningAreaCapture("Chestnut Hill", "overlay-sparse-chestnut-hill"),
      localAreaCapture("Africatown", "local-west-africatown"),
      localAreaCapture(
        "Fishtown Frankford Avenue",
        "overlay-fishtown-kensington",
        "Frankford Avenue Arts Corridor",
      ),
      citySectorCapture("Somerton", "city-sector-far-northeast", "far-northeast"),
      citySectorCapture("Hunting Park", "city-sector-north", "north"),
      citySectorCapture("Eastwick", "city-sector-southwest", "southwest"),
      citySectorCapture("Whitman", "city-sector-lower-south", "lower-south"),
    ];
    validateCitySectorTargets(neighborhoods);
    for (const neighborhood of neighborhoods) {
      const neighborhoodPage = await browser.newPage({ viewport: { width: 1440, height: 960 } });
      results.push(await capture(neighborhoodPage, 8, neighborhood));
      await neighborhoodPage.close();
    }
  }
  const interactionResults = await interactions(browser, meta);
  report = {
    schemaVersion: 2,
    success: true,
    runId,
    generatedAt: new Date().toISOString(),
    startedAt,
    gitSha,
    gitDirty: gitStatus.length > 0,
    releaseMode,
    browser: {
      version: browserVersion,
      executable: process.env.CHROMIUM_PATH ?? "/usr/bin/chromium",
    },
    viewport: { desktop: [1440, 960], mobile: [390, 844], deviceScaleFactor: 1 },
    visualTime: VISUAL_TIME,
    worldSha256: activeScene.world_sha256,
    activeSceneSha256,
    releaseBinarySha256: binarySha256,
    tileVersion: meta.tile_version,
    rendering,
    pyramidAudit,
    textureCoverageAudit,
    views: results,
    interactions: interactionResults,
  };
  const auditFailures = [...pyramidAudit.failures, ...textureCoverageAudit.failures];
  if (auditFailures.length > 0) {
    throw new Error(`visual regression audit failed:\n${auditFailures.join("\n")}`);
  }
  await publishSuccessAfterTeardown(
    async () => {
      await browser?.close();
      browser = undefined;
    },
    stopServer,
    {
      reportPath: `${runDir}/report.json`,
      currentPath: `${artifactRoot}/current.json`,
      reportReference: `runs/${runId}/report.json`,
      report,
      current: {
        schemaVersion: 1,
        success: true,
        runId,
        gitSha,
        gitDirty: gitStatus.length > 0,
        releaseMode,
        tileVersion: meta.tile_version,
        worldSha256: activeScene.world_sha256,
        activeSceneSha256,
        releaseBinarySha256: binarySha256,
      },
    },
  );
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} catch (error) {
  const failedReport = {
    ...(report ?? {}),
    schemaVersion: 2,
    success: false,
    runId,
    startedAt,
    finishedAt: new Date().toISOString(),
    gitSha,
    gitDirty: gitStatus.length > 0,
    releaseMode,
    error: error instanceof Error ? (error.stack ?? error.message) : String(error),
  };
  await mkdir(runDir, { recursive: true });
  await writeJsonAtomic(`${runDir}/report.json`, failedReport);
  process.stderr.write(`\nserver output:\n${serverOutput}\n`);
  throw error;
} finally {
  try {
    await browser?.close();
  } finally {
    await stopServer();
  }
}
