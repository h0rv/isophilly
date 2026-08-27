import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = fileURLToPath(new URL("..", import.meta.url));
const artifactDir = fileURLToPath(new URL("../artifacts/visual", import.meta.url));
const port = Number.parseInt(process.env.GEO_PHILLY_VISUAL_PORT ?? "3107", 10);
const origin = `http://127.0.0.1:${port}`;
const server = spawn("target/release/geo-philly", ["serve", "--port", String(port)], {
  cwd: root,
  env: { ...process.env, RUST_LOG: "geo_philly=warn,tower_http=warn" },
  stdio: ["ignore", "pipe", "pipe"],
});
let serverOutput = "";
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

/** @param {import("playwright-core").Page} page @param {number} zoom */
async function capture(page, zoom) {
  const errors = [];
  const tileRequests = [];
  page.on("pageerror", (error) => errors.push(`page: ${error.message}`));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(`console: ${message.text()}`);
  });
  page.on("response", async (response) => {
    if (response.url().includes("/tiles/")) {
      tileRequests.push({
        status: response.status(),
        cache: (await response.allHeaders())["x-tile-cache"] ?? "unknown",
      });
    } else if (response.status() >= 400 && !response.url().endsWith("/favicon.ico")) {
      errors.push(`${response.status()} ${response.url()}`);
    }
  });
  await page.goto(`${origin}/?z=${zoom}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(`canvas[data-zoom="${zoom}"]`);
  await page.waitForFunction(
    () => {
      const canvas = document.querySelector("#map");
      return canvas instanceof HTMLCanvasElement && canvas.dataset.pending === "0";
    },
    undefined,
    { timeout: 60_000 },
  );
  await page.waitForTimeout(100);
  const screenshot = `z${zoom}.png`;
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
      pending: Number(element.dataset.pending),
      uncovered: Number(element.dataset.uncovered),
      nonGroundRatio: Number((1 - ground / samples).toFixed(4)),
      sampledColors: colors.size,
    };
  });
  if (errors.length > 0) throw new Error(`z${zoom} browser errors:\n${errors.join("\n")}`);
  if (canvas.pending !== 0 || canvas.uncovered !== 0) {
    throw new Error(`z${zoom} has unfinished tiles: ${JSON.stringify(canvas)}`);
  }
  if (canvas.sampledColors < 8 || canvas.nonGroundRatio < 0.01) {
    throw new Error(`z${zoom} appears blank: ${JSON.stringify(canvas)}`);
  }
  return {
    zoom,
    screenshot,
    canvas,
    tileResponses: {
      total: tileRequests.length,
      rendered: tileRequests.filter((request) => request.cache === "rendered").length,
      disk: tileRequests.filter((request) => request.cache === "disk").length,
      volatile: tileRequests.filter((request) => request.cache === "volatile").length,
      empty: tileRequests.filter((request) => request.cache === "empty").length,
    },
  };
}

/** @param {Record<string, unknown>} meta */
async function profile(meta) {
  const bounds = /** @type {number[]} */ (meta.iso_bounds);
  const hall = /** @type {number[]} */ (meta.city_hall);
  const zoom = Math.min(8, /** @type {number} */ (meta.max_zoom));
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  const count = 2 ** zoom;
  const x = Math.floor(((hall[0] - bounds[0]) / side) * count);
  const y = Math.floor(((hall[1] - bounds[1]) / side) * count);
  const url = `${origin}/tiles/${zoom}/${x}/${y}.png?v=${meta.tile_version}`;
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
  const tileCoordinates = (level) => ({
    x: Math.floor(((hall[0] - bounds[0]) / side) * 2 ** level),
    y: Math.floor(((hall[1] - bounds[1]) / side) * 2 ** level),
  });
  const deepZoom = Math.min(9, /** @type {number} */ (meta.max_zoom));
  const deep = tileCoordinates(deepZoom);
  const deepResponse = await fetch(
    `${origin}/tiles/${deepZoom}/${deep.x}/${deep.y}.png?v=${meta.tile_version}`,
  );
  const emptyZoom = /** @type {number} */ (meta.max_zoom);
  const emptyResponse = await fetch(`${origin}/tiles/${emptyZoom}/0/0.png?v=${meta.tile_version}`);
  const policy = {
    deep: deepResponse.headers.get("x-tile-cache"),
    empty: emptyResponse.headers.get("x-tile-cache"),
  };
  await Promise.all([deepResponse.arrayBuffer(), emptyResponse.arrayBuffer()]);
  if (policy.deep !== "volatile" || policy.empty !== "empty") {
    throw new Error(`tile cache policy failed: ${JSON.stringify(policy)}`);
  }
  return { zoom, x, y, first, second, policy };
}

let browser;
try {
  await mkdir(artifactDir, { recursive: true });
  const meta = await waitForServer();
  const rendering = await profile(meta);
  browser = await chromium.launch({
    executablePath: process.env.CHROMIUM_PATH ?? "/usr/bin/chromium",
    headless: true,
    args: ["--no-sandbox", "--disable-gpu"],
  });
  const results = [];
  for (const zoom of [3, 4, 5, 7, 9]) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    results.push(await capture(page, zoom));
    await page.close();
  }
  const report = {
    generatedAt: new Date().toISOString(),
    rendering,
    views: results,
  };
  await writeFile(`${artifactDir}/report.json`, `${JSON.stringify(report, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
} finally {
  await browser?.close();
  server.kill("SIGTERM");
}
