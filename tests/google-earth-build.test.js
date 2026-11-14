import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, mkdir, mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import sharp from "sharp";
import { buildGoogleEarth } from "../scripts/google-earth-build.mjs";
import { DECODER, RENDERER } from "../scripts/google-earth-source.mjs";
import { WATER_RENDERER } from "../scripts/google-earth-water-lib.mjs";

const script = fileURLToPath(new URL("../scripts/google-earth-build.mjs", import.meta.url));
const sha256 = (data) => createHash("sha256").update(data).digest("hex");
const load = async (file) => JSON.parse(await readFile(file, "utf8"));
const save = (file, data) => writeFile(file, JSON.stringify(data));
const tile = (base, z, x, y) => path.join(base, "tiles", `${z}`, `${x}`, `${y}.webp`);
const beige = [222, 212, 196, 255];

function sourceMeta() {
  return {
    schema_version: 2,
    tile_version: "original-scene",
    iso_bounds: [0, 0, 256, 256],
    city_hall: [100, 100],
    counts: { buildings: 1, building_meshes: 1 },
    landmarks: [],
    home_zoom: 5,
    max_tile_zoom: 8,
    max_zoom: 10,
    google_imagery_sha256: "a".repeat(64),
    google_attributions: ["Stale credit"],
    rich: {
      home_zoom: 7,
      views: ["se", "sw", "nw", "ne"].map((id) => ({
        id,
        label: id,
        tile_version: `old-${id}`,
        iso_bounds: [0, 0, 256, 256],
        city_hall: null,
        landmarks: [],
      })),
    },
  };
}
async function fixture(t, size = 256) {
  const root = await mkdtemp(path.join(os.tmpdir(), "google-earth-build-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  const output = path.join(root, "site");
  const plan = {
    schema_version: 1,
    source: "google-earth-public",
    source_meta: sourceMeta(),
    zoom: 8,
    tile_size: size,
    source_level: 19,
    height_origin: 12,
    views: [
      {
        id: "se",
        angle: 0,
        iso_bounds: [-10, -20, 300, 400],
        tiles: [
          { x: 130, y: 67 },
          { x: 131, y: 67 },
        ],
      },
    ],
  };
  // Whitespace is intentionally part of the signed plan, not normalized JSON.
  const planBytes = Buffer.from(`${JSON.stringify(plan, null, 2)}\n`);
  await writeFile(path.join(root, "plan.json"), planBytes);
  const images = [];
  for (const [index, { x, y }] of plan.views[0].tiles.entries()) {
    const data = Buffer.alloc(size * size * 4);
    const color = index === 0 ? [100, 150, 200, 255] : [50, 100, 150, 255];
    for (let i = 0; i < data.length; i += 4) data.set(color, i);
    if (index === 0) data[4] = 101; // Preserve a one-channel, one-pixel detail.
    const bytes = await sharp(data, { raw: { width: size, height: size, channels: 4 } })
      .png()
      .toBuffer();
    const file = path.join(root, "raw", "se", "8", `${x}`, `${y}.png`);
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, bytes);
    images.push({
      view: "se",
      x,
      y,
      sha256: sha256(bytes),
      captured_at: "2026-01-01T00:00:00Z",
      attributions: ["Google Earth", index === 0 ? "Provider A" : "Provider B"],
    });
  }
  const manifest = { schema_version: 1, plan_sha256: sha256(planBytes), images };
  await save(path.join(root, "manifest.json"), manifest);
  return { root, output, plan, manifest, options: { root, output } };
}
async function pixel(file, x, y) {
  const { data, info } = await sharp(file)
    .ensureAlpha()
    .raw()
    .toBuffer({ resolveWithObject: true });
  return [...data.subarray((y * info.width + x) * 4, (y * info.width + x) * 4 + 4)];
}
async function rewritePlan(f, plan) {
  const bytes = Buffer.from(JSON.stringify(plan));
  await writeFile(path.join(f.root, "plan.json"), bytes);
  f.manifest.plan_sha256 = sha256(bytes);
  await save(path.join(f.root, "manifest.json"), f.manifest);
}

test("builds styled lossless leaves, correctly placed parents, coverage, metadata and static viewer", async (t) => {
  const f = await fixture(t);
  const report = await buildGoogleEarth(f.options);
  assert.equal(report.complete, true);
  assert.deepEqual(report.views, [{ id: "se", planned: 2, present: 2, missing: 0 }]);
  assert.equal(report.missing_count, 0);
  assert.deepEqual(await load(path.join(f.output, "build.json")), report);
  const base = f.output;
  const leaf = tile(base, 8, 130, 67);
  assert.deepEqual(await pixel(leaf, 0, 0), [93, 152, 211, 255]);
  assert.deepEqual(await pixel(leaf, 1, 0), [94, 152, 211, 255]);
  assert.deepEqual(await pixel(tile(base, 8, 131, 67), 0, 0), [41, 100, 159, 255]);
  const parent = tile(base, 7, 65, 33);
  assert.deepEqual(await pixel(parent, 64, 64), beige);
  assert.deepEqual(await pixel(parent, 192, 64), beige);
  assert.deepEqual(await pixel(parent, 64, 192), [93, 152, 211, 255]);
  assert.deepEqual(await pixel(parent, 192, 192), [41, 100, 159, 255]);
  assert.deepEqual(await pixel(tile(base, 6, 32, 16), 16, 16), beige);
  assert.deepEqual(await pixel(tile(base, 6, 32, 16), 160, 224), [93, 152, 211, 255]);
  const coverage = await load(path.join(base, "coverage.json"));
  assert.deepEqual(coverage.tiles, [
    "0/0/0",
    "1/1/0",
    "2/2/1",
    "3/4/2",
    "4/8/4",
    "5/16/8",
    "6/32/16",
    "7/65/33",
    "8/130/67",
    "8/131/67",
  ]);
  assert.deepEqual(await load(path.join(f.output, "coverage.json")), coverage);
  const meta = await load(path.join(f.output, "meta"));
  assert.ok(meta.tile_version.startsWith(`google-earth-${f.manifest.plan_sha256.slice(0, 16)}-`));
  assert.match(meta.tile_version, /-[a-f0-9]{16}-se$/);
  assert.equal(coverage.tile_version, meta.tile_version);
  assert.deepEqual(meta.iso_bounds, f.plan.views[0].iso_bounds);
  assert.deepEqual(meta.rich, f.plan.source_meta.rich);
  assert.equal(meta.rich_reference_only, true);
  assert.equal(meta.imagery_source, "google-earth-public");
  assert.deepEqual(meta.reference_counts, f.plan.source_meta.counts);
  assert.deepEqual(meta.counts, { buildings: 0, building_meshes: 0, capture_tiles: 2 });
  assert.equal(meta.world_sha256, undefined);
  assert.deepEqual(meta.imagery_attributions, ["Google Earth", "Provider A", "Provider B"]);
  assert.equal(meta.google_imagery_sha256, undefined);
  assert.equal(meta.google_attributions, undefined);
  assert.equal(meta.max_tile_zoom, 8);
  assert.equal(meta.max_zoom, 10);
  await assert.rejects(lstat(path.join(f.output, "views", "se", "tiles")), { code: "ENOENT" });
  // Do not export the default pyramid twice: duplicated files exceed the
  // existing static host's file limit even when they are hard-linked locally.
  for (const name of [
    "app.js",
    "city-overlay.js",
    "index.html",
    "neighborhoods.json",
    "_headers",
  ]) {
    assert.deepEqual(
      await readFile(path.join(f.output, name)),
      await readFile(new URL(`../static/${name}`, import.meta.url)),
    );
  }
  assert.deepEqual((await load(path.join(f.output, "views.json"))).views[0], {
    id: "se",
    angle: 0,
    iso_bounds: f.plan.views[0].iso_bounds,
    tile_version: meta.tile_version,
    meta: "/meta",
    coverage: "/coverage.json",
    tiles: "/tiles/{z}/{x}/{y}.webp",
  });
  // A successful second build replaces, rather than merges with, the old output.
  await writeFile(path.join(f.output, "stale"), "old");
  await buildGoogleEarth(f.options);
  await assert.rejects(lstat(path.join(f.output, "stale")), { code: "ENOENT" });
});

test("requires explicit partial mode and reports only present leaf coverage", async (t) => {
  const f = await fixture(t);
  await rm(path.join(f.root, "raw/se/8/131/67.png"));
  await assert.rejects(buildGoogleEarth(f.options), /1 planned images missing/);
  await assert.rejects(lstat(f.output), { code: "ENOENT" });
  const report = await buildGoogleEarth({ ...f.options, allowPartial: true });
  assert.equal(report.complete, false);
  assert.equal(report.missing_count, 1);
  assert.deepEqual(report.views, [{ id: "se", planned: 2, present: 1, missing: 1 }]);
  const coverage = await load(path.join(f.output, "coverage.json"));
  const partialVersion = coverage.tile_version;
  assert.equal(coverage.tiles.includes("8/130/67"), true);
  assert.equal(coverage.tiles.includes("8/131/67"), false);
  assert.equal(coverage.tiles.length, 9);
  assert.deepEqual(await pixel(tile(f.output, 7, 65, 33), 192, 192), beige);
  assert.deepEqual((await load(path.join(f.output, "meta"))).imagery_attributions, [
    "Google Earth",
    "Provider A",
  ]);
  // A raw file without a manifest entry is not a verified capture.
  f.manifest.images = [];
  await save(path.join(f.root, "manifest.json"), f.manifest);
  const empty = await buildGoogleEarth({ ...f.options, allowPartial: true });
  assert.equal(empty.missing_count, 2);
  assert.notEqual(
    (await load(path.join(f.output, "coverage.json"))).tile_version,
    partialVersion,
    "Different parent pixels must never reuse immutable tile URLs",
  );
  assert.deepEqual((await load(path.join(f.output, "coverage.json"))).tiles, []);
  await assert.rejects(lstat(tile(f.output, 0, 0, 0)), { code: "ENOENT" });
});

test("rejects exact-plan and raw PNG hash mismatches without changing an existing output", async (t) => {
  const f = await fixture(t);
  await mkdir(f.output);
  await writeFile(path.join(f.output, "sentinel"), "keep me");
  const bytes = await readFile(path.join(f.root, "plan.json"));
  await writeFile(path.join(f.root, "plan.json"), Buffer.concat([bytes, Buffer.from(" ")]));
  await assert.rejects(buildGoogleEarth(f.options), /plan_sha256/);
  await writeFile(path.join(f.root, "plan.json"), bytes);
  f.manifest.images[1].sha256 = "0".repeat(64);
  await save(path.join(f.root, "manifest.json"), f.manifest);
  await assert.rejects(
    buildGoogleEarth({ ...f.options, allowPartial: true }),
    /PNG sha256 mismatch/,
  );
  assert.equal(await readFile(path.join(f.output, "sentinel"), "utf8"), "keep me");
  assert.deepEqual(
    (await readdir(f.root)).filter((name) => name.startsWith(".site")),
    [],
  );
});

test("validates schema, limits, safe views, coordinates, duplicates and provenance", async (t) => {
  const f = await fixture(t);
  const invalidPlans = [
    { schema_version: 2 },
    { source: "other" },
    { zoom: 7 },
    { tile_size: 128 },
    { source_level: 17 },
    { height_origin: null },
    { source_meta: {} },
    { views: [] },
    { views: Array(361).fill(f.plan.views[0]) },
    { views: [{ ...f.plan.views[0], id: "../se" }] },
    { views: [{ ...f.plan.views[0], id: "SE" }] },
    { views: [{ ...f.plan.views[0], id: "sw" }] },
    { views: [{ ...f.plan.views[0], angle: null }] },
    { views: [{ ...f.plan.views[0], iso_bounds: [0, 0, 0, 1] }] },
    { views: [{ ...f.plan.views[0], tiles: [{ x: 256, y: 0 }] }] },
    { views: [{ ...f.plan.views[0], tiles: [{ x: -1, y: 0 }] }] },
    { views: [{ ...f.plan.views[0], tiles: [{ x: 1.5, y: 0 }] }] },
    { views: [{ ...f.plan.views[0], tiles: [] }] },
    {
      views: [
        {
          ...f.plan.views[0],
          tiles: [
            { x: 1, y: 2 },
            { x: 1, y: 2 },
          ],
        },
      ],
    },
    { views: [f.plan.views[0], f.plan.views[0]] },
  ];
  for (const change of invalidPlans) {
    await rewritePlan(f, { ...f.plan, ...change });
    await assert.rejects(buildGoogleEarth(f.options));
  }
  await rewritePlan(f, f.plan);
  const valid = structuredClone(f.manifest);
  const badImages = [
    { view: "missing" },
    { x: 256 },
    { x: 129 },
    { y: -1 },
    { sha256: "bad" },
    { captured_at: "yesterday" },
    { captured_at: "2026-01-01" },
    { attributions: [] },
    { attributions: [""] },
    { attributions: ["   "] },
    { attributions: [42] },
  ];
  for (const change of badImages) {
    await save(path.join(f.root, "manifest.json"), {
      ...valid,
      images: [{ ...valid.images[0], ...change }],
    });
    await assert.rejects(buildGoogleEarth({ ...f.options, allowPartial: true }));
  }
  await save(path.join(f.root, "manifest.json"), {
    ...valid,
    images: [valid.images[0], valid.images[0]],
  });
  await assert.rejects(buildGoogleEarth(f.options), /Duplicate manifest/);
  await assert.rejects(buildGoogleEarth({ ...f.options, output: f.root }), /Output must not/);
});

test("rejects transparent, RGB, wrongly sized and non-PNG captures, even in partial mode", async (t) => {
  const f = await fixture(t);
  const create = (width, channels, alpha = 1) =>
    sharp({
      create: {
        width,
        height: 256,
        channels,
        background: { r: 100, g: 150, b: 200, alpha },
      },
    });
  for (const bytes of [
    await create(256, 4, 0.5).png().toBuffer(),
    await create(256, 3).png().toBuffer(),
    await create(128, 4).png().toBuffer(),
    await create(256, 4).webp({ lossless: true }).toBuffer(),
  ]) {
    await writeFile(path.join(f.root, "raw/se/8/130/67.png"), bytes);
    f.manifest.images[0].sha256 = sha256(bytes);
    await save(path.join(f.root, "manifest.json"), f.manifest);
    await assert.rejects(
      buildGoogleEarth({ ...f.options, allowPartial: true }),
      /opaque RGBA|RGBA:/,
    );
  }
});

for (const size of [512, 1024]) {
  test(`keeps ${size}px tile dimensions throughout the pyramid`, async (t) => {
    const f = await fixture(t, size);
    await buildGoogleEarth(f.options);
    for (const [z, x, y] of [
      [8, 130, 67],
      [7, 65, 33],
      [0, 0, 0],
    ]) {
      const meta = await sharp(tile(f.output, z, x, y)).metadata();
      assert.equal(meta.width, size);
      assert.equal(meta.height, size);
    }
    assert.deepEqual(
      await pixel(tile(f.output, 7, 65, 33), size / 4, (size * 3) / 4),
      [93, 152, 211, 255],
    );
  });
}

test("exports independent rotation views and supports the standalone CLI", async (t) => {
  const f = await fixture(t);
  f.plan.views.push({
    id: "v045",
    angle: 45,
    iso_bounds: [-100, -100, 100, 100],
    tiles: [{ x: 0, y: 0 }],
  });
  await rewritePlan(f, f.plan);
  const result = spawnSync(
    process.execPath,
    [script, "--root", f.root, "--output", f.output, "--allow-partial"],
    {
      encoding: "utf8",
      timeout: 30_000,
    },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stderr, /"v045":1/);
  const views = await load(path.join(f.output, "views.json"));
  assert.equal(views.default_view, "se");
  assert.equal(views.views[1].id, "v045");
  assert.equal(views.views[1].angle, 45);
  assert.deepEqual((await load(path.join(f.output, "views/v045/coverage.json"))).tiles, []);
  assert.deepEqual(
    (await load(path.join(f.output, "views/v045/meta"))).iso_bounds,
    [-100, -100, 100, 100],
  );
  assert.equal((await load(path.join(f.output, "build.json"))).complete, false);
  assert.equal(spawnSync(process.execPath, [script, "--unknown"], { encoding: "utf8" }).status, 1);
});

async function waterFixture(t) {
  const f = await fixture(t);
  f.plan.source_meta.iso_bounds = f.plan.views[0].iso_bounds;
  await rewritePlan(f, f.plan);
  const root = path.join(f.root, "water");
  await mkdir(path.join(root, "reference"), { recursive: true });
  const reference = Buffer.from("{}");
  await writeFile(path.join(root, "reference", "water.json"), reference);
  await writeFile(path.join(root, "reference", "water.geojson"), reference);
  const source = { decoder: DECODER, renderer: RENDERER, root_epoch: 1, radius: 6371010 };
  await save(path.join(f.root, "source.json"), source);
  await save(path.join(root, "source.json"), { ...source, renderer: WATER_RENDERER });
  const plan = {
    ...f.plan,
    capture_mode: "water-mask",
    water_renderer: WATER_RENDERER,
    photo_plan_sha256: sha256(await readFile(path.join(f.root, "plan.json"))),
    photo_manifest_sha256: sha256(await readFile(path.join(f.root, "manifest.json"))),
    water_sha256: sha256(reference),
    water_source_sha256: sha256(reference),
    water_source: "https://example.invalid/water",
  };
  await save(path.join(root, "plan.json"), plan);
  const images = [];
  for (const [index, tile] of f.plan.views[0].tiles.entries()) {
    const raw = Buffer.alloc(f.plan.tile_size ** 2 * 4);
    for (let i = 3; i < raw.length; i += 4) raw[i] = 255;
    if (index === 0) raw.set([255, 255, 255, 255], 0);
    const bytes = await sharp(raw, {
      raw: { width: f.plan.tile_size, height: f.plan.tile_size, channels: 4 },
    })
      .png()
      .toBuffer();
    const file = path.join(root, "raw", "se", "8", `${tile.x}`, `${tile.y}.png`);
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(file, bytes);
    images.push({ ...tile, view: "se", sha256: sha256(bytes), water_pixels: index === 0 ? 1 : 0 });
  }
  const manifest = {
    schema_version: 1,
    decoder: DECODER,
    renderer: WATER_RENDERER,
    plan_sha256: sha256(await readFile(path.join(root, "plan.json"))),
    images,
  };
  await save(path.join(root, "manifest.json"), manifest);
  return { ...f, waterRoot: root, waterPlan: plan, waterManifest: manifest };
}

test("City water grading changes only masked pixels, preserves raw photographs, and versions the result", async (t) => {
  const f = await waterFixture(t);
  await buildGoogleEarth(f.options);
  const before = await pixel(tile(f.output, 8, 130, 67), 0, 0);
  const adjacent = await pixel(tile(f.output, 8, 130, 67), 1, 0);
  const land = await pixel(tile(f.output, 8, 131, 67), 0, 0);
  const originalVersion = (await load(path.join(f.output, "meta"))).tile_version;
  const raw = await readFile(path.join(f.root, "raw/se/8/130/67.png"));
  const result = await buildGoogleEarth({ ...f.options, waterStyle: true });
  assert.equal(result.water_style, "city");
  assert.notDeepEqual(await pixel(tile(f.output, 8, 130, 67), 0, 0), before);
  assert.deepEqual(await pixel(tile(f.output, 8, 130, 67), 1, 0), adjacent);
  assert.deepEqual(await pixel(tile(f.output, 8, 131, 67), 0, 0), land);
  assert.deepEqual(await readFile(path.join(f.root, "raw/se/8/130/67.png")), raw);
  assert.notEqual((await load(path.join(f.output, "meta"))).tile_version, originalVersion);
  assert.equal((await load(path.join(f.output, "meta"))).water_style, "city");
  await assert.rejects(
    buildGoogleEarth({ root: f.waterRoot, output: path.join(f.root, "not-a-photo-map") }),
    /not photography/,
  );
});

test("stale, partial, malformed or corrupted water masks never replace an existing atlas", async (t) => {
  for (const mutation of ["binding", "missing", "corrupt", "epoch", "camera", "gray"])
    await t.test(mutation, async (t) => {
      const f = await waterFixture(t);
      await buildGoogleEarth(f.options);
      const before = await readFile(path.join(f.output, "meta"));
      if (mutation === "binding") f.waterPlan.photo_manifest_sha256 = "f".repeat(64);
      if (mutation === "camera") f.waterPlan.height_origin += 1;
      if (["binding", "camera"].includes(mutation)) {
        await save(path.join(f.waterRoot, "plan.json"), f.waterPlan);
        f.waterManifest.plan_sha256 = sha256(await readFile(path.join(f.waterRoot, "plan.json")));
      }
      if (mutation === "missing") f.waterManifest.images.pop();
      const file = path.join(f.waterRoot, "raw/se/8/130/67.png");
      if (mutation === "corrupt") await writeFile(file, "broken");
      if (mutation === "gray") {
        const bytes = await sharp({
          create: {
            width: 256,
            height: 256,
            channels: 4,
            background: { r: 100, g: 100, b: 100, alpha: 1 },
          },
        })
          .png()
          .toBuffer();
        await writeFile(file, bytes);
        f.waterManifest.images[0].sha256 = sha256(bytes);
      }
      if (mutation === "epoch")
        await save(path.join(f.waterRoot, "source.json"), {
          decoder: DECODER,
          renderer: WATER_RENDERER,
          root_epoch: 2,
          radius: 6371010,
        });
      await save(path.join(f.waterRoot, "manifest.json"), f.waterManifest);
      await assert.rejects(buildGoogleEarth({ ...f.options, waterStyle: true }));
      assert.deepEqual(await readFile(path.join(f.output, "meta")), before);
    });
});
