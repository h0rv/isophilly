import { execFileSync, spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("..", import.meta.url));
const zooms = (process.env.GEO_PHILLY_VISUAL_ZOOMS ?? "3,4,5,7,9,10")
  .split(",")
  .map((value) => Number.parseInt(value, 10));
if (zooms.some((zoom) => !Number.isInteger(zoom) || zoom < 0 || zoom > 10)) {
  throw new Error(`invalid GEO_PHILLY_VISUAL_ZOOMS: ${process.env.GEO_PHILLY_VISUAL_ZOOMS}`);
}
const artifactDir = fileURLToPath(new URL("../artifacts/visual", import.meta.url));
const port = Number.parseInt(process.env.GEO_PHILLY_VISUAL_PORT ?? "3107", 10);
const tileTimeout = Number.parseInt(process.env.GEO_PHILLY_VISUAL_TIMEOUT ?? "180000", 10);
const settleBudget = Number.parseInt(process.env.GEO_PHILLY_SETTLE_BUDGET_MS ?? "5000", 10);
const origin = `http://127.0.0.1:${port}`;
const server = spawn("target/release/geo-philly", ["serve", "--port", String(port)], {
  cwd: root,
  env: { ...process.env, RUST_LOG: "geo_philly=warn,tower_http=warn" },
  stdio: ["ignore", "pipe", "pipe"],
});
let serverOutput = "";
let tileZoomLimit = 0;
server.stdout.on("data", (chunk) => {
  serverOutput += chunk.toString();
});
server.stderr.on("data", (chunk) => {
  serverOutput += chunk.toString();
});

/** @param {number} milliseconds */
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function waitForServer() {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (server.exitCode !== null) throw new Error(`server exited early:\n${serverOutput}`);
    try {
      const response = await fetch(`${origin}/meta`);
      if (response.ok) return response.json();
    } catch {
      // The listener is still starting.
    }
    await delay(100);
  }
  throw new Error(`server did not become ready:\n${serverOutput}`);
}

/**
 * @param {import("playwright-core").Page} page
 * @param {number} zoom
 * @param {{ name: string, center?: [number, number] }} view
 */
async function capture(page, zoom, view = { name: "city-hall" }) {
  const started = performance.now();
  const errors = [];
  const tileRequests = [];
  page.on("pageerror", (error) => errors.push(`page: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  page.on("response", async (response) => {
    if (response.url().includes("/tiles/")) {
      const headers = await response.allHeaders();
      tileRequests.push({
        status: response.status(),
        cache: headers["x-tile-cache"] ?? "unknown",
        bytes: Number.parseInt(headers["content-length"] ?? "0", 10),
        contentType: headers["content-type"] ?? "unknown",
        cacheControl: headers["cache-control"] ?? "",
      });
    } else if (response.status() >= 400 && !response.url().endsWith("/favicon.ico")) {
      errors.push(`${response.status()} ${response.url()}`);
    }
  });
  const parameters = new URLSearchParams({ z: String(zoom) });
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
  const settledTileRequests = [...tileRequests];
  await page.waitForTimeout(100);
  const screenshot = view.name === "city-hall" ? `z${zoom}.png` : `${view.name}-z${zoom}.png`;
  await page.screenshot({ path: `${artifactDir}/${screenshot}` });
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
      nonGroundRatio: Number((1 - ground / samples).toFixed(4)),
      sampledColors: colors.size,
    };
  });
  if (errors.length > 0) throw new Error(`z${zoom} browser errors:\n${errors.join("\n")}`);
  if (canvas.pending !== 0 || canvas.uncovered !== 0 || canvas.failed !== 0) {
    throw new Error(`z${zoom} has unfinished tiles: ${JSON.stringify(canvas)}`);
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
  const expectedTileZoom = Math.min(zoom, tileZoomLimit);
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
    zoom,
    screenshot,
    performance: {
      domContentLoadedMs: Number(domContentLoadedMs.toFixed(1)),
      firstMapMs: Number(firstMapMs.toFixed(1)),
      settledMs: Number(settledMs.toFixed(1)),
      tileBytes: settledTileRequests.reduce((total, request) => total + request.bytes, 0),
    },
    canvas,
    tileResponses: {
      total: tileRequests.length,
      rendered: tileRequests.filter((request) => request.cache === "rendered").length,
      disk: tileRequests.filter((request) => request.cache === "disk").length,
      empty: tileRequests.filter((request) => request.cache === "empty").length,
    },
  };
}

/**
 * @param {import("playwright-core").Browser} browser
 * @param {Record<string, unknown>} meta
 */
async function interactions(browser, meta) {
  const page = await browser.newPage({ viewport: { width: 390, height: 844 } });
  await page.goto(`${origin}/?z=8`, { waitUntil: "domcontentloaded" });
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
      .filter((button) => !button.hidden)
      .map((button) => {
        const bounds = button.getBoundingClientRect();
        return { id: button.id, width: bounds.width, height: bounds.height };
      }),
  );
  if (controls.some((control) => control.width < 44 || control.height < 44)) {
    throw new Error(`mobile controls are too small: ${JSON.stringify(controls)}`);
  }
  await page.locator("#zoom-in").click();
  await page.waitForSelector('canvas[data-zoom="9"]');
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
  await page.locator("#home").click();
  await page.locator("details").click();
  await page.screenshot({ path: `${artifactDir}/mobile.png` });

  const landmarks = /** @type {{ name: string }[]} */ (meta.landmarks);
  if (!landmarks.some((landmark) => landmark.name === "Rocky")) {
    throw new Error("Rocky landmark is missing");
  }
  return {
    viewport: [390, 844],
    controls,
    keyboardPan: true,
    prefetchedPanMs: Number(prefetchedPanMs.toFixed(1)),
    rocky: true,
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

let browser;
try {
  await mkdir(artifactDir, { recursive: true });
  const meta = await waitForServer();
  tileZoomLimit = meta.max_tile_zoom;
  if (!Number.isInteger(tileZoomLimit) || tileZoomLimit < 0 || tileZoomLimit > meta.max_zoom) {
    throw new Error(`server has an invalid tile zoom limit: ${JSON.stringify(meta)}`);
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
  const rendering = await profile(meta);
  browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH ?? "/usr/bin/chromium",
    headless: true,
    args: ["--no-sandbox", "--disable-gpu"],
  });
  const results = [];
  for (const zoom of zooms) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(await capture(page, zoom));
    await page.close();
  }
  if (process.env.GEO_PHILLY_VISUAL_SECONDARY !== "0") {
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
      { name: "passyunk", center: [984479.11, 311909.39] },
      { name: "stadiums", center: [981156.04, 313684.68] },
      { name: "manayunk", center: [987130.87, 303737.71] },
      { name: "northeast", center: [1006582.35, 307977.81] },
      { name: "west-philly", center: [979452.36, 307652.14] },
    ];
    for (const neighborhood of neighborhoods) {
      const neighborhoodPage = await browser.newPage({ viewport: { width: 1440, height: 960 } });
      results.push(await capture(neighborhoodPage, 8, neighborhood));
      await neighborhoodPage.close();
    }
  }
  const interactionResults = await interactions(browser, meta);
  const gitStatus = execFileSync("git", ["status", "--porcelain"], {
    cwd: root,
    encoding: "utf8",
  }).trim();
  const report = {
    generatedAt: new Date().toISOString(),
    gitSha: execFileSync("git", ["rev-parse", "--short", "HEAD"], {
      cwd: root,
      encoding: "utf8",
    }).trim(),
    gitDirty: gitStatus.length > 0,
    tileVersion: meta.tile_version,
    rendering,
    views: results,
    interactions: interactionResults,
  };
  await writeFile(`${artifactDir}/report.json`, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} catch (error) {
  process.stderr.write(`\nserver output:\n${serverOutput}\n`);
  throw error;
} finally {
  await browser?.close();
  server.kill("SIGTERM");
}
