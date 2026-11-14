import { GoogleCloudAuthPlugin } from "3d-tiles-renderer/core/plugins";
import { TilesRenderer } from "3d-tiles-renderer/three";
import { GLTFExtensionsPlugin } from "3d-tiles-renderer/three/plugins";
import proj4 from "proj4";
import * as THREE from "three";
import { DRACOLoader } from "three/addons/loaders/DRACOLoader.js";
import { geodeticToEcef, inverseIsometric, projectEcef } from "./projection.js";

const STATE_PLANE =
  "+proj=lcc +lat_1=40.96666666666667 +lat_2=39.93333333333333 +lat_0=39.33333333333334 +lon_0=-77.75 +x_0=600000 +y_0=0 +ellps=GRS80 +units=m +no_defs";
const SIZE = 256;
const BLEED = 16;
const PIXELS = SIZE + BLEED * 2;

/** Reproject vertices before rasterizing so every capture uses the existing map's pixel grid.
 * Camera-only ENU approximations can shift adjacent captures and do not match our vertical scale.
 */
function projectModel(source, center, heightOrigin) {
  source.updateMatrixWorld(true);
  const group = new THREE.Group();
  source.traverse((mesh) => {
    if (!mesh.isMesh) return;
    const geometry = mesh.geometry.clone();
    const original = mesh.geometry.attributes.position;
    const positions = new Float32Array(original.count * 3);
    const point = new THREE.Vector3();
    for (let i = 0; i < original.count; i++) {
      point.fromBufferAttribute(original, i).applyMatrix4(mesh.matrixWorld);
      const [x, y, depth] = projectEcef(point.x, point.y, point.z, heightOrigin);
      positions[i * 3] = x - center[0];
      positions[i * 3 + 1] = center[1] - y;
      positions[i * 3 + 2] = depth - center[1];
    }
    geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geometry.computeBoundingSphere();
    const convert = (material) =>
      new THREE.MeshBasicMaterial({
        map: material.map,
        color: material.color,
        side: THREE.DoubleSide,
        transparent: false,
        toneMapped: false,
      });
    const material = Array.isArray(mesh.material)
      ? mesh.material.map(convert)
      : convert(mesh.material);
    group.add(new THREE.Mesh(geometry, material));
  });
  return group;
}

function disposeProjected(group) {
  group.traverse((mesh) => {
    if (!mesh.isMesh) return;
    mesh.geometry.dispose();
    for (const material of Array.isArray(mesh.material) ? mesh.material : [mesh.material])
      material.dispose();
  });
  group.removeFromParent();
}

/** Approximate physical camera for tile selection only. Output vertices use exact State Plane above. */
function selectionCamera(center, side, heightOrigin) {
  const [x, y] = inverseIsometric(...center);
  const [lon, lat] = proj4(STATE_PLANE, "EPSG:4326", [x, y]);
  const target = new THREE.Vector3(...geodeticToEcef(lon, lat, heightOrigin));
  const local = (dx, dy, h) => {
    const [longitude, latitude] = proj4(STATE_PLANE, "EPSG:4326", [x + dx, y + dy]);
    return new THREE.Vector3(...geodeticToEcef(longitude, latitude, heightOrigin + h))
      .sub(target)
      .normalize();
  };
  const east = local(10, 0, 0);
  const north = local(0, 10, 0);
  const up = local(0, 0, 10);
  // Ground screen-down vector for the Broad Street rotation.
  const groundDown = east
    .clone()
    .multiplyScalar(0.5 * (0.9907696 - 0.13555646))
    .addScaledVector(north, -0.5 * (0.9907696 + 0.13555646));
  const forward = groundDown.clone().addScaledVector(up, 0.5);
  const screenUp = groundDown.clone().negate().add(up);
  const camera = new THREE.OrthographicCamera(-side, side, side, -side, 1, 100000);
  // Deliberately wider than the output, including the largest possible building-height displacement.
  camera.position.copy(target).addScaledVector(forward.normalize(), 10000);
  camera.up.copy(screenUp.normalize());
  camera.lookAt(target);
  camera.updateMatrixWorld(true);
  return camera;
}

async function capture({ apiKey, bounds, heightOrigin, timeoutMs, tilesetUrl }) {
  const side = bounds[2] - bounds[0];
  // Rust's square raster uses bounds.width() for both axes, even when f32
  // addition makes max_y - min_y differ by a fraction of a metre.
  const center = [bounds[0] + side / 2, bounds[1] + side / 2];
  const padded = (side * PIXELS) / SIZE;
  const renderer = new THREE.WebGLRenderer({
    alpha: true,
    antialias: false,
    preserveDrawingBuffer: true,
  });
  renderer.setPixelRatio(1);
  renderer.setSize(PIXELS, PIXELS);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.setClearColor(0, 0);
  document.body.replaceChildren(renderer.domElement);
  const scene = new THREE.Scene();
  const camera = new THREE.OrthographicCamera(
    -padded / 2,
    padded / 2,
    padded / 2,
    -padded / 2,
    1,
    100000,
  );
  camera.position.z = 20000;
  camera.updateMatrixWorld(true);
  const selector = selectionCamera(center, padded, heightOrigin);
  const tiles = new TilesRenderer(tilesetUrl);
  tiles.registerPlugin(
    new GoogleCloudAuthPlugin({ apiToken: apiKey, useRecommendedSettings: false }),
  );
  const dracoLoader = new DRACOLoader();
  dracoLoader.setDecoderPath("/draco/");
  tiles.registerPlugin(new GLTFExtensionsPlugin({ dracoLoader, autoDispose: false }));
  tiles.errorTarget = 1;
  tiles.lruCache.maxSize = 1200;
  tiles.lruCache.maxBytesSize = 768 * 1024 * 1024;
  tiles.setCamera(selector);
  tiles.setResolution(selector, PIXELS * 2, PIXELS * 2);
  const projected = new Map();
  let error = false;
  let lastChange = performance.now();
  const changed = () => {
    lastChange = performance.now();
  };
  tiles.addEventListener("load-error", () => {
    error = true;
  });
  tiles.addEventListener("load-model", ({ scene: model, tile }) => {
    const copy = projectModel(model, center, heightOrigin);
    copy.visible = tiles.visibleTiles.has(tile);
    projected.set(tile, copy);
    scene.add(copy);
    changed();
  });
  tiles.addEventListener("tile-visibility-change", ({ tile, visible }) => {
    const copy = projected.get(tile);
    if (copy) copy.visible = visible;
    changed();
  });
  tiles.addEventListener("dispose-model", ({ tile }) => {
    const copy = projected.get(tile);
    if (copy) disposeProjected(copy);
    projected.delete(tile);
    changed();
  });
  const started = performance.now();
  try {
    while (true) {
      if (error)
        throw new Error("Google 3D tiles failed to load. Check API access, quota, and billing.");
      if (performance.now() - started > timeoutMs)
        throw new Error("3D imagery did not settle before the timeout.");
      tiles.group.updateMatrixWorld(true);
      tiles.update();
      renderer.render(scene, camera);
      if (tiles.visibleTiles.size > 0 && !tiles.isLoading && performance.now() - lastChange > 1800)
        break;
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    const output = document.createElement("canvas");
    output.width = SIZE;
    output.height = SIZE;
    const context = output.getContext("2d", { willReadFrequently: true });
    context.drawImage(renderer.domElement, BLEED, BLEED, SIZE, SIZE, 0, 0, SIZE, SIZE);
    const pixels = context.getImageData(0, 0, SIZE, SIZE).data;
    let opaquePixels = 0;
    for (let i = 3; i < pixels.length; i += 4) if (pixels[i] === 255) opaquePixels++;
    const credits = tiles
      .getAttributions([])
      .filter((credit) => credit.type === "string" && credit.value.trim());
    const attributions = ["Google Maps", ...new Set(credits.map((credit) => credit.value))].sort();
    const maxError = Math.max(...[...tiles.visibleTiles].map((tile) => tile.traversal.error));
    return {
      png: output.toDataURL("image/png").split(",")[1],
      opaquePixels,
      visibleTiles: tiles.visibleTiles.size,
      maxError,
      attributions,
    };
  } finally {
    tiles.dispose();
    for (const copy of projected.values()) disposeProjected(copy);
    dracoLoader.dispose();
    renderer.dispose();
    renderer.forceContextLoss();
  }
}

window.captureGoogleTile = capture;
window.captureReady = true;
