// @ts-check

import { isometricLonLat, lightingState, solarPosition } from "./city-overlay.js";

/**
 * @typedef {{
 *   iso_bounds: [number, number, number, number],
 *   city_hall: [number, number] | null,
 *   landmarks: { name: string, point: [number, number], min_zoom: number, color: string }[],
 *   counts: { buildings: number, building_meshes: number },
 *   tile_version: string,
 *   max_tile_zoom: number,
 *   max_zoom: number,
 *   home_zoom: number,
 *   rich: { home_zoom: number, max_tile_zoom: number, views: RichView[] },
 * }} Meta
 */

/** @typedef {{ id: "se" | "sw" | "nw" | "ne", label: string, iso_bounds: [number, number, number, number], city_hall: [number, number] | null, landmarks: { name: string, point: [number, number], min_zoom: number, color: string }[], tile_version: string }} RichView */

/** @typedef {{ schema_version: 1, tile_version: string, tiles: string[] }} TileCoverage */

/** @typedef {{ name: string, kind: "planning_neighborhood" | "local_area", label: [number, number], rings: [number, number][][], source?: string, note?: string, priority?: number, display?: boolean, display_label?: string, display_tier?: 1 | 2 | 3, draw_geometry?: boolean, relevance?: string, rationale?: string, associations?: string[], planning_parents?: string[], suppresses?: string[], overlap_group?: string }} Neighborhood */
/** @typedef {{ source: string, disclaimer: string, features: Neighborhood[] }} Neighborhoods */
const canvasElement = document.querySelector("#map");
const statusElement = document.querySelector("#status");
const homeElement = document.querySelector("#home");
const rockyElement = document.querySelector("#rocky");
const zoomInElement = document.querySelector("#zoom-in");
const zoomOutElement = document.querySelector("#zoom-out");
const retryElement = document.querySelector("#retry");
const neighborhoodsElement = document.querySelector("#neighborhoods-toggle");
const localAreasElement = document.querySelector("#local-areas-toggle");
const colorElement = document.querySelector("#color-toggle");
const richElement = document.querySelector("#rich-toggle");
const rotateLeftElement = document.querySelector("#rotate-left");
const rotateRightElement = document.querySelector("#rotate-right");
const orientationElement = document.querySelector("#orientation");
const sunElement = document.querySelector("#sun-state");
if (
  !(canvasElement instanceof HTMLCanvasElement) ||
  !(statusElement instanceof HTMLSpanElement) ||
  !(homeElement instanceof HTMLButtonElement) ||
  !(rockyElement instanceof HTMLButtonElement) ||
  !(zoomInElement instanceof HTMLButtonElement) ||
  !(zoomOutElement instanceof HTMLButtonElement) ||
  !(retryElement instanceof HTMLButtonElement) ||
  !(neighborhoodsElement instanceof HTMLButtonElement) ||
  !(localAreasElement instanceof HTMLButtonElement) ||
  !(colorElement instanceof HTMLButtonElement) ||
  !(richElement instanceof HTMLButtonElement) ||
  !(rotateLeftElement instanceof HTMLButtonElement) ||
  !(rotateRightElement instanceof HTMLButtonElement) ||
  !(orientationElement instanceof HTMLSpanElement) ||
  !(sunElement instanceof HTMLSpanElement)
) {
  throw new Error("map controls are missing");
}
const canvas = canvasElement;
const statusText = statusElement;
const home = homeElement;
const rocky = rockyElement;
const zoomIn = zoomInElement;
const zoomOut = zoomOutElement;
const retry = retryElement;
const neighborhoodsToggle = neighborhoodsElement;
const localAreasToggle = localAreasElement;
const colorToggle = colorElement;
const richToggle = richElement;
const rotateLeft = rotateLeftElement;
const rotateRight = rotateRightElement;
const orientation = orientationElement;
const sunState = sunElement;
const context = canvas.getContext("2d");
if (context === null) throw new Error("2D canvas is unavailable");
const ctx = context;

/** @type {Meta | undefined} */
let meta;
/** @type {Set<string> | undefined} */
let availableTiles;
let zoom = 1;
let cameraX = 0;
let cameraY = 0;
let viewportWidth = 0;
let viewportHeight = 0;
/** @type {Map<number, { x: number, y: number }>} */
const pointers = new Map();
/** @type {{ distance: number, zoom: number, anchor: [number, number] } | undefined} */
let pinch;
/** @type {{ x: number, y: number } | undefined} */
let lastPointer;
let drawing = false;
/** @type {Neighborhoods | undefined} */
let neighborhoodData;
/** @type {WeakMap<Neighborhood, { rings: [number, number][][], bounds: [number, number, number, number] }>} */
const areaProjectionCache = new WeakMap();
let showNeighborhoods = false;
let showLocalAreas = false;
let vividColors = true;
let richMode = true;
let richViewIndex = 0;
/** @type {Map<string, HTMLImageElement>} */
const tiles = new Map();
/** @type {Map<string, { attempts: number, retryAt: number, terminal: boolean }>} */
const failures = new Map();
const MIN_ZOOM = 0.7;
const BASE_TILE_ZOOM = 2;
const PREVIEW_TILE_ZOOM = 5;
const MAX_CACHED_TILES = 512;
const MAX_TILE_ATTEMPTS = 4;
const PREFETCH_VIEW_LIMIT = 64;
let activeView = "";
/** @type {string | undefined} */
let scheduledPrefetch;
const prefetchedViews = new Set();
const LIGHTING_TIME = new URLSearchParams(location.search).get("time");

function city() {
  if (meta === undefined) throw new Error("city metadata is not loaded");
  return meta;
}

/** @returns {RichView | undefined} */
function richView() {
  return richMode ? city().rich.views[richViewIndex] : undefined;
}

/** @returns {[number, number, number, number]} */
function sceneBounds() {
  return richView()?.iso_bounds ?? city().iso_bounds;
}

/** @returns {[number, number] | null} */
function sceneCityHall() {
  return richView()?.city_hall ?? city().city_hall;
}

function sceneTileVersion() {
  return richView()?.tile_version ?? city().tile_version;
}

function sceneHomeZoom() {
  return richMode ? city().rich.home_zoom : city().home_zoom;
}

function sceneMaxTileZoom() {
  return richMode ? city().rich.max_tile_zoom : city().max_tile_zoom;
}

function sceneMaxZoom() {
  return richMode ? city().rich.max_tile_zoom + 1 : city().max_zoom;
}

function sceneKey() {
  return richView()?.id ?? "city";
}

function resize() {
  viewportWidth = innerWidth;
  viewportHeight = innerHeight;
  const pixelRatio = Math.min(devicePixelRatio, 2);
  canvas.width = Math.round(viewportWidth * pixelRatio);
  canvas.height = Math.round(viewportHeight * pixelRatio);
  canvas.style.width = `${viewportWidth}px`;
  canvas.style.height = `${viewportHeight}px`;
  ctx.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
  ctx.imageSmoothingEnabled = false;
  draw();
}

/** @param {number} z @param {number} x @param {number} y */
function key(z, x, y) {
  return `${sceneKey()}/${z}/${x}/${y}`;
}

/** @param {number} z @param {number} x @param {number} y */
function hasTile(z, x, y) {
  return availableTiles?.has(`${z}/${x}/${y}`) ?? false;
}

function pruneTiles() {
  while (tiles.size >= MAX_CACHED_TILES) {
    const oldest = tiles.keys().next().value;
    if (typeof oldest !== "string") return;
    tiles.delete(oldest);
  }
}

/** @param {HTMLImageElement} image */
function imageSettled(image) {
  if (image.complete) return Promise.resolve();
  return new Promise((resolve) => {
    image.addEventListener("load", resolve, { once: true });
    image.addEventListener("error", resolve, { once: true });
  });
}

/**
 * @param {string} view
 * @param {number} z
 * @param {{ x: number, y: number }[]} coordinates
 * @param {number} count
 */
function schedulePrefetch(view, z, coordinates, count) {
  if (coordinates.length === 0 || prefetchedViews.has(view) || scheduledPrefetch === view) return;
  scheduledPrefetch = view;
  canvas.dataset.prefetch = "scheduled";
  const run = () => {
    if (scheduledPrefetch === view) scheduledPrefetch = undefined;
    if (activeView !== view) return;
    prefetchedViews.add(view);
    while (prefetchedViews.size > PREFETCH_VIEW_LIMIT) {
      const oldest = prefetchedViews.values().next().value;
      if (typeof oldest !== "string") break;
      prefetchedViews.delete(oldest);
    }
    const visible = new Set(coordinates.map(({ x, y }) => `${x}/${y}`));
    const xs = coordinates.map(({ x }) => x);
    const ys = coordinates.map(({ y }) => y);
    const minX = Math.max(0, Math.min(...xs) - 1);
    const maxX = Math.min(count - 1, Math.max(...xs) + 1);
    const minY = Math.max(0, Math.min(...ys) - 1);
    const maxY = Math.min(count - 1, Math.max(...ys) + 1);
    const images = [];
    for (let y = minY; y <= maxY; y += 1) {
      for (let x = minX; x <= maxX; x += 1) {
        if (visible.has(`${x}/${y}`)) continue;
        const image = requestTile(z, x, y);
        if (image !== undefined) images.push(imageSettled(image));
      }
    }
    canvas.dataset.prefetch = images.length === 0 ? "ready" : "loading";
    void Promise.all(images).then(() => {
      if (activeView === view) canvas.dataset.prefetch = "ready";
    });
  };
  if ("requestIdleCallback" in window) window.requestIdleCallback(run, { timeout: 750 });
  else setTimeout(run, 50);
}

/** @param {number} z @param {number} x @param {number} y */
function requestTile(z, x, y) {
  const id = key(z, x, y);
  if (!hasTile(z, x, y)) {
    if (z > 0) {
      const parentZ = Math.min(z - 1, PREVIEW_TILE_ZOOM);
      requestTile(parentZ, x >> (z - parentZ), y >> (z - parentZ));
    }
    return undefined;
  }
  const cached = tiles.get(id);
  if (cached !== undefined) {
    tiles.delete(id);
    tiles.set(id, cached);
    return cached;
  }
  const failure = failures.get(id);
  if (failure?.terminal || (failure !== undefined && failure.retryAt > Date.now())) {
    return undefined;
  }
  if (z > 0) {
    const parentZ = Math.min(z - 1, PREVIEW_TILE_ZOOM);
    requestTile(parentZ, x >> (z - parentZ), y >> (z - parentZ));
  }
  pruneTiles();
  const image = new Image();
  image.onload = () => {
    failures.delete(id);
    draw();
  };
  image.onerror = () => {
    tiles.delete(id);
    const attempts = (failure?.attempts ?? 0) + 1;
    const terminal = attempts >= MAX_TILE_ATTEMPTS;
    const delay = Math.min(8_000, 500 * 2 ** (attempts - 1));
    failures.set(id, { attempts, retryAt: Date.now() + delay, terminal });
    if (terminal) retry.hidden = false;
    setTimeout(draw, terminal ? 0 : delay);
  };
  const prefix = richView() === undefined ? "" : `/rich/${richView()?.id}`;
  image.src = `${prefix}/tiles/${z}/${x}/${y}.webp?v=${encodeURIComponent(sceneTileVersion())}`;
  tiles.set(id, image);
  return image;
}

/** @param {number} z @param {number} x @param {number} y @param {{ left: number, top: number, width: number, height: number }} destination */
function drawParent(z, x, y, destination) {
  for (let parentZ = z - 1; parentZ >= 0; parentZ--) {
    const factor = 2 ** (z - parentZ);
    const image = tiles.get(key(parentZ, x >> (z - parentZ), y >> (z - parentZ)));
    if (image === undefined || !image.complete || !image.naturalWidth) continue;
    const sourceSize = 256 / factor;
    ctx.drawImage(
      image,
      (x % factor) * sourceSize,
      (y % factor) * sourceSize,
      sourceSize,
      sourceSize,
      destination.left,
      destination.top,
      destination.width,
      destination.height,
    );
    return true;
  }
  return false;
}

/** @param {number} panX @param {number} panY @param {number} tileSize @param {number} x @param {number} y */
function tileRectangle(panX, panY, tileSize, x, y) {
  const left = Math.round(panX + x * tileSize);
  const top = Math.round(panY + y * tileSize);
  const right = Math.round(panX + (x + 1) * tileSize);
  const bottom = Math.round(panY + (y + 1) * tileSize);
  return { left, top, width: right - left, height: bottom - top };
}

function draw() {
  if (drawing) return;
  drawing = true;
  requestAnimationFrame(() => {
    drawing = false;
    drawNow();
  });
}

function drawNow() {
  if (meta === undefined) return;
  const { counts } = city();
  const maxZoom = sceneMaxZoom();
  const maxTileZoom = sceneMaxTileZoom();
  const bounds = sceneBounds();
  const cityHall = sceneCityHall();
  ctx.fillStyle = "#d9d1c3";
  ctx.fillRect(0, 0, viewportWidth, viewportHeight);
  const viewZoom = Math.max(0, Math.min(maxZoom, Math.round(Math.log2(zoom) + BASE_TILE_ZOOM)));
  const z = Math.min(viewZoom, maxTileZoom);
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  const scale = worldScale();
  const count = 2 ** z;
  const tileSize = (side / count) * scale;
  clampCamera();
  const panX = viewportWidth / 2 - (cameraX - bounds[0]) * scale;
  const panY = viewportHeight / 2 - (cameraY - bounds[1]) * scale;
  const firstX = Math.floor(-panX / tileSize);
  const firstY = Math.floor(-panY / tileSize);
  const lastX = Math.ceil((viewportWidth - panX) / tileSize);
  const lastY = Math.ceil((viewportHeight - panY) / tileSize);
  const view = `${z}/${firstX}/${firstY}/${lastX}/${lastY}`;
  if (activeView !== view) {
    activeView = view;
    canvas.dataset.prefetch = "idle";
  }
  const centerTileX = (viewportWidth / 2 - panX) / tileSize;
  const centerTileY = (viewportHeight / 2 - panY) / tileSize;
  const coordinates = [];
  for (let y = Math.max(0, firstY); y < Math.min(count, lastY); y++) {
    for (let x = Math.max(0, firstX); x < Math.min(count, lastX); x++) {
      coordinates.push({ x, y });
    }
  }
  coordinates.sort(
    (left, right) =>
      (left.x - centerTileX) ** 2 +
      (left.y - centerTileY) ** 2 -
      ((right.x - centerTileX) ** 2 + (right.y - centerTileY) ** 2),
  );
  let requested = 0;
  let loaded = 0;
  let uncovered = 0;
  let failed = 0;
  for (const { x, y } of coordinates) {
    const present = hasTile(z, x, y);
    if (present) requested += 1;
    const destination = tileRectangle(panX, panY, tileSize, x, y);
    const image = requestTile(z, x, y);
    if (image?.complete && image.naturalWidth) {
      loaded += 1;
      ctx.drawImage(
        image,
        destination.left,
        destination.top,
        destination.width,
        destination.height,
      );
    } else {
      if (present && failures.get(key(z, x, y))?.terminal) failed += 1;
      if (present && !drawParent(z, x, y, destination)) {
        uncovered += 1;
      } else if (!present) {
        drawParent(z, x, y, destination);
      }
    }
  }
  drawLighting();
  canvas.dataset.areaLabels = "[]";
  canvas.dataset.planningAreas = "[]";
  canvas.dataset.localAreas = "[]";
  if ((showNeighborhoods || showLocalAreas) && !richMode) {
    drawAreaOverlays(viewZoom, panX, panY, scale);
  }
  if (cityHall !== null) drawCityHall(cityHall, panX, panY, scale);
  canvas.dataset.landmarks = JSON.stringify(drawLandmarks(viewZoom, panX, panY, scale));
  const pending = requested - loaded - failed;
  const scope =
    richView() === undefined
      ? `${counts.buildings.toLocaleString()} citywide buildings`
      : `Center City · ${richView()?.label}`;
  statusText.textContent = `${scope} · z${viewZoom}${pending > 0 ? ` · loading ${pending}` : ""}${failed > 0 ? ` · ${failed} failed` : ""}`;
  canvas.dataset.zoom = String(viewZoom);
  canvas.dataset.tileZoom = String(z);
  canvas.dataset.requested = String(requested);
  canvas.dataset.loaded = String(loaded);
  canvas.dataset.pending = String(pending);
  canvas.dataset.uncovered = String(uncovered);
  canvas.dataset.failed = String(failed);
  canvas.dataset.cameraX = String(cameraX);
  canvas.dataset.cameraY = String(cameraY);
  if (pending === 0 && failed === 0) schedulePrefetch(view, z, coordinates, count);
}

function drawLighting() {
  const requested = LIGHTING_TIME === null ? new Date() : new Date(LIGHTING_TIME);
  const instant = Number.isNaN(requested.getTime()) ? new Date() : requested;
  const solar = solarPosition(instant, 39.9526, -75.1652);
  const lighting = lightingState(solar.altitude);
  canvas.dataset.light = lighting.phase;
  sunState.textContent = `${lighting.phase} · sun ${Math.round(solar.altitude)}°`;
  if (lighting.alpha === 0) return;
  ctx.save();
  ctx.globalAlpha = lighting.alpha;
  ctx.fillStyle = lighting.color;
  ctx.fillRect(0, 0, viewportWidth, viewportHeight);
  ctx.restore();
}

/** @param {number} z @param {number} panX @param {number} panY @param {number} scale */
function drawAreaOverlays(z, panX, panY, scale) {
  if (neighborhoodData === undefined || z < 3) return;
  const planningAreas = showNeighborhoods
    ? neighborhoodData.features.filter(
        (area) =>
          area.kind === "planning_neighborhood" && areaNearViewport(area, panX, panY, scale),
      )
    : [];
  const localAreas = showLocalAreas
    ? neighborhoodData.features.filter(
        (area) =>
          area.kind === "local_area" &&
          localAreaVisible(area, z) &&
          areaNearViewport(area, panX, panY, scale),
      )
    : [];
  const suppressedParents = new Set(localAreas.flatMap((area) => area.suppresses ?? []));
  /** @type {{ area: Neighborhood, rings: { x: number, y: number }[][] }[]} */
  const projectedPlanning = planningAreas.map((area) => ({
    area,
    rings: projectArea(area, panX, panY, scale),
  }));
  /** @type {{ area: Neighborhood, rings: { x: number, y: number }[][] }[]} */
  const projectedLocal = localAreas.map((area) => ({
    area,
    rings: projectArea(area, panX, panY, scale),
  }));

  for (const projected of projectedPlanning) drawAreaGeometry(projected.rings, "planning");
  for (const projected of projectedLocal) {
    if (projected.area.draw_geometry === true) {
      drawAreaGeometry(projected.rings, "local");
    }
  }

  /** @type {{ left: number, right: number, top: number, bottom: number }[]} */
  const occupiedLabels = [];
  /** @type {string[]} */
  const paintedLabels = [];
  const labelBudget = Math.max(3, Math.floor((viewportWidth * viewportHeight) / 130_000));
  const areas = [
    ...localAreas,
    ...planningAreas.filter((area) => !suppressedParents.has(area.name)),
  ].toSorted((left, right) => {
    const priority = (right.priority ?? 0) - (left.priority ?? 0);
    if (priority !== 0) return priority;
    if (left.kind !== right.kind) return left.kind === "local_area" ? -1 : 1;
    return left.name.localeCompare(right.name);
  });
  for (const area of areas) {
    if (paintedLabels.length >= labelBudget) break;
    if (z < (area.kind === "local_area" ? 6 : 5)) continue;
    const [isoX, isoY] = isometricLonLat(area.label[0], area.label[1]);
    const x = panX + (isoX - city().iso_bounds[0]) * scale;
    const y = panY + (isoY - city().iso_bounds[1]) * scale;
    const topInset = viewportWidth <= 620 ? 64 : 76;
    if (x < 8 || y < topInset || x > viewportWidth - 8 || y > viewportHeight - 8) continue;
    const label = areaLabel(area);
    ctx.font =
      area.kind === "local_area"
        ? "600 11px ui-sans-serif, system-ui"
        : "500 10px ui-sans-serif, system-ui";
    const width = ctx.measureText(label).width;
    const labelBox = { left: x - 2, right: x + width + 2, top: y - 12, bottom: y + 3 };
    if (
      occupiedLabels.some(
        (occupied) =>
          labelBox.left < occupied.right &&
          labelBox.right > occupied.left &&
          labelBox.top < occupied.bottom &&
          labelBox.bottom > occupied.top,
      )
    ) {
      continue;
    }
    occupiedLabels.push(labelBox);
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#f6f0e6dd";
    ctx.strokeText(label, x, y);
    ctx.fillStyle = area.kind === "local_area" ? "#7a3e25" : "#302d28";
    ctx.fillText(label, x, y);
    paintedLabels.push(label);
  }
  canvas.dataset.areaLabels = JSON.stringify(paintedLabels);
  canvas.dataset.planningAreas = JSON.stringify(planningAreas.map((area) => area.name));
  canvas.dataset.localAreas = JSON.stringify(localAreas.map((area) => area.name));
}

/** @param {Neighborhood} area @param {number} z */
function localAreaVisible(area, z) {
  if (area.display !== true || area.display_tier === undefined) return false;
  return z >= (area.display_tier === 1 ? 6 : area.display_tier === 2 ? 7 : 8);
}

/** @param {Neighborhood} area */
function areaLabel(area) {
  return area.kind === "local_area" ? (area.display_label ?? area.name) : area.name;
}

/** @param {Neighborhood} area @param {number} panX @param {number} panY @param {number} scale */
function areaNearViewport(area, panX, panY, scale) {
  const projected = cachedAreaProjection(area);
  const left = panX + (projected.bounds[0] - city().iso_bounds[0]) * scale;
  const top = panY + (projected.bounds[1] - city().iso_bounds[1]) * scale;
  const right = panX + (projected.bounds[2] - city().iso_bounds[0]) * scale;
  const bottom = panY + (projected.bounds[3] - city().iso_bounds[1]) * scale;
  return right >= -80 && left <= viewportWidth + 80 && bottom >= 36 && top <= viewportHeight + 80;
}

/** @param {Neighborhood} area */
function cachedAreaProjection(area) {
  const cached = areaProjectionCache.get(area);
  if (cached !== undefined) return cached;
  const rings = area.rings.map((ring) => ring.map((point) => isometricLonLat(point[0], point[1])));
  const points = rings.flat();
  const projection = {
    rings,
    bounds: /** @type {[number, number, number, number]} */ ([
      Math.min(...points.map((point) => point[0])),
      Math.min(...points.map((point) => point[1])),
      Math.max(...points.map((point) => point[0])),
      Math.max(...points.map((point) => point[1])),
    ]),
  };
  areaProjectionCache.set(area, projection);
  return projection;
}

/** @param {Neighborhood} area @param {number} panX @param {number} panY @param {number} scale */
function projectArea(area, panX, panY, scale) {
  return cachedAreaProjection(area).rings.map((ring) =>
    ring.map(([isoX, isoY]) => {
      return {
        x: panX + (isoX - city().iso_bounds[0]) * scale,
        y: panY + (isoY - city().iso_bounds[1]) * scale,
      };
    }),
  );
}

/** @param {{ x: number, y: number }[][]} rings @param {"planning" | "local"} kind */
function drawAreaGeometry(rings, kind) {
  ctx.save();
  ctx.strokeStyle = kind === "local" ? "#e4935248" : "#24201d70";
  ctx.fillStyle = "#ee9b4f14";
  ctx.lineWidth = 0.8;
  for (const ring of rings) {
    if (ring.length === 0) continue;
    const xs = ring.map(({ x }) => x);
    const ys = ring.map(({ y }) => y);
    const visible =
      Math.max(...xs) >= -40 &&
      Math.min(...xs) <= viewportWidth + 40 &&
      Math.max(...ys) >= 36 &&
      Math.min(...ys) <= viewportHeight + 40;
    if (!visible) continue;
    ctx.beginPath();
    for (let index = 0; index < ring.length; index += 1) {
      const point = ring[index];
      if (point === undefined) continue;
      if (index === 0) ctx.moveTo(point.x, point.y);
      else ctx.lineTo(point.x, point.y);
    }
    ctx.closePath();
    if (kind === "local") ctx.fill();
    ctx.stroke();
  }
  ctx.restore();
}

/** @param {number} z @param {number} panX @param {number} panY @param {number} scale */
function drawLandmarks(z, panX, panY, scale) {
  const landmarks = richView()?.landmarks ?? city().landmarks;
  const painted = [];
  for (const landmark of landmarks) {
    if (z < landmark.min_zoom) continue;
    const x = panX + (landmark.point[0] - sceneBounds()[0]) * scale;
    const y = panY + (landmark.point[1] - sceneBounds()[1]) * scale;
    if (x < -100 || y < -30 || x > viewportWidth + 30 || y > viewportHeight + 30) continue;
    const pixelX = Math.round(x);
    const pixelY = Math.round(y);
    ctx.fillStyle = landmark.color;
    drawRaisedFigure(pixelX, pixelY);
    ctx.font = "600 12px ui-sans-serif, system-ui";
    ctx.lineJoin = "round";
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#f6f0e6";
    ctx.strokeText(landmark.name, pixelX + 10, pixelY - 11);
    ctx.fillStyle = "#191714";
    ctx.fillText(landmark.name, pixelX + 10, pixelY - 11);
    painted.push(landmark.name);
  }
  return painted;
}

/** Draw Rocky as a tiny raised-arms figure instead of an ambiguous map pin. */
/** @param {number} x @param {number} y */
function drawRaisedFigure(x, y) {
  ctx.fillRect(x - 1, y - 16, 3, 3);
  ctx.fillRect(x - 2, y - 12, 5, 7);
  ctx.fillRect(x - 5, y - 14, 3, 2);
  ctx.fillRect(x - 7, y - 16, 3, 2);
  ctx.fillRect(x + 3, y - 14, 3, 2);
  ctx.fillRect(x + 5, y - 16, 3, 2);
  ctx.fillRect(x - 2, y - 5, 2, 5);
  ctx.fillRect(x + 1, y - 5, 2, 5);
  ctx.fillRect(x - 4, y, 9, 2);
}

/** @param {[number, number]} cityHall @param {number} panX @param {number} panY @param {number} scale */
function drawCityHall(cityHall, panX, panY, scale) {
  const cityX = panX + (cityHall[0] - sceneBounds()[0]) * scale;
  const cityY = panY + (cityHall[1] - sceneBounds()[1]) * scale;
  if (cityX < -80 || cityY < -20 || cityX > viewportWidth + 20 || cityY > viewportHeight + 20) {
    return;
  }
  ctx.fillStyle = "#191714";
  ctx.beginPath();
  ctx.arc(cityX, cityY, 3.5, 0, 2 * Math.PI);
  ctx.fill();
  ctx.font = "600 12px ui-sans-serif, system-ui";
  ctx.lineJoin = "round";
  ctx.lineWidth = 3;
  ctx.strokeStyle = "#f6f0e6";
  ctx.strokeText("City Hall", cityX + 8, cityY - 8);
  ctx.fillStyle = "#191714";
  ctx.fillText("City Hall", cityX + 8, cityY - 8);
}

function worldScale() {
  const bounds = sceneBounds();
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  return (Math.min(viewportWidth, viewportHeight) * zoom) / side;
}

function clampCamera() {
  const [minX, minY, maxX, maxY] = sceneBounds();
  const scale = worldScale();
  /** @param {number} value @param {number} min @param {number} max @param {number} half */
  const clampAxis = (value, min, max, half) =>
    half * 2 >= max - min ? (min + max) / 2 : Math.max(min + half, Math.min(max - half, value));
  cameraX = clampAxis(cameraX, minX, maxX, viewportWidth / scale / 2);
  cameraY = clampAxis(cameraY, minY, maxY, viewportHeight / scale / 2);
}

/** @param {number} nextZoom */
function setZoom(nextZoom) {
  const maxZoom = 2 ** (sceneMaxZoom() - BASE_TILE_ZOOM);
  zoom = Math.max(MIN_ZOOM, Math.min(maxZoom, nextZoom));
  draw();
}

/** @param {number} tileZoom */
function centerCityHall(tileZoom = sceneHomeZoom()) {
  centerAt(sceneCityHall() ?? boundsCenter(), tileZoom);
}

function centerRocky() {
  const landmark = (richView()?.landmarks ?? city().landmarks).find(
    (candidate) => candidate.name === "Rocky",
  );
  if (landmark !== undefined) centerAt(landmark.point, sceneMaxZoom());
}

/** @returns {[number, number]} */
function boundsCenter() {
  const [minX, minY, maxX, maxY] = sceneBounds();
  return [(minX + maxX) / 2, (minY + maxY) / 2];
}

/** @param {[number, number]} point @param {number} tileZoom */
function centerAt(point, tileZoom) {
  if (meta === undefined) return;
  zoom = 2 ** (tileZoom - BASE_TILE_ZOOM);
  cameraX = point[0];
  cameraY = point[1];
  draw();
}

function initialTileZoom() {
  const requested = Number.parseInt(new URLSearchParams(location.search).get("z") ?? "", 10);
  return Number.isInteger(requested)
    ? Math.max(0, Math.min(sceneMaxZoom(), requested))
    : sceneHomeZoom();
}

/** @returns {[number, number]} */
function initialCenter() {
  const parameters = new URLSearchParams(location.search);
  const x = Number.parseFloat(parameters.get("cx") ?? "");
  const y = Number.parseFloat(parameters.get("cy") ?? "");
  return Number.isFinite(x) && Number.isFinite(y) ? [x, y] : (sceneCityHall() ?? boundsCenter());
}

function pointerDistance() {
  const [first, second] = [...pointers.values()];
  return first === undefined || second === undefined
    ? 0
    : Math.hypot(second.x - first.x, second.y - first.y);
}

function pointerMiddle() {
  const [first, second] = [...pointers.values()];
  return first === undefined || second === undefined
    ? { x: viewportWidth / 2, y: viewportHeight / 2 }
    : { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 };
}

function startPinch() {
  const middle = pointerMiddle();
  const scale = worldScale();
  pinch = {
    distance: pointerDistance(),
    zoom,
    anchor: [
      cameraX + (middle.x - viewportWidth / 2) / scale,
      cameraY + (middle.y - viewportHeight / 2) / scale,
    ],
  };
}

canvas.addEventListener("pointerdown", (event) => {
  canvas.focus({ preventScroll: true });
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  lastPointer = { x: event.clientX, y: event.clientY };
  if (pointers.size === 2) startPinch();
  canvas.classList.add("dragging");
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  if (!pointers.has(event.pointerId)) return;
  pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
  if (pointers.size >= 2 && pinch !== undefined) {
    const middle = pointerMiddle();
    setZoom(pinch.zoom * (pointerDistance() / Math.max(1, pinch.distance)));
    const scale = worldScale();
    cameraX = pinch.anchor[0] - (middle.x - viewportWidth / 2) / scale;
    cameraY = pinch.anchor[1] - (middle.y - viewportHeight / 2) / scale;
    draw();
    return;
  }
  if (lastPointer === undefined) return;
  const scale = worldScale();
  cameraX -= (event.clientX - lastPointer.x) / scale;
  cameraY -= (event.clientY - lastPointer.y) / scale;
  lastPointer = { x: event.clientX, y: event.clientY };
  draw();
});
/** @param {PointerEvent} event */
function stopDragging(event) {
  pointers.delete(event.pointerId);
  pinch = undefined;
  const remaining = [...pointers.values()][0];
  lastPointer = remaining;
  if (pointers.size === 0) canvas.classList.remove("dragging");
}
canvas.addEventListener("pointerup", stopDragging);
canvas.addEventListener("pointercancel", stopDragging);
canvas.addEventListener("lostpointercapture", stopDragging);
canvas.addEventListener("dblclick", () => setZoom(zoom * 2));
canvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const beforeScale = worldScale();
    const pointX = cameraX + (event.clientX - viewportWidth / 2) / beforeScale;
    const pointY = cameraY + (event.clientY - viewportHeight / 2) / beforeScale;
    const delta = event.deltaY * (event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 : 1);
    const factor = Math.max(0.75, Math.min(1.33, Math.exp(-delta * 0.0015)));
    setZoom(zoom * factor);
    const afterScale = worldScale();
    cameraX = pointX - (event.clientX - viewportWidth / 2) / afterScale;
    cameraY = pointY - (event.clientY - viewportHeight / 2) / afterScale;
    draw();
  },
  { passive: false },
);
addEventListener("keydown", (event) => {
  if (event.target !== canvas && event.target !== document.body) return;
  if (event.key === "+" || event.key === "=") setZoom(zoom * 2);
  else if (event.key === "-") setZoom(zoom / 2);
  else if (event.key === "0") centerCityHall();
  else if (event.key.toLowerCase() === "r") centerRocky();
  else if (event.key.startsWith("Arrow")) {
    const distance = 64 / worldScale();
    if (event.key === "ArrowLeft") cameraX -= distance;
    else if (event.key === "ArrowRight") cameraX += distance;
    else if (event.key === "ArrowUp") cameraY -= distance;
    else cameraY += distance;
    draw();
  } else return;
  event.preventDefault();
});
addEventListener("resize", resize);
home.addEventListener("click", () => centerCityHall());
rocky.addEventListener("click", centerRocky);
zoomIn.addEventListener("click", () => setZoom(zoom * 2));
zoomOut.addEventListener("click", () => setZoom(zoom / 2));
retry.addEventListener("click", () => {
  failures.clear();
  retry.hidden = true;
  draw();
});
neighborhoodsToggle.addEventListener("click", () => {
  showNeighborhoods = !showNeighborhoods;
  neighborhoodsToggle.setAttribute("aria-pressed", String(showNeighborhoods));
  draw();
});
localAreasToggle.addEventListener("click", () => {
  showLocalAreas = !showLocalAreas;
  localAreasToggle.setAttribute("aria-pressed", String(showLocalAreas));
  draw();
});
colorToggle.addEventListener("click", () => {
  vividColors = !vividColors;
  canvas.classList.toggle("vivid", vividColors);
  colorToggle.setAttribute("aria-pressed", String(vividColors));
  draw();
});
richToggle.addEventListener("click", () => void setRichMode(!richMode));
rotateLeft.addEventListener("click", () => void rotateRich(-1));
rotateRight.addEventListener("click", () => void rotateRich(1));

/** @param {boolean} enabled */
async function setRichMode(enabled) {
  if (meta === undefined || enabled === richMode) return;
  richMode = enabled;
  richToggle.setAttribute("aria-pressed", String(enabled));
  rotateLeft.hidden = !enabled;
  rotateRight.hidden = !enabled;
  neighborhoodsToggle.disabled = enabled;
  localAreasToggle.disabled = enabled;
  syncSceneUrl();
  await activateScene();
}

/** @param {number} direction */
async function rotateRich(direction) {
  if (!richMode || meta === undefined) return;
  richViewIndex = (richViewIndex + direction + city().rich.views.length) % city().rich.views.length;
  syncSceneUrl();
  await activateScene();
}

function syncSceneUrl() {
  const url = new URL(location.href);
  if (richMode) {
    url.searchParams.delete("mode");
    url.searchParams.set("view", richView()?.id ?? "se");
  } else {
    url.searchParams.set("mode", "city");
    url.searchParams.delete("view");
  }
  history.replaceState(null, "", url);
}

/** @param {boolean} initial */
async function activateScene(initial = false) {
  const expectedScene = sceneKey();
  availableTiles = undefined;
  activeView = "";
  scheduledPrefetch = undefined;
  failures.clear();
  const view = richView();
  const arrows = ["↗", "↘", "↙", "↖"];
  orientation.textContent =
    view === undefined ? "↗ N" : `${arrows[richViewIndex]} N · ${view.label}`;
  statusText.textContent =
    view === undefined ? "loading city…" : `loading ${view.label.toLowerCase()} view…`;
  const url = view === undefined ? "/coverage.json" : `/rich/${view.id}/coverage.json`;
  canvas.dataset.mode = view === undefined ? "city" : "detailed";
  canvas.dataset.view = view?.id ?? "city";
  try {
    const coverage = await loadCoverage(url, sceneTileVersion());
    if (sceneKey() !== expectedScene) return;
    availableTiles = new Set(coverage.tiles);
    retry.hidden = true;
    if (initial) centerAt(initialCenter(), initialTileZoom());
    else centerCityHall();
  } catch {
    if (sceneKey() === expectedScene) statusText.textContent = "This detailed view is unavailable.";
  }
}

/** @param {string} url @param {string} expectedVersion @returns {Promise<TileCoverage>} */
async function loadCoverage(url, expectedVersion) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`coverage request failed: ${response.status}`);
  /** @type {unknown} */
  const loaded = await response.json();
  if (!isTileCoverage(loaded) || loaded.tile_version !== expectedVersion) {
    throw new Error("coverage and scene versions do not match");
  }
  return loaded;
}

async function loadNeighborhoods() {
  try {
    const response = await fetch("/neighborhoods.json");
    if (!response.ok) throw new Error(`neighborhood request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isNeighborhoods(loaded)) throw new Error("neighborhood data has the wrong shape");
    neighborhoodData = loaded;
    neighborhoodsToggle.title = loaded.disclaimer;
    localAreasToggle.title = "Toggle selected local cultural and commercial areas";
    neighborhoodsToggle.disabled = richMode;
    localAreasToggle.disabled = richMode;
    draw();
  } catch {
    neighborhoodsToggle.disabled = true;
    localAreasToggle.disabled = true;
    neighborhoodsToggle.title = "Neighborhood boundaries are unavailable";
    localAreasToggle.title = "Local-area overlays are unavailable";
  }
}

async function loadMeta() {
  try {
    const response = await fetch("/meta");
    if (!response.ok) throw new Error(`metadata request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isMeta(loaded)) throw new Error("metadata response has the wrong shape");
    meta = loaded;
    const parameters = new URLSearchParams(location.search);
    richMode = parameters.get("mode") !== "city";
    const requestedView = parameters.get("view");
    const requestedIndex = loaded.rich.views.findIndex((view) => view.id === requestedView);
    richViewIndex = requestedIndex < 0 ? 0 : requestedIndex;
    richToggle.setAttribute("aria-pressed", String(richMode));
    rotateLeft.hidden = !richMode;
    rotateRight.hidden = !richMode;
    neighborhoodsToggle.disabled = richMode;
    localAreasToggle.disabled = richMode;
    resize();
    await activateScene(true);
    void loadNeighborhoods();
  } catch {
    statusText.textContent = "Run the ingest command to load city geometry.";
  }
}

/** @param {unknown} value @returns {value is TileCoverage} */
function isTileCoverage(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  if (
    candidate.schema_version !== 1 ||
    typeof candidate.tile_version !== "string" ||
    !Array.isArray(candidate.tiles) ||
    !candidate.tiles.every(isTileKey)
  ) {
    return false;
  }
  return new Set(candidate.tiles).size === candidate.tiles.length;
}

/** @param {unknown} value */
function isTileKey(value) {
  if (typeof value !== "string") return false;
  const match = /^(?<z>[0-8])\/(?<x>0|[1-9]\d*)\/(?<y>0|[1-9]\d*)$/.exec(value);
  if (match?.groups === undefined) return false;
  const z = Number(match.groups.z);
  const count = 2 ** z;
  return Number(match.groups.x) < count && Number(match.groups.y) < count;
}

/** @param {unknown} value @returns {value is Neighborhoods} */
function isNeighborhoods(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  return (
    typeof candidate.source === "string" &&
    typeof candidate.disclaimer === "string" &&
    Array.isArray(candidate.features) &&
    candidate.features.every((feature) => {
      if (typeof feature !== "object" || feature === null) return false;
      const area = /** @type {Record<string, unknown>} */ (feature);
      const validLocalPresentation =
        area.kind !== "local_area" ||
        (typeof area.display === "boolean" &&
          typeof area.display_label === "string" &&
          [1, 2, 3].includes(/** @type {number} */ (area.display_tier)) &&
          typeof area.draw_geometry === "boolean" &&
          typeof area.relevance === "string" &&
          typeof area.rationale === "string" &&
          Array.isArray(area.associations) &&
          area.associations.every((association) => typeof association === "string") &&
          Array.isArray(area.planning_parents) &&
          area.planning_parents.every((parent) => typeof parent === "string") &&
          Array.isArray(area.suppresses) &&
          area.suppresses.every((parent) => typeof parent === "string"));
      return (
        typeof area.name === "string" &&
        (area.kind === "planning_neighborhood" || area.kind === "local_area") &&
        (area.priority === undefined || Number.isFinite(area.priority)) &&
        Array.isArray(area.label) &&
        area.label.length === 2 &&
        area.label.every(Number.isFinite) &&
        Array.isArray(area.rings) &&
        validLocalPresentation
      );
    })
  );
}

/** @param {unknown} value @returns {value is Meta} */
function isMeta(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  return (
    Array.isArray(candidate.iso_bounds) &&
    candidate.iso_bounds.length === 4 &&
    candidate.iso_bounds.every(Number.isFinite) &&
    (candidate.city_hall === null ||
      (Array.isArray(candidate.city_hall) &&
        candidate.city_hall.length === 2 &&
        candidate.city_hall.every(Number.isFinite))) &&
    typeof candidate.counts === "object" &&
    candidate.counts !== null &&
    Number.isInteger(/** @type {Record<string, unknown>} */ (candidate.counts).buildings) &&
    Number.isInteger(/** @type {Record<string, unknown>} */ (candidate.counts).building_meshes) &&
    Array.isArray(candidate.landmarks) &&
    candidate.landmarks.every(isLandmark) &&
    typeof candidate.tile_version === "string" &&
    Number.isInteger(candidate.max_tile_zoom) &&
    Number.isInteger(candidate.max_zoom) &&
    Number.isInteger(candidate.home_zoom) &&
    Number.isInteger(candidate.max_tile_zoom) &&
    isRichScene(candidate.rich)
  );
}

/** @param {unknown} value */
function isRichScene(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  const ids = ["se", "sw", "nw", "ne"];
  return (
    Number.isInteger(candidate.home_zoom) &&
    Array.isArray(candidate.views) &&
    candidate.views.length === ids.length &&
    candidate.views.every((view, index) => {
      if (typeof view !== "object" || view === null) return false;
      const item = /** @type {Record<string, unknown>} */ (view);
      return (
        item.id === ids[index] &&
        typeof item.label === "string" &&
        typeof item.tile_version === "string" &&
        Array.isArray(item.iso_bounds) &&
        item.iso_bounds.length === 4 &&
        item.iso_bounds.every(Number.isFinite) &&
        Array.isArray(item.landmarks) &&
        item.landmarks.every(isLandmark) &&
        (item.city_hall === null ||
          (Array.isArray(item.city_hall) &&
            item.city_hall.length === 2 &&
            item.city_hall.every(Number.isFinite)))
      );
    })
  );
}

/** @param {unknown} value */
function isLandmark(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  return (
    typeof candidate.name === "string" &&
    Array.isArray(candidate.point) &&
    candidate.point.length === 2 &&
    candidate.point.every(Number.isFinite) &&
    Number.isInteger(candidate.min_zoom) &&
    typeof candidate.color === "string"
  );
}

setInterval(draw, 60_000);
void loadMeta();
