import { mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import proj4 from "proj4";
import { isIdentityProjection, validateProjection } from "../static/map-projection.js";
import { inverseIsometric, projectStatePlane } from "../tools/google-imagery/projection.js";
import { STATE_PLANE, sha256 } from "./google-imagery-lib.mjs";

const DEFAULT_META = "https://isophilly.horv.co/meta";
const DEFAULT_COVERAGE = "https://isophilly.horv.co/coverage.json";
const LOCAL_REFERENCE = fileURLToPath(
  new URL("../tools/google-imagery/reference/", import.meta.url),
);
const order = (a, b) => a.y - b.y || a.x - b.x;
const viewId = (angle) =>
  ({ 0: "se", 90: "sw", 180: "nw", 270: "ne" })[angle] ?? `v${String(angle).padStart(3, "0")}`;
const validAngle = (angle) =>
  Number.isInteger(angle) && angle >= 0 && angle <= 315 && angle % 45 === 0;
const validBounds = (b) =>
  Array.isArray(b) && b.length === 4 && b.every(Number.isFinite) && b[0] < b[2] && b[1] < b[3];
const inPhilly = ([lon, lat]) => lon >= -75.4 && lon <= -74.9 && lat >= 39.8 && lat <= 40.2;
function requireValid(ok, message) {
  if (!ok) throw new Error(message);
}
async function optionalBytes(file) {
  try {
    return await readFile(file);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
}
function envelope(points) {
  return points.reduce(
    (b, [x, y]) => [Math.min(b[0], x), Math.min(b[1], y), Math.max(b[2], x), Math.max(b[3], y)],
    [Infinity, Infinity, -Infinity, -Infinity],
  );
}
const corners = ([l, t, r, b]) => [
  [l, t],
  [r, t],
  [r, b],
  [l, b],
];

export function validateEarthPlan(plan) {
  const projection = validateProjection(plan?.projection);
  const camera = !isIdentityProjection(projection);
  requireValid(
    !camera || plan.capture_mode !== "water-mask",
    "Water masks require the identity projection",
  );
  requireValid(
    !camera ||
      (plan.views?.length === 1 &&
        plan.views[0]?.angle === 0 &&
        JSON.stringify(plan.source_meta?.projection) === JSON.stringify(projection)),
    "Camera projection requires angle 0 and matching source metadata",
  );
  requireValid(
    plan?.schema_version === 1 &&
      plan.source === "google-earth-public" &&
      plan.zoom === 8 &&
      [256, 512, 1024].includes(plan.tile_size) &&
      [18, 19, 20].includes(plan.source_level) &&
      Number.isFinite(plan.height_origin) &&
      plan.source_meta?.schema_version === 2 &&
      typeof plan.source_meta.tile_version === "string" &&
      validBounds(plan.source_meta.iso_bounds),
    "Invalid Earth plan schema or source metadata",
  );
  requireValid(
    plan.capture_mode === undefined ||
      (plan.capture_mode === "water-mask" &&
        [
          plan.photo_plan_sha256,
          plan.photo_manifest_sha256,
          plan.water_sha256,
          plan.water_source_sha256,
        ].every((value) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value))),
    "Invalid capture mode or water-mask provenance",
  );
  requireValid(
    Array.isArray(plan.views) &&
      plan.views.length > 0 &&
      plan.views.length <= 8 &&
      plan.views[0]?.id === "se" &&
      plan.views[0]?.angle === 0,
    "Plan must start with angle 0 (se)",
  );
  const ids = new Set();
  for (const view of plan.views) {
    requireValid(
      validAngle(view.angle) &&
        view.id === viewId(view.angle) &&
        !ids.has(view.id) &&
        validBounds(view.iso_bounds),
      "Invalid or duplicate Earth view",
    );
    ids.add(view.id);
    requireValid(
      view.angle !== 0 ||
        JSON.stringify(view.iso_bounds) === JSON.stringify(plan.source_meta.iso_bounds),
      "SE bounds must exactly match source_meta",
    );
    requireValid(
      Array.isArray(view.tiles) && view.tiles.length > 0 && view.tiles.length <= 65536,
      "Invalid tile count",
    );
    const keys = new Set();
    for (const tile of view.tiles) {
      const key = `${tile?.x}/${tile?.y}`;
      requireValid(
        Number.isInteger(tile?.x) &&
          Number.isInteger(tile?.y) &&
          tile.x >= 0 &&
          tile.x < 256 &&
          tile.y >= 0 &&
          tile.y < 256 &&
          !keys.has(key),
        "Invalid or duplicate Earth tile",
      );
      keys.add(key);
    }
  }
  return plan;
}

/** Continuous double-precision global grid; adjacent tiles share the exact same edge. */
export function earthTileBounds(_plan, view, tile) {
  const [l, t, r, b] = view.iso_bounds;
  const side = Math.max(r - l, b - t) / 256;
  return [l + tile.x * side, t + tile.y * side, l + (tile.x + 1) * side, t + (tile.y + 1) * side];
}

export function blocksForPlan(plan, blockSize = 4) {
  validateEarthPlan(plan);
  requireValid(
    Number.isInteger(blockSize) && blockSize >= 1 && blockSize <= 256,
    "blockSize must be an integer from 1 to 256",
  );
  return plan.views.flatMap((view) => {
    const groups = new Map();
    for (const tile of view.tiles) {
      const x = Math.floor(tile.x / blockSize) * blockSize,
        y = Math.floor(tile.y / blockSize) * blockSize;
      const key = `${x}/${y}`;
      if (!groups.has(key))
        groups.set(key, {
          view: view.id,
          angle: view.angle,
          x,
          y,
          width: Math.min(blockSize, 256 - x),
          height: Math.min(blockSize, 256 - y),
          tiles: [],
        });
      groups.get(key).tiles.push({ x: tile.x, y: tile.y });
    }
    return [...groups.values()].sort(order).map((block) => {
      const first = earthTileBounds(plan, view, block);
      const last = earthTileBounds(plan, view, {
        x: block.x + block.width - 1,
        y: block.y + block.height - 1,
      });
      return {
        ...block,
        ...(plan.projection === undefined ? {} : { projection: plan.projection }),
        bounds: [first[0], first[1], last[2], last[3]],
        tiles: block.tiles.sort(order),
      };
    });
  });
}

/** SDK points {latitude, longitude}; include ground behind visible 400 m buildings. */
export function sourcePoints(
  bounds,
  angle,
  level,
  marginHeight = 400,
  heightOrigin = 0,
  projection,
) {
  const camera = validateProjection(projection);
  const identity = isIdentityProjection(camera);
  requireValid(
    validBounds(bounds) &&
      validAngle(angle) &&
      [16, 17, 18, 19, 20].includes(level) &&
      Number.isFinite(heightOrigin) &&
      Number.isFinite(marginHeight) &&
      marginHeight >= 0 &&
      marginHeight <= 2000,
    "Invalid source footprint settings",
  );
  const footprint = corners(bounds).flatMap(([x, y]) =>
    [0, marginHeight].map((h) =>
      proj4(
        STATE_PLANE,
        "EPSG:4326",
        inverseIsometric(
          x,
          identity ? y + h - heightOrigin : y + (h - heightOrigin) * camera.heightScale,
          angle,
          projection,
        ),
      ),
    ),
  );
  requireValid(
    footprint.every((p) => p.every(Number.isFinite) && inPhilly(p)),
    "Source footprint is outside Philadelphia",
  );
  const [west, south, east, north] = envelope(footprint);
  const cell = 90 / 2 ** (level - 2);
  const x0 = Math.floor(west / cell) - 1,
    x1 = Math.floor(east / cell) + 1;
  const y0 = Math.floor(south / cell) - 1,
    y1 = Math.floor(north / cell) + 1;
  requireValid(
    (x1 - x0 + 1) * (y1 - y0 + 1) <= 10000,
    "Source footprint exceeds 10000 samples; reduce block size",
  );
  const points = [];
  for (let y = y0; y <= y1; y++)
    for (let x = x0; x <= x1; x++) {
      const longitude = (x + 0.5) * cell,
        latitude = (y + 0.5) * cell;
      requireValid(
        inPhilly([longitude, latitude]),
        "Padded source footprint is outside Philadelphia",
      );
      points.push({ latitude, longitude });
    }
  return points;
}

async function snapshotBytes(value, local, url) {
  if (value !== undefined) {
    if (Buffer.isBuffer(value)) return value;
    if (typeof value === "string") return readFile(value);
    return Buffer.from(`${JSON.stringify(value)}\n`);
  }
  if (local) {
    const bytes = await optionalBytes(join(LOCAL_REFERENCE, local));
    if (bytes) return bytes;
  }
  const response = await fetch(url);
  requireValid(response.ok, `Reference snapshot returned HTTP ${response.status}: ${url}`);
  return Buffer.from(await response.arrayBuffer());
}
function referenceData(metaBytes, coverageBytes) {
  const meta = JSON.parse(metaBytes),
    coverage = JSON.parse(coverageBytes);
  requireValid(
    meta?.schema_version === 2 &&
      validBounds(meta.iso_bounds) &&
      typeof meta.tile_version === "string" &&
      coverage?.schema_version === 1 &&
      coverage.tile_version === meta.tile_version &&
      Array.isArray(coverage.tiles),
    "Reference coverage version must match valid source meta",
  );
  const tiles = [];
  const seen = new Set();
  for (const key of coverage.tiles) {
    requireValid(
      typeof key === "string" && /^(?:[0-8])\/\d+\/\d+$/.test(key) && !seen.has(key),
      "Invalid or duplicate coverage tile",
    );
    seen.add(key);
    const [z, x, y] = key.split("/").map(Number);
    requireValid(x < 2 ** z && y < 2 ** z, "Out-of-grid coverage tile");
    if (z === 8) tiles.push({ x, y });
  }
  requireValid(tiles.length > 0, "Reference coverage contains no zoom 8 leaves");
  return { meta, tiles: tiles.sort(order) };
}

/** Rasterize conservative transformed original leaf footprints on one continuous grid. */
function cameraView(meta, tiles, projection) {
  const original = { iso_bounds: meta.iso_bounds };
  const boxes = tiles.map((tile) =>
    envelope(
      corners(earthTileBounds(null, original, tile)).flatMap(([x, y]) =>
        [0, 400].map((h) => [
          projection.xScale * x,
          projection.groundScale * y + (projection.groundScale - projection.heightScale) * h,
        ]),
      ),
    ),
  );
  const tight = envelope(boxes.flatMap(corners));
  const span = Math.max(tight[2] - tight[0], tight[3] - tight[1]);
  const cx = (tight[0] + tight[2]) / 2,
    cy = (tight[1] + tight[3]) / 2;
  const iso_bounds = [cx - span / 2, cy - span / 2, cx + span / 2, cy + span / 2];
  const side = span / 256;
  const keys = new Set();
  for (const [l, t, r, b] of boxes) {
    // Include both sides of exact grid edges: these boxes are conservative.
    const x0 = Math.max(0, Math.ceil((l - iso_bounds[0]) / side) - 1);
    const x1 = Math.min(255, Math.floor((r - iso_bounds[0]) / side));
    const y0 = Math.max(0, Math.ceil((t - iso_bounds[1]) / side) - 1);
    const y1 = Math.min(255, Math.floor((b - iso_bounds[1]) / side));
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) keys.add(`${x}/${y}`);
  }
  return {
    id: "se",
    angle: 0,
    iso_bounds,
    tiles: [...keys]
      .map((key) => {
        const [x, y] = key.split("/").map(Number);
        return { x, y };
      })
      .sort(order),
  };
}

/** Check complete geographic coverage against the pinned reference, including a changed camera. */
export function earthCoverageScope(plan, referenceMeta, referenceCoverage) {
  validateEarthPlan(plan);
  const original = referenceCoverage.tiles
    .filter((tile) => tile.startsWith("8/"))
    .map((tile) => {
      const [, x, y] = tile.split("/").map(Number);
      return { x, y };
    });
  const expected = isIdentityProjection(plan.projection)
    ? { iso_bounds: referenceMeta.iso_bounds, tiles: original }
    : cameraView(referenceMeta, original, validateProjection(plan.projection));
  const keys = new Set(expected.tiles.map((tile) => `${tile.x}/${tile.y}`));
  const actual = plan.views[0];
  const complete =
    JSON.stringify(actual.iso_bounds) === JSON.stringify(expected.iso_bounds) &&
    actual.tiles.length === keys.size &&
    actual.tiles.every((tile) => keys.has(`${tile.x}/${tile.y}`));
  return {
    scope: complete ? "citywide" : "pilot",
    expectedLeafCount: keys.size,
    referenceLeafCount: original.length,
  };
}

/** Pin exact reference bytes once. Planning never calls Google or loads the Earth SDK. */
export async function createEarthPlan({
  root,
  metaUrl = DEFAULT_META,
  coverageUrl = DEFAULT_COVERAGE,
  sourceMeta,
  sourceCoverage,
  angles = [0],
  tileSize = 512,
  level = 18,
  heightOrigin = 0,
  projection,
  center,
  radius = 350,
  limit,
} = {}) {
  requireValid(typeof root === "string" && root.length > 0, "root is required");
  const camera = validateProjection(projection);
  const cameraEnabled = !isIdentityProjection(camera);
  requireValid(
    !cameraEnabled || (angles.length === 1 && angles[0] === 0),
    "Camera projection currently supports angle 0 only",
  );
  requireValid(
    Array.isArray(angles) &&
      angles.length > 0 &&
      angles.every(validAngle) &&
      new Set(angles).size === angles.length &&
      angles.includes(0),
    "Angles must be unique multiples of 45 from 0 to 315 and include 0",
  );
  requireValid(
    [256, 512, 1024].includes(tileSize) &&
      [18, 19, 20].includes(level) &&
      Number.isFinite(heightOrigin),
    "Invalid tile size, source level or height origin",
  );
  requireValid(
    limit === undefined || (Number.isInteger(limit) && limit > 0 && limit <= 65536),
    "limit must be an integer from 1 to 65536",
  );
  requireValid(
    Number.isFinite(radius) &&
      radius > 0 &&
      (!center ||
        (Array.isArray(center) &&
          center.length === 2 &&
          center.every(Number.isFinite) &&
          inPhilly(center))),
    "Invalid Philadelphia center or radius",
  );
  requireValid(
    (!center && limit === undefined) || angles.length === 1,
    "Pilot center/limit filters only support angle 0",
  );
  const reference = join(root, "reference");
  let metaBytes = await optionalBytes(join(reference, "meta.json"));
  let coverageBytes;
  if (metaBytes) {
    coverageBytes = await readFile(join(reference, "coverage.json"));
    const hashes = JSON.parse(await readFile(join(reference, "hashes.json"), "utf8"));
    requireValid(
      hashes.meta?.sha256 === sha256(metaBytes) &&
        hashes.coverage?.sha256 === sha256(coverageBytes),
      "Pinned reference hash mismatch",
    );
    for (const [value, bytes] of [
      [sourceMeta, metaBytes],
      [sourceCoverage, coverageBytes],
    ]) {
      if (value !== undefined)
        requireValid(
          sha256(await snapshotBytes(value)) === sha256(bytes),
          "Reference changed; use a new root",
        );
    }
  } else {
    // An existing but incomplete reference directory must not be repaired from the network.
    try {
      const { stat } = await import("node:fs/promises");
      await stat(reference);
      throw new Error("Incomplete pinned reference; use a new root");
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    [metaBytes, coverageBytes] = await Promise.all([
      snapshotBytes(sourceMeta, metaUrl === DEFAULT_META ? "meta.json" : null, metaUrl),
      snapshotBytes(
        sourceCoverage,
        coverageUrl === DEFAULT_COVERAGE ? "coverage.json" : null,
        coverageUrl,
      ),
    ]);
    referenceData(metaBytes, coverageBytes);
    await mkdir(root, { recursive: true });
    const stage = `${reference}.${process.pid}.part`;
    await mkdir(stage);
    try {
      await writeFile(join(stage, "meta.json"), metaBytes);
      await writeFile(join(stage, "coverage.json"), coverageBytes);
      await writeFile(
        join(stage, "hashes.json"),
        `${JSON.stringify(
          {
            schema_version: 1,
            meta: { url: metaUrl, sha256: sha256(metaBytes), bytes: metaBytes.length },
            coverage: {
              url: coverageUrl,
              sha256: sha256(coverageBytes),
              bytes: coverageBytes.length,
            },
          },
          null,
          2,
        )}\n`,
      );
      await rename(stage, reference);
    } finally {
      await rm(stage, { recursive: true, force: true });
    }
  }
  const { meta, tiles } = referenceData(metaBytes, coverageBytes);
  const plan = {
    schema_version: 1,
    source: "google-earth-public",
    source_meta: meta,
    zoom: 8,
    tile_size: tileSize,
    source_level: level,
    height_origin: heightOrigin,
    views: [],
  };
  const se = cameraEnabled
    ? cameraView(meta, tiles, camera)
    : { id: "se", angle: 0, iso_bounds: [...meta.iso_bounds], tiles };
  if (cameraEnabled) {
    plan.projection = camera;
    plan.source_meta = {
      ...meta,
      reference_iso_bounds: [...meta.iso_bounds],
      iso_bounds: [...se.iso_bounds],
      city_hall: [meta.city_hall[0] * camera.xScale, meta.city_hall[1] * camera.groundScale],
      landmarks: [],
      projection: camera,
    };
  }
  plan.views.push(se);
  // Cache inverse-projected leaf quads once, then reuse them for all future angles.
  const quads =
    angles.length > 1
      ? tiles.map((tile) =>
          corners(earthTileBounds(plan, se, tile)).map(([x, y]) => inverseIsometric(x, y, 0)),
        )
      : [];
  for (const angle of [...angles].sort((a, b) => a - b).filter((a) => a !== 0)) {
    const boxes = quads.map((quad) =>
      envelope(quad.flatMap(([x, y]) => [0, 400].map((h) => projectStatePlane(x, y, h, angle)))),
    );
    const tight = envelope(boxes.flatMap(corners));
    // Do not accidentally zoom in and triple the tile count when rotating a
    // long, narrow city. Preserve the first view's metres per output pixel.
    const referenceSide = Math.max(
      meta.iso_bounds[2] - meta.iso_bounds[0],
      meta.iso_bounds[3] - meta.iso_bounds[1],
    );
    const span = Math.max(referenceSide, tight[2] - tight[0], tight[3] - tight[1]);
    const cx = (tight[0] + tight[2]) / 2,
      cy = (tight[1] + tight[3]) / 2;
    const iso_bounds = [cx - span / 2, cy - span / 2, cx + span / 2, cy + span / 2];
    const side = span / 256;
    const keys = new Set();
    for (const [l, t, r, b] of boxes) {
      const x0 = Math.max(0, Math.floor((l - iso_bounds[0]) / side)),
        x1 = Math.min(255, Math.floor((r - iso_bounds[0]) / side));
      const y0 = Math.max(0, Math.floor((t - iso_bounds[1]) / side)),
        y1 = Math.min(255, Math.floor((b - iso_bounds[1]) / side));
      for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) keys.add(`${x}/${y}`);
    }
    plan.views.push({
      id: viewId(angle),
      angle,
      iso_bounds,
      tiles: [...keys]
        .map((key) => {
          const [x, y] = key.split("/").map(Number);
          return { x, y };
        })
        .sort(order),
    });
  }
  if (center || limit !== undefined) {
    const projected = center
      ? projectStatePlane(
          ...proj4("EPSG:4326", STATE_PLANE, center),
          0,
          0,
          cameraEnabled ? camera : undefined,
        )
      : null;
    const candidateTiles = se.tiles;
    const selected = candidateTiles.filter((tile) => {
      if (!projected) return true;
      const [l, t, r, b] = earthTileBounds(plan, se, tile);
      return (
        l <= projected[0] + radius * Math.SQRT2 &&
        r >= projected[0] - radius * Math.SQRT2 &&
        t <= projected[1] + radius &&
        b >= projected[1] - radius
      );
    });
    if (projected) {
      const distance = (tile) => {
        const [l, t, r, b] = earthTileBounds(plan, se, tile);
        return ((l + r) / 2 - projected[0]) ** 2 + ((t + b) / 2 - projected[1]) ** 2;
      };
      selected.sort((a, b) => distance(a) - distance(b) || order(a, b));
    }
    se.tiles = selected.slice(0, limit ?? selected.length).sort(order);
    requireValid(se.tiles.length > 0, "Pilot selects no source leaves");
    console.info(
      `Pilot selected ${se.tiles.length} leaves; omitted ${candidateTiles.length - se.tiles.length} of ${candidateTiles.length} source leaves (center/radius/limit).`,
    );
  }
  validateEarthPlan(plan);
  const file = join(root, "plan.json"),
    previous = await optionalBytes(file);
  if (previous) {
    requireValid(
      JSON.stringify(JSON.parse(previous)) === JSON.stringify(plan),
      "Earth plan changed; use a new root instead of mixing captures",
    );
    return validateEarthPlan(JSON.parse(previous));
  }
  await writeFile(file, `${JSON.stringify(plan, null, 2)}\n`, { flag: "wx" });
  return plan;
}
