// @ts-check

/**
 * @typedef {{
 *   iso_bounds: [number, number, number, number],
 *   city_hall: [number, number],
 *   counts: { buildings: number, water: number, parks: number },
 * }} Meta
 */

const canvasElement = document.querySelector("#map");
const statusElement = document.querySelector("#status");
const homeElement = document.querySelector("#home");
if (
  !(canvasElement instanceof HTMLCanvasElement) ||
  !(statusElement instanceof HTMLSpanElement) ||
  !(homeElement instanceof HTMLButtonElement)
) {
  throw new Error("map controls are missing");
}
const canvas = canvasElement;
const statusText = statusElement;
const home = homeElement;
const context = canvas.getContext("2d");
if (context === null) throw new Error("2D canvas is unavailable");
const ctx = context;

/** @type {Meta | undefined} */
let meta;
let zoom = 1;
let cameraX = 0;
let cameraY = 0;
let dragging = false;
/** @type {PointerEvent | undefined} */
let last;
let drawing = false;
/** @type {Map<string, HTMLImageElement>} */
const tiles = new Map();
const MIN_ZOOM = 0.35;
const MAX_TILE_ZOOM = 12;
const MAX_ZOOM = 2 ** (MAX_TILE_ZOOM - 3);
const TILE_VERSION = "20260826-2";

function city() {
  if (meta === undefined) throw new Error("city metadata is not loaded");
  return meta;
}

function resize() {
  canvas.width = innerWidth;
  canvas.height = innerHeight;
  draw();
}

/** @param {number} z @param {number} x @param {number} y */
function key(z, x, y) {
  return `${z}/${x}/${y}`;
}

/** @param {number} z @param {number} x @param {number} y */
function requestTile(z, x, y) {
  const id = key(z, x, y);
  const cached = tiles.get(id);
  if (cached !== undefined) return cached;
  if (z > 0) requestTile(z - 1, x >> 1, y >> 1);
  const image = new Image();
  image.onload = draw;
  image.onerror = () => {
    tiles.delete(id);
    setTimeout(draw, 250);
  };
  image.src = `/tiles/${z}/${x}/${y}.png?v=${TILE_VERSION}`;
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
      left,
      top,
      size,
      size,
    );
    return;
  }
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
  const { iso_bounds: bounds, city_hall: cityHall, counts } = city();
  ctx.fillStyle = "#d9d1c3";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  const z = Math.max(0, Math.min(MAX_TILE_ZOOM, Math.round(Math.log2(zoom) + 3)));
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  const scale = worldScale();
  const count = 2 ** z;
  const tileSize = (side / count) * scale;
  clampCamera();
  const panX = canvas.width / 2 - (cameraX - bounds[0]) * scale;
  const panY = canvas.height / 2 - (cameraY - bounds[1]) * scale;
  const firstX = Math.floor(-panX / tileSize) - 1;
  const firstY = Math.floor(-panY / tileSize) - 1;
  const lastX = Math.ceil((canvas.width - panX) / tileSize) + 1;
  const lastY = Math.ceil((canvas.height - panY) / tileSize) + 1;
  for (let y = Math.max(0, firstY); y < Math.min(count, lastY); y++) {
    for (let x = Math.max(0, firstX); x < Math.min(count, lastX); x++) {
      const left = panX + x * tileSize;
      const top = panY + y * tileSize;
      const image = requestTile(z, x, y);
      if (image.complete && image.naturalWidth) ctx.drawImage(image, left, top, tileSize, tileSize);
      else drawParent(z, x, y, left, top, tileSize);
    }
  }
  const cityX = panX + (cityHall[0] - bounds[0]) * scale;
  const cityY = panY + (cityHall[1] - bounds[1]) * scale;
  ctx.fillStyle = "#191714";
  ctx.beginPath();
  ctx.arc(cityX, cityY, 4, 0, 2 * Math.PI);
  ctx.fill();
  ctx.fillStyle = "#f6f0e6";
  ctx.font = "12px ui-sans-serif, system-ui";
  ctx.fillText("City Hall", cityX + 8, cityY - 8);
  statusText.textContent = `${counts.buildings.toLocaleString()} buildings · z${z}`;
}

function worldScale() {
  const bounds = city().iso_bounds;
  const side = Math.max(bounds[2] - bounds[0], bounds[3] - bounds[1]);
  return (Math.min(canvas.width, canvas.height) * zoom) / side;
}

function clampCamera() {
  const [minX, minY, maxX, maxY] = city().iso_bounds;
  const scale = worldScale();
  /** @param {number} value @param {number} min @param {number} max @param {number} half */
  const clampAxis = (value, min, max, half) =>
    half * 2 >= max - min ? (min + max) / 2 : Math.max(min + half, Math.min(max - half, value));
  cameraX = clampAxis(cameraX, minX, maxX, canvas.width / scale / 2);
  cameraY = clampAxis(cameraY, minY, maxY, canvas.height / scale / 2);
}

function centerCityHall() {
  if (meta === undefined) return;
  zoom = 1;
  cameraX = meta.city_hall[0];
  cameraY = meta.city_hall[1];
  draw();
}

canvas.addEventListener("pointerdown", (event) => {
  dragging = true;
  last = event;
  canvas.classList.add("dragging");
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  if (!dragging || last === undefined) return;
  const scale = worldScale();
  cameraX -= (event.clientX - last.clientX) / scale;
  cameraY -= (event.clientY - last.clientY) / scale;
  last = event;
  draw();
});
canvas.addEventListener("pointerup", () => {
  dragging = false;
  last = undefined;
  canvas.classList.remove("dragging");
});
canvas.addEventListener(
  "wheel",
  (event) => {
    event.preventDefault();
    const beforeScale = worldScale();
    const pointX = cameraX + (event.clientX - canvas.width / 2) / beforeScale;
    const pointY = cameraY + (event.clientY - canvas.height / 2) / beforeScale;
    zoom = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, zoom * Math.exp(-event.deltaY * 0.0015)));
    const afterScale = worldScale();
    cameraX = pointX - (event.clientX - canvas.width / 2) / afterScale;
    cameraY = pointY - (event.clientY - canvas.height / 2) / afterScale;
    draw();
  },
  { passive: false },
);
addEventListener("resize", resize);
home.addEventListener("click", centerCityHall);

async function loadMeta() {
  try {
    const response = await fetch("/meta");
    if (!response.ok) throw new Error(`metadata request failed: ${response.status}`);
    /** @type {unknown} */
    const loaded = await response.json();
    if (!isMeta(loaded)) throw new Error("metadata response has the wrong shape");
    meta = loaded;
    centerCityHall();
    resize();
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
    candidate.counts !== null
  );
}

loadMeta();
