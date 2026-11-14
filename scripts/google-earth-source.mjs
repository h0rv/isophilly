import { createHash } from "node:crypto";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { triangleStripToTriangles } from "@nakasyou/google-earth-internal-api-client";
import sharp from "sharp";
import { projectEarth } from "../tools/google-imagery/projection.js";
export const DECODER = "earth-client-0.1.1-partial-octants-v1";
export const RENDERER = "spherical-isophilly-raw-v1";
export const digest = (data) => createHash("sha256").update(data).digest("hex");

export async function writeAtomic(path, data) {
  await mkdir(dirname(path), { recursive: true });
  const temporary = `${path}.${process.pid}.part`;
  await writeFile(temporary, data);
  await rename(temporary, path);
}
export async function writeJson(path, value) {
  await writeAtomic(path, `${JSON.stringify(value, null, 2)}\n`);
}

/** Upstream 0.1.1 rejects valid final partial octant groups found in Philadelphia.
 * The original RockTree decoder permits them. Pad the final boundary using ceil,
 * without relaxing stream-length or index validation. Never mutate node_modules.
 */
export function correctPartialOctants(source) {
  const before =
    'if (count > 72 || count % 8 !== 0) throw new ProtocolError("Invalid layer/octant group count");';
  const after = 'if (count > 72) throw new ProtocolError("Invalid layer/octant group count");';
  const fill = "bounds.fill(total, count / 8);";
  if (source.split(before).length !== 2 || source.split(fill).length !== 2) {
    throw new Error(
      "Earth SDK changed. Review the partial-octant decoder correction before capture.",
    );
  }
  return (
    source.replace(before, after).replace(fill, "bounds.fill(total, Math.ceil(count / 8));") +
    "\nexport { Message as RockTreeMessage };\n"
  );
}

export async function loadEarthSdk(cacheRoot) {
  const original = await readFile(
    fileURLToPath(import.meta.resolve("@nakasyou/google-earth-internal-api-client")),
    "utf8",
  );
  const patched = correctPartialOctants(original).replace(
    'from "fast-xml-parser"',
    `from ${JSON.stringify(import.meta.resolve("fast-xml-parser"))}`,
  );
  const path = join(cacheRoot, "decoder", `${DECODER}-${digest(patched).slice(0, 16)}.mjs`);
  await writeAtomic(path, patched);
  return import(pathToFileURL(path).href);
}

/** Public unauthenticated endpoints only. No browser cookies, tokens, or API keys. */
export async function createEarthSource(
  root,
  { maxBytes = 20 * 1024 ** 3, fetchImpl = fetch } = {},
) {
  const sdk = await loadEarthSdk(root);
  const files = new Map();
  let downloadedBytes = 0;
  let requests = 0;
  let nextRequest = 0;
  const cachedFetch = async (input, options = {}) => {
    const url = new URL(input);
    if (
      url.origin !== "https://kh.google.com" ||
      !url.pathname.startsWith("/rt/earth/") ||
      url.username ||
      url.password ||
      url.searchParams.has("key") ||
      url.searchParams.has("token")
    ) {
      throw new Error("Earth capture only allows the public, credential-free RockTree endpoint.");
    }
    const key = digest(url.href);
    const path = join(root, "responses", key.slice(0, 2), `${key}.bin`);
    let bytes;
    try {
      bytes = await readFile(path);
      const recorded = JSON.parse(await readFile(`${path}.json`, "utf8"));
      if (recorded.sha256 !== digest(bytes) || recorded.url !== url.href) {
        throw new Error(`Cached Earth response is corrupt: ${path}`);
      }
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      // No bulk retry loop: throttling and authentication failures stop the run.
      const slot = Math.max(Date.now(), nextRequest);
      nextRequest = slot + 100; // At most ten new public-source requests per second.
      if (slot > Date.now()) await new Promise((accept) => setTimeout(accept, slot - Date.now()));
      const response = await fetchImpl(url, { ...options, credentials: "omit" });
      requests++;
      if (!response.ok)
        throw new Error(
          `Earth source returned HTTP ${response.status}. Capture stopped; resume later.`,
        );
      const chunks = [];
      let size = 0;
      for await (const chunk of response.body) {
        size += chunk.length;
        if (size > 32 * 1024 ** 2 || downloadedBytes + chunk.length > maxBytes) {
          throw new Error(
            "Earth source download budget exceeded. Capture stopped without publishing a partial response.",
          );
        }
        // Charge bytes as they arrive, including all concurrent streams.
        // Failed partial responses still consume the invocation's budget.
        downloadedBytes += chunk.length;
        chunks.push(chunk);
      }
      bytes = Buffer.concat(chunks);
      await writeAtomic(path, bytes);
      await writeJson(`${path}.json`, {
        url: url.href,
        sha256: digest(bytes),
        bytes: bytes.length,
        fetched_at: new Date().toISOString(),
      });
    }
    files.set(url.href, { path, sha256: digest(bytes) });
    return new Response(bytes, {
      status: 200,
      headers: { "Content-Type": "application/octet-stream" },
    });
  };
  const client = new sdk.EarthClient({
    fetch: cachedFetch,
    cacheEntries: 512,
    cacheTtlMs: 86400000,
    timeoutMs: 30000,
  });
  return {
    client,
    sdk,
    stats: () => ({ requests, downloadedBytes, responses: files.size }),
    copyright: async (epoch) => {
      const url = `https://kh.google.com/rt/earth/Copyrights/pb=!1u${epoch}`;
      const response = await cachedFetch(url);
      const message = new sdk.RockTreeMessage(new Uint8Array(await response.arrayBuffer()));
      return message
        .messages(1)
        .map((entry) => ({ id: entry.number(1), text: entry.string(3) || entry.string(2) }));
    },
  };
}

export function earthTileIntersects(tile, bounds, radius, heightOrigin, angle, projection) {
  const m = tile.matrixGlobeFromMesh;
  const projected = [];
  // RockTree's packed local vertices lie in this 8-bit cube. Projecting its
  // corners is conservative at these small source cells; a two-metre pad
  // exceeds the sub-metre projection curvature across a level-16+ cell.
  for (const x of [0, 255])
    for (const y of [0, 255])
      for (const z of [0, 255]) {
        projected.push(
          projectEarth(
            m[0] * x + m[4] * y + m[8] * z + m[12],
            m[1] * x + m[5] * y + m[9] * z + m[13],
            m[2] * x + m[6] * y + m[10] * z + m[14],
            radius,
            heightOrigin,
            angle,
            projection,
          ),
        );
      }
  const xs = projected.map((p) => p[0]),
    ys = projected.map((p) => p[1]);
  return (
    Math.min(...xs) <= bounds[2] + 2 &&
    Math.max(...xs) >= bounds[0] - 2 &&
    Math.min(...ys) <= bounds[3] + 2 &&
    Math.max(...ys) >= bounds[1] - 2
  );
}

export async function packEarthTile(tile, directory) {
  const parts = [];
  const meshes = [];
  const groundMeshes = [];
  let offset = 0;
  const append = (array) => {
    const result = { offset, count: array.length };
    const bytes = Buffer.from(array.buffer, array.byteOffset, array.byteLength);
    parts.push(bytes);
    offset += bytes.length;
    return result;
  };
  const name = `${tile.path}-${tile.epoch}`;
  for (const [index, mesh] of tile.meshes.entries()) {
    const hiddenGround = triangleStripToTriangles(
      mesh.triangleStrip.subarray(mesh.layerBounds[3], mesh.layerBounds[4]),
    );
    if (!mesh.indices.length && !hiddenGround.length) continue;
    if (
      !mesh.uv ||
      mesh.uv.length !== (mesh.positions.length / 3) * 2 ||
      mesh.textures.length !== 1
    ) {
      throw new Error(
        `Unsupported texture layout at Earth node ${tile.path}. No partial mesh published.`,
      );
    }
    const image = mesh.textures[0];
    if (image.format !== 1 || !image.data.length)
      throw new Error(`JPEG unavailable at ${tile.path}`);
    const jpeg = Buffer.concat(image.data.map((bytes) => Buffer.from(bytes)));
    const info = await sharp(jpeg, { failOn: "warning" }).metadata();
    if (info.format !== "jpeg" || info.width !== image.width || info.height !== image.height) {
      throw new Error(`Invalid texture dimensions at ${tile.path}`);
    }
    const texture = `${name}-${index}.jpg`;
    await writeAtomic(join(directory, texture), jpeg);
    const attributes = { positions: append(mesh.positions), uv: append(mesh.uv), texture };
    const buildings = triangleStripToTriangles(mesh.triangleStrip.subarray(0, mesh.layerBounds[2]));
    const terrain = triangleStripToTriangles(
      mesh.triangleStrip.subarray(mesh.layerBounds[2], mesh.layerBounds[3]),
    );
    if (buildings.length)
      meshes.push({ ...attributes, terrain: false, indices: append(buildings) });
    if (terrain.length) meshes.push({ ...attributes, terrain: true, indices: append(terrain) });
    // Google's own terrain beneath the photogrammetry is a genuine source
    // underlay, not guessed pixels. Keep it below every visible surface.
    if (hiddenGround.length) groundMeshes.push({ ...attributes, indices: append(hiddenGround) });
  }
  const binary = `${name}.bin`;
  await writeAtomic(join(directory, binary), Buffer.concat(parts));
  const header = {
    path: tile.path,
    epoch: tile.epoch,
    matrix: [...tile.matrixGlobeFromMesh],
    binary,
    copyrightIds: tile.copyrightIds,
    meshes,
  };
  await writeJson(join(directory, `${name}.json`), header);
  const groundName = `${name}-ground.json`;
  if (groundMeshes.length)
    await writeJson(join(directory, groundName), { ...header, meshes: groundMeshes });
  return {
    name: `${name}.json`,
    meshes: meshes.length,
    groundName: groundMeshes.length ? groundName : null,
    groundMeshes: groundMeshes.length,
    copyrightIds: tile.copyrightIds,
  };
}
