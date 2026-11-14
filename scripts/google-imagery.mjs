import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, open, readFile, rename, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { build } from "esbuild";
import { chromium } from "playwright-core";
import {
  selectTiles,
  sha256,
  tileBounds,
  validateCaptureResult,
  validateManifest,
  validatePlan,
} from "./google-imagery-lib.mjs";

const root = fileURLToPath(new URL("..", import.meta.url));
const help = `Google 3D imagery capture for IsoPhilly

  npm run google:plan -- --center=-75.23,39.96 --radius=350 --limit=4 --height-origin=0
  npm run google:capture
  uv run --locked poe prebuild --google-imagery data/google-imagery

Options:
  --output PATH        Capture workspace (default data/google-imagery)
  --center LON,LAT     Neighborhood center; existing photographed coverage is excluded
  --radius METRES      Ground selection radius (default 350)
  --limit N            Maximum planned/new captures per invocation (default 4)
  --height-origin M    Fixed WGS84 ellipsoid height subtracted across this entire capture set
  --timeout MS         Per-capture timeout (default 120000)
  --browser PATH       Chromium executable (default installed Google Chrome)

Planning is local only. Capture requires GOOGLE_MAPS_API_KEY and an enabled, billed Map Tiles API.
A .env file is read locally if present. Keys are never saved in plans or imagery manifests.
Height origin defaults to zero ellipsoid metres, not surveyed City ground. Inspect a pilot and
set a reviewed origin before expanding. Use a new output directory when settings change.
`;

const { values, positionals } = parseArgs({
  allowPositionals: true,
  options: {
    output: { type: "string", default: "data/google-imagery" },
    center: { type: "string" },
    radius: { type: "string", default: "350" },
    limit: { type: "string", default: "4" },
    "height-origin": { type: "string", default: "0" },
    timeout: { type: "string", default: "120000" },
    browser: { type: "string" },
    help: { type: "boolean" },
  },
});

function boundedNumber(value, name, min, max, integer = false) {
  const number = Number(value);
  if (
    !value?.trim() ||
    !Number.isFinite(number) ||
    number < min ||
    number > max ||
    (integer && !Number.isInteger(number))
  ) {
    throw new Error(
      `${name} must be ${integer ? "an integer" : "a number"} from ${min} to ${max}.`,
    );
  }
  return number;
}

async function atomicWrite(path, data) {
  await mkdir(dirname(path), { recursive: true });
  const part = `${path}.${process.pid}.part`;
  await writeFile(part, data);
  await rename(part, path);
}

async function writeJson(path, value) {
  await atomicWrite(path, `${JSON.stringify(value, null, 2)}\n`);
}

async function makePlan(output, limit) {
  const cargo = existsSync(join(homedir(), ".cargo/bin/cargo"))
    ? join(homedir(), ".cargo/bin/cargo")
    : "cargo";
  const result = spawnSync(cargo, ["run", "--release", "--locked", "--", "google-plan"], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.error || result.status !== 0)
    throw new Error("Cannot plan captures. Run ingest first and check the Rust toolchain.");
  const eligible = validatePlan(JSON.parse(result.stdout));
  const center = values.center === undefined ? null : values.center.split(",").map(Number);
  // Citywide planning is deliberate. Never infer authorization for an unbounded paid capture.
  if (!center && limit <= 4)
    throw new Error(
      "Choose --center=longitude,latitude for the pilot, or an explicit --limit above 4 for wider coverage.",
    );
  const radius = boundedNumber(values.radius, "radius", 1, 50000);
  const selected = selectTiles(eligible, center, radius, limit);
  const heightOrigin = boundedNumber(values["height-origin"], "height-origin", -200, 500);
  const plan = {
    ...eligible,
    tiles: selected.tiles,
    capture: { height_origin: heightOrigin, renderer: "state-plane-unlit-v1" },
  };
  await mkdir(output, { recursive: true });
  if (existsSync(join(output, "manifest.json"))) {
    validateManifest(
      JSON.parse(await readFile(join(output, "manifest.json"), "utf8")),
      plan,
      plan.capture,
    );
  }
  await writeJson(join(output, "plan.json"), plan);
  console.log(
    `Planned ${plan.tiles.length} eligible tiles. ${selected.omitted} additional candidates omitted by --limit. No Google requests made.`,
  );
  console.log(`Plan saved to ${join(output, "plan.json")}`);
}

async function localCaptureServer() {
  const temporary = await mkdtemp(join(tmpdir(), "isophilly-google-"));
  try {
    const bundle = join(temporary, "capture.js");
    await build({
      entryPoints: [join(root, "tools/google-imagery/capture.js")],
      bundle: true,
      format: "esm",
      outfile: bundle,
      logLevel: "silent",
    });
    const files = new Map([["/capture.js", [bundle, "text/javascript"]]]);
    for (const name of ["draco_decoder.js", "draco_decoder.wasm", "draco_wasm_wrapper.js"]) {
      files.set(`/draco/${name}`, [
        join(root, "node_modules/three/examples/jsm/libs/draco/gltf", name),
        name.endsWith("wasm") ? "application/wasm" : "text/javascript",
      ]);
    }
    const server = createServer(async (request, response) => {
      try {
        response.setHeader("Cache-Control", "no-store");
        if (request.url === "/") {
          response.setHeader("Content-Type", "text/html");
          response.end(
            '<!doctype html><html><head><meta charset="utf-8"><link rel="icon" href="data:,"><style>body{margin:0}</style></head><body><script type="module" src="/capture.js"></script></body></html>',
          );
        } else if (files.has(request.url)) {
          const [path, type] = files.get(request.url);
          response.setHeader("Content-Type", type);
          response.end(await readFile(path));
        } else {
          response.writeHead(404).end();
        }
      } catch {
        response.writeHead(500).end();
      }
    });
    await new Promise((accept, reject) => {
      server.once("error", reject);
      server.listen(0, "127.0.0.1", accept);
    });
    return {
      url: `http://127.0.0.1:${server.address().port}`,
      close: async () => {
        server.closeAllConnections();
        await new Promise((accept) => server.close(accept));
        await rm(temporary, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await rm(temporary, { recursive: true, force: true });
    throw error;
  }
}

async function captureTiles(output, limit) {
  if (existsSync(join(root, ".env"))) process.loadEnvFile(join(root, ".env"));
  const apiKey = process.env.GOOGLE_MAPS_API_KEY;
  if (!apiKey?.trim())
    throw new Error(
      "Set GOOGLE_MAPS_API_KEY in your environment or .env before capturing. No Google requests made.",
    );
  const lockPath = join(output, ".capture.lock");
  const lock = await open(lockPath, "wx").catch((error) => {
    if (error.code !== "EEXIST") throw error;
    throw new Error(
      `Another capture may be running. Check ${lockPath} before removing a stale lock.`,
    );
  });
  let host;
  let browser;
  try {
    const plan = validatePlan(JSON.parse(await readFile(join(output, "plan.json"), "utf8")));
    if (
      !plan.capture ||
      !Number.isFinite(plan.capture.height_origin) ||
      plan.capture.renderer !== "state-plane-unlit-v1"
    ) {
      throw new Error("Capture settings are missing. Run google:plan again.");
    }
    const manifestPath = join(output, "manifest.json");
    const manifest = existsSync(manifestPath)
      ? validateManifest(JSON.parse(await readFile(manifestPath, "utf8")), plan, plan.capture)
      : { ...plan, tiles: [] };
    // Verify every resumed artifact before spending money or publishing a mixed manifest.
    const completed = new Set();
    for (const tile of manifest.tiles) {
      const bytes = await readFile(join(output, `8/${tile.x}/${tile.y}.png`));
      if (sha256(bytes) !== tile.sha256)
        throw new Error(
          `Capture hash mismatch at 8/${tile.x}/${tile.y}. Restore or remove the broken capture and its manifest entry.`,
        );
      completed.add(`${tile.x}/${tile.y}`);
    }
    const pending = plan.tiles.filter((tile) => !completed.has(`${tile.x}/${tile.y}`));
    const selected = pending.slice(0, limit);
    console.log(
      `${selected.length} new captures this run; ${pending.length - selected.length} pending tiles omitted by --limit.`,
    );
    if (!selected.length) return;
    const timeoutMs = boundedNumber(values.timeout, "timeout", 5000, 600000, true);
    host = await localCaptureServer();
    const executablePath =
      values.browser ||
      process.env.ISOPHILLY_CHROMIUM_PATH ||
      (existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        : undefined);
    browser = await chromium.launch({ executablePath, headless: true });
    const context = await browser.newContext({
      viewport: { width: 288, height: 288 },
      deviceScaleFactor: 1,
      serviceWorkers: "block",
    });
    // Sessions and source tile downloads stay in this ephemeral browser context. Only reviewed renders persist.
    const page = await context.newPage();
    page.setDefaultTimeout(timeoutMs + 10000);
    for (const tile of selected) {
      let result;
      for (let attempt = 1; attempt <= 2; attempt++) {
        try {
          await page.goto(host.url);
          await page.waitForFunction(() => window.captureReady === true);
          result = await page.evaluate((settings) => window.captureGoogleTile(settings), {
            apiKey,
            bounds: tileBounds(plan, tile),
            heightOrigin: plan.capture.height_origin,
            timeoutMs,
          });
          validateCaptureResult(result);
          break;
        } catch {
          if (attempt === 2)
            throw new Error(
              `Capture 8/${tile.x}/${tile.y} failed twice. No image or manifest entry published. Check API access, quota, and the capture timeout.`,
            );
          console.log(`Retrying unsettled capture 8/${tile.x}/${tile.y} once.`);
        }
      }
      const bytes = Buffer.from(result.png, "base64");
      await atomicWrite(join(output, `8/${tile.x}/${tile.y}.png`), bytes);
      manifest.tiles.push({
        x: tile.x,
        y: tile.y,
        sha256: sha256(bytes),
        attributions: result.attributions,
        captured_at: new Date().toISOString(),
      });
      manifest.tiles.sort((a, b) => a.y - b.y || a.x - b.x);
      await writeJson(manifestPath, manifest);
      console.log(`Captured 8/${tile.x}/${tile.y} (${manifest.tiles.length} total).`);
    }
    console.log(`Ready for prebuild --google-imagery ${output}`);
  } finally {
    try {
      await browser?.close();
    } finally {
      try {
        await host?.close();
      } finally {
        await lock.close();
        await rm(lockPath, { force: true });
      }
    }
  }
}

try {
  if (values.help) console.log(help);
  else {
    const limit = boundedNumber(values.limit, "limit", 1, 65536, true);
    const output = resolve(root, values.output);
    if (positionals.length !== 1) throw new Error(help);
    if (positionals[0] === "plan") await makePlan(output, limit);
    else if (positionals[0] === "capture") await captureTiles(output, limit);
    else throw new Error(help);
  }
} catch (error) {
  const message = error instanceof Error ? error.message : "Google imagery command failed.";
  const key = process.env.GOOGLE_MAPS_API_KEY;
  console.error(key ? message.replaceAll(key, "[redacted]") : message);
  process.exitCode = 1;
}
