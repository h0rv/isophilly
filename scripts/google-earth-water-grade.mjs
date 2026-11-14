import { lstat, readFile } from "node:fs/promises";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import sharp from "sharp";
import { inverseIsometric } from "../tools/google-imagery/projection.js";
import { validateEarthPlan } from "./google-earth-lib.mjs";
import { DECODER, digest } from "./google-earth-source.mjs";
import { WATER_RENDERER } from "./google-earth-water-lib.mjs";

const key = ({ view, x, y }) => `${view}/${x}/${y}`;
function requireValid(condition, message) {
  if (!condition) throw new Error(message);
}
async function bounded(file, limit = 128 * 1024 ** 2) {
  const info = await lstat(file);
  requireValid(
    info.isFile() && info.size > 0 && info.size <= limit,
    `Invalid water-mask file: ${file}`,
  );
  const bytes = await readFile(file);
  requireValid(bytes.length <= limit, `Oversized water-mask file: ${file}`);
  return bytes;
}

export async function loadWaterMasks(root, photoBytes, photoManifestBytes) {
  const photo = JSON.parse(photoBytes),
    base = join(root, "water");
  const maskPlanBytes = await bounded(join(base, "plan.json"));
  const plan = validateEarthPlan(JSON.parse(maskPlanBytes));
  requireValid(
    plan.capture_mode === "water-mask" &&
      plan.water_renderer === WATER_RENDERER &&
      plan.photo_plan_sha256 === digest(photoBytes) &&
      plan.photo_manifest_sha256 === digest(photoManifestBytes),
    "Water masks do not match the exact photo archive",
  );
  requireValid(
    ["zoom", "tile_size", "source_level", "height_origin"].every(
      (field) => plan[field] === photo[field],
    ) && isDeepStrictEqual(plan.source_meta, photo.source_meta),
    "Water mask camera or source grid mismatch",
  );
  const projected = await bounded(join(base, "reference", "water.json"));
  const source = await bounded(join(base, "reference", "water.geojson"));
  requireValid(
    digest(projected) === plan.water_sha256 && digest(source) === plan.water_source_sha256,
    "Water reference hash mismatch",
  );
  const photoSource = JSON.parse(await bounded(join(root, "source.json")));
  const maskSource = JSON.parse(await bounded(join(base, "source.json")));
  requireValid(
    maskSource.renderer === WATER_RENDERER &&
      maskSource.decoder === DECODER &&
      photoSource.decoder === DECODER &&
      Number.isSafeInteger(photoSource.root_epoch) &&
      photoSource.root_epoch > 0 &&
      photoSource.root_epoch === maskSource.root_epoch &&
      Number.isFinite(photoSource.radius) &&
      photoSource.radius > 0 &&
      photoSource.radius === maskSource.radius,
    "Water mask source epoch, radius or decoder mismatch",
  );
  const planned = new Set();
  for (const view of plan.views) {
    const original = photo.views.find((candidate) => candidate.id === view.id);
    requireValid(
      original &&
        view.angle === original.angle &&
        isDeepStrictEqual(view.iso_bounds, original.iso_bounds),
      "Water view mismatch",
    );
    const originalTiles = new Set(original.tiles.map((tile) => `${tile.x}/${tile.y}`));
    for (const tile of view.tiles) {
      requireValid(originalTiles.has(`${tile.x}/${tile.y}`), "Unplanned water-mask coordinate");
      planned.add(key({ ...tile, view: view.id }));
    }
  }
  const bytes = await bounded(join(base, "manifest.json")),
    manifest = JSON.parse(bytes);
  requireValid(
    manifest.schema_version === 1 &&
      manifest.plan_sha256 === digest(maskPlanBytes) &&
      manifest.decoder === DECODER &&
      manifest.renderer === WATER_RENDERER &&
      Array.isArray(manifest.images),
    "Invalid water-mask manifest",
  );
  const images = new Map();
  for (const image of manifest.images) {
    requireValid(
      Number.isInteger(image?.x) &&
        Number.isInteger(image?.y) &&
        planned.has(key(image)) &&
        !images.has(key(image)) &&
        typeof image.sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(image.sha256) &&
        Number.isInteger(image.water_pixels) &&
        image.water_pixels >= 0 &&
        image.water_pixels <= photo.tile_size ** 2,
      "Invalid or duplicate water-mask image",
    );
    const file = join(base, "raw", image.view, "8", `${image.x}`, `${image.y}.png`);
    await bounded(file, 32 * 1024 ** 2);
    images.set(key(image), { ...image, file });
  }
  requireValid(
    images.size === planned.size,
    "Water masks are incomplete; raw photography remains unchanged",
  );
  return {
    images,
    sha256: digest(bytes),
    source_sha256: plan.water_source_sha256,
    source_url: plan.water_source,
  };
}

const f = Math.fround;
function mix(left, right, amount) {
  const t = f(amount);
  return Math.max(0, Math.min(255, Math.round(f(f(left * f(1 - t)) + f(right * t)))));
}
export function gradeWaterPixels(rgba, mask, tile, view, size) {
  const palette = [42, 132, 172],
    highlight = [126, 196, 210],
    shadow = [27, 101, 147];
  const side =
    Math.max(view.iso_bounds[2] - view.iso_bounds[0], view.iso_bounds[3] - view.iso_bounds[1]) /
    256;
  let selected = 0;
  for (let i = 0; i < rgba.length; i += 4) {
    requireValid(
      mask[i + 3] === 255 &&
        mask[i] === mask[i + 1] &&
        mask[i] === mask[i + 2] &&
        [0, 255].includes(mask[i]),
      "Water masks must be opaque binary RGBA",
    );
    if (mask[i] === 0) continue;
    selected++;
    const pixel = i / 4,
      col = pixel % size,
      row = Math.floor(pixel / size);
    const x = view.iso_bounds[0] + (tile.x + (col + 0.5) / size) * side;
    const y = view.iso_bounds[1] + (tile.y + (row + 0.5) / size) * side;
    const [east, north] = inverseIsometric(x, y, view.angle);
    const band = ((f(f(east) * f(0.075) + f(f(north) * f(0.035))) % 29) + 29) % 29;
    for (let c = 0; c < 3; c++) {
      const base = mix(rgba[i + c], palette[c], 0.56);
      rgba[i + c] =
        band < f(1.4)
          ? mix(base, highlight[c], 0.16)
          : band >= 14.5 && band < f(15.3)
            ? mix(base, shadow[c], 0.12)
            : base;
    }
  }
  return selected;
}

export async function applyWaterMask(rgba, record, tile, view, size) {
  const bytes = await bounded(record.file, 32 * 1024 ** 2);
  requireValid(digest(bytes) === record.sha256, `Water mask hash mismatch: ${key(tile)}`);
  const decoder = sharp(bytes, { limitInputPixels: size * size, failOn: "warning" });
  const meta = await decoder.metadata();
  requireValid(
    meta.format === "png" &&
      meta.width === size &&
      meta.height === size &&
      meta.channels === 4 &&
      meta.hasAlpha &&
      meta.depth === "uchar" &&
      (meta.pages ?? 1) === 1,
    "Water mask must match the photo's RGBA dimensions",
  );
  const mask = await decoder.raw().toBuffer();
  requireValid(
    mask.length === rgba.length && rgba.length === size * size * 4,
    "Water mask pixel size mismatch",
  );
  requireValid(
    gradeWaterPixels(rgba, mask, tile, view, size) === record.water_pixels,
    "Water mask pixel count mismatch",
  );
}
