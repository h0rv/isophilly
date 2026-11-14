import { createHash } from "node:crypto";
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  rename,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";
import { applyWaterMask, loadWaterMasks } from "./google-earth-water-grade.mjs";

const REPO = fileURLToPath(new URL("../", import.meta.url));
const STATIC = [
  "app.js",
  "city-overlay.js",
  "map-presentation.js",
  "map-search.js",
  "map-projection.js",
  "presentation-config.js",
  "index.html",
  "neighborhoods.json",
  "_headers",
];
const BEIGE = { r: 222, g: 212, b: 196, alpha: 1 };
const MAX_BYTES = 128 * 1024 * 1024;
const MAX_PNG_BYTES = 32 * 1024 * 1024;
const MAX_TILES = 8 * 65536;
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const object = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const coord = ({ x, y }) => `${x}/${y}`;
const key = (image) => `${image.view}/${coord(image)}`;
const json = (file, value) => writeFile(file, `${JSON.stringify(value)}\n`);
function requireValid(condition, message) {
  if (!condition) throw new Error(message);
}
function bounds(value) {
  return (
    Array.isArray(value) &&
    value.length === 4 &&
    value.every(Number.isFinite) &&
    value[0] < value[2] &&
    value[1] < value[3]
  );
}
function point(value) {
  return (
    value === null || (Array.isArray(value) && value.length === 2 && value.every(Number.isFinite))
  );
}
function landmarks(value) {
  return (
    Array.isArray(value) &&
    value.every(
      (item) =>
        object(item) &&
        typeof item.name === "string" &&
        point(item.point) &&
        item.point !== null &&
        Number.isInteger(item.min_zoom) &&
        typeof item.color === "string",
    )
  );
}
function validateMeta(meta) {
  requireValid(
    object(meta) &&
      meta.schema_version === 2 &&
      bounds(meta.iso_bounds) &&
      point(meta.city_hall) &&
      object(meta.counts) &&
      Number.isInteger(meta.counts.buildings) &&
      Number.isInteger(meta.counts.building_meshes) &&
      landmarks(meta.landmarks) &&
      typeof meta.tile_version === "string" &&
      [meta.max_tile_zoom, meta.max_zoom, meta.home_zoom].every(Number.isInteger) &&
      object(meta.rich) &&
      Number.isInteger(meta.rich.home_zoom) &&
      Array.isArray(meta.rich.views) &&
      meta.rich.views.length === 4 &&
      meta.rich.views.every(
        (view, index) =>
          object(view) &&
          view.id === ["se", "sw", "nw", "ne"][index] &&
          typeof view.label === "string" &&
          typeof view.tile_version === "string" &&
          bounds(view.iso_bounds) &&
          point(view.city_hall) &&
          landmarks(view.landmarks),
      ),
    "Invalid source_meta schema 2",
  );
}
function validatePlan(plan) {
  requireValid(
    object(plan) &&
      plan.schema_version === 1 &&
      plan.source === "google-earth-public" &&
      plan.zoom === 8 &&
      [256, 512, 1024].includes(plan.tile_size) &&
      [18, 19, 20].includes(plan.source_level) &&
      Number.isFinite(plan.height_origin),
    "Invalid plan schema",
  );
  validateMeta(plan.source_meta);
  requireValid(
    Array.isArray(plan.views) && plan.views.length > 0 && plan.views.length <= 360,
    "Invalid view count (maximum 360)",
  );
  const planned = new Map();
  const ids = new Set();
  for (const view of plan.views) {
    requireValid(
      object(view) &&
        typeof view.id === "string" &&
        /^[a-z][a-z0-9-]*$/.test(view.id) &&
        view.id.length <= 64 &&
        !ids.has(view.id) &&
        Number.isFinite(view.angle) &&
        bounds(view.iso_bounds),
      "Invalid or duplicate view",
    );
    ids.add(view.id);
    requireValid(
      Array.isArray(view.tiles) && view.tiles.length > 0 && view.tiles.length <= 65536,
      `Invalid tile count for ${view.id}`,
    );
    for (const tile of view.tiles) {
      requireValid(
        object(tile) &&
          Number.isInteger(tile.x) &&
          Number.isInteger(tile.y) &&
          tile.x >= 0 &&
          tile.x < 256 &&
          tile.y >= 0 &&
          tile.y < 256,
        `Out-of-grid tile in ${view.id}`,
      );
      const id = key({ ...tile, view: view.id });
      requireValid(!planned.has(id), `Duplicate planned tile ${id}`);
      planned.set(id, view.id);
      requireValid(planned.size <= MAX_TILES, `Plan exceeds ${MAX_TILES} tiles`);
    }
  }
  requireValid(ids.has("se"), "Plan must include the default se view");
  return planned;
}
function timestamp(value) {
  if (
    typeof value !== "string" ||
    !/^\d{4}-\d{2}-\d{2}T(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|[+-](?:[01]\d|2[0-3]):[0-5]\d)$/.test(
      value,
    ) ||
    !Number.isFinite(Date.parse(value))
  )
    return false;
  // Date.parse otherwise silently accepts dates such as February 30.
  return (
    new Date(`${value.slice(0, 10)}T00:00:00Z`).toISOString().slice(0, 10) === value.slice(0, 10)
  );
}
function validateManifest(manifest, digest, planned) {
  requireValid(
    object(manifest) && manifest.schema_version === 1 && manifest.plan_sha256 === digest,
    "Manifest plan_sha256 does not match exact plan bytes or schema is invalid",
  );
  requireValid(
    Array.isArray(manifest.images) && manifest.images.length <= MAX_TILES,
    "Invalid manifest image count",
  );
  const images = new Map();
  for (const image of manifest.images) {
    requireValid(
      object(image) &&
        typeof image.view === "string" &&
        Number.isInteger(image.x) &&
        Number.isInteger(image.y) &&
        planned.has(key(image)),
      "Manifest image is unplanned or out-of-grid",
    );
    requireValid(!images.has(key(image)), `Duplicate manifest image ${key(image)}`);
    requireValid(
      typeof image.sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(image.sha256) &&
        timestamp(image.captured_at) &&
        Array.isArray(image.attributions) &&
        image.attributions.length > 0 &&
        image.attributions.every(
          (credit) => typeof credit === "string" && credit.trim().length > 0,
        ),
      `Invalid provenance for ${key(image)}`,
    );
    images.set(key(image), image);
  }
  return images;
}
async function readBounded(file, limit = MAX_BYTES) {
  const stat = await lstat(file);
  requireValid(
    stat.isFile() && stat.size > 0 && stat.size <= limit,
    `Invalid file size/type: ${file}`,
  );
  const bytes = await readFile(file);
  requireValid(bytes.length <= limit, `File exceeds ${limit} bytes: ${file}`);
  return bytes;
}
export function applyDisplayFinish(rgba) {
  // Match palette::display_finish's f32 operations, including the final RGB rounding.
  const f = Math.fround;
  for (let i = 0; i < rgba.length; i += 4) {
    requireValid(rgba[i + 3] === 255, "Raw PNG must be fully opaque RGBA");
    const luma = f(
      f(f(rgba[i] * f(0.213)) + f(rgba[i + 1] * f(0.715))) + f(rgba[i + 2] * f(0.072)),
    );
    for (let c = 0; c < 3; c++) {
      const saturated = f(luma + f(f(rgba[i + c] - luma) * f(1.14)));
      rgba[i + c] = Math.min(
        255,
        Math.max(0, Math.round(f(f(f(saturated - 127.5) * f(1.04)) + 127.5))),
      );
    }
  }
}
const tilePath = (base, z, x, y) => path.join(base, "tiles", `${z}`, `${x}`, `${y}.webp`);
async function saveTile(base, z, x, y, pipeline) {
  const file = tilePath(base, z, x, y);
  await mkdir(path.dirname(file), { recursive: true });
  await pipeline.webp({ lossless: true }).toFile(file);
}
async function leaf(base, image, file, size, water, view, sourceColors) {
  const bytes = await readBounded(file, MAX_PNG_BYTES);
  requireValid(hash(bytes) === image.sha256, `PNG sha256 mismatch for ${key(image)}`);
  const decoder = sharp(bytes, { limitInputPixels: size * size, failOn: "warning" });
  const meta = await decoder.metadata();
  requireValid(
    meta.format === "png" &&
      meta.width === size &&
      meta.height === size &&
      meta.channels === 4 &&
      meta.hasAlpha &&
      (meta.pages ?? 1) === 1 &&
      meta.depth === "uchar",
    `Raw PNG must be ${size}x${size} RGBA: ${key(image)}`,
  );
  const { data, info } = await decoder.raw().toBuffer({ resolveWithObject: true });
  requireValid(info.channels === 4 && data.length === size * size * 4, "Invalid decoded RGBA size");
  if (sourceColors) {
    for (let i = 3; i < data.length; i += 4)
      requireValid(data[i] === 255, "Raw PNG must be fully opaque RGBA");
  } else {
    if (water) await applyWaterMask(data, water, image, view, size);
    applyDisplayFinish(data);
  }
  await saveTile(
    base,
    8,
    image.x,
    image.y,
    sharp(data, { raw: { width: size, height: size, channels: 4 } }),
  );
}
async function parents(base, leaves, size, sourceColors) {
  let children = new Set(leaves);
  const coverage = [...children].map((tile) => `8/${tile}`);
  for (let z = 7; z >= 0; z--) {
    const next = new Set(
      [...children].map((tile) => {
        const [x, y] = tile.split("/").map(Number);
        return `${Math.floor(x / 2)}/${Math.floor(y / 2)}`;
      }),
    );
    for (const tile of next) {
      const [x, y] = tile.split("/").map(Number);
      const inputs = [];
      for (let dy = 0; dy < 2; dy++)
        for (let dx = 0; dx < 2; dx++) {
          if (!children.has(`${x * 2 + dx}/${y * 2 + dy}`)) continue;
          inputs.push({
            input: await sharp(tilePath(base, z + 1, x * 2 + dx, y * 2 + dy))
              .resize(size / 2, size / 2, sourceColors ? { kernel: sharp.kernel.linear } : {})
              .png()
              .toBuffer(),
            left: (dx * size) / 2,
            top: (dy * size) / 2,
          });
        }
      await saveTile(
        base,
        z,
        x,
        y,
        sharp({
          create: {
            width: size,
            height: size,
            channels: 4,
            background: BEIGE,
          },
        }).composite(inputs),
      );
      coverage.push(`${z}/${tile}`);
    }
    children = next;
  }
  return coverage.sort((a, b) => {
    const left = a.split("/").map(Number),
      right = b.split("/").map(Number);
    return left[0] - right[0] || left[1] - right[1] || left[2] - right[2];
  });
}
async function publish(stage, output) {
  // A single symlink rename publishes the entire generation without an output gap.
  // Never move/delete an unmanaged directory just to make room for a new build.
  try {
    requireValid(
      (await lstat(output)).isSymbolicLink(),
      `Atomic publication requires absent output or a symlink; existing output left unchanged: ${output}`,
    );
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  const pointer = `${stage}.link`;
  try {
    await symlink(path.basename(stage), pointer, "dir");
    await rename(pointer, output);
  } catch (error) {
    await rm(pointer, { force: true });
    throw error;
  }
  // Keep prior generations valid for preview servers and readers that already
  // resolved their paths. They can be removed after those readers have stopped.
}

/** Build a local, captured Google Earth atlas. No capture or network requests are made. */
export async function buildGoogleEarth({
  root,
  output,
  allowPartial = false,
  waterStyle = false,
  sourceColors = false,
}) {
  requireValid(
    typeof root === "string" &&
      root.length > 0 &&
      typeof output === "string" &&
      output.length > 0 &&
      typeof allowPartial === "boolean" &&
      typeof waterStyle === "boolean" &&
      typeof sourceColors === "boolean",
    "Expected root, output and boolean allowPartial",
  );
  requireValid(!sourceColors || !waterStyle, "Source colors cannot be combined with water styling");
  root = path.resolve(root);
  output = path.resolve(output);
  requireValid(
    root !== output &&
      !root.startsWith(`${output}${path.sep}`) &&
      output !== path.join(root, "raw") &&
      !output.startsWith(`${path.join(root, "raw")}${path.sep}`),
    "Output must not replace the workspace or raw captures",
  );
  const planBytes = await readBounded(path.join(root, "plan.json"));
  const plan = JSON.parse(planBytes.toString("utf8"));
  requireValid(
    plan.capture_mode === undefined,
    "A water-mask atlas is not photography and cannot be published as a map",
  );
  const digest = hash(planBytes);
  const planned = validatePlan(plan);
  const manifestBytes = await readBounded(path.join(root, "manifest.json"));
  const images = validateManifest(JSON.parse(manifestBytes), digest, planned);
  const missing = Object.fromEntries(plan.views.map((view) => [view.id, view.tiles.length]));
  const present = new Map(plan.views.map((view) => [view.id, []]));
  for (const image of images.values()) {
    const file = path.join(root, "raw", image.view, "8", `${image.x}`, `${image.y}.png`);
    try {
      await lstat(file);
    } catch (error) {
      if (error.code === "ENOENT") continue;
      throw error;
    }
    present.get(image.view).push({ image, file });
    missing[image.view]--;
  }
  const missingCount = Object.values(missing).reduce((a, b) => a + b, 0);
  if (missingCount)
    console.warn(`Missing Google Earth tiles: ${JSON.stringify(missing)} (${missingCount} total)`);
  requireValid(
    allowPartial || missingCount === 0,
    `${missingCount} planned images missing; use --allow-partial explicitly`,
  );
  const water = waterStyle ? await loadWaterMasks(root, planBytes, manifestBytes) : undefined;
  await mkdir(path.dirname(output), { recursive: true });
  const stage = await mkdtemp(
    path.join(path.dirname(output), `.${path.basename(output)}-building-`),
  );
  try {
    for (const name of STATIC)
      await copyFile(path.join(REPO, "static", name), path.join(stage, name));
    const views = [];
    for (const view of plan.views) {
      const base = view.id === "se" ? stage : path.join(stage, "views", view.id);
      await mkdir(base, { recursive: true });
      const captures = present.get(view.id);
      // Deliberately serial: only one leaf or four resized parent quadrants are resident.
      for (const { image, file } of captures)
        await leaf(
          base,
          image,
          file,
          plan.tile_size,
          water?.images.get(key(image)),
          view,
          sourceColors,
        );
      const tiles = await parents(
        base,
        captures.map(({ image }) => coord(image)),
        plan.tile_size,
        sourceColors,
      );
      const contentDigest = hash(
        Buffer.from(
          JSON.stringify({
            renderer: waterStyle ? "earth-atlas-city-water-v1" : "earth-atlas-v1",
            water_masks: water?.sha256,
            ...(sourceColors ? { sourceColors: true } : {}),
            plan: digest,
            images: captures
              .map(({ image }) => [image.x, image.y, image.sha256])
              .sort((a, b) => a[1] - b[1] || a[0] - b[0]),
          }),
        ),
      );
      const tileVersion = `google-earth-${digest.slice(0, 16)}-${contentDigest.slice(0, 16)}-${view.id}`;
      const meta = {
        ...plan.source_meta,
        tile_version: tileVersion,
        iso_bounds: view.iso_bounds,
        max_tile_zoom: 8,
        max_zoom: 10,
        home_zoom: 7,
        landmarks: [],
        tile_size: plan.tile_size,
        imagery_source: "google-earth-public",
        ...(sourceColors ? { raw_colors: true, imagery_style: "source" } : {}),
        water_style: waterStyle ? "city" : "source",
        water_mask_sha256: water?.sha256,
        water_source: water?.source_url,
        source_plan_sha256: digest,
        reference_world_sha256: plan.source_meta.world_sha256,
        reference_counts: plan.source_meta.counts,
        counts: { buildings: 0, building_meshes: 0, capture_tiles: captures.length },
        counts_note: "Captured imagery; individual buildings and trees have not been counted.",
        rich_reference_only: true,
        imagery_attributions: [
          ...new Set(captures.flatMap(({ image }) => image.attributions)),
        ].sort(),
      };
      delete meta.google_imagery_sha256;
      delete meta.google_attributions;
      delete meta.world_sha256;
      delete meta.land_cover_sha256;
      delete meta.terrain_sha256;
      await json(path.join(base, "meta"), meta);
      await json(path.join(base, "coverage.json"), {
        schema_version: 1,
        tile_version: tileVersion,
        tiles,
      });
      views.push({
        id: view.id,
        angle: view.angle,
        iso_bounds: view.iso_bounds,
        tile_version: tileVersion,
        meta: view.id === "se" ? "/meta" : `/views/${view.id}/meta`,
        coverage: view.id === "se" ? "/coverage.json" : `/views/${view.id}/coverage.json`,
        tiles:
          view.id === "se" ? "/tiles/{z}/{x}/{y}.webp" : `/views/${view.id}/tiles/{z}/{x}/{y}.webp`,
      });
    }
    const report = {
      schema_version: 1,
      plan_sha256: digest,
      complete: missingCount === 0,
      ...(sourceColors ? { raw_colors: true, imagery_style: "source" } : {}),
      water_style: waterStyle ? "city" : "source",
      ...(water ? { water_mask_sha256: water.sha256 } : {}),
      missing_count: missingCount,
      views: plan.views.map((view) => ({
        id: view.id,
        planned: view.tiles.length,
        present: present.get(view.id).length,
        missing: missing[view.id],
      })),
    };
    await json(path.join(stage, "views.json"), { schema_version: 1, default_view: "se", views });
    await json(path.join(stage, "build.json"), report);
    await publish(stage, output);
    return report;
  } catch (error) {
    await rm(stage, { recursive: true, force: true });
    throw error;
  }
}

async function main(args) {
  const options = {
    root: "data/google-earth",
    output: "data/google-earth/site",
    allowPartial: false,
  };
  for (let i = 0; i < args.length; i++) {
    const flag = args[i];
    if (flag === "--allow-partial") options.allowPartial = true;
    else if (flag === "--water") options.waterStyle = true;
    else if (flag === "--source-colors") options.sourceColors = true;
    else if (
      ["--root", "--output"].includes(flag) &&
      args[i + 1] &&
      !args[i + 1].startsWith("--")
    ) {
      options[flag.slice(2)] = args[++i];
    } else throw new Error(`Unknown or incomplete argument: ${flag}`);
  }
  const report = await buildGoogleEarth(options);
  console.log(`Built Google Earth atlas at ${options.output} (complete: ${report.complete})`);
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
