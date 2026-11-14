import { createHash } from "node:crypto";
import { isometricLonLat } from "../static/city-overlay.js";

export const STYLE = "isophilly-google-v1";
export const TILE_SIZE = 256;
export const ZOOM = 8;
export const STATE_PLANE =
  "+proj=lcc +lat_1=40.96666666666667 +lat_2=39.93333333333333 +lat_0=39.33333333333334 +lon_0=-77.75 +x_0=600000 +y_0=0 +ellps=GRS80 +units=m +no_defs";

export function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

export function validatePlan(value) {
  if (
    value?.schema_version !== 1 ||
    value.zoom !== ZOOM ||
    value.tile_size !== TILE_SIZE ||
    value.style !== STYLE ||
    !/^[a-f0-9]{64}$/.test(value.world_sha256) ||
    !Array.isArray(value.iso_bounds) ||
    value.iso_bounds.length !== 4 ||
    !value.iso_bounds.every(Number.isFinite) ||
    value.iso_bounds[0] >= value.iso_bounds[2] ||
    value.iso_bounds[1] >= value.iso_bounds[3] ||
    !Array.isArray(value.tiles) ||
    value.tiles.length === 0 ||
    value.tiles.length > 65536
  )
    throw new Error("Invalid Google imagery plan. Run google:plan against the current world.");
  const keys = new Set();
  for (const tile of value.tiles) {
    if (
      !Number.isInteger(tile.x) ||
      !Number.isInteger(tile.y) ||
      tile.x < 0 ||
      tile.x >= 256 ||
      tile.y < 0 ||
      tile.y >= 256 ||
      keys.has(`${tile.x}/${tile.y}`)
    )
      throw new Error("Invalid or duplicate capture tile.");
    keys.add(`${tile.x}/${tile.y}`);
  }
  return value;
}

export function tileBounds(plan, tile) {
  const [left, top, right, bottom] = plan.iso_bounds;
  // Match Bounds::tile's f32 arithmetic, including rounding at City-scale coordinates.
  const f32 = Math.fround;
  const side = f32(Math.max(f32(right - left), f32(bottom - top)) / 2 ** ZOOM);
  return [
    f32(left + f32(tile.x * side)),
    f32(top + f32(tile.y * side)),
    f32(left + f32((tile.x + 1) * side)),
    f32(top + f32((tile.y + 1) * side)),
  ];
}

/** Select whole canonical tiles. No differently sized neighborhood raster is introduced. */
export function selectTiles(plan, center, radius, limit) {
  validatePlan(plan);
  if (
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > 65536 ||
    !Number.isFinite(radius) ||
    radius <= 0
  ) {
    throw new Error("Capture limit and radius must be positive.");
  }
  if (
    center &&
    (center.length !== 2 ||
      !center.every(Number.isFinite) ||
      center[0] < -75.4 ||
      center[0] > -74.9 ||
      center[1] < 39.8 ||
      center[1] > 40.2)
  ) {
    throw new Error("Center must be longitude,latitude in Philadelphia.");
  }
  const projected = center ? isometricLonLat(...center) : null;
  const selected = plan.tiles.filter((tile) => {
    if (!projected) return true;
    const [left, top, right, bottom] = tileBounds(plan, tile);
    // The isometric horizontal axis is sqrt(2) times the source ground distance.
    return (
      left <= projected[0] + radius * Math.SQRT2 &&
      right >= projected[0] - radius * Math.SQRT2 &&
      top <= projected[1] + radius &&
      bottom >= projected[1] - radius
    );
  });
  if (projected)
    selected.sort((a, b) => {
      const distance = (tile) => {
        const [l, t, r, b] = tileBounds(plan, tile);
        return ((l + r) / 2 - projected[0]) ** 2 + ((t + b) / 2 - projected[1]) ** 2;
      };
      return distance(a) - distance(b) || a.y - b.y || a.x - b.x;
    });
  if (!selected.length)
    throw new Error("No eligible tiles here. Existing photographed meshes are protected.");
  return { tiles: selected.slice(0, limit), omitted: Math.max(0, selected.length - limit) };
}

export function validateManifest(manifest, plan, capture) {
  validatePlan(manifest);
  for (const key of ["world_sha256", "iso_bounds", "zoom", "tile_size", "style"]) {
    if (JSON.stringify(manifest[key]) !== JSON.stringify(plan[key])) {
      throw new Error(
        "Capture source changed. Use a new output directory rather than mixing grids.",
      );
    }
  }
  if (JSON.stringify(manifest.capture) !== JSON.stringify(capture)) {
    throw new Error(
      "Capture settings changed. Use a new output directory rather than mixing cameras.",
    );
  }
  for (const tile of manifest.tiles) {
    if (
      !/^[a-f0-9]{64}$/.test(tile.sha256) ||
      !Array.isArray(tile.attributions) ||
      tile.attributions.length < 2 ||
      !tile.attributions.includes("Google Maps") ||
      !tile.attributions.every((v) => typeof v === "string" && v.trim()) ||
      typeof tile.captured_at !== "string" ||
      !Number.isFinite(Date.parse(tile.captured_at))
    ) {
      throw new Error("Invalid capture provenance.");
    }
  }
  return manifest;
}

/** Reject holes and an entirely empty capture before publishing its manifest entry. */
export function validateCaptureResult(result) {
  if (
    result?.opaquePixels !== TILE_SIZE * TILE_SIZE ||
    result.visibleTiles < 1 ||
    !Number.isFinite(result.maxError) ||
    result.maxError > 4 ||
    !Array.isArray(result.attributions) ||
    result.attributions.length < 2 ||
    !result.attributions.includes("Google Maps") ||
    !result.attributions.every((v) => typeof v === "string" && v.trim())
  ) {
    throw new Error(
      "Capture has holes, missing attribution, or insufficient detail. Nothing was published.",
    );
  }
}
