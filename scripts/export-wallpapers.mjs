import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";
import sharp from "sharp";
import { digest } from "./google-earth-source.mjs";
import { childHasExited, stopChild } from "./visual-check-lib.mjs";

const { values } = parseArgs({
  options: {
    root: { type: "string", default: "data/google-earth-preview-perspective" },
    output: { type: "string", default: "data/wallpapers" },
    port: { type: "string", default: "3113" },
    sizes: { type: "string" },
  },
});
const port = Number(values.port);
assert.ok(Number.isInteger(port) && port >= 1024 && port <= 65535, "Invalid export port");
const root = resolve(values.root);
const site = await realpath(join(root, "site"));
const metaBytes = await readFile(join(site, "meta"));
const meta = JSON.parse(metaBytes);
const build = JSON.parse(await readFile(join(site, "build.json"), "utf8"));
const plan = JSON.parse(await readFile(join(root, "plan.json"), "utf8"));
assert.equal(build.complete, true, "Use a complete atlas");
assert.equal(meta.raw_colors, true, "Wallpapers must preserve the original colors");
assert.ok(
  Array.isArray(meta.city_hall) && meta.city_hall.length === 2,
  "Missing framing reference",
);
assert.ok(meta.imagery_attributions?.length > 0, "Missing imagery credits");
const output = resolve(values.output);
try {
  await stat(output);
  throw new Error(`Output already exists: ${output}. Choose a new --output to preserve it.`);
} catch (error) {
  if (error.code !== "ENOENT") throw error;
}
const presets = [
  { width: 1920, height: 1080, label: "Full HD" },
  { width: 2560, height: 1440, label: "1440p" },
  { width: 3840, height: 2160, label: "4K" },
  { width: 5120, height: 2880, label: "5K" },
  { width: 2880, height: 1800, label: "Laptop · 16:10" },
  { width: 3440, height: 1440, label: "Ultrawide" },
  { width: 5120, height: 1440, label: "Super ultrawide" },
  { width: 1440, height: 3120, label: "Phone" },
];
const selectedSizes = values.sizes?.split(",");
assert.ok(
  !selectedSizes ||
    selectedSizes.every((size) => presets.some((p) => `${p.width}x${p.height}` === size)),
  "Choose supported --sizes, e.g. 1920x1080,3840x2160",
);
const sizes = presets.filter(
  (p) => !selectedSizes || selectedSizes.includes(`${p.width}x${p.height}`),
);
const compositions = [
  { id: "skyline", title: "Center City", zoom: 7, x: -90, y: -30 },
  { id: "city", title: "Wider city view", zoom: 6, x: 0, y: -100 },
];
const escapeText = (text) =>
  String(text).replace(
    /[&<>"']/g,
    (c) =>
      ({
        "&": "&amp;",
        "<": "&lt;",
        ">": "&gt;",
        '"': "&quot;",
        "'": "&#39;",
      })[c],
  );
const credit = meta.imagery_attributions.join(" · ");
await mkdir(dirname(output), { recursive: true });
const stage = await mkdtemp(join(dirname(output), ".wallpapers-export-"));
const exported = [];
const server = spawn(
  process.execPath,
  ["scripts/google-earth.mjs", "serve", "--output", site, "--port", `${port}`],
  {
    stdio: ["ignore", "pipe", "pipe"],
  },
);
let browser;
let published = false;
try {
  await new Promise((accept, reject) => {
    let log = "";
    const timeout = setTimeout(
      () => reject(new Error(`Export server startup timed out: ${log}`)),
      15000,
    );
    const done = (error) => {
      clearTimeout(timeout);
      if (error) reject(error);
      else accept();
    };
    server.once("error", done);
    server.once("exit", (code) => done(new Error(`Export server exited ${code}: ${log}`)));
    server.stderr.on("data", (bytes) => {
      log = (log + bytes).slice(-4096);
    });
    server.stdout.on("data", (bytes) => {
      if (String(bytes).includes("Serving ")) done();
    });
  });
  const origin = `http://127.0.0.1:${port}`;
  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.ISOPHILLY_CHROMIUM_PATH ??
      (process.platform === "darwin"
        ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        : undefined),
  });
  for (const composition of compositions) {
    for (const size of sizes) {
      const context = await browser.newContext({
        viewport: { width: size.width, height: size.height },
        deviceScaleFactor: 1,
      });
      try {
        // Only the export's metadata response is changed. No files or live map
        // are modified. Null removes the permanent City Hall label and dot.
        await context.route("**/*", async (route) => {
          const url = new URL(route.request().url());
          if (url.origin !== origin) return route.abort("blockedbyclient");
          if (url.pathname === "/meta")
            return route.fulfill({
              status: 200,
              contentType: "application/json",
              body: JSON.stringify({ ...meta, city_hall: null, landmarks: [] }),
            });
          return route.continue();
        });
        const page = await context.newPage();
        const failures = [];
        page.on("pageerror", (error) => failures.push(error.message));
        page.on("response", (response) => {
          if (new URL(response.url()).pathname.startsWith("/tiles/") && !response.ok()) {
            failures.push(`Tile HTTP ${response.status()}`);
          }
        });
        const query = new URLSearchParams({
          z: `${composition.zoom}`,
          cx: `${meta.city_hall[0] + composition.x}`,
          cy: `${meta.city_hall[1] + composition.y}`,
        });
        await page.goto(`${origin}/?${query}`, { waitUntil: "domcontentloaded" });
        await page.locator("#neighborhoods-toggle").click();
        await page.addStyleTag({
          content:
            "body > :not(#map) { display: none !important; } #map { outline: none !important; }",
        });
        await page.waitForFunction(
          () => {
            const d = document.querySelector("#map")?.dataset;
            return (
              d &&
              d.names === "false" &&
              d.borders === "false" &&
              Number(d.requested) > 0 &&
              d.pending === "0" &&
              d.failed === "0" &&
              d.uncovered === "0"
            );
          },
          null,
          { timeout: 90000 },
        );
        assert.deepEqual(failures, [], "Export must not contain failed imagery");
        const state = await page.locator("#map").evaluate((canvas) => ({ ...canvas.dataset }));
        const screenshot = await page.screenshot({ animations: "disabled" });
        const font = Math.max(12, Math.round(Math.min(size.width, size.height) / 80));
        const inset = Math.round(font * 1.2);
        const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${size.width}" height="${size.height}"><text x="${size.width - inset}" y="${size.height - inset}" text-anchor="end" font-family="Arial,sans-serif" font-size="${font}" fill="white" stroke="#202020" stroke-opacity="0.8" stroke-width="3" paint-order="stroke">${escapeText(credit)}</text></svg>`;
        const png = await sharp(screenshot)
          .composite([{ input: Buffer.from(svg) }])
          .png({ compressionLevel: 9 })
          .toBuffer();
        const imageMeta = await sharp(png).metadata();
        assert.equal(imageMeta.width, size.width);
        assert.equal(imageMeta.height, size.height);
        const filename = `isophilly-${composition.id}-${size.width}x${size.height}`;
        const jpeg = await sharp(png)
          .jpeg({ quality: 95, chromaSubsampling: "4:4:4", mozjpeg: true })
          .toBuffer();
        await writeFile(join(stage, `${filename}.png`), png);
        await writeFile(join(stage, `${filename}.jpg`), jpeg);
        if (size === sizes[0])
          await sharp(png)
            .resize({ width: 1200 })
            .webp({ quality: 90 })
            .toFile(join(stage, `${composition.id}-preview.webp`));
        exported.push({
          composition: composition.id,
          ...size,
          png: `${filename}.png`,
          jpg: `${filename}.jpg`,
          png_bytes: png.length,
          jpg_bytes: jpeg.length,
          png_sha256: digest(png),
          jpg_sha256: digest(jpeg),
          camera: { x: Number(state.cameraX), y: Number(state.cameraY), zoom: Number(state.zoom) },
          tile_zoom: Number(state.tileZoom),
        });
        console.log(`${composition.title}: ${size.width}×${size.height} PNG and JPG`);
      } finally {
        await context.close();
      }
    }
  }
  const mb = (bytes) => `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  const cards = compositions
    .map(
      (composition) =>
        `<section><h2>${escapeText(composition.title)}</h2><img src="${composition.id}-preview.webp" width="1200" height="675" alt="Philadelphia ${escapeText(composition.title.toLowerCase())}, without labels" loading="lazy"><table><thead><tr><th>Screen</th><th>Size</th><th>Download</th></tr></thead><tbody>${exported
          .filter((image) => image.composition === composition.id)
          .map(
            (image) =>
              `<tr><td>${escapeText(image.label)}</td><td>${image.width} × ${image.height}</td><td><a href="${image.jpg}" download>JPG <small>${mb(image.jpg_bytes)}</small></a> <a href="${image.png}" download>PNG <small>${mb(image.png_bytes)}</small></a></td></tr>`,
          )
          .join("")}</tbody></table></section>`,
    )
    .join("\n");
  await writeFile(
    join(stage, "index.html"),
    `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>IsoPhilly wallpapers</title><style>
*{box-sizing:border-box}body{margin:0;background:#f3efe7;color:#27251f;font:16px/1.5 system-ui,sans-serif}main{max-width:1120px;margin:auto;padding:40px 20px}h1{font-size:clamp(30px,5vw,50px);letter-spacing:-.04em;margin:12px 0}h2{font-size:22px}p{max-width:740px;color:#625d51}section{margin-top:44px}img{display:block;width:100%;height:auto;border-radius:8px}table{width:100%;border-collapse:collapse;margin-top:16px;text-align:left}th,td{padding:12px 8px;border-bottom:1px solid #d9d2c6}a{color:#315747;text-underline-offset:4px}td a{display:inline-block;padding:6px 8px}small{font-size:12px;color:#756e60}footer{margin-top:48px;font-size:13px;color:#756e60}@media(max-width:520px){main{padding:24px 12px}th,td{padding:10px 3px;font-size:13px}td a{display:block;padding:8px 3px}}
</style></head><body><main><a href="https://isophilly.horv.co">Open the map</a><h1>Philadelphia, for your desktop.</h1><p>These wallpapers use the map's original imagery and camera, without labels or controls. Choose a screen size below. PNG is lossless. JPG is a smaller download.</p>${cards}<footer>Imagery: ${escapeText(credit)}. Rendered with IsoPhilly. A small source credit is included in each image. No new source downloads or AI image processing were used.</footer></main></body></html>\n`,
  );
  await writeFile(
    join(stage, "manifest.json"),
    `${JSON.stringify(
      {
        schema_version: 1,
        tile_version: meta.tile_version,
        source_level: plan.source_level,
        meta_sha256: digest(metaBytes),
        projection: meta.projection,
        imagery_attributions: meta.imagery_attributions,
        source_colors: true,
        labels: false,
        controls: false,
        images: exported,
      },
      null,
      2,
    )}\n`,
  );
  await rename(stage, output);
  published = true;
  console.log(`Wallpapers and download page: ${output}/index.html`);
} finally {
  try {
    await browser?.close();
  } finally {
    try {
      await stopChild(server, (timeout) => {
        if (childHasExited(server)) return Promise.resolve(true);
        return new Promise((accept) => {
          const done = () => {
            clearTimeout(timer);
            server.off("exit", done);
            accept(true);
          };
          const timer = setTimeout(() => {
            server.off("exit", done);
            accept(false);
          }, timeout);
          server.once("exit", done);
          if (childHasExited(server)) done();
        });
      });
    } finally {
      if (!published) await rm(stage, { recursive: true, force: true });
    }
  }
}
