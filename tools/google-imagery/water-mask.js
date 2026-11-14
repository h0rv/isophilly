import * as THREE from "three";

// Use City water polygons and the visible fine terrain's elevation. The
// final mask can include low photogrammetric water faces, but not elevated
// road decks. Coarse and hidden vertices cannot determine the water level.
export function waterContext(polygons, bounds, width, height, heightOrigin) {
  const center = [(bounds[0] + bounds[2]) / 2, (bounds[1] + bounds[3]) / 2];
  const rectangle = [
    bounds[0],
    bounds[1] - 100 - heightOrigin,
    bounds[2],
    bounds[3] + 400 - heightOrigin,
  ];
  const scale = width / (rectangle[2] - rectangle[0]);
  if (Math.abs(height / (bounds[3] - bounds[1]) - scale) > 1e-6)
    throw new Error("Water mask requires square world pixels");
  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = Math.ceil((rectangle[3] - rectangle[1]) * scale);
  if (canvas.width > 4096 || canvas.height > 4096)
    throw new Error("Water mask exceeds rendering budget");
  const ctx = canvas.getContext("2d");
  ctx.fillStyle = "black";
  ctx.fillRect(0, 0, canvas.width, canvas.height);
  ctx.fillStyle = "white";
  for (const { rings } of polygons) {
    ctx.beginPath();
    for (const ring of rings) {
      ring.forEach(([x, y], i) => {
        const a = (x - rectangle[0]) * scale,
          b = (y - rectangle[1]) * scale;
        if (i) ctx.lineTo(a, b);
        else ctx.moveTo(a, b);
      });
      ctx.closePath();
    }
    ctx.fill("evenodd");
  }
  const image = new THREE.CanvasTexture(canvas);
  image.flipY = false;
  image.minFilter = image.magFilter = THREE.NearestFilter;
  image.generateMipmaps = false;
  const side = 64;
  const columns = Math.ceil((rectangle[2] - rectangle[0]) / side) + 1;
  const rows = Math.ceil((rectangle[3] - rectangle[1]) / side) + 1;
  const data = new Float32Array(columns * rows).fill(1e6);
  const heights = new THREE.DataTexture(data, columns, rows, THREE.RedFormat, THREE.FloatType);
  heights.minFilter = heights.magFilter = THREE.NearestFilter;
  heights.generateMipmaps = false;
  const uniforms = {
    waterCollect: { value: 1 },
    waterHeightOrigin: { value: heightOrigin },
    waterGeography: { value: image },
    waterHeights: { value: heights },
    waterRectangle: {
      value: new THREE.Vector4(
        rectangle[0] - center[0],
        rectangle[1] - center[1],
        rectangle[2] - rectangle[0],
        canvas.height / scale,
      ),
    },
    waterGrid: {
      value: new THREE.Vector4(
        rectangle[0] - center[0],
        rectangle[1] - center[1],
        columns * side,
        rows * side,
      ),
    },
  };
  let candidates = 0;
  return {
    attach(mesh, coordinates, terrain) {
      mesh.geometry.setAttribute("waterGround", new THREE.BufferAttribute(coordinates, 3));
      mesh.material.onBeforeCompile = (shader) => {
        Object.assign(shader.uniforms, uniforms, { waterTerrain: { value: terrain ? 1 : 0 } });
        shader.vertexShader =
          `attribute vec3 waterGround; varying vec3 vWaterGround;\n${shader.vertexShader}`.replace(
            "#include <begin_vertex>",
            "#include <begin_vertex>\nvWaterGround = waterGround;",
          );
        shader.fragmentShader = `varying vec3 vWaterGround;
          uniform sampler2D waterGeography; uniform sampler2D waterHeights;
          uniform vec4 waterRectangle; uniform vec4 waterGrid; uniform float waterTerrain;
          uniform float waterCollect; uniform float waterHeightOrigin;\n${shader.fragmentShader}`.replace(
          "#include <colorspace_fragment>",
          `#include <colorspace_fragment>
            vec2 p = (vWaterGround.xy - waterRectangle.xy) / waterRectangle.zw;
            bool inside = all(greaterThanEqual(p, vec2(0.0))) && all(lessThanEqual(p, vec2(1.0)));
            bool candidate = waterTerrain > 0.5 && inside && texture2D(waterGeography, p).r > 0.5;
            if (waterCollect > 0.5) {
              float encoded = floor(clamp((vWaterGround.z + waterHeightOrigin + 100.0) * 100.0, 0.0, 65535.0) + 0.5);
              gl_FragColor = vec4(candidate ? 1.0 : 0.0, floor(encoded / 256.0) / 255.0, mod(encoded, 256.0) / 255.0, 1.0);
            } else {
              vec2 g = (vWaterGround.xy - waterGrid.xy) / waterGrid.zw;
              float reference = texture2D(waterHeights, g).r;
              float selected = reference < 1e5 && inside && texture2D(waterGeography, p).r > 0.5
                && vWaterGround.z <= reference + 1.5 ? 1.0 : 0.0;
              gl_FragColor = vec4(vec3(selected), 1.0);
            }
          `,
        );
      };
      mesh.material.customProgramCacheKey = () => "geographic-water-mask-v2";
    },
    finish(pixels) {
      const cells = new Map();
      for (let i = 0; i < width * height; i++) {
        if (pixels[i * 4] !== 255 || pixels[i * 4 + 3] !== 255) continue;
        const z = (pixels[i * 4 + 1] * 256 + pixels[i * 4 + 2]) / 100 - 100 - heightOrigin;
        const x = bounds[0] + ((i % width) + 0.5) / scale;
        const y = bounds[3] - (Math.floor(i / width) + 0.5) / scale + z;
        const c = Math.floor((x - rectangle[0]) / side),
          r = Math.floor((y - rectangle[1]) / side);
        if (c < 0 || c >= columns || r < 0 || r >= rows) continue;
        const key = r * columns + c;
        if (!cells.has(key)) cells.set(key, new Map());
        const histogram = cells.get(key),
          bin = Math.round(z * 2);
        histogram.set(bin, (histogram.get(bin) ?? 0) + 1);
        candidates++;
      }
      for (let y = 0; y < rows; y++)
        for (let x = 0; x < columns; x++) {
          const histogram = new Map();
          let total = 0;
          for (let dy = -3; dy <= 3; dy++)
            for (let dx = -3; dx <= 3; dx++) {
              if (x + dx < 0 || x + dx >= columns || y + dy < 0 || y + dy >= rows) continue;
              for (const [bin, count] of cells.get((y + dy) * columns + x + dx) ?? []) {
                histogram.set(bin, (histogram.get(bin) ?? 0) + count);
                total += count;
              }
            }
          let best = 0,
            reference = 1e6;
          for (const bin of histogram.keys()) {
            let count = 0,
              sum = 0;
            for (let delta = -2; delta <= 2; delta++) {
              const weight = histogram.get(bin + delta) ?? 0;
              count += weight;
              sum += (weight * (bin + delta)) / 2;
            }
            if (count > best && count >= total * 0.6) {
              best = count;
              reference = sum / count;
            }
          }
          data[y * columns + x] = reference;
        }
      heights.needsUpdate = true;
      uniforms.waterCollect.value = 0;
    },
    stats() {
      const known = [...data].filter((value) => value < 1e5);
      return {
        fine_water_candidates: candidates,
        reference_cells: known.length,
        min_height: known.length ? Math.min(...known) : null,
        max_height: known.length ? Math.max(...known) : null,
      };
    },
    dispose() {
      image.dispose();
      heights.dispose();
    },
  };
}
