// @ts-check

import {
  isometricLonLat,
  isVehicleSnapshot,
  lightingState,
  solarPosition,
} from "./city-overlay.js";

/**
 * @typedef {{
 *   iso_bounds: [number, number, number, number],
 *   city_hall: [number, number] | null,
 *   landmarks: { name: string, point: [number, number], min_zoom: number, color: string }[],
 *   counts: { buildings: number, building_parts: number, building_meshes: number, water: number, parks: number, streets: number },
 *   tile_version: string,
 *   max_tile_zoom: number,
 *   max_zoom: number,
 *   home_zoom: number,
 * }} Meta
 */

/** @typedef {{ name: string, kind: "planning_neighborhood" | "cultural_area", label: [number, number], rings: [number, number][][], note?: string }} Neighborhood */
/** @typedef {{ source: string, disclaimer: string, features: Neighborhood[] }} Neighborhoods */
/** @typedef {{ id: string, mode: "surface" | "regional_rail", route: string, label: string, destination: string, latitude: number, longitude: number, heading: number }} Vehicle */

const canvasElement = document.querySelector("#map");
const statusElement = document.querySelector("#status");
const homeElement = document.querySelector("#home");
const zoomInElement = document.querySelector("#zoom-in");
const zoomOutElement = document.querySelector("#zoom-out");
const retryElement = document.querySelector("#retry");
const transitElement = document.querySelector("#transit-toggle");
const neighborhoodsElement = document.querySelector("#neighborhoods-toggle");
const sunElement = document.querySelector("#sun-state");
if (
  !(canvasElement instanceof HTMLCanvasElement) ||
  !(statusElement instanceof HTMLSpanElement) ||
  !(homeElement instanceof HTMLButtonElement) ||
  !(zoomInElement instanceof HTMLButtonElement) ||
  !(zoomOutElement instanceof HTMLButtonElement) ||
  !(retryElement instanceof HTMLButtonElement) ||
  !(transitElement instanceof HTMLButtonElement) ||
  !(neighborhoodsElement instanceof HTMLButtonElement) ||
  !(sunElement instanceof HTMLSpanElement)
) {
  throw new Error("map controls are missing");
}
const canvas = canvasElement;
const statusText = statusElement;
const home = homeElement;
const zoomIn = zoomInElement;
const zoomOut = zoomOutElement;
const retry = retryElement;
const transitToggle = transitElement;
const neighborhoodsToggle = neighborhoodsElement;
const sunState = sunElement;
const context = canvas.getContext("2d");
if (context === null) throw new Error("2D canvas is unavailable");
const ctx = context;

/** @type {Meta | undefined} */
let meta;
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
let showNeighborhoods = false;
let showTransit = true;
/** @type {Map<string, { vehicle: Vehicle, from: [number, number], to: [number, number], changedAt: number }>} */
let liveVehicles = new Map();
let vehicleUpdatedAt = 0;
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
const VEHICLE_INTERPOLATION_MS = 15_000;
const LIGHTING_TIME = new URLSearchParams(location.search).get("time");

function city() {
  if (meta === undefined) throw new Error("city metadata is not loaded");
  return meta;
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
  return `${z}/${x}/${y}`;
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
  image.src = `/tiles/${z}/${x}/${y}.webp?v=${encodeURIComponent(city().tile_version)}`;
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
  const {
    iso_bounds: bounds,
    city_hall: cityHall,
    counts,
    max_tile_zoom: maxTileZoom,
    max_zoom: maxZoom,
  } = city();
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
    requested += 1;
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
      if (failures.get(key(z, x, y))?.terminal) failed += 1;
      if (!drawParent(z, x, y, destination)) {
        uncovered += 1;
      }
    }
  }
  drawLighting();
  if (showNeighborhoods) drawNeighborhoods(viewZoom, panX, panY, scale);
  if (showTransit) drawVehicles(viewZoom, panX, panY, scale);
  if (cityHall !== null) drawCityHall(cityHall, panX, panY, scale);
  drawLandmarks(viewZoom, panX, panY, scale);
  const pending = requested - loaded - failed;
  statusText.textContent = `${counts.buildings.toLocaleString()} buildings · z${viewZoom}${pending > 0 ? ` · loading ${pending}` : ""}${failed > 0 ? ` · ${failed} failed` : ""}`;
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
function drawNeighborhoods(z, panX, panY, scale) {
  if (neighborhoodData === undefined || z < 3) return;
  for (const area of neighborhoodData.features) {
    ctx.save();
    ctx.strokeStyle = area.kind === "cultural_area" ? "#f2ad66" : "#2d2924aa";
    ctx.lineWidth = area.kind === "cultural_area" ? 2 : 1;
    if (area.kind === "cultural_area") ctx.setLineDash([5, 4]);
    for (const ring of area.rings) {
      ctx.beginPath();
      for (let index = 0; index < ring.length; index += 1) {
        const point = ring[index];
        if (point === undefined) continue;
        const [isoX, isoY] = isometricLonLat(point[0], point[1]);
        const x = panX + (isoX - city().iso_bounds[0]) * scale;
        const y = panY + (isoY - city().iso_bounds[1]) * scale;
        if (index === 0) ctx.moveTo(x, y);
        else ctx.lineTo(x, y);
      }
      ctx.stroke();
    }
    ctx.restore();
    if (z < 5) continue;
    const [isoX, isoY] = isometricLonLat(area.label[0], area.label[1]);
    const x = panX + (isoX - city().iso_bounds[0]) * scale;
    const y = panY + (isoY - city().iso_bounds[1]) * scale;
    if (x < -80 || y < -20 || x > viewportWidth + 80 || y > viewportHeight + 20) continue;
    const label = area.kind === "cultural_area" ? `${area.name} · cultural area` : area.name;
    ctx.font =
      area.kind === "cultural_area"
        ? "600 11px ui-sans-serif, system-ui"
        : "500 10px ui-sans-serif, system-ui";
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#f6f0e6dd";
    ctx.strokeText(label, x, y);
    ctx.fillStyle = area.kind === "cultural_area" ? "#7a3e25" : "#302d28";
    ctx.fillText(label, x, y);
  }
}

/** @param {number} z @param {number} panX @param {number} panY @param {number} scale */
function drawVehicles(z, panX, panY, scale) {
  if (z < 4) return;
  const now = Date.now();
  for (const current of liveVehicles.values()) {
    const progress = Math.min(1, Math.max(0, (now - current.changedAt) / VEHICLE_INTERPOLATION_MS));
    const isoX = current.from[0] + (current.to[0] - current.from[0]) * progress;
    const isoY = current.from[1] + (current.to[1] - current.from[1]) * progress;
    const x = panX + (isoX - city().iso_bounds[0]) * scale;
    const y = panY + (isoY - city().iso_bounds[1]) * scale;
    if (x < -15 || y < -15 || x > viewportWidth + 15 || y > viewportHeight + 15) continue;
    ctx.save();
    ctx.fillStyle = current.vehicle.mode === "regional_rail" ? "#54c5d2" : "#f2ad50";
    ctx.strokeStyle = "#191714";
    ctx.lineWidth = 1.5;
    ctx.translate(Math.round(x), Math.round(y));
    ctx.rotate((current.vehicle.heading * Math.PI) / 180);
    ctx.beginPath();
    if (current.vehicle.mode === "regional_rail") {
      ctx.moveTo(0, -5);
      ctx.lineTo(5, 0);
      ctx.lineTo(0, 5);
      ctx.lineTo(-5, 0);
    } else {
      ctx.moveTo(0, -5);
      ctx.lineTo(4, 4);
      ctx.lineTo(-4, 4);
    }
    ctx.closePath();
    ctx.fill();
    ctx.stroke();
    ctx.restore();
    if (z >= 7) {
      const label = current.vehicle.route || current.vehicle.label;
      ctx.font = "600 9px ui-sans-serif, system-ui";
      ctx.fillStyle = "#191714";
      ctx.fillText(label, x + 6, y - 5);
    }
  }
  canvas.dataset.vehicles = String(liveVehicles.size);
  canvas.dataset.vehiclesUpdated = String(vehicleUpdatedAt);
}

/** @param {number} z @param {number} panX @param {number} panY @param {number} scale */
function drawLandmarks(z, panX, panY, scale) {
  for (const landmark of city().landmarks) {
    if (z < landmark.min_zoom) continue;
    const x = panX + (landmark.point[0] - city().iso_bounds[0]) * scale;
    const y = panY + (landmark.point[1] - city().iso_bounds[1]) * scale;
    if (x < -100 || y < -30 || x > viewportWidth + 30 || y > viewportHeight + 30) continue;
    ctx.fillStyle = landmark.color;
    ctx.fillRect(Math.round(x) - 3, Math.round(y) - 7, 7, 7);
    ctx.font = "600 12px ui-sans-serif, system-ui";
    ctx.lineJoin = "round";
    ctx.lineWidth = 3;
    ctx.strokeStyle = "#f6f0e6";
    ctx.strokeText(landmark.name, x + 8, y - 8);
    ctx.fillStyle = "#191714";
    ctx.fillText(landmark.name, x + 8, y - 8);
  }
}

/** @param {[number, number]} cityHall @param {number} panX @param {number} panY @param {number} scale */
function drawCityHall(cityHall, panX, panY, scale) {
  const cityX = panX + (cityHall[0] - city().iso_bounds[0]) * scale;
  const cityY = panY + (cityHall[1] - city().iso_bounds[1]) * scale;
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
  const bounds = city().iso_bounds;
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  return (Math.min(viewportWidth, viewportHeight) * zoom) / side;
}

function clampCamera() {
  const [minX, minY, maxX, maxY] = city().iso_bounds;
  const scale = worldScale();
  /** @param {number} value @param {number} min @param {number} max @param {number} half */
  const clampAxis = (value, min, max, half) =>
    half * 2 >= max - min ? (min + max) / 2 : Math.max(min + half, Math.min(max - half, value));
  cameraX = clampAxis(cameraX, minX, maxX, viewportWidth / scale / 2);
  cameraY = clampAxis(cameraY, minY, maxY, viewportHeight / scale / 2);
}

/** @param {number} nextZoom */
function setZoom(nextZoom) {
  const maxZoom = 2 ** (city().max_zoom - BASE_TILE_ZOOM);
  zoom = Math.max(MIN_ZOOM, Math.min(maxZoom, nextZoom));
  draw();
}

/** @param {number} tileZoom */
function centerCityHall(tileZoom = city().home_zoom) {
  centerAt(city().city_hall ?? boundsCenter(), tileZoom);
}

/** @returns {[number, number]} */
function boundsCenter() {
  const [minX, minY, maxX, maxY] = city().iso_bounds;
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
    ? Math.max(0, Math.min(city().max_zoom, requested))
    : city().home_zoom;
}

/** @returns {[number, number]} */
function initialCenter() {
  const parameters = new URLSearchParams(location.search);
  const x = Number.parseFloat(parameters.get("cx") ?? "");
  const y = Number.parseFloat(parameters.get("cy") ?? "");
  return Number.isFinite(x) && Number.isFinite(y) ? [x, y] : (city().city_hall ?? boundsCenter());
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
zoomIn.addEventListener("click", () => setZoom(zoom * 2));
zoomOut.addEventListener("click", () => setZoom(zoom / 2));
retry.addEventListener("click", () => {
  failures.clear();
  retry.hidden = true;
  draw();
});
transitToggle.addEventListener("click", () => {
  showTransit = !showTransit;
  transitToggle.setAttribute("aria-pressed", String(showTransit));
  if (showTransit && liveVehicles.size === 0) void loadVehicles();
  draw();
});
neighborhoodsToggle.addEventListener("click", () => {
  showNeighborhoods = !showNeighborhoods;
  neighborhoodsToggle.setAttribute("aria-pressed", String(showNeighborhoods));
  draw();
});

async function loadNeighborhoods() {
  try {
    const response = await fetch("/neighborhoods.json");
    if (!response.ok) throw new Error(`neighborhood request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isNeighborhoods(loaded)) throw new Error("neighborhood data has the wrong shape");
    neighborhoodData = loaded;
    neighborhoodsToggle.title = loaded.disclaimer;
    draw();
  } catch {
    neighborhoodsToggle.disabled = true;
    neighborhoodsToggle.title = "Neighborhood boundaries are unavailable";
  }
}

async function loadVehicles() {
  try {
    const response = await fetch("/api/vehicles");
    if (!response.ok) throw new Error(`vehicle request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isVehicleSnapshot(loaded)) throw new Error("vehicle data has the wrong shape");
    const now = Date.now();
    const next = new Map();
    for (const vehicle of loaded.vehicles) {
      const target = isometricLonLat(vehicle.longitude, vehicle.latitude);
      const previous = liveVehicles.get(vehicle.id);
      let from = target;
      if (previous !== undefined) {
        const progress = Math.min(
          1,
          Math.max(0, (now - previous.changedAt) / VEHICLE_INTERPOLATION_MS),
        );
        from = [
          previous.from[0] + (previous.to[0] - previous.from[0]) * progress,
          previous.from[1] + (previous.to[1] - previous.from[1]) * progress,
        ];
      }
      next.set(vehicle.id, { vehicle, from, to: target, changedAt: now });
    }
    liveVehicles = next;
    vehicleUpdatedAt = loaded.updated_at;
    transitToggle.title = loaded.stale
      ? `${loaded.vehicles.length} SEPTA vehicles · cached data`
      : `${loaded.vehicles.length} live SEPTA vehicles`;
    draw();
  } catch {
    transitToggle.title = "Live SEPTA positions are temporarily unavailable";
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
    retry.hidden = true;
    resize();
    centerAt(initialCenter(), initialTileZoom());
    void loadNeighborhoods();
    void loadVehicles();
  } catch {
    statusText.textContent = "Run the ingest command to load city geometry.";
  }
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
      return (
        typeof area.name === "string" &&
        (area.kind === "planning_neighborhood" || area.kind === "cultural_area") &&
        Array.isArray(area.label) &&
        area.label.length === 2 &&
        area.label.every(Number.isFinite) &&
        Array.isArray(area.rings)
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
    Number.isInteger(/** @type {Record<string, unknown>} */ (candidate.counts).building_parts) &&
    Number.isInteger(/** @type {Record<string, unknown>} */ (candidate.counts).building_meshes) &&
    Array.isArray(candidate.landmarks) &&
    candidate.landmarks.every(isLandmark) &&
    typeof candidate.tile_version === "string" &&
    Number.isInteger(candidate.max_zoom) &&
    Number.isInteger(candidate.home_zoom)
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

setInterval(() => {
  if (showTransit) draw();
}, 1_000);
setInterval(() => {
  if (showTransit) void loadVehicles();
}, 15_000);
setInterval(draw, 60_000);
void loadMeta();
