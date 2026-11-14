import { lstat, mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import proj4 from "proj4";
import { projectStatePlane } from "../tools/google-imagery/projection.js";
import { earthTileBounds, validateEarthPlan } from "./google-earth-lib.mjs";
import { DECODER, digest, RENDERER, writeAtomic, writeJson } from "./google-earth-source.mjs";
import { STATE_PLANE } from "./google-imagery-lib.mjs";

export const WATER_URL =
  "https://services.arcgis.com/fLeGjb7u4uXqeF9q/arcgis/rest/services/Hydrographic_Features_Poly/FeatureServer/1/query?outFields=*&where=1%3D1&f=geojson&geometry=-75.4,39.8,-74.9,40.2&geometryType=esriGeometryEnvelope&inSR=4326&spatialRel=esriSpatialRelIntersects&outSR=4326&orderByFields=OBJECTID&resultRecordCount=2000&maxAllowableOffset=0.000005&geometryPrecision=7";
export const WATER_RENDERER = "spherical-isophilly-water-mask-v2";
const MAX_SOURCE_BYTES = 64 * 1024 ** 2;
const key = (tile) => `${tile.view}/${tile.x}/${tile.y}`;
function requireValid(ok, message) {
  if (!ok) throw new Error(message);
}

/** Each polygon retains its exterior and every hole, including separate island polygons. */
export function projectWater(geojson, angle = 0) {
  requireValid(Number.isFinite(angle), "Invalid water projection angle");
  requireValid(
    geojson?.type === "FeatureCollection" &&
      Array.isArray(geojson.features) &&
      geojson.features.length > 0 &&
      !geojson.exceededTransferLimit &&
      !geojson.properties?.exceededTransferLimit,
    "Water must be a nonempty, complete GeoJSON FeatureCollection (exceededTransferLimit rejected)",
  );
  requireValid(
    !geojson.crs ||
      (geojson.crs.type === "name" &&
        ["EPSG:4326", "urn:ogc:def:crs:EPSG::4326", "urn:ogc:def:crs:OGC:1.3:CRS84"].includes(
          geojson.crs.properties?.name,
        )),
    "Water coordinates must use WGS84 longitude/latitude",
  );
  return geojson.features.flatMap((feature) => {
    const geometry = feature?.geometry;
    requireValid(
      feature?.type === "Feature" &&
        ["Polygon", "MultiPolygon"].includes(geometry?.type) &&
        Array.isArray(geometry.coordinates) &&
        geometry.coordinates.length > 0,
      "Every water feature must contain a Polygon or MultiPolygon",
    );
    const polygons = geometry.type === "Polygon" ? [geometry.coordinates] : geometry.coordinates;
    return polygons.map((polygon) => {
      requireValid(Array.isArray(polygon) && polygon.length > 0, "Empty water polygon");
      const bounds = [Infinity, Infinity, -Infinity, -Infinity];
      const rings = polygon.map((ring) => {
        requireValid(
          Array.isArray(ring) && ring.length >= 4,
          "Water rings need at least 4 vertices",
        );
        for (const point of ring)
          requireValid(
            Array.isArray(point) &&
              point.length >= 2 &&
              point.every(Number.isFinite) &&
              Math.abs(point[0]) <= 180 &&
              Math.abs(point[1]) <= 90,
            "Invalid WGS84 longitude/latitude water coordinate",
          );
        requireValid(isDeepStrictEqual(ring[0], ring.at(-1)), "Water rings must be closed");
        return ring.map(([lon, lat]) => {
          const [x, y] = projectStatePlane(
            ...proj4("EPSG:4326", STATE_PLANE, [lon, lat]),
            0,
            angle,
          );
          requireValid(
            Number.isFinite(x) && Number.isFinite(y),
            "Nonfinite projected water coordinate",
          );
          bounds[0] = Math.min(bounds[0], x);
          bounds[1] = Math.min(bounds[1], y);
          bounds[2] = Math.max(bounds[2], x);
          bounds[3] = Math.max(bounds[3], y);
          return [x, y];
        });
      });
      return { bounds, rings };
    });
  });
}

/** Flat water can lie behind raised terrain, or below the capture height origin. */
export function waterForBlock(polygons, bounds, heightOrigin = 0) {
  return polygons.filter(
    ({ bounds: [l, t, r, b] }) =>
      l <= bounds[2] &&
      r >= bounds[0] &&
      t <= bounds[3] + 400 - heightOrigin &&
      b >= bounds[1] - 100 - heightOrigin,
  );
}

async function optionalBytes(file) {
  try {
    return await readFile(file);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
}
async function fetchWater(fetchImpl, sourceUrl) {
  let size = 0,
    offset = 0;
  const pages = new Set(),
    features = [];
  while (true) {
    const url = new URL(sourceUrl);
    if (offset) {
      url.searchParams.set("resultOffset", `${offset}`);
      url.searchParams.set("resultRecordCount", "2000");
    }
    const response = await fetchImpl(url.href);
    requireValid(response.ok, `Water source returned HTTP ${response.status}`);
    const chunks = [];
    const body = response.body ?? [Buffer.from(await response.arrayBuffer())];
    for await (const chunk of body) {
      size += chunk.length;
      requireValid(size <= MAX_SOURCE_BYTES, "Water source exceeds explicit 64 MiB limit");
      chunks.push(Buffer.from(chunk));
    }
    const bytes = Buffer.concat(chunks),
      page = JSON.parse(bytes);
    requireValid(!pages.has(digest(bytes)), "Water source repeated a pagination page");
    pages.add(digest(bytes));
    requireValid(
      page.type === "FeatureCollection" && Array.isArray(page.features) && page.features.length > 0,
      "Invalid water source page",
    );
    features.push(...page.features);
    if (!page.exceededTransferLimit && !page.properties?.exceededTransferLimit) {
      if (pages.size === 1) return bytes;
      return Buffer.from(JSON.stringify({ type: "FeatureCollection", features }));
    }
    offset += page.features.length;
  }
}

/** Bind a separate mask workspace to exact, complete photo archive metadata, never raw pixels. */
export async function createWaterMaskPlan(
  photoRoot,
  { fetchImpl = fetch, sourceUrl = WATER_URL } = {},
) {
  const root = join(photoRoot, "water");
  const [photoBytes, manifestBytes, sourceBytes, metaBytes, coverageBytes] = await Promise.all([
    readFile(join(photoRoot, "plan.json")),
    readFile(join(photoRoot, "manifest.json")),
    readFile(join(photoRoot, "source.json")),
    readFile(join(photoRoot, "reference", "meta.json")),
    readFile(join(photoRoot, "reference", "coverage.json")),
  ]);
  const photo = validateEarthPlan(JSON.parse(photoBytes));
  requireValid(!Object.hasOwn(photo, "capture_mode"), "Input photo plan already has capture_mode");
  const manifest = JSON.parse(manifestBytes);
  requireValid(
    manifest?.schema_version === 1 &&
      manifest.plan_sha256 === digest(photoBytes) &&
      manifest.decoder === DECODER &&
      (manifest.renderer ?? RENDERER) === RENDERER &&
      Array.isArray(manifest.images),
    "Invalid photo manifest or plan_sha256 binding",
  );
  const planned = new Set(
    photo.views.flatMap((view) => view.tiles.map((tile) => key({ ...tile, view: view.id }))),
  );
  const seen = new Set();
  for (const image of manifest.images) {
    requireValid(
      Number.isInteger(image?.x) &&
        Number.isInteger(image?.y) &&
        planned.has(key(image)) &&
        !seen.has(key(image)) &&
        typeof image.sha256 === "string" &&
        /^[a-f0-9]{64}$/.test(image.sha256),
      "Invalid, duplicate or unplanned photo manifest image",
    );
    seen.add(key(image));
  }
  requireValid(seen.size === planned.size, "Photo archive is not complete");
  // Check existence/type/size only. Capture and build verify the multi-gigabyte pixel hashes.
  for (const image of manifest.images) {
    const file = join(photoRoot, "raw", image.view, "8", `${image.x}`, `${image.y}.png`);
    const info = await lstat(file);
    requireValid(info.isFile() && info.size > 0, `Missing or empty photo: ${key(image)}`);
  }
  const source = JSON.parse(sourceBytes);
  requireValid(
    source?.decoder === DECODER &&
      (source.renderer ?? RENDERER) === RENDERER &&
      Number.isSafeInteger(source.root_epoch) &&
      source.root_epoch > 0 &&
      Number.isFinite(source.radius) &&
      source.radius > 0,
    "Invalid pinned photo source decoder, renderer, epoch or radius",
  );
  const maskSource = { ...source, renderer: WATER_RENDERER };
  const meta = JSON.parse(metaBytes);
  const coverage = JSON.parse(coverageBytes);
  requireValid(
    isDeepStrictEqual(meta, photo.source_meta) &&
      coverage?.schema_version === 1 &&
      coverage.tile_version === meta.tile_version &&
      Array.isArray(coverage.tiles),
    "Photo reference metadata does not match plan",
  );
  const existingBytes = await optionalBytes(join(root, "plan.json"));
  const existing = existingBytes ? validateEarthPlan(JSON.parse(existingBytes)) : null;
  const immutable = "Water workspace changed or is incomplete; use a new mask workspace";
  if (existing)
    requireValid(
      existing.capture_mode === "water-mask" &&
        existing.photo_plan_sha256 === digest(photoBytes) &&
        existing.photo_manifest_sha256 === digest(manifestBytes) &&
        existing.water_source === sourceUrl,
      immutable,
    );
  else {
    try {
      await lstat(root);
      throw new Error(immutable);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
  }
  const reference = join(root, "reference");
  const raw = existing
    ? await readFile(join(reference, "water.geojson"))
    : await fetchWater(fetchImpl, sourceUrl);
  requireValid(raw.length <= MAX_SOURCE_BYTES, "Water source exceeds explicit 64 MiB limit");
  const geojson = JSON.parse(raw);
  const views = Object.fromEntries(photo.views.map((v) => [v.id, projectWater(geojson, v.angle)]));
  const water = { schema_version: 1, views };
  const waterBytes = Buffer.from(`${JSON.stringify(water, null, 2)}\n`);
  const plan = {
    ...photo,
    capture_mode: "water-mask",
    water_renderer: WATER_RENDERER,
    photo_plan_sha256: digest(photoBytes),
    photo_manifest_sha256: digest(manifestBytes),
    water_sha256: digest(waterBytes),
    water_source_sha256: digest(raw),
    water_source: sourceUrl,
    views: photo.views
      .map((view) => ({
        ...view,
        tiles: view.tiles.filter(
          (tile) =>
            waterForBlock(views[view.id], earthTileBounds(photo, view, tile), photo.height_origin)
              .length > 0,
        ),
      }))
      .filter((view) => view.id === "se" || view.tiles.length > 0),
  };
  validateEarthPlan(plan);
  if (existing) {
    const [pinnedWater, pinnedRawMeta, pinnedCoverage, pinnedSource] = await Promise.all([
      readFile(join(reference, "water.json")),
      readFile(join(reference, "meta.json")),
      readFile(join(reference, "coverage.json")),
      readFile(join(root, "source.json")),
    ]);
    requireValid(
      isDeepStrictEqual(existing, plan) &&
        digest(pinnedWater) === existing.water_sha256 &&
        pinnedRawMeta.equals(metaBytes) &&
        pinnedCoverage.equals(coverageBytes) &&
        isDeepStrictEqual(JSON.parse(pinnedSource), maskSource),
      immutable,
    );
    return { root, plan: existing };
  }
  await mkdir(root); // Exclusive: never repair or overwrite an unrelated existing workspace.
  await mkdir(reference);
  await writeAtomic(join(reference, "water.geojson"), raw);
  await writeJson(join(reference, "water.json"), water);
  await writeAtomic(join(reference, "meta.json"), metaBytes);
  await writeAtomic(join(reference, "coverage.json"), coverageBytes);
  await writeJson(join(root, "source.json"), maskSource);
  await writeJson(join(root, "plan.json"), plan); // Commit marker, written last.
  return { root, plan };
}
