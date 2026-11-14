import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, realpath, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { chromium } from "playwright-core";
import sharp from "sharp";
import { isometricLonLat } from "../static/city-overlay.js";
import { earthCoverageScope } from "./google-earth-lib.mjs";
import { digest, writeJson } from "./google-earth-source.mjs";
import { childHasExited, stopChild } from "./visual-check-lib.mjs";

const { values } = parseArgs({
  options: {
    root: { type: "string", default: "data/google-earth" },
    site: { type: "string" },
    baseline: { type: "string", default: "https://isophilly.horv.co" },
    targets: { type: "string" },
    zooms: { type: "string" },
    width: { type: "string", default: "1280" },
    height: { type: "string", default: "900" },
    dpr: { type: "string", default: "1" },
    time: { type: "string", default: "2026-06-21T16:00:00Z" },
    neighborhoods: { type: "boolean", default: false },
    "local-areas": { type: "boolean", default: false },
    port: { type: "string", default: "3119" },
    "allow-partial": { type: "boolean", default: false },
  },
});
const root = resolve(values.root);
const visualRoot = join(root, "visual");
const site = await realpath(resolve(values.site ?? join(root, "site")));
const width = Number(values.width),
  height = Number(values.height),
  port = Number(values.port);
const dpr = Number(values.dpr);
const runId = `${new Date().toISOString().replace(/[:.]/g, "-")}-${width}x${height}-dpr${dpr}-${process.pid}`;
const output = join(visualRoot, "runs", runId);
const zooms = (values.zooms ?? "3,5,7,8,10").split(",").map(Number);
if (
  ![width, height, port].every(Number.isInteger) ||
  width < 390 ||
  height < 300 ||
  ![1, 2, 3].includes(dpr) ||
  !zooms.length ||
  !zooms.every((z) => Number.isInteger(z) && z >= 0 && z <= 10)
)
  throw new Error("Invalid visual-audit dimensions or zooms.");
if (!Number.isFinite(Date.parse(values.time)) || !values.time.endsWith("Z"))
  throw new Error("Use an ISO UTC lighting time ending in Z.");
const build = JSON.parse(await readFile(join(site, "build.json"), "utf8"));
if (!build.complete && !values["allow-partial"])
  throw new Error(
    "Visual release gate requires a complete atlas. Use --allow-partial only for a clearly labelled pilot.",
  );
const atlasMetaBytes = await readFile(join(site, "meta"));
const atlasMeta = JSON.parse(atlasMetaBytes);
const plan = JSON.parse(await readFile(join(root, "plan.json"), "utf8"));
const referenceCoverage = JSON.parse(await readFile(join(root, "reference/coverage.json"), "utf8"));
const referenceMeta = JSON.parse(await readFile(join(root, "reference/meta.json"), "utf8"));
const { scope } = earthCoverageScope(plan, referenceMeta, referenceCoverage);
const neighborhoods = JSON.parse(await readFile("static/neighborhoods.json", "utf8"));
const area = (name) => {
  const match = neighborhoods.features.find(
    (f) => f.name === name && f.kind === "planning_neighborhood",
  );
  assert.ok(match, `Missing QA neighborhood ${name}`);
  return isometricLonLat(...match.label);
};
const [minX, minY, maxX, maxY] = plan.source_meta.iso_bounds;
const targets = [
  { id: "overview", center: [(minX + maxX) / 2, (minY + maxY) / 2] },
  { id: "city-hall", center: plan.source_meta.city_hall },
  { id: "rocky", center: plan.source_meta.landmarks.find((l) => l.name === "Rocky").point },
  { id: "west", center: isometricLonLat(-75.23, 39.961) },
  { id: "far-northeast", center: area("Somerton") },
  { id: "north", center: area("Hunting Park") },
  { id: "southwest", center: area("Eastwick") },
  { id: "south", center: area("Whitman") },
  { id: "rittenhouse", center: [985167.68, 310418.65] },
  { id: "stadiums", center: [981156.04, 313684.68] },
];
const requested = values.targets?.split(",");
if (requested?.some((id) => !targets.some((target) => target.id === id)))
  throw new Error(`Unknown visual target. Choose: ${targets.map((target) => target.id).join(",")}`);
const selected = requested ? targets.filter((t) => requested.includes(t.id)) : targets;
if (!selected.length) throw new Error("No requested visual targets matched.");
await mkdir(output, { recursive: true });
const server = spawn(
  process.execPath,
  ["scripts/google-earth.mjs", "serve", "--output", site, "--port", `${port}`],
  { stdio: ["ignore", "pipe", "pipe"] },
);
let serverErrors = "";
server.stderr.on("data", (bytes) => {
  serverErrors += bytes;
});
let browser;
const report = {
  schema_version: 1,
  complete_atlas: build.complete,
  atlas_tile_version: atlasMeta.tile_version,
  atlas_meta_sha256: digest(atlasMetaBytes),
  water_style: atlasMeta.water_style ?? "source",
  scope,
  viewport: { width, height, dpr },
  lighting_time: values.time,
  overlays: { neighborhoods: values.neighborhoods, local_areas: values["local-areas"] },
  plan_sha256: build.plan_sha256,
  started_at: new Date().toISOString(),
  captures: [],
};
try {
  await new Promise((accept, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`Preview server did not start: ${serverErrors}`)),
      15000,
    );
    server.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    server.once("exit", () => {
      clearTimeout(timer);
      reject(new Error(serverErrors || "Preview server exited early"));
    });
    server.stdout.on("data", (bytes) => {
      if (String(bytes).includes("Serving ")) {
        clearTimeout(timer);
        accept();
      }
    });
  });
  browser = await chromium.launch({
    headless: true,
    executablePath:
      process.env.ISOPHILLY_CHROMIUM_PATH ||
      (existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        : undefined),
  });
  for (const target of selected)
    for (const zoom of target.id === "overview" && !values.zooms ? [2] : zooms) {
      const pair = { target: target.id, zoom, center: target.center, images: [] };
      for (const [label, origin] of [
        ["baseline", values.baseline],
        ["google", `http://127.0.0.1:${port}`],
      ]) {
        const page = await browser.newPage({ viewport: { width, height }, deviceScaleFactor: dpr });
        const failures = [];
        page.on("pageerror", (error) => failures.push(error.message));
        page.on("response", (response) => {
          if (/\/tiles\//.test(response.url()) && !response.ok())
            failures.push(`Tile HTTP ${response.status()}`);
        });
        const query = new URLSearchParams({
          z: `${zoom}`,
          cx: `${target.center[0]}`,
          cy: `${target.center[1]}`,
          time: values.time,
        });
        const url = `${origin}/?${query}`;
        try {
          await page.goto(url, { waitUntil: "domcontentloaded", timeout: 60000 });
          await page.waitForFunction(
            () => {
              const c = document.querySelector("#map");
              return (
                c?.dataset.requested &&
                Number(c.dataset.requested) > 0 &&
                c.dataset.pending === "0" &&
                c.dataset.failed === "0" &&
                c.dataset.uncovered === "0"
              );
            },
            null,
            { timeout: 90000 },
          );
          assert.deepEqual(failures, [], `${label} ${target.id} z${zoom}`);
          for (const [enabled, selector] of [
            [values.neighborhoods, "#neighborhoods-toggle"],
            [values["local-areas"], "#local-areas-toggle"],
          ]) {
            if (enabled) {
              if ((await page.locator(selector).getAttribute("aria-pressed")) !== "true")
                await page.locator(selector).click();
              assert.equal(await page.locator(selector).getAttribute("aria-pressed"), "true");
            }
          }
          await page.evaluate(
            () =>
              new Promise((accept) => requestAnimationFrame(() => requestAnimationFrame(accept))),
          );
          const state = await page.locator("#map").evaluate((canvas) => ({ ...canvas.dataset }));
          const filename = `${target.id}-z${zoom}-${label}.png`;
          const screenshot = await page.screenshot({ path: join(output, filename) });
          const stats = await sharp(screenshot).stats();
          // A whole-city view deliberately includes much more background than a
          // neighborhood close-up. The same tile-settlement gate still applies.
          assert.ok(
            stats.entropy > (target.id === "overview" ? 0.5 : 1.5),
            `Nearly empty screenshot ${filename}`,
          );
          const creditElement = page.locator("#google-imagery-credit");
          const credit = (await creditElement.count()) ? await creditElement.textContent() : "";
          if (label === "google")
            assert.match(credit, /Google Earth/, "Google source attribution must be visible");
          pair.images.push({
            label,
            file: filename,
            sha256: digest(screenshot),
            bytes: screenshot.length,
            entropy: stats.entropy,
            state,
            credit,
          });
        } finally {
          await page.close();
        }
      }
      report.captures.push(pair);
      await writeFile(
        join(output, "report.in-progress.json"),
        `${JSON.stringify(report, null, 2)}\n`,
      );
      console.log(`Captured matching ${target.id} z${zoom} baseline/Google views.`);
    }
  report.finished_at = new Date().toISOString();
  await writeFile(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  const rows = report.captures
    .map(
      (pair) =>
        `<section><h2>${pair.target}, zoom ${pair.zoom}</h2><div>${pair.images.map((image) => `<figure><figcaption>${image.label}</figcaption><img loading="lazy" src="${image.file}"></figure>`).join("")}</div></section>`,
    )
    .join("\n");
  await writeFile(
    join(output, "index.html"),
    `<!doctype html><meta charset="utf-8"><title>IsoPhilly Google imagery review</title><style>body{background:#191714;color:#f6f0e6;font:16px system-ui;margin:20px}section>div{display:flex;gap:12px}figure{margin:0;width:50%}img{width:100%;image-rendering:pixelated}h2{font-size:18px}</style><h1>Baseline and Google imagery</h1><p>Matching camera, zoom, viewport and lighting. This is a ${build.complete ? "complete" : "partial"} ${scope} capture. Automated capture checks are not a substitute for reviewing the images.</p>${rows}`,
  );
  await writeJson(join(visualRoot, "current.json"), {
    run: runId,
    report: `runs/${runId}/report.json`,
    sha256: digest(await readFile(join(output, "report.json"))),
  });
  await writeFile(
    join(visualRoot, "index.html"),
    `<!doctype html><meta charset="utf-8"><title>Latest visual review</title><a href="runs/${runId}/index.html">Latest baseline and Google comparison (${scope}, ${width} by ${height}, DPR ${dpr})</a>`,
  );
  console.log(`Visual evidence saved to ${output}/index.html`);
} finally {
  try {
    await browser?.close();
  } finally {
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
  }
}
