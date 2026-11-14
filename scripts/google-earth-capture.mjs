import { existsSync } from "node:fs";
import {
  mkdir,
  mkdtemp,
  open,
  readdir,
  readFile,
  rename,
  rm,
  statfs,
  writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";
import { chromium } from "playwright-core";
import sharp from "sharp";
import { blocksForPlan, sourcePoints, validateEarthPlan } from "./google-earth-lib.mjs";
import {
  createEarthSource,
  DECODER,
  digest,
  earthTileIntersects,
  packEarthTile,
  RENDERER,
  writeJson,
} from "./google-earth-source.mjs";
import { WATER_RENDERER, waterForBlock } from "./google-earth-water-lib.mjs";

const REPO = fileURLToPath(new URL("..", import.meta.url));
const key = (tile) => `${tile.view}/${tile.x}/${tile.y}`;
const rawPath = (root, image) => join(root, "raw", image.view, "8", `${image.x}`, `${image.y}.png`);

export async function startServer(assets) {
  const bundle = await build({
    entryPoints: [join(REPO, "tools/google-imagery/earth-capture.js")],
    bundle: true,
    format: "esm",
    write: false,
    logLevel: "silent",
  });
  const server = createServer(async (req, res) => {
    try {
      res.setHeader("Cache-Control", "no-store");
      const url = new URL(req.url, "http://localhost");
      if (url.pathname === "/") {
        res.setHeader("Content-Type", "text/html");
        return res.end(
          '<!doctype html><html><head><link rel="icon" href="data:,"><style>body{margin:0}</style></head><body><script type="module" src="/capture.js"></script></body></html>',
        );
      }
      if (url.pathname === "/capture.js") {
        res.setHeader("Content-Type", "text/javascript");
        return res.end(bundle.outputFiles[0].contents);
      }
      const path = resolve(assets, `.${decodeURIComponent(url.pathname)}`);
      if (!path.startsWith(`${assets}${sep}`)) return res.writeHead(403).end();
      res.setHeader(
        "Content-Type",
        { ".json": "application/json", ".jpg": "image/jpeg", ".bin": "application/octet-stream" }[
          extname(path)
        ] || "application/octet-stream",
      );
      res.end(await readFile(path));
    } catch {
      res.writeHead(404).end();
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
    },
  };
}

export async function nodesFor(source, points, level) {
  const nodes = new Map();
  for (const point of points) {
    const matches = await source.client.findNodes(point, { level, maxNodes: 64 });
    for (const node of matches) nodes.set(node.path, node);
  }
  return [...nodes.values()].sort((a, b) => a.path.localeCompare(b.path));
}

async function loadBlock(source, block, level, assets, copyrights, radius, heightOrigin) {
  const output = [];
  const credits = new Set(["Google Earth"]);
  for (const current of [level - 2, level - 1, level]) {
    const points = sourcePoints(
      block.bounds,
      block.angle,
      current,
      400,
      heightOrigin,
      block.projection,
    );
    const nodes = await nodesFor(source, points, current);
    // Node metadata is deduplicated before downloading. A missing current-level
    // patch remains visibly recorded as coarse coverage, never described as fine.
    for (let i = 0; i < nodes.length; i += 4) {
      const batch = await Promise.all(
        nodes.slice(i, i + 4).map(async (node) => {
          const tile = await source.client.getMesh(node.path, { metadata: node, textureFormat: 1 });
          if (
            !earthTileIntersects(
              tile,
              block.bounds,
              radius,
              heightOrigin,
              block.angle,
              block.projection,
            )
          )
            return [];
          const packed = await packEarthTile(tile, assets);
          for (const id of packed.copyrightIds) {
            // 255 is the unlabelled Google/default imagery entry in the source table.
            const text = copyrights.get(id);
            if (text) credits.add(text);
            else if (id !== 255)
              throw new Error(`Unresolved imagery copyright ID ${id}; stop before publication.`);
          }
          const records = [
            {
              url: `/${packed.name}`,
              level: current,
              coarse: current < level,
              meshes: packed.meshes,
            },
          ];
          if (packed.groundName)
            records.push({
              url: `/${packed.groundName}`,
              level: current - 100,
              coarse: true,
              meshes: packed.groundMeshes,
            });
          return records;
        }),
      );
      output.push(...batch.flat().filter((entry) => entry.meshes > 0));
    }
  }
  if (!output.length) throw new Error("No Google Earth geometry available for this block.");
  return {
    tiles: output,
    attributions: [...credits].sort(),
    fineNodes: output.filter((tile) => !tile.coarse).length,
  };
}

export async function captureEarth({
  root,
  cacheRoot,
  limit = Infinity,
  blockSize = 4,
  timeoutMs = 120000,
}) {
  root = resolve(root);
  if (
    !(limit === Infinity || (Number.isInteger(limit) && limit > 0)) ||
    !Number.isInteger(blockSize) ||
    blockSize < 1 ||
    blockSize > 4
  )
    throw new Error("Invalid capture limits.");
  await mkdir(root, { recursive: true });
  const lockPath = join(root, ".capture.lock");
  const lock = await open(lockPath, "wx").catch((error) => {
    if (error.code !== "EEXIST") throw error;
    throw new Error(
      `Capture is already running. Inspect ${lockPath} before removing a stale lock.`,
    );
  });
  let browser;
  let host;
  let assets;
  try {
    const planBytes = await readFile(join(root, "plan.json"));
    const plan = validateEarthPlan(JSON.parse(planBytes));
    const sourceFile = join(root, "source.json");
    if (existsSync(sourceFile)) {
      const pinned = JSON.parse(await readFile(sourceFile, "utf8"));
      if (JSON.stringify(pinned.projection) !== JSON.stringify(plan.projection))
        throw new Error("Pinned Earth source projection does not match this immutable plan.");
    }
    const effectiveBlockSize = Math.min(
      blockSize,
      Math.floor(2048 / plan.tile_size),
      Math.max(1, 4 / 2 ** (plan.source_level - 18)),
    );
    if (effectiveBlockSize !== blockSize)
      console.log(`Using ${effectiveBlockSize}-tile blocks to bound high-detail rendering memory.`);
    const planHash = digest(planBytes);
    const maskingWater = plan.capture_mode === "water-mask";
    const renderer = maskingWater ? WATER_RENDERER : RENDERER;
    let water;
    if (maskingWater) {
      if (plan.water_renderer !== WATER_RENDERER)
        throw new Error("Water-mask renderer changed; use a new mask workspace.");
      const data = await readFile(join(root, "reference", "water.json"));
      if (digest(data) !== plan.water_sha256) throw new Error("Water reference hash mismatch.");
      water = JSON.parse(data);
    }
    const manifestPath = join(root, "manifest.json");
    const manifest = existsSync(manifestPath)
      ? JSON.parse(await readFile(manifestPath, "utf8"))
      : {
          schema_version: 1,
          plan_sha256: planHash,
          decoder: DECODER,
          renderer,
          images: [],
        };
    if (
      manifest.schema_version !== 1 ||
      manifest.plan_sha256 !== planHash ||
      manifest.decoder !== DECODER ||
      !Array.isArray(manifest.images) ||
      (manifest.renderer ?? "spherical-isophilly-raw-v1") !== renderer
    )
      throw new Error("Manifest does not match this immutable plan.");
    manifest.renderer = renderer;
    const planned = new Set(
      plan.views.flatMap((view) => view.tiles.map((tile) => key({ ...tile, view: view.id }))),
    );
    const completed = new Set();
    for (const image of manifest.images) {
      if (
        !Number.isInteger(image.x) ||
        !Number.isInteger(image.y) ||
        !planned.has(key(image)) ||
        !/^[a-f0-9]{64}$/.test(image.sha256)
      )
        throw new Error("Invalid or unplanned image in capture manifest.");
      if (completed.has(key(image))) throw new Error("Duplicate image in capture manifest.");
      if (digest(await readFile(rawPath(root, image))) !== image.sha256)
        throw new Error(`Capture is corrupt: ${key(image)}`);
      completed.add(key(image));
    }
    const needed = plan.views.reduce((count, view) => count + view.tiles.length, 0);
    console.log(
      `${completed.size}/${needed} captures verified; ${needed - completed.size} remaining.`,
    );
    if (completed.size === needed) return { completed: completed.size, total: needed };
    assets = await mkdtemp(join(tmpdir(), "isophilly-earth-render-"));
    host = await startServer(assets);
    const executablePath =
      process.env.ISOPHILLY_CHROMIUM_PATH ||
      (existsSync("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        : undefined);
    browser = await chromium.launch({ executablePath, headless: true });
    const page = await browser.newPage({
      viewport: { width: 1024, height: 1024 },
      deviceScaleFactor: 1,
    });
    page.setDefaultTimeout(timeoutMs);
    const source = await createEarthSource(
      cacheRoot === undefined ? join(root, "cache") : resolve(cacheRoot),
    );
    const metadata = await source.client.getPlanetoid();
    if (existsSync(sourceFile)) {
      const pinned = JSON.parse(await readFile(sourceFile, "utf8"));
      if (
        pinned.root_epoch !== metadata.rootEpoch ||
        pinned.radius !== metadata.radius ||
        pinned.decoder !== DECODER ||
        (pinned.renderer ?? "spherical-isophilly-raw-v1") !== renderer ||
        JSON.stringify(pinned.projection) !== JSON.stringify(plan.projection)
      ) {
        throw new Error(
          "Earth source or decoder changed. Use the pinned source cache rather than mixing epochs.",
        );
      }
    }
    const copyrights = new Map(
      (await source.copyright(metadata.rootEpoch)).map((entry) => [entry.id, entry.text]),
    );
    await writeJson(sourceFile, {
      decoder: DECODER,
      renderer,
      ...(plan.projection === undefined ? {} : { projection: plan.projection }),
      root_epoch: metadata.rootEpoch,
      radius: metadata.radius,
      copyright: [...copyrights].map(([id, text]) => ({ id, text })),
    });
    let written = 0;
    const failed = [];
    const started = Date.now();
    for (const block of blocksForPlan(plan, effectiveBlockSize)) {
      const pending = block.tiles
        .map((tile) => ({ ...tile, view: block.view }))
        .filter((tile) => !completed.has(key(tile)))
        .slice(0, limit - written);
      if (!pending.length) {
        if (written >= limit) break;
        continue;
      }
      const disk = await statfs(root);
      if (disk.bavail * disk.bsize < 10 * 1024 ** 3)
        throw new Error("Less than 10 GiB free. Capture paused safely; free space and resume.");
      const blockStart = Date.now();
      console.log(
        `Loading ${block.view} block ${block.x},${block.y}; ${pending.length} new images...`,
      );
      const loaded = await loadBlock(
        source,
        block,
        plan.source_level,
        assets,
        copyrights,
        metadata.radius,
        plan.height_origin,
      );
      await page.goto(host.url);
      await page.waitForFunction(() => window.captureReady === true);
      const result = await page.evaluate((settings) => window.captureEarthBlock(settings), {
        bounds: block.bounds,
        width: block.width * plan.tile_size,
        height: block.height * plan.tile_size,
        heightOrigin: plan.height_origin,
        tileSize: plan.tile_size,
        angle: block.angle,
        ...(plan.projection === undefined ? {} : { projection: plan.projection }),
        radius: metadata.radius,
        tiles: loaded.tiles,
        attributions: loaded.attributions,
        waterPolygons: maskingWater
          ? waterForBlock(water.views[block.view], block.bounds, plan.height_origin)
          : undefined,
      });
      if (maskingWater) console.log("Water terrain:", JSON.stringify(result.waterStats));
      const frame = Buffer.from(result.raw, "base64");
      const frameWidth = block.width * plan.tile_size;
      const frameHeight = block.height * plan.tile_size;
      if (
        result.width !== frameWidth ||
        result.height !== frameHeight ||
        frame.length !== frameWidth * frameHeight * 4 ||
        !Array.isArray(result.finePixels) ||
        result.finePixels.length !== block.width * block.height
      ) {
        throw new Error("Renderer returned an invalid pixel frame; no images published.");
      }
      for (const tile of pending) {
        const stride = plan.tile_size * 4;
        const pixels = Buffer.alloc(plan.tile_size * stride);
        for (let row = 0; row < plan.tile_size; row++) {
          const offset =
            (((tile.y - block.y) * plan.tile_size + row) * frameWidth +
              (tile.x - block.x) * plan.tile_size) *
            4;
          frame.copy(pixels, row * stride, offset, offset + stride);
        }
        let holes = 0;
        for (let i = 3; i < pixels.length; i += 4) if (pixels[i] !== 255) holes++;
        if (holes) {
          const rejected = join(root, "rejected", tile.view, "8", `${tile.x}`, `${tile.y}.png`);
          await mkdir(dirname(rejected), { recursive: true });
          await sharp(pixels, {
            raw: { width: plan.tile_size, height: plan.tile_size, channels: 4 },
          })
            .png()
            .toFile(rejected);
          const points = [];
          for (let i = 3; i < pixels.length; i += 4)
            if (pixels[i] !== 255)
              points.push([
                ((i - 3) / 4) % plan.tile_size,
                Math.floor((i - 3) / 4 / plan.tile_size),
              ]);
          await writeJson(`${rejected}.json`, { ...tile, holes, points, bounds: block.bounds });
          failed.push({ ...tile, holes });
          console.warn(`Rejected ${key(tile)}: ${holes} uncovered pixels.`);
          continue;
        }
        const image = await sharp(pixels, {
          raw: { width: plan.tile_size, height: plan.tile_size, channels: 4 },
        })
          .png()
          .toBuffer();
        const destination = rawPath(root, tile);
        await mkdir(dirname(destination), { recursive: true });
        const temporary = `${destination}.part`;
        await writeFile(temporary, image);
        await rename(temporary, destination);
        manifest.images.push({
          ...tile,
          ...(maskingWater
            ? { water_pixels: pixels.filter((value, i) => i % 4 === 0 && value === 255).length }
            : {}),
          sha256: digest(image),
          captured_at: new Date().toISOString(),
          attributions: loaded.attributions,
          source_level: plan.source_level,
          background_levels: [plan.source_level - 2, plan.source_level - 1],
          terrain_underlay: true,
          fine_nodes: loaded.fineNodes,
          fine_pixels: result.finePixels[(tile.y - block.y) * block.width + tile.x - block.x],
          coarse_fallback_pixels:
            plan.tile_size ** 2 -
            result.finePixels[(tile.y - block.y) * block.width + tile.x - block.x],
        });
        completed.add(key(tile));
        written++;
      }
      await writeJson(manifestPath, manifest);
      await writeJson(join(root, "progress.json"), {
        completed: completed.size,
        total: needed,
        failed,
        elapsed_seconds: (Date.now() - started) / 1000,
        source: source.stats(),
        updated_at: new Date().toISOString(),
      });
      for (const file of await readdir(assets)) await rm(join(assets, file), { force: true });
      console.log(
        `${completed.size}/${needed} captured; block ${((Date.now() - blockStart) / 1000).toFixed(1)}s; source ${(source.stats().downloadedBytes / 1024 ** 2).toFixed(1)} MiB this run.`,
      );
      if (written >= limit) break;
    }
    return { completed: completed.size, total: needed, rejected: failed.length };
  } finally {
    try {
      await browser?.close();
    } finally {
      try {
        await host?.close();
      } finally {
        if (assets) await rm(assets, { recursive: true, force: true });
        await lock.close();
        await rm(lockPath, { force: true });
      }
    }
  }
}
