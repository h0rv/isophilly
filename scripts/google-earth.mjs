import { readFile, realpath, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { validateProjection } from "../static/map-projection.js";
import { createEarthPlan, validateEarthPlan } from "./google-earth-lib.mjs";
import { sha256 } from "./google-imagery-lib.mjs";

const help = `Google Earth native mesh atlas (public source, no API key required)

  node scripts/google-earth.mjs plan
  node scripts/google-earth.mjs capture
  node scripts/google-earth.mjs status
  node scripts/google-earth.mjs build
  node scripts/google-earth.mjs serve

Options:
  --root PATH           Workspace (default data/google-earth)
  --angles 0,45,...      Unique multiples of 45; must include 0 (default 0)
  --tile-size N         256, 512 or 1024 pixels (default 512)
  --level N             Earth source level 18, 19 or 20 (default 18)
  --height-origin M     Fixed height above the Earth source sphere (default 0)
  --projection PATH     Camera projection JSON (default identity; angle 0 only)
  --cache PATH          Shared source cache (default ROOT/cache)
  --center LON,LAT      Optional angle-0 pilot center in Philadelphia
  --radius M            Pilot radius (default 350)
  --limit N             Optional planned/new image limit; otherwise full city
  --block-size N        Capture block side in tiles (default 4)
  --timeout MS          Capture timeout (default 120000)
  --output PATH         Generated site (default ROOT/site)
  --allow-partial       Explicitly allow an incomplete build
  --source-colors       Preserve the captured imagery without color changes
  --water               Apply reviewed City water masks during build
  --port N              Local serve port (default 3110)
  --help                Show this help

Planning pins local reference snapshots, or fetches them from isophilly.horv.co
when unavailable. It never calls Google. Existing plans cannot be changed;
use a new root for different settings. Capture uses the authorized citywide
public Earth source without credentials. No subprocesses run in the background.
Only angle 0 currently preserves source landmark positions in the viewer.
`;
function number(value, name, min, max = Infinity, integer = true) {
  const n = Number(value);
  if (
    !value?.trim() ||
    !Number.isFinite(n) ||
    n < min ||
    n > max ||
    (integer && !Number.isInteger(n))
  )
    throw new Error(`Invalid --${name}: ${value}`);
  return n;
}

async function status(root) {
  const bytes = await readFile(join(root, "plan.json"));
  const plan = validateEarthPlan(JSON.parse(bytes));
  let manifest;
  try {
    manifest = JSON.parse(await readFile(join(root, "manifest.json"), "utf8"));
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  if (
    manifest &&
    (manifest.schema_version !== 1 ||
      manifest.plan_sha256 !== sha256(bytes) ||
      !Array.isArray(manifest.images))
  )
    throw new Error("Manifest plan_sha256 does not match exact plan bytes or schema is invalid");
  const planned = new Set(plan.views.flatMap((v) => v.tiles.map((t) => `${v.id}/${t.x}/${t.y}`)));
  const seen = new Set(),
    completed = new Map(plan.views.map((v) => [v.id, 0]));
  for (const image of manifest?.images ?? []) {
    const key = `${image.view}/${image.x}/${image.y}`;
    if (!planned.has(key) || seen.has(key) || !/^[a-f0-9]{64}$/.test(image.sha256))
      throw new Error(`Invalid, duplicate or unplanned manifest image: ${key}`);
    seen.add(key);
    try {
      const file = await stat(join(root, "raw", image.view, "8", `${image.x}`, `${image.y}.png`));
      if (file.isFile() && file.size > 0) completed.set(image.view, completed.get(image.view) + 1);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  console.log(
    `Plan sha256 ${sha256(bytes)}${manifest ? " (manifest matches)" : " (capture not started)"}`,
  );
  for (const view of plan.views)
    console.log(
      `${view.id}: ${completed.get(view.id)}/${view.tiles.length} completed; ${view.tiles.length - completed.get(view.id)} pending`,
    );
}

async function serve(output, port) {
  const base = await realpath(output);
  const within = (file) => file === base || file.startsWith(`${base}${sep}`);
  const mime = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".json": "application/json",
    ".css": "text/css",
    ".webp": "image/webp",
    ".png": "image/png",
    ".svg": "image/svg+xml",
    ".ico": "image/x-icon",
    ".wasm": "application/wasm",
  };
  const server = createServer(async (request, response) => {
    try {
      if (!["GET", "HEAD"].includes(request.method)) {
        response.writeHead(405).end();
        return;
      }
      const pathname = decodeURIComponent(request.url.split("?")[0]);
      let file = resolve(base, `.${pathname}`);
      if (!pathname.startsWith("/") || pathname.includes("\0") || !within(file)) {
        response.writeHead(403).end();
        return;
      }
      file = await realpath(file);
      if (!within(file)) {
        response.writeHead(403).end();
        return;
      }
      if ((await stat(file)).isDirectory()) file = await realpath(join(file, "index.html"));
      if (!within(file)) {
        response.writeHead(403).end();
        return;
      }
      const bytes = await readFile(file);
      const url = new URL(request.url, "http://localhost");
      const etag = `"${sha256(bytes)}"`;
      const headers = {
        "Content-Type":
          mime[extname(file)] ??
          (file.endsWith(`${sep}meta`) ? "application/json" : "application/octet-stream"),
        "Cache-Control":
          pathname.startsWith("/tiles/") && url.searchParams.has("v")
            ? "public, max-age=31536000, immutable"
            : "no-cache",
        ETag: etag,
      };
      if (request.headers["if-none-match"] === etag) {
        response.writeHead(304, headers).end();
        return;
      }
      response.writeHead(200, { ...headers, "Content-Length": bytes.length });
      response.end(request.method === "HEAD" ? undefined : bytes);
    } catch (error) {
      response
        .writeHead(
          error instanceof URIError
            ? 400
            : ["ENOENT", "ENOTDIR", "EISDIR"].includes(error.code)
              ? 404
              : 500,
        )
        .end();
    }
  });
  await new Promise((accept, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", accept);
  });
  console.log(`Serving ${output} at http://127.0.0.1:${port}/ (view index: /views.json)`);
  // Remain in the foreground. Ctrl-C stops the server without leaving a capture process.
}

async function main() {
  const { values: v, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      root: { type: "string", default: "data/google-earth" },
      cache: { type: "string" },
      angles: { type: "string", default: "0" },
      "tile-size": { type: "string", default: "512" },
      level: { type: "string", default: "18" },
      "height-origin": { type: "string", default: "0" },
      projection: { type: "string" },
      center: { type: "string" },
      radius: { type: "string", default: "350" },
      limit: { type: "string" },
      "block-size": { type: "string", default: "4" },
      timeout: { type: "string", default: "120000" },
      "allow-partial": { type: "boolean", default: false },
      water: { type: "boolean", default: false },
      "source-colors": { type: "boolean", default: false },
      output: { type: "string" },
      port: { type: "string", default: "3110" },
      help: { type: "boolean" },
    },
  });
  if (v.help || positionals.length === 0) {
    console.log(help);
    return;
  }
  if (
    positionals.length !== 1 ||
    !["plan", "capture", "status", "build", "serve"].includes(positionals[0])
  )
    throw new Error("Expected one command: plan, capture, status, build or serve. Use --help.");
  const root = resolve(v.root),
    output = resolve(v.output ?? join(root, "site"));
  const limit = v.limit === undefined ? undefined : number(v.limit, "limit", 1);
  switch (positionals[0]) {
    case "plan": {
      const plan = await createEarthPlan({
        root,
        angles: v.angles.split(",").map((s) => number(s, "angles", 0, 315)),
        tileSize: number(v["tile-size"], "tile-size", 256, 1024),
        level: number(v.level, "level", 18, 20),
        heightOrigin: number(v["height-origin"], "height-origin", -Infinity, Infinity, false),
        projection:
          v.projection === undefined
            ? undefined
            : validateProjection(JSON.parse(await readFile(resolve(v.projection), "utf8"))),
        center:
          v.center === undefined
            ? undefined
            : v.center.split(",").map((s) => number(s, "center", -180, 180, false)),
        radius: number(v.radius, "radius", Number.MIN_VALUE, Infinity, false),
        limit,
      });
      console.log(
        `Planned ${plan.views.reduce((n, view) => n + view.tiles.length, 0)} tiles across ${plan.views.length} view(s). No Google requests made.\n${join(root, "plan.json")}`,
      );
      break;
    }
    case "capture": {
      const { captureEarth } = await import("./google-earth-capture.mjs");
      const result = await captureEarth({
        root,
        cacheRoot: v.cache === undefined ? undefined : resolve(v.cache),
        limit: limit ?? Infinity,
        blockSize: number(v["block-size"], "block-size", 1, 256),
        timeoutMs: number(v.timeout, "timeout", 1),
      });
      if (result.rejected) process.exitCode = 1;
      break;
    }
    case "status":
      await status(root);
      break;
    case "build": {
      const { buildGoogleEarth } = await import("./google-earth-build.mjs");
      const report = await buildGoogleEarth({
        root,
        output,
        allowPartial: v["allow-partial"],
        waterStyle: v.water,
        sourceColors: v["source-colors"],
      });
      console.log(`Built ${output} (complete: ${report.complete})`);
      break;
    }
    case "serve":
      await serve(output, number(v.port, "port", 1, 65535));
      break;
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    if (error.cause instanceof Error) console.error(`Cause: ${error.cause.message}`);
    process.exitCode = 1;
  });
}
