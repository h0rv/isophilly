// @ts-check

/**
 * @typedef {{
 *   iso_bounds: [number, number, number, number],
 *   city_hall: [number, number],
 *   counts: { buildings: number, water: number, parks: number, streets: number },
 *   tile_version: string,
 *   max_zoom: number,
 *   home_zoom: number,
 *   texture: "none" | "full" | "pixel",
 * }} Meta
 */

const canvasElement = document.querySelector("#map");
const statusElement = document.querySelector("#status");
const homeElement = document.querySelector("#home");
const zoomInElement = document.querySelector("#zoom-in");
const zoomOutElement = document.querySelector("#zoom-out");
if (
  !(canvasElement instanceof HTMLCanvasElement) ||
  !(statusElement instanceof HTMLSpanElement) ||
  !(homeElement instanceof HTMLButtonElement) ||
  !(zoomInElement instanceof HTMLButtonElement) ||
  !(zoomOutElement instanceof HTMLButtonElement)
) {
  throw new Error("map controls are missing");
}
const canvas = canvasElement;
const statusText = statusElement;
const home = homeElement;
const zoomIn = zoomInElement;
const zoomOut = zoomOutElement;
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
let dragging = false;
/** @type {PointerEvent | undefined} */
let lastPointer;
let drawing = false;
/** @type {Map<string, HTMLImageElement>} */
const tiles = new Map();
const MIN_ZOOM = 0.7;
const BASE_TILE_ZOOM = 2;
const MAX_CACHED_TILES = 512;

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

/** @param {number} z @param {number} x @param {number} y */
function requestTile(z, x, y) {
  const id = key(z, x, y);
  const cached = tiles.get(id);
  if (cached !== undefined) {
    tiles.delete(id);
    tiles.set(id, cached);
    return cached;
  }
  if (z > 0) requestTile(z - 1, x >> 1, y >> 1);
  pruneTiles();
  const image = new Image();
  image.onload = draw;
  image.onerror = () => {
    tiles.delete(id);
    setTimeout(draw, 250);
  };
  image.src = `/tiles/${z}/${x}/${y}.png?v=${encodeURIComponent(city().tile_version)}`;
  tiles.set(id, image);
  return image;
}

/** @param {number} z @param {number} x @param {number} y @param {number} left @param {number} top @param {number} size */
function drawParent(z, x, y, left, top, size) {
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
      Math.floor(left),
      Math.floor(top),
      Math.ceil(size) + 1,
      Math.ceil(size) + 1,
    );
    return true;
  }
  return false;
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
  const { iso_bounds: bounds, city_hall: cityHall, counts, max_zoom: maxZoom, texture } = city();
  ctx.fillStyle = "#d9d1c3";
  ctx.fillRect(0, 0, viewportWidth, viewportHeight);
  const z = Math.max(0, Math.min(maxZoom, Math.round(Math.log2(zoom) + BASE_TILE_ZOOM)));
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  const scale = worldScale();
  const count = 2 ** z;
  const tileSize = (side / count) * scale;
  clampCamera();
  const panX = viewportWidth / 2 - (cameraX - bounds[0]) * scale;
  const panY = viewportHeight / 2 - (cameraY - bounds[1]) * scale;
  const firstX = Math.floor(-panX / tileSize) - 1;
  const firstY = Math.floor(-panY / tileSize) - 1;
  const lastX = Math.ceil((viewportWidth - panX) / tileSize) + 1;
  const lastY = Math.ceil((viewportHeight - panY) / tileSize) + 1;
  let requested = 0;
  let loaded = 0;
  let uncovered = 0;
  for (let y = Math.max(0, firstY); y < Math.min(count, lastY); y++) {
    for (let x = Math.max(0, firstX); x < Math.min(count, lastX); x++) {
      requested += 1;
      const left = panX + x * tileSize;
      const top = panY + y * tileSize;
      const image = requestTile(z, x, y);
      if (image.complete && image.naturalWidth) {
        loaded += 1;
        ctx.drawImage(
          image,
          Math.floor(left),
          Math.floor(top),
          Math.ceil(tileSize) + 1,
          Math.ceil(tileSize) + 1,
        );
      } else if (!drawParent(z, x, y, left, top, tileSize)) {
        uncovered += 1;
      }
    }
  }
  drawCityHall(cityHall, panX, panY, scale);
  statusText.textContent = `${counts.buildings.toLocaleString()} buildings · ${texture} · z${z}`;
  canvas.dataset.zoom = String(z);
  canvas.dataset.requested = String(requested);
  canvas.dataset.pending = String(requested - loaded);
  canvas.dataset.uncovered = String(uncovered);
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
  if (meta === undefined) return;
  zoom = 2 ** (tileZoom - BASE_TILE_ZOOM);
  cameraX = meta.city_hall[0];
  cameraY = meta.city_hall[1];
  draw();
}

function initialTileZoom() {
  const requested = Number.parseInt(new URLSearchParams(location.search).get("z") ?? "", 10);
  return Number.isInteger(requested)
    ? Math.max(0, Math.min(city().max_zoom, requested))
    : city().home_zoom;
}

canvas.addEventListener("pointerdown", (event) => {
  dragging = true;
  lastPointer = event;
  canvas.classList.add("dragging");
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  if (!dragging || lastPointer === undefined) return;
  const scale = worldScale();
  cameraX -= (event.clientX - lastPointer.clientX) / scale;
  cameraY -= (event.clientY - lastPointer.clientY) / scale;
  lastPointer = event;
  draw();
});
function stopDragging() {
  dragging = false;
  lastPointer = undefined;
  canvas.classList.remove("dragging");
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

async function loadMeta() {
  try {
    const response = await fetch("/meta");
    if (!response.ok) throw new Error(`metadata request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isMeta(loaded)) throw new Error("metadata response has the wrong shape");
    meta = loaded;
    resize();
    centerCityHall(initialTileZoom());
  } catch {
    statusText.textContent = "Run the ingest command to load city geometry.";
  }
}

/** @param {unknown} value @returns {value is Meta} */
function isMeta(value) {
  if (typeof value !== "object" || value === null) return false;
  const candidate = /** @type {Record<string, unknown>} */ (value);
  return (
    Array.isArray(candidate.iso_bounds) &&
    candidate.iso_bounds.length === 4 &&
    candidate.iso_bounds.every(Number.isFinite) &&
    Array.isArray(candidate.city_hall) &&
    candidate.city_hall.length === 2 &&
    candidate.city_hall.every(Number.isFinite) &&
    typeof candidate.counts === "object" &&
    candidate.counts !== null &&
    typeof candidate.tile_version === "string" &&
    Number.isInteger(candidate.max_zoom) &&
    Number.isInteger(candidate.home_zoom) &&
    (candidate.texture === "none" || candidate.texture === "full" || candidate.texture === "pixel")
  );
}

loadMeta();
