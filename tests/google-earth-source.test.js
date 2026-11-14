import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { decodeMeshTile } from "@nakasyou/google-earth-internal-api-client";
import sharp from "sharp";
import {
  correctPartialOctants,
  createEarthSource,
  digest,
  earthTileIntersects,
  loadEarthSdk,
  packEarthTile,
} from "../scripts/google-earth-source.mjs";
import {
  inverseIsometric,
  projectEarth,
  projectStatePlane,
} from "../tools/google-imagery/projection.js";

const varint = (value) => {
  const bytes = [];
  do {
    const low = value & 127;
    value >>>= 7;
    bytes.push(low | (value ? 128 : 0));
  } while (value);
  return Buffer.from(bytes);
};
const field = (id, bytes) => Buffer.concat([varint(id * 8 + 2), varint(bytes.length), bytes]);

function partialFixture() {
  const matrix = Buffer.from(
    new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]).buffer,
  );
  const mesh = Buffer.concat([
    field(1, Buffer.from([0, 1, 255, 0, 0, 1, 0, 0, 0])),
    field(3, Buffer.from([3, 0, 0, 0])),
    field(8, Buffer.from([1, 3])),
  ]);
  return Buffer.concat([field(1, matrix), field(2, mesh)]);
}
function planetoidFixture() {
  const radius = Buffer.alloc(5);
  radius[0] = 2 * 8 + 5;
  radius.writeFloatLE(6371000, 1);
  return Buffer.concat([field(1, Buffer.from([5 * 8, 100])), radius]);
}

test("the reviewed decoder correction accepts a partial final octant group, without changing source packages", async () => {
  const root = await mkdtemp(join(tmpdir(), "earth-decoder-test-"));
  try {
    const bytes = partialFixture();
    assert.throws(() => decodeMeshTile(bytes, "2142", 1), /layer\/octant/);
    const sdk = await loadEarthSdk(root);
    const mesh = sdk.decodeMeshTile(bytes, "2142", 1).meshes[0];
    assert.deepEqual([...mesh.indices], [0, 1, 2]);
    assert.equal(mesh.layerBounds[0], 0);
    assert.equal(mesh.layerBounds[1], 3);
    assert.throws(() => correctPartialOctants("unreviewed SDK source"), /SDK changed/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("all eight planned angles inverse-project to the original State Plane position", () => {
  for (let angle = 0; angle < 360; angle += 45) {
    const [x, y] = projectStatePlane(820000, 72000, 60, angle);
    const source = inverseIsometric(x, y + 60, angle);
    assert.ok(Math.abs(source[0] - 820000) < 1e-6);
    assert.ok(Math.abs(source[1] - 72000) < 1e-6);
  }
});

test("RockTree spherical positions are not interpreted as WGS84 ECEF", () => {
  const radius = 6371010;
  const lon = (-75.23 * Math.PI) / 180;
  const lat = (39.961 * Math.PI) / 180;
  const h = 25;
  const xyz = [
    (radius + h) * Math.cos(lat) * Math.cos(lon),
    (radius + h) * Math.cos(lat) * Math.sin(lon),
    (radius + h) * Math.sin(lat),
  ];
  const projected = projectEarth(...xyz, radius, 0, 0);
  const ground = inverseIsometric(projected[0], projected[1] + h);
  const again = projectStatePlane(...ground, h);
  assert.ok(Math.abs(again[0] - projected[0]) < 1e-6);
  assert.ok(Math.abs(again[1] - projected[1]) < 1e-6);
  const reference = projectEarth(...xyz, radius, 10, 0);
  assert.ok(Math.abs(reference[1] - projected[1] - 10) < 1e-6);
});

test("public source responses are hash-checked and reused across runs without credentials", async () => {
  const root = await mkdtemp(join(tmpdir(), "earth-cache-test-"));
  try {
    let calls = 0;
    const fetchImpl = async (url, options) => {
      calls++;
      assert.equal(options.credentials, "omit");
      assert.ok(!String(url).includes("key="));
      return new Response(planetoidFixture());
    };
    const first = await createEarthSource(root, { fetchImpl });
    assert.equal((await first.client.getPlanetoid()).rootEpoch, 100);
    const second = await createEarthSource(root, { fetchImpl });
    assert.equal((await second.client.getPlanetoid()).rootEpoch, 100);
    assert.equal(calls, 1);
    const hash = digest("https://kh.google.com/rt/earth/PlanetoidMetadata");
    const file = join(root, "responses", hash.slice(0, 2), `${hash}.bin`);
    assert.deepEqual(await readFile(file), planetoidFixture());
    await writeFile(file, "corrupt");
    const third = await createEarthSource(root, { fetchImpl });
    await assert.rejects(third.client.getPlanetoid(), (error) =>
      /corrupt/.test(error.cause?.message),
    );
    assert.equal(calls, 1);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("concurrent source streams share one byte budget", async () => {
  const root = await mkdtemp(join(tmpdir(), "earth-concurrent-budget-test-"));
  try {
    const body = planetoidFixture();
    const controllers = [];
    const source = await createEarthSource(root, {
      maxBytes: body.length + 2,
      fetchImpl: async () =>
        new Response(
          new ReadableStream({
            start(controller) {
              controllers.push(controller);
              if (controllers.length === 4)
                for (const stream of controllers) {
                  stream.enqueue(body);
                  stream.close();
                }
            },
          }),
        ),
    });
    const results = await Promise.allSettled(
      Array.from({ length: 4 }, () => source.client.getPlanetoid()),
    );
    assert.equal(results.filter((result) => result.status === "fulfilled").length, 1);
    assert.equal(source.stats().downloadedBytes, body.length);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("provider hidden terrain is packed separately from visible geometry", async () => {
  const root = await mkdtemp(join(tmpdir(), "earth-ground-test-"));
  try {
    const jpeg = await sharp({ create: { width: 1, height: 1, channels: 3, background: "green" } })
      .jpeg()
      .toBuffer();
    const result = await packEarthTile(
      {
        path: "2142",
        epoch: 1,
        matrixGlobeFromMesh: new Float64Array([1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]),
        copyrightIds: [255],
        meshes: [
          {
            positions: new Float32Array(18),
            uv: new Float32Array(12),
            indices: new Uint32Array([0, 1, 2]),
            triangleStrip: new Uint32Array([0, 1, 2, 3, 4, 5]),
            layerBounds: new Uint32Array([0, 3, 3, 3, 6, 6, 6, 6, 6, 6]),
            textures: [{ format: 1, width: 1, height: 1, data: [jpeg] }],
          },
        ],
      },
      root,
    );
    assert.equal(result.meshes, 1);
    assert.equal(result.groundMeshes, 1);
    const surface = JSON.parse(await readFile(join(root, result.name), "utf8"));
    const ground = JSON.parse(await readFile(join(root, result.groundName), "utf8"));
    assert.deepEqual(surface.meshes[0].positions, ground.meshes[0].positions);
    const bytes = await readFile(join(root, surface.binary));
    const indices = (record) =>
      Array.from({ length: record.count }, (_, i) => bytes.readUInt32LE(record.offset + i * 4));
    assert.deepEqual(indices(surface.meshes[0].indices), [0, 1, 2]);
    assert.deepEqual(indices(ground.meshes[0].indices), [3, 4, 5]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("source-node culling preserves visible geometry and excludes distant cells", () => {
  const radius = 6371010,
    lon = (-75.23 * Math.PI) / 180,
    lat = (39.961 * Math.PI) / 180;
  const xyz = [
    radius * Math.cos(lat) * Math.cos(lon),
    radius * Math.cos(lat) * Math.sin(lon),
    radius * Math.sin(lat),
  ];
  const matrix = new Float64Array([0.1, 0, 0, 0, 0, 0.1, 0, 0, 0, 0, 0.1, 0, ...xyz, 1]);
  const [x, y] = projectEarth(...xyz, radius, 0);
  assert.equal(
    earthTileIntersects(
      { matrixGlobeFromMesh: matrix },
      [x - 50, y - 50, x + 50, y + 50],
      radius,
      0,
      0,
    ),
    true,
  );
  assert.equal(
    earthTileIntersects(
      { matrixGlobeFromMesh: matrix },
      [x + 1000, y + 1000, x + 1100, y + 1100],
      radius,
      0,
      0,
    ),
    false,
  );
});

test("the viewer crops 512px parents using their actual dimensions and preserves safe attribution text", async () => {
  const app = await readFile(new URL("../static/app.js", import.meta.url), "utf8");
  assert.match(app, /const sourceSize = image\.naturalWidth \/ factor/);
  assert.match(app, /imageryCredit\.textContent = credits\.join/);
  assert.match(app, /element\.hidden = credits\.includes\("Google Earth"\)/);
});

test("a source download budget stops before publishing an oversized response", async () => {
  const root = await mkdtemp(join(tmpdir(), "earth-budget-test-"));
  try {
    const source = await createEarthSource(root, {
      maxBytes: 4,
      fetchImpl: async () => new Response(planetoidFixture()),
    });
    await assert.rejects(source.client.getPlanetoid(), (error) =>
      /budget exceeded/.test(error.cause?.message),
    );
    assert.equal(source.stats().downloadedBytes, 0);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
