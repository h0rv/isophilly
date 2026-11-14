// @ts-check

// Imagery stays at full CSS resolution. Its colors and vegetation are never
// changed in the browser; zoomed-out detail is reduced by image filtering.
export const DEFAULT_PRESENTATION = {
  pixelSize: 1,
  edgeBlur: 2,
};

export const VIEWER_CONFIG = {
  decodedCacheMiB: 128,
  touchDecodedCacheMiB: 64,
  prefetchTiles: 16,
  touchPrefetchTiles: 8,
  prefetchIdleTimeout: 750,
};

/** @typedef {typeof DEFAULT_PRESENTATION} PresentationConfig */

/** @returns {PresentationConfig} */
export function presentationConfig() {
  return { ...DEFAULT_PRESENTATION };
}
