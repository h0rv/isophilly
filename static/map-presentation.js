// @ts-check

import { DEFAULT_PRESENTATION } from "./presentation-config.js";

/** @typedef {{panX: number, panY: number, scale: number, side: number, coverage: Set<string>, version: string}} MapPresentationLayout */
const BACKGROUND = "#ded4c4";
const COVERAGE_SIDE = 256;
const MASK_SIDE = 1024;

/** @param {HTMLCanvasElement} canvas */
function context2D(canvas) {
  const context = canvas.getContext("2d");
  if (context === null) throw new Error("2D canvas is unavailable");
  return context;
}

/** A full-resolution imagery layer. Labels are painted separately afterward.
 * @param {import('./presentation-config.js').PresentationConfig} [config]
 */
export function createMapPresentation(config = DEFAULT_PRESENTATION) {
  const canvas = document.createElement("canvas");
  const context = context2D(canvas);
  const padding = Math.ceil(config.edgeBlur * 4);
  let viewportWidth = 1;
  let viewportHeight = 1;
  /** @type {HTMLCanvasElement | undefined} */
  let mask;
  /** @type {Set<string> | undefined} */
  let maskCoverage;
  let maskVersion = "";

  /** @param {number} width @param {number} height */
  function resize(width, height) {
    const w = Number.isFinite(width) && width > 0 ? width : 1;
    const h = Number.isFinite(height) && height > 0 ? height : 1;
    const pw = Math.ceil(w / config.pixelSize);
    const ph = Math.ceil(h / config.pixelSize);
    const changed =
      canvas.width !== pw || canvas.height !== ph || viewportWidth !== w || viewportHeight !== h;
    viewportWidth = w;
    viewportHeight = h;
    if (changed) {
      canvas.width = pw;
      canvas.height = ph;
    }
    context.setTransform(pw / w, 0, 0, ph / h, 0, 0);
    context.imageSmoothingEnabled = false;
    context.imageSmoothingQuality = "high";
    if (changed) {
      context.fillStyle = BACKGROUND;
      context.fillRect(0, 0, w, h);
    }
  }

  /** Share exact backing-pixel edges, so fractional CSS positions cannot leave bars.
   * @param {number} panX @param {number} panY @param {number} tileSize
   * @param {number} x @param {number} y
   */
  function tileRectangle(panX, panY, tileSize, x, y) {
    const rx = canvas.width / viewportWidth,
      ry = canvas.height / viewportHeight;
    const left = Math.round((panX + x * tileSize) * rx) / rx;
    const right = Math.round((panX + (x + 1) * tileSize) * rx) / rx;
    const top = Math.round((panY + y * tileSize) * ry) / ry;
    const bottom = Math.round((panY + (y + 1) * tileSize) * ry) / ry;
    return { left, top, width: right - left, height: bottom - top };
  }

  /** Average detail when shrinking; keep native pixels when enlarging.
   * @param {{width: number, height: number}} destination
   * @param {number} sourceWidth @param {number} [sourceHeight]
   */
  function sampleTile(destination, sourceWidth, sourceHeight = sourceWidth) {
    context.imageSmoothingEnabled =
      (destination.width * canvas.width) / viewportWidth < sourceWidth ||
      (destination.height * canvas.height) / viewportHeight < sourceHeight;
  }

  /** @param {MapPresentationLayout} layout */
  function coverageMask(layout) {
    if (maskCoverage === layout.coverage && maskVersion === layout.version) return mask;
    maskCoverage = layout.coverage;
    maskVersion = layout.version;
    mask = undefined;
    // Flood only the planned union's exterior. Enclosed empty cells must not
    // punch holes through parent imagery while finer images are loading.
    const cells = new Uint8Array(COVERAGE_SIDE ** 2);
    let planned = 0;
    for (const key of layout.coverage) {
      const match = /^8\/(\d{1,3})\/(\d{1,3})$/.exec(key);
      if (match === null) continue;
      const x = Number(match[1]),
        y = Number(match[2]);
      if (x >= COVERAGE_SIDE || y >= COVERAGE_SIDE) continue;
      const index = y * COVERAGE_SIDE + x;
      if (cells[index] === 0) planned++;
      cells[index] = 1;
    }
    if (planned === 0) return undefined;
    const queue = new Uint32Array(cells.length);
    let end = 0;
    /** @param {number} index */
    function visit(index) {
      if (cells[index] !== 0) return;
      cells[index] = 2;
      queue[end++] = index;
    }
    for (let i = 0; i < COVERAGE_SIDE; i++) {
      visit(i);
      visit((COVERAGE_SIDE - 1) * COVERAGE_SIDE + i);
      visit(i * COVERAGE_SIDE);
      visit(i * COVERAGE_SIDE + COVERAGE_SIDE - 1);
    }
    for (let start = 0; start < end; start++) {
      const index = queue[start],
        x = index % COVERAGE_SIDE,
        y = Math.floor(index / COVERAGE_SIDE);
      if (x > 0) visit(index - 1);
      if (x + 1 < COVERAGE_SIDE) visit(index + 1);
      if (y > 0) visit(index - COVERAGE_SIDE);
      if (y + 1 < COVERAGE_SIDE) visit(index + COVERAGE_SIDE);
    }
    const padded = document.createElement("canvas");
    padded.width = MASK_SIDE + padding * 2;
    padded.height = padded.width;
    const paint = context2D(padded);
    paint.fillStyle = "#fff";
    const cellSize = MASK_SIDE / COVERAGE_SIDE;
    for (let y = 0; y < COVERAGE_SIDE; y++) {
      let x = 0;
      while (x < COVERAGE_SIDE) {
        if (cells[y * COVERAGE_SIDE + x] === 2) {
          x++;
          continue;
        }
        const first = x;
        while (x < COVERAGE_SIDE && cells[y * COVERAGE_SIDE + x] !== 2) x++;
        paint.fillRect(
          padding + first * cellSize,
          padding + y * cellSize,
          (x - first) * cellSize,
          cellSize,
        );
      }
    }
    mask = document.createElement("canvas");
    mask.width = MASK_SIDE;
    mask.height = MASK_SIDE;
    const paintMask = context2D(mask);
    paintMask.filter = `blur(${config.edgeBlur}px)`;
    paintMask.drawImage(padded, -padding, -padding);
    paintMask.filter = "none";
    return mask;
  }

  /** No palette, recoloring, sharpening, pixel readback, or photograph filters.
   * @param {CanvasRenderingContext2D} target @param {number} width @param {number} height
   * @param {MapPresentationLayout} [layout]
   */
  function drawTo(target, width, height, layout) {
    if (!(width > 0 && height > 0 && Number.isFinite(width) && Number.isFinite(height))) return;
    if (
      layout !== undefined &&
      Number.isFinite(layout.panX) &&
      Number.isFinite(layout.panY) &&
      Number.isFinite(layout.side * layout.scale) &&
      layout.side * layout.scale > 0
    ) {
      const alpha = coverageMask(layout);
      if (alpha !== undefined) {
        context.save();
        try {
          context.globalCompositeOperation = "destination-in";
          context.globalAlpha = 1;
          context.filter = "none";
          context.imageSmoothingEnabled = true;
          const size = layout.side * layout.scale;
          context.drawImage(alpha, layout.panX, layout.panY, size, size);
        } finally {
          context.restore();
        }
      }
    }
    target.save();
    try {
      target.globalCompositeOperation = "source-over";
      target.globalAlpha = 1;
      target.filter = "none";
      target.imageSmoothingEnabled = false;
      target.drawImage(canvas, 0, 0, width, height);
    } finally {
      target.restore();
    }
  }
  resize(1, 1);
  return { canvas, context, resize, drawTo, tileRectangle, sampleTile };
}
