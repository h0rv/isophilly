// Offline browser test of the actual capture renderer. It never contacts Google or needs a key.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright-core";
import { isometricLonLat } from "../static/city-overlay.js";
import { geodeticToEcef } from "../tools/google-imagery/projection.js";
import { validateCaptureResult } from "./google-imagery-lib.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const directory = await mkdtemp(join(tmpdir(), "isophilly-google-smoke-"));
let browser;
let server;
try {
  const bundled = await build({
    entryPoints: [join(root, "tools/google-imagery/capture.js")],
    bundle: true,
    format: "esm",
    write: false,
    logLevel: "silent",
  });
  const center = geodeticToEcef(-75.23, 39.96, 0);
  const lon = (-75.23 * Math.PI) / 180;
  const lat = (39.96 * Math.PI) / 180;
  const east = [-Math.sin(lon), Math.cos(lon), 0];
  const north = [-Math.sin(lat) * Math.cos(lon), -Math.sin(lat) * Math.sin(lon), Math.cos(lat)];
  const up = [Math.cos(lat) * Math.cos(lon), Math.cos(lat) * Math.sin(lon), Math.sin(lat)];
  const transform = [...east, 0, ...north, 0, ...up, 0, ...center, 1];
  const positions = new Float32Array([
    -2000, -2000, 0, 2000, -2000, 0, 2000, 2000, 0, -2000, -2000, 0, 2000, 2000, 0, -2000, 2000, 0,
  ]);
  const buffer = Buffer.from(positions.buffer);
  const gltf = {
    asset: { version: "2.0", copyright: "Synthetic test fixture, not Google imagery" },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0 }],
    buffers: [
      {
        uri: `data:application/octet-stream;base64,${buffer.toString("base64")}`,
        byteLength: buffer.length,
      },
    ],
    bufferViews: [{ buffer: 0, byteOffset: 0, byteLength: buffer.length }],
    accessors: [
      {
        bufferView: 0,
        componentType: 5126,
        count: 6,
        type: "VEC3",
        min: [-2000, -2000, 0],
        max: [2000, 2000, 0],
      },
    ],
    meshes: [{ primitives: [{ attributes: { POSITION: 0 }, material: 0 }] }],
    materials: [
      { pbrMetallicRoughness: { baseColorFactor: [0.3, 0.5, 0.2, 1] }, doubleSided: true },
    ],
  };
  const tileset = {
    asset: { version: "1.0", gltfUpAxis: "Z" },
    session: "synthetic",
    geometricError: 0,
    root: {
      transform,
      boundingVolume: { sphere: [0, 0, 0, 3000] },
      geometricError: 0,
      refine: "REPLACE",
      content: { uri: "fixture.gltf" },
    },
  };
  server = createServer((request, response) => {
    const path = request.url.split("?")[0].replace(/^\/+/, "/");
    response.setHeader("Cache-Control", "no-store");
    if (path === "/") {
      response.setHeader("Content-Type", "text/html");
      response.end(
        '<!doctype html><html><head><link rel="icon" href="data:,"></head><body><script type="module" src="/capture.js"></script></body></html>',
      );
    } else if (path === "/capture.js") {
      response.setHeader("Content-Type", "text/javascript");
      response.end(bundled.outputFiles[0].contents);
    } else if (path === "/root.json" || path === "/fixture.gltf") {
      response.setHeader("Content-Type", "application/json");
      response.end(JSON.stringify(path === "/root.json" ? tileset : gltf));
    } else {
      response.writeHead(404).end();
    }
  });
  await new Promise((accept, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", accept);
  });
  const origin = `http://127.0.0.1:${server.address().port}`;
  const executablePath =
    process.env.ISOPHILLY_CHROMIUM_PATH ||
    (existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
      ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      : undefined);
  browser = await chromium.launch({ executablePath, headless: true });
  const context = await browser.newContext();
  const requests = [];
  await context.route("**/*", async (route) => {
    const url = route.request().url();
    requests.push(url);
    if (url.startsWith(origin) || url.startsWith("data:")) await route.continue();
    else await route.abort();
  });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") errors.push(message.text());
  });
  await page.goto(origin);
  await page
    .waitForFunction(() => window.captureReady, null, { timeout: 10000 })
    .catch(() => {
      throw new Error(`Capture bundle did not initialize: ${errors.join("; ")}`);
    });
  const [x, y] = isometricLonLat(-75.23, 39.96);
  const settings = {
    apiKey: "synthetic-not-a-key",
    tilesetUrl: `${origin}/root.json`,
    bounds: [x - 150, y - 150, x + 150, y + 150],
    heightOrigin: 0,
    timeoutMs: 15000,
  };
  const result = await page
    .evaluate((input) => window.captureGoogleTile(input), settings)
    .catch((error) => {
      throw new Error(
        `${error.message}\nBrowser diagnostics: ${errors.join("; ")}\nRequests: ${requests.join("; ")}`,
      );
    });
  validateCaptureResult(result);
  assert.equal(result.opaquePixels, 65536);
  assert.ok(result.attributions.includes("Synthetic test fixture, not Google imagery"));
  assert.deepEqual(errors, []);
  assert.ok(requests.every((url) => !url.includes("googleapis.com")));
  // A second capture verifies decoder/renderer disposal does not leave a broken browser session.
  await page.goto(origin);
  await page.waitForFunction(() => window.captureReady);
  const again = await page.evaluate((input) => window.captureGoogleTile(input), settings);
  validateCaptureResult(again);
  assert.equal(result.png, again.png);
  await writeFile(join(directory, "fixture.png"), Buffer.from(result.png, "base64"));
  const png = await readFile(join(directory, "fixture.png"));
  assert.equal(png.readUInt32BE(16), 256);
  assert.equal(png[25], 6, "capture PNG must be RGBA for the Rust importer");
  console.log(
    "Offline Google capture smoke passed: exact projection, full coverage, provider attribution, deterministic pixels, and clean second capture. No Google requests.",
  );
} finally {
  try {
    await browser?.close();
  } finally {
    if (server) {
      server.closeAllConnections();
      await new Promise((accept) => server.close(accept));
    }
    await rm(directory, { recursive: true, force: true });
  }
}
