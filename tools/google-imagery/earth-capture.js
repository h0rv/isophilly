import * as THREE from "three";
import { isIdentityProjection, validateProjection } from "../../static/map-projection.js";
import { projectEarth } from "./projection.js";
import { waterContext } from "./water-mask.js";

function release(scene) {
  scene.traverse((mesh) => {
    if (!mesh.isMesh) return;
    mesh.geometry.dispose();
    mesh.material.map?.dispose();
    mesh.material.dispose();
  });
}

async function model(url, center, heightOrigin, angle, radius, water, projection) {
  const response = await fetch(url);
  if (!response.ok) throw new Error("Missing source mesh header");
  const header = await response.json();
  const binaryResponse = await fetch(new URL(header.binary, url));
  if (!binaryResponse.ok) throw new Error("Missing source mesh buffer");
  const bytes = await binaryResponse.arrayBuffer();
  const matrix = new THREE.Matrix4().fromArray(header.matrix);
  const point = new THREE.Vector3();
  // Depth remains in the original ground+height coordinates, not camera-scaled Y.
  const depthCenter = isIdentityProjection(projection)
    ? center[1]
    : center[1] / projection.groundScale;
  const group = new THREE.Group();
  for (const record of header.meshes) {
    const positions = new Float32Array(bytes, record.positions.offset, record.positions.count);
    const projected = new Float32Array(positions.length);
    const ground = water ? new Float32Array(positions.length) : undefined;
    for (let i = 0; i < positions.length; i += 3) {
      point.set(positions[i], positions[i + 1], positions[i + 2]).applyMatrix4(matrix);
      const [x, y, depth] = projectEarth(
        point.x,
        point.y,
        point.z,
        radius,
        heightOrigin,
        angle,
        projection,
      );
      projected[i] = x - center[0];
      projected[i + 1] = center[1] - y;
      projected[i + 2] = depth - depthCenter;
      if (ground) {
        ground[i] = x - center[0];
        ground[i + 1] = (depth + y) / 2 - center[1];
        ground[i + 2] = (depth - y) / 2;
      }
    }
    const geometry = new THREE.BufferGeometry();
    geometry.setAttribute("position", new THREE.BufferAttribute(projected, 3));
    geometry.setAttribute(
      "uv",
      new THREE.BufferAttribute(new Float32Array(bytes, record.uv.offset, record.uv.count), 2),
    );
    geometry.setIndex(
      new THREE.BufferAttribute(
        new Uint32Array(bytes, record.indices.offset, record.indices.count),
        1,
      ),
    );
    const texture = await new THREE.TextureLoader().loadAsync(new URL(record.texture, url).href);
    texture.colorSpace = THREE.SRGBColorSpace;
    const material = new THREE.MeshBasicMaterial({
      map: texture,
      side: THREE.DoubleSide,
      toneMapped: false,
    });
    const mesh = new THREE.Mesh(geometry, material);
    if (water) water.attach(mesh, ground, record.terrain === true);
    mesh.frustumCulled = false;
    group.add(mesh);
  }
  return group;
}

async function capture({
  bounds,
  width,
  height,
  heightOrigin,
  angle,
  radius,
  tiles,
  attributions,
  tileSize = width,
  waterPolygons,
  projection,
}) {
  validateProjection(projection);
  if (waterPolygons && !isIdentityProjection(projection))
    throw new Error("Water masks require the identity projection");
  if (!isIdentityProjection(projection) && angle !== 0)
    throw new Error("Camera projection currently supports angle 0 only");
  const gl = new THREE.WebGLRenderer({
    alpha: true,
    antialias: false,
    preserveDrawingBuffer: true,
  });
  gl.setPixelRatio(1);
  gl.setSize(width, height);
  gl.outputColorSpace = THREE.SRGBColorSpace;
  gl.setClearColor(0, 0);
  document.body.replaceChildren(gl.domElement);
  const fine = new THREE.Scene();
  const backgrounds = new Map();
  const center = [(bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2];
  const camera = new THREE.OrthographicCamera(
    -(bounds[2] - bounds[0]) / 2,
    (bounds[2] - bounds[0]) / 2,
    (bounds[3] - bounds[1]) / 2,
    -(bounds[3] - bounds[1]) / 2,
    1,
    100000,
  );
  camera.position.z = 20000;
  camera.updateMatrixWorld(true);
  const water = waterPolygons
    ? waterContext(waterPolygons, bounds, width, height, heightOrigin)
    : undefined;
  try {
    for (const tile of tiles) {
      const group = await model(
        new URL(tile.url, location.href).href,
        center,
        heightOrigin,
        angle,
        radius,
        water,
        projection,
      );
      if (tile.coarse) {
        const level = tile.level ?? 0;
        if (!backgrounds.has(level)) backgrounds.set(level, new THREE.Scene());
        backgrounds.get(level).add(group);
      } else fine.add(group);
    }
    // Measure fine coverage before adding the background pass. Metadata nodes
    // alone do not establish that fine geometry covers any output pixel.
    gl.render(fine, camera);
    const pixels = new Uint8Array(width * height * 4);
    const context = gl.getContext();
    context.readPixels(0, 0, width, height, context.RGBA, context.UNSIGNED_BYTE, pixels);
    const columns = width / tileSize;
    const finePixels = new Array((columns * height) / tileSize).fill(0);
    for (let y = 0; y < height; y++)
      for (let x = 0; x < width; x++) {
        if (pixels[(y * width + x) * 4 + 3] === 255) {
          finePixels[
            Math.floor((height - 1 - y) / tileSize) * columns + Math.floor(x / tileSize)
          ]++;
        }
      }
    if (water) {
      water.finish(pixels);
      // Refresh the collection flag even for scenes containing one material.
      for (const scene of [fine, ...backgrounds.values()])
        scene.traverse((mesh) => {
          if (mesh.isMesh) mesh.material.needsUpdate = true;
        });
    }
    // The lower-detail pass only fills pixels not covered by fine triangles;
    // its approximate geometry must never occlude better source geometry.
    gl.autoClear = false;
    gl.clear();
    for (const level of [...backgrounds.keys()].sort((a, b) => a - b)) {
      gl.clearDepth();
      gl.render(backgrounds.get(level), camera);
    }
    gl.clearDepth();
    gl.render(fine, camera);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    ctx.drawImage(gl.domElement, 0, 0);
    const rgba = ctx.getImageData(0, 0, width, height).data;
    let holes = 0;
    for (let i = 3; i < rgba.length; i += 4) if (rgba[i] !== 255) holes++;
    // Transfer the block's pixels once. Encoding a giant PNG here and decoding
    // it separately for every output tile wastes most of the capture time.
    const raw = await new Promise((accept, reject) => {
      const reader = new FileReader();
      reader.onload = () => accept(String(reader.result).split(",")[1]);
      reader.onerror = () => reject(new Error("Could not transfer rendered pixels"));
      reader.readAsDataURL(new Blob([rgba], { type: "application/octet-stream" }));
    });
    return {
      raw,
      holes,
      width,
      height,
      sourceTiles: tiles.length,
      waterStats: water?.stats(),
      finePixels,
      attributions,
    };
  } finally {
    water?.dispose();
    release(fine);
    for (const background of backgrounds.values()) release(background);
    gl.dispose();
    gl.forceContextLoss();
  }
}

window.captureEarthBlock = (settings) => capture(settings);
window.captureReady = true;
