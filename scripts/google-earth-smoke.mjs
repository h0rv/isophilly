// Test native Earth projection, fine-coverage accounting, and terrain priority.
// Every mesh and image is synthetic; this command makes no Google requests.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright-core";
import proj4 from "proj4";
import sharp from "sharp";
import { projectLonLat } from "../static/city-overlay.js";
import { inverseIsometric, projectStatePlane } from "../tools/google-imagery/projection.js";
import { packEarthTile } from "./google-earth-source.mjs";
import { STATE_PLANE } from "./google-imagery-lib.mjs";

const radius = 6371010;
const center = projectStatePlane(...projectLonLat(-75.23, 39.961), 0);
const bounds = [center[0] - 20, center[1] - 20, center[0] + 20, center[1] + 20];
const sphere = (lon, lat, height) => {
  const a = (lon * Math.PI) / 180,
    b = (lat * Math.PI) / 180;
  return [
    (radius + height) * Math.cos(b) * Math.cos(a),
    (radius + height) * Math.cos(b) * Math.sin(a),
    (radius + height) * Math.sin(b),
  ];
};
const origin = sphere(-75.23, 39.961, 0);
const matrix = new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, ...origin, 1]);
const temporary = await mkdtemp(join(tmpdir(), "earth-render-test-"));
let browser, server;
async function fixture(path, color, height, rectangle, terrain = false) {
  const [l, t, r, b] = rectangle;
  const positions = [];
  for (const [x, y] of [
    [l, t],
    [r, t],
    [r, b],
    [l, b],
  ]) {
    const [lon, lat] = proj4(STATE_PLANE, "EPSG:4326", inverseIsometric(x, y + height));
    const xyz = sphere(lon, lat, height);
    positions.push(...xyz.map((value, index) => value - origin[index]));
  }
  const jpeg = await sharp({ create: { width: 1, height: 1, channels: 3, background: color } })
    .jpeg()
    .toBuffer();
  return packEarthTile(
    {
      path,
      epoch: 1,
      matrixGlobeFromMesh: matrix,
      copyrightIds: [255],
      meshes: [
        {
          positions: new Float32Array(positions),
          uv: new Float32Array([0, 0, 1, 0, 1, 1, 0, 1]),
          indices: new Uint32Array([0, 1, 2, 0, 2, 3]),
          triangleStrip: new Uint32Array([0, 1, 2, 0, 2, 3]),
          layerBounds: new Uint32Array(
            terrain ? [0, 0, 0, 6, 6, 6, 6, 6, 6, 6] : [0, 6, 6, 6, 6, 6, 6, 6, 6, 6],
          ),
          textures: [{ format: 1, width: 1, height: 1, data: [jpeg] }],
        },
      ],
    },
    temporary,
  );
}
try {
  const fine = await fixture("2142", "#ff0000", 0, [
    bounds[0] - 10,
    bounds[1] - 10,
    bounds[2] + 10,
    bounds[3] + 10,
  ]);
  const underlay = await fixture("2143", "#0000ff", 50, [
    bounds[0] - 10,
    bounds[1] - 10,
    bounds[2] + 10,
    bounds[3] + 10,
  ]);
  const half = await fixture("2144", "#ff0000", 0, [
    bounds[0] - 10,
    bounds[1] - 10,
    center[0],
    bounds[3] + 10,
  ]);
  const ground = await fixture(
    "2145",
    "#0000ff",
    0,
    [bounds[0] - 10, bounds[1] - 10, bounds[2] + 10, bounds[3] + 10],
    true,
  );
  const bridge = await fixture("2146", "#888888", 10, [
    center[0],
    bounds[1] - 10,
    bounds[2] + 10,
    bounds[3] + 10,
  ]);
  const road = await fixture(
    "2147",
    "#888888",
    10,
    [center[0], bounds[1] - 10, bounds[2] + 10, bounds[3] + 10],
    true,
  );
  const bundle = await build({
    entryPoints: [
      fileURLToPath(new URL("../tools/google-imagery/earth-capture.js", import.meta.url)),
    ],
    bundle: true,
    format: "esm",
    write: false,
    logLevel: "silent",
  });
  server = createServer(async (request, response) => {
    try {
      if (request.url === "/") {
        response.setHeader("Content-Type", "text/html");
        return response.end(
          '<!doctype html><link rel="icon" href="data:,"><script type="module" src="/capture.js"></script>',
        );
      }
      if (request.url === "/capture.js") {
        response.setHeader("Content-Type", "text/javascript");
        return response.end(bundle.outputFiles[0].contents);
      }
      const file = resolve(temporary, `.${request.url}`);
      if (!file.startsWith(`${temporary}${sep}`)) return response.writeHead(403).end();
      response.setHeader(
        "Content-Type",
        file.endsWith(".jpg")
          ? "image/jpeg"
          : file.endsWith(".json")
            ? "application/json"
            : "application/octet-stream",
      );
      response.end(await readFile(file));
    } catch {
      response.writeHead(404).end();
    }
  });
  await new Promise((accept, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", accept);
  });
  const url = `http://127.0.0.1:${server.address().port}`;
  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.ISOPHILLY_CHROMIUM_PATH ||
      (existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        : undefined),
  });
  const page = await browser.newPage();
  await page.route("**/*", (route) =>
    route.request().url().startsWith(url) ? route.continue() : route.abort(),
  );
  await page.goto(url);
  await page.waitForFunction(() => window.captureReady);
  const settings = {
    bounds,
    width: 64,
    height: 64,
    tileSize: 64,
    heightOrigin: 0,
    angle: 0,
    radius,
    attributions: ["Synthetic fixture, not Google imagery"],
  };
  const render = (tiles) =>
    page.evaluate((input) => window.captureEarthBlock(input), { ...settings, tiles });
  const base = { url: `/${underlay.name}`, coarse: true, level: -82 };
  const red = { url: `/${fine.name}`, coarse: false, level: 18 };
  const result = await render([base, red]);
  assert.equal(result.holes, 0);
  assert.deepEqual(result.finePixels, [4096]);
  const data = Buffer.from(result.raw, "base64");
  const pixel = (x, y, bytes = data) => [...bytes.subarray((y * 64 + x) * 4, (y * 64 + x) * 4 + 4)];
  assert.ok(
    pixel(32, 32)[0] > 240 && pixel(32, 32)[2] < 10,
    "Hidden terrain must not occlude the better visible surface",
  );
  const onlyGround = await render([base]);
  assert.equal(onlyGround.holes, 0);
  assert.deepEqual(onlyGround.finePixels, [0]);
  assert.ok(pixel(32, 32, Buffer.from(onlyGround.raw, "base64"))[2] > 240);
  const partial = await render([base, { ...red, url: `/${half.name}` }]);
  assert.equal(partial.holes, 0);
  assert.ok(partial.finePixels[0] >= 2000 && partial.finePixels[0] <= 2100);
  const mixed = Buffer.from(partial.raw, "base64");
  assert.ok(pixel(8, 32, mixed)[0] > 240);
  assert.ok(pixel(56, 32, mixed)[2] > 240);
  const polygon = ([l, t, r, b]) => ({
    bounds: [l, t, r, b],
    rings: [
      [
        [l, t],
        [r, t],
        [r, b],
        [l, b],
        [l, t],
      ],
    ],
  });
  const allWater = polygon([center[0] - 100, center[1] - 100, center[0] + 100, center[1] + 100]);
  const groundTile = { url: `/${ground.name}`, level: 18, coarse: false };
  const mask = async (tiles, polygons) => {
    const result = await page.evaluate((input) => window.captureEarthBlock(input), {
      ...settings,
      tiles,
      waterPolygons: polygons,
    });
    assert.equal(result.holes, 0);
    return Buffer.from(result.raw, "base64");
  };
  const complete = await mask([groundTile], [allWater]);
  assert.equal(pixel(32, 32, complete)[0], 255);
  for (const raised of [bridge, road]) {
    const selected = await mask(
      [groundTile, { ...groundTile, url: `/${raised.name}` }],
      [allWater],
    );
    assert.equal(
      pixel(8, 32, selected)[0],
      raised === bridge ? 255 : 0,
      "Ambiguous terrain elevations must leave the photograph unchanged",
    );
    assert.equal(
      pixel(56, 32, selected)[0],
      0,
      "Water masks must preserve photographed bridges and raised road decks",
    );
  }
  const top = await mask(
    [groundTile],
    [polygon([center[0] - 100, center[1] - 100, center[0] + 100, center[1]])],
  );
  assert.equal(pixel(32, 8, top)[0], 255);
  assert.equal(pixel(32, 56, top)[0], 0, "The geographic mask must not flip north/south");
  const island = structuredClone(allWater);
  island.rings.push(polygon([center[0] - 5, center[1] - 5, center[0] + 5, center[1] + 5]).rings[0]);
  const islands = await mask([groundTile], [island]);
  assert.equal(pixel(32, 32, islands)[0], 0, "Polygon holes must preserve islands");
  assert.equal(pixel(8, 32, islands)[0], 255);
  console.log(
    "Native Earth smoke passed: sphere projection, fine counts, terrain priority, geographic water masks, islands, and bridge protection. No Google requests.",
  );
} finally {
  try {
    await browser?.close();
  } finally {
    if (server) {
      server.closeAllConnections();
      await new Promise((accept) => server.close(accept));
    }
    await rm(temporary, { recursive: true, force: true });
  }
}
