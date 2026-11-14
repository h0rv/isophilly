import assert from "node:assert/strict";
import { lstat, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import proj4 from "proj4";
import { validateEarthPlan } from "../scripts/google-earth-lib.mjs";
import { DECODER, digest, RENDERER } from "../scripts/google-earth-source.mjs";
import {
  createWaterMaskPlan,
  projectWater,
  WATER_RENDERER,
  WATER_URL,
  waterForBlock,
} from "../scripts/google-earth-water-lib.mjs";
import { STATE_PLANE } from "../scripts/google-imagery-lib.mjs";
import { projectStatePlane } from "../tools/google-imagery/projection.js";

const ring = (lon = -75.16, lat = 39.95, size = 0.0001) => [
  [lon, lat],
  [lon + size, lat],
  [lon + size, lat + size],
  [lon, lat + size],
  [lon, lat],
];
const collection = (type = "Polygon", coordinates = [ring()]) => ({
  type: "FeatureCollection",
  features: [{ type: "Feature", properties: {}, geometry: { type, coordinates } }],
});
const load = async (file) => JSON.parse(await readFile(file));
const save = (file, data) => writeFile(file, `${JSON.stringify(data, null, 2)}\n`);
const iso = ([lon, lat], angle = 0) =>
  projectStatePlane(...proj4("EPSG:4326", STATE_PLANE, [lon, lat]), 0, angle).slice(0, 2);

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "google-earth-water-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const [x, y] = iso(ring()[0]);
  const bounds = [x - 50, y - 50, x + 25550, y + 25550];
  const meta = { schema_version: 2, tile_version: "original-photo", iso_bounds: bounds };
  const coverage = {
    schema_version: 1,
    tile_version: meta.tile_version,
    tiles: ["8/0/0", "8/20/0"],
  };
  const [sx, sy] = iso(ring()[0], 90);
  const plan = {
    schema_version: 1,
    source: "google-earth-public",
    source_meta: meta,
    zoom: 8,
    tile_size: 512,
    source_level: 18,
    height_origin: 23,
    views: [
      {
        id: "se",
        angle: 0,
        iso_bounds: bounds,
        tiles: [
          { x: 0, y: 0 },
          { x: 20, y: 0 },
        ],
      },
      {
        id: "sw",
        angle: 90,
        iso_bounds: [sx - 50, sy - 50, sx + 25550, sy + 25550],
        tiles: [{ x: 0, y: 0 }],
      },
      { id: "nw", angle: 180, iso_bounds: [0, 0, 25600, 25600], tiles: [{ x: 0, y: 0 }] },
    ],
  };
  const photoBytes = Buffer.from(` ${JSON.stringify(plan)}\n\n`);
  await writeFile(join(root, "plan.json"), photoBytes);
  const images = [];
  for (const view of plan.views)
    for (const tile of view.tiles) {
      // Planning must not decode or hash raw images. These are existence-only fixtures.
      const bytes = Buffer.from("photo archive fixture");
      const file = join(root, "raw", view.id, "8", `${tile.x}`, `${tile.y}.png`);
      await mkdir(dirname(file), { recursive: true });
      await writeFile(file, bytes);
      images.push({ ...tile, view: view.id, sha256: digest(bytes) });
    }
  const manifest = {
    schema_version: 1,
    plan_sha256: digest(photoBytes),
    decoder: DECODER,
    renderer: RENDERER,
    images,
  };
  await save(join(root, "manifest.json"), manifest);
  const source = {
    root_epoch: 100,
    radius: 6371010,
    decoder: DECODER,
    renderer: RENDERER,
    copyright: [{ id: 1, text: "Google Earth" }],
  };
  await save(join(root, "source.json"), source);
  await mkdir(join(root, "reference"));
  await writeFile(join(root, "reference", "meta.json"), ` ${JSON.stringify(meta)}\n`);
  await save(join(root, "reference", "coverage.json"), coverage);
  const raw = Buffer.from(` ${JSON.stringify(collection())}\n`);
  let fetchCount = 0;
  const options = {
    fetchImpl: async (url) => {
      assert.equal(url, WATER_URL);
      fetchCount++;
      return new Response(raw);
    },
  };
  return { root, plan, manifest, source, photoBytes, raw, options, fetchCount: () => fetchCount };
}

test("projects every MultiPolygon island and hole using EPSG:32129 and each view angle", () => {
  const outer = ring();
  const hole = ring(-75.15998, 39.95002, 0.00002).reverse();
  const island = ring(-75.15, 39.96);
  for (const angle of [0, 45, 90, 270]) {
    const polygons = projectWater(collection("MultiPolygon", [[outer, hole], [island]]), angle);
    assert.equal(polygons.length, 2);
    assert.deepEqual(polygons[0].rings, [
      outer.map((p) => iso(p, angle)),
      hole.map((p) => iso(p, angle)),
    ]);
    assert.deepEqual(polygons[1].rings, [island.map((p) => iso(p, angle))]);
    for (const { bounds, rings } of polygons) {
      const points = rings.flat();
      assert.deepEqual(bounds, [
        Math.min(...points.map((p) => p[0])),
        Math.min(...points.map((p) => p[1])),
        Math.max(...points.map((p) => p[0])),
        Math.max(...points.map((p) => p[1])),
      ]);
    }
  }
});

test("rejects invalid, incomplete, non-geographic or unsupported City geometries", () => {
  const invalid = [
    null,
    {},
    { type: "FeatureCollection", features: [] },
    { ...collection(), exceededTransferLimit: true },
    { ...collection(), properties: { exceededTransferLimit: true } },
    { ...collection(), crs: { type: "name", properties: { name: "EPSG:32129" } } },
    collection("LineString", ring()),
    collection("Polygon", []),
    collection("MultiPolygon", [[]]),
    collection("Polygon", [ring().slice(0, 3)]),
    collection("Polygon", [ring().slice(0, 4)]),
    collection("Polygon", [[[500000, 200000], ...ring()]]),
    collection("Polygon", [[[NaN, 40], ...ring()]]),
    collection("Polygon", [[[-75, Infinity], ...ring()]]),
    collection("Polygon", [[["-75", 40], ...ring()]]),
    collection("Polygon", [[[-75, 91], ...ring()]]),
    collection("Polygon", [[[-181, 40], ...ring()]]),
    { ...collection(), features: [...collection().features, { type: "Feature", geometry: null }] },
  ];
  for (const data of invalid) assert.throws(() => projectWater(data));
  const dense = Array.from({ length: 20000 }, (_, i) => [
    -75.16 + 0.0001 * Math.cos((i * 2 * Math.PI) / 20000),
    39.95 + 0.0001 * Math.sin((i * 2 * Math.PI) / 20000),
  ]);
  dense.push(dense[0]);
  assert.equal(projectWater(collection("Polygon", [dense]))[0].rings[0].length, 20001);
});

test("conservative block filtering includes exact x edges and both height halo edges", () => {
  const boxes = [
    [0, -100, 0, -100],
    [10, 410, 10, 410],
    [0, -101, 10, -101],
    [0, 411, 10, 411],
    [-2, 0, -1, 10],
    [11, 0, 12, 10],
  ].map((bounds) => ({ bounds, rings: [] }));
  assert.deepEqual(waterForBlock(boxes, [0, 0, 10, 10]), boxes.slice(0, 2));
  const shifted = boxes.map((p) => ({
    ...p,
    bounds: p.bounds.map((n, i) => (i % 2 ? n - 37 : n)),
  }));
  assert.deepEqual(waterForBlock(shifted, [0, 0, 10, 10], 37), shifted.slice(0, 2));
  assert.deepEqual(waterForBlock([], [0, 0, 10, 10]), []);
});

test("pins exact bindings, copied references and source settings; resume never fetches", async (t) => {
  const f = await fixture(t);
  const manifestBytes = await readFile(join(f.root, "manifest.json"));
  const { root, plan } = await createWaterMaskPlan(f.root, f.options);
  assert.equal(root, join(f.root, "water"));
  validateEarthPlan(plan);
  assert.equal(plan.capture_mode, "water-mask");
  assert.equal(plan.photo_plan_sha256, digest(f.photoBytes));
  assert.equal(plan.photo_manifest_sha256, digest(manifestBytes));
  assert.equal(plan.water_source_sha256, digest(f.raw));
  assert.equal(plan.water_source, WATER_URL);
  for (const setting of [
    "schema_version",
    "source",
    "source_meta",
    "zoom",
    "tile_size",
    "source_level",
    "height_origin",
  ])
    assert.deepEqual(plan[setting], f.plan[setting]);
  assert.deepEqual(
    plan.views.map((v) => v.id),
    ["se", "sw"],
  );
  for (const view of plan.views) {
    assert.deepEqual(view.tiles, [{ x: 0, y: 0 }]);
    assert.deepEqual(view.iso_bounds, f.plan.views.find((v) => v.id === view.id).iso_bounds);
  }
  const waterBytes = await readFile(join(root, "reference", "water.json"));
  assert.equal(plan.water_sha256, digest(waterBytes));
  const water = JSON.parse(waterBytes);
  assert.equal(water.schema_version, 1);
  assert.deepEqual(water.views.sw, projectWater(collection(), 90));
  assert.ok(water.views.nw);
  assert.deepEqual(await readFile(join(root, "reference", "water.geojson")), f.raw);
  for (const name of ["meta.json", "coverage.json"])
    assert.deepEqual(
      await readFile(join(root, "reference", name)),
      await readFile(join(f.root, "reference", name)),
    );
  assert.deepEqual(await load(join(root, "source.json")), {
    ...f.source,
    renderer: WATER_RENDERER,
  });
  assert.equal(f.fetchCount(), 1);
  const before = await readFile(join(root, "plan.json"));
  const resumed = await createWaterMaskPlan(f.root, {
    fetchImpl: () => assert.fail("resume fetched"),
  });
  assert.deepEqual(resumed, { root, plan });
  assert.deepEqual(await readFile(join(root, "plan.json")), before);
  assert.deepEqual(await readFile(join(f.root, "plan.json")), f.photoBytes);
  assert.deepEqual(await readFile(join(f.root, "manifest.json")), manifestBytes);
  await assert.rejects(lstat(join(root, "raw")), /ENOENT/);
});

test("rejects missing, corrupt, duplicate, unplanned or incomplete photos before fetching", async (t) => {
  const mutations = {
    incomplete: (f) => {
      f.manifest.images.pop();
    },
    duplicate: (f) => {
      f.manifest.images.push(f.manifest.images[0]);
    },
    unplanned: (f) => {
      f.manifest.images[0].x = 255;
    },
    corruptHash: (f) => {
      f.manifest.images[0].sha256 = "broken";
    },
    stale: (f) => {
      f.manifest.plan_sha256 = "0".repeat(64);
    },
    decoder: (f) => {
      f.manifest.decoder = "wrong";
    },
    renderer: (f) => {
      f.manifest.renderer = WATER_RENDERER;
    },
    missing: (f) => rm(join(f.root, "raw", "se", "8", "0", "0.png")),
    empty: (f) => writeFile(join(f.root, "raw", "se", "8", "0", "0.png"), ""),
    maskInput: async (f) => {
      f.plan.capture_mode = "water-mask";
      await save(join(f.root, "plan.json"), f.plan);
    },
  };
  for (const [name, mutate] of Object.entries(mutations))
    await t.test(name, async (t) => {
      const f = await fixture(t);
      await mutate(f);
      await save(join(f.root, "manifest.json"), f.manifest);
      await assert.rejects(createWaterMaskPlan(f.root, f.options));
      assert.equal(f.fetchCount(), 0);
    });
});

test("rejects invalid source epochs/radii without downloading", async (t) => {
  for (const [field, values] of Object.entries({
    root_epoch: [0, -1, 1.5, "100"],
    radius: [0, -1, null, "6371010"],
    decoder: ["wrong"],
    renderer: [WATER_RENDERER],
  }))
    for (const value of values)
      await t.test(`${field}=${value}`, async (t) => {
        const f = await fixture(t);
        await save(join(f.root, "source.json"), { ...f.source, [field]: value });
        await assert.rejects(createWaterMaskPlan(f.root, f.options), /source/);
        assert.equal(f.fetchCount(), 0);
      });
});

test("rejects stale bindings and tampered pinned data without overwriting or fetching", async (t) => {
  const changes = {
    manifestBytes: async (f) =>
      writeFile(join(f.root, "manifest.json"), `${JSON.stringify(f.manifest)}\n\n`),
    photoBytes: async (f) => {
      await save(join(f.root, "plan.json"), f.plan);
      f.manifest.plan_sha256 = digest(await readFile(join(f.root, "plan.json")));
      await save(join(f.root, "manifest.json"), f.manifest);
    },
    projected: (_f, root) => writeFile(join(root, "reference", "water.json"), "{}\n"),
    raw: (_f, root) =>
      writeFile(join(root, "reference", "water.geojson"), JSON.stringify(collection())),
    reference: (_f, root) => writeFile(join(root, "reference", "meta.json"), "{}"),
    maskEpoch: (f, root) =>
      save(join(root, "source.json"), { ...f.source, renderer: WATER_RENDERER, root_epoch: 101 }),
    photoRadius: (f) => save(join(f.root, "source.json"), { ...f.source, radius: 6371000 }),
  };
  for (const [name, change] of Object.entries(changes))
    await t.test(name, async (t) => {
      const f = await fixture(t);
      const { root } = await createWaterMaskPlan(f.root, f.options);
      const before = await readFile(join(root, "plan.json"));
      await change(f, root);
      await assert.rejects(
        createWaterMaskPlan(f.root, { fetchImpl: () => assert.fail("fetched") }),
        /new mask workspace/,
      );
      assert.deepEqual(await readFile(join(root, "plan.json")), before);
    });
});

test("does not reuse an unrelated workspace or silently change the source URL", async (t) => {
  const f = await fixture(t);
  const root = join(f.root, "water");
  await mkdir(root);
  await writeFile(join(root, "unrelated"), "keep");
  await assert.rejects(createWaterMaskPlan(f.root, f.options), /new mask workspace/);
  assert.equal(f.fetchCount(), 0);
  assert.equal(await readFile(join(root, "unrelated"), "utf8"), "keep");
  await rm(root, { recursive: true });
  await createWaterMaskPlan(f.root, f.options);
  await assert.rejects(
    createWaterMaskPlan(f.root, { ...f.options, sourceUrl: "https://example.invalid/water" }),
    /new mask workspace/,
  );
  assert.equal(f.fetchCount(), 1);
});

test("rejects HTTP failures, truncated collections and explicit 64 MiB source overflow", async (t) => {
  for (const response of [
    () => new Response("unavailable", { status: 503 }),
    () => new Response(JSON.stringify({ ...collection(), exceededTransferLimit: true })),
    () => new Response(Buffer.alloc(64 * 1024 ** 2 + 1, 32)),
  ]) {
    const f = await fixture(t);
    await assert.rejects(createWaterMaskPlan(f.root, { fetchImpl: async () => response() }));
    await assert.rejects(lstat(join(f.root, "water")), /ENOENT/);
  }
});
