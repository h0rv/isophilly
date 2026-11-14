import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from "node:fs/promises";
import { createServer, request } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import proj4 from "proj4";
import {
  blocksForPlan,
  createEarthPlan,
  earthTileBounds,
  sourcePoints,
  validateEarthPlan,
} from "../scripts/google-earth-lib.mjs";
import { STATE_PLANE } from "../scripts/google-imagery-lib.mjs";
import { inverseIsometric, projectStatePlane } from "../tools/google-imagery/projection.js";

const cli = fileURLToPath(new URL("../scripts/google-earth.mjs", import.meta.url));
const hash = (bytes) => createHash("sha256").update(bytes).digest("hex");
const load = async (file) => JSON.parse(await readFile(file, "utf8"));
const run = (...args) => spawnSync(process.execPath, [cli, ...args], { encoding: "utf8" });
async function fixture(t, overrides = {}) {
  const root = await mkdtemp(join(tmpdir(), "earth-plan-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  return {
    root,
    sourceMeta: {
      schema_version: 2,
      tile_version: "fixture-v1",
      iso_bounds: [985000.123, 309000.456, 1010600.789, 321800.222],
      city_hall: [986286.75, 310337.9],
      landmarks: [],
      counts: { buildings: 1, building_meshes: 1 },
      home_zoom: 3,
      max_zoom: 10,
      max_tile_zoom: 8,
      rich: { home_zoom: 4, views: [] },
    },
    sourceCoverage: {
      schema_version: 1,
      tile_version: "fixture-v1",
      tiles: ["0/0/0", "8/3/4", "8/2/3", "8/255/255"],
    },
    ...overrides,
  };
}

test("pins exact reference bytes and immutable plans without any Google calls", async (t) => {
  const f = await fixture(t);
  const metaBytes = Buffer.from(`${JSON.stringify(f.sourceMeta, null, 1)}\n\n`);
  f.sourceMeta = metaBytes;
  const plan = await createEarthPlan(f);
  assert.equal(plan.source, "google-earth-public");
  assert.equal(plan.tile_size, 512);
  assert.equal(plan.source_level, 18);
  assert.equal(plan.height_origin, 0);
  assert.deepEqual(plan.views[0], {
    id: "se",
    angle: 0,
    iso_bounds: JSON.parse(metaBytes).iso_bounds,
    tiles: [
      { x: 2, y: 3 },
      { x: 3, y: 4 },
      { x: 255, y: 255 },
    ],
  });
  assert.deepEqual(await readFile(join(f.root, "reference/meta.json")), metaBytes);
  const hashes = await load(join(f.root, "reference/hashes.json"));
  assert.equal(hashes.meta.sha256, hash(metaBytes));
  assert.equal(
    hashes.coverage.sha256,
    hash(await readFile(join(f.root, "reference/coverage.json"))),
  );
  const before = await readFile(join(f.root, "plan.json"));
  const modified = (await stat(join(f.root, "plan.json"))).mtimeMs;
  assert.deepEqual(await createEarthPlan(f), plan);
  assert.equal((await stat(join(f.root, "plan.json"))).mtimeMs, modified);
  for (const change of [
    { tileSize: 256 },
    { heightOrigin: 12 },
    { level: 20 },
    { angles: [0, 45] },
    { limit: 1 },
  ]) {
    await assert.rejects(createEarthPlan({ ...f, ...change }), /new root/);
    assert.deepEqual(await readFile(join(f.root, "plan.json")), before);
  }
  await writeFile(join(f.root, "reference/meta.json"), `${metaBytes} `);
  await assert.rejects(createEarthPlan({ root: f.root }), /hash mismatch/);
});

test("fetches reference endpoints only once and checks matching versions before pinning", async (t) => {
  const f = await fixture(t);
  const meta = Buffer.from(` ${JSON.stringify(f.sourceMeta)}\n`),
    coverage = Buffer.from(JSON.stringify(f.sourceCoverage));
  let calls = 0;
  const server = createServer((req, res) => {
    calls++;
    res.end(req.url === "/meta" ? meta : coverage);
  });
  await new Promise((accept) => server.listen(0, "127.0.0.1", accept));
  t.after(() => new Promise((accept) => server.close(accept)));
  const base = `http://127.0.0.1:${server.address().port}`;
  const opts = { root: f.root, metaUrl: `${base}/meta`, coverageUrl: `${base}/coverage.json` };
  const plan = await createEarthPlan(opts);
  assert.equal(calls, 2);
  assert.deepEqual(await readFile(join(f.root, "reference/meta.json")), meta);
  assert.deepEqual(await createEarthPlan(opts), plan);
  assert.equal(calls, 2);
  const bad = await fixture(t);
  bad.sourceCoverage.tile_version = "different";
  await assert.rejects(createEarthPlan(bad), /version/);
  await assert.rejects(stat(join(bad.root, "reference")), { code: "ENOENT" });
  await mkdir(join(bad.root, "reference"));
  await assert.rejects(createEarthPlan(bad), /Incomplete pinned/);
});

test("default SE planning preserves all 9214 locally cached zoom-8 leaves", async (t) => {
  const f = await fixture(t);
  const plan = await createEarthPlan({ root: f.root });
  const reference = fileURLToPath(new URL("../data/reference/", import.meta.url));
  const meta = await load(join(reference, "meta.json")),
    coverage = await load(join(reference, "coverage.json"));
  assert.equal(plan.views[0].tiles.length, 9214);
  assert.deepEqual(plan.source_meta, meta);
  assert.deepEqual(plan.views[0].iso_bounds, meta.iso_bounds);
  assert.deepEqual(
    new Set(plan.views[0].tiles.map(({ x, y }) => `8/${x}/${y}`)),
    new Set(coverage.tiles.filter((k) => k.startsWith("8/"))),
  );
});

test("all eight views conservatively include each transformed source quad and building height", async (t) => {
  const f = await fixture(t);
  const plan = await createEarthPlan({ ...f, angles: [315, 270, 225, 180, 135, 90, 45, 0] });
  assert.deepEqual(
    plan.views.map((v) => v.id),
    ["se", "v045", "sw", "v135", "nw", "v225", "ne", "v315"],
  );
  for (const view of plan.views.slice(1)) {
    const keys = new Set(view.tiles.map(({ x, y }) => `${x}/${y}`));
    const [left, top, right, bottom] = view.iso_bounds;
    const side = Math.max(right - left, bottom - top) / 256;
    for (const tile of plan.views[0].tiles) {
      const [l, t0, r, b] = earthTileBounds(plan, plan.views[0], tile);
      // Check corners, edges and interiors at both extremes of building height.
      for (const x of [l, (l + r) / 2, r])
        for (const y of [t0, (t0 + b) / 2, b])
          for (const h of [0, 200, 400]) {
            const p = projectStatePlane(...inverseIsometric(x, y, 0), h, view.angle);
            assert.ok(
              p[0] >= left - 1e-8 &&
                p[0] <= right + 1e-8 &&
                p[1] >= top - 1e-8 &&
                p[1] <= bottom + 1e-8,
            );
            const tx = Math.max(0, Math.min(255, Math.floor((p[0] - left) / side)));
            const ty = Math.max(0, Math.min(255, Math.floor((p[1] - top) / side)));
            assert.ok(keys.has(`${tx}/${ty}`), `${view.id} missed ${tx}/${ty}`);
          }
    }
    assert.ok(
      view.tiles.length < 100,
      "must use per-leaf quads, not a citywide bounding rectangle",
    );
  }
});

test("continuous grid and deterministic blocks include unplanned block cells, clipping at grid edge", async (t) => {
  const plan = await createEarthPlan(await fixture(t));
  const view = plan.views[0],
    a = earthTileBounds(plan, view, { x: 2, y: 3 }),
    b = earthTileBounds(plan, view, { x: 3, y: 3 });
  assert.equal(a[2], b[0]);
  assert.notEqual(a[0], Math.fround(a[0]));
  const blocks = blocksForPlan(plan, 4);
  assert.deepEqual(
    blocks.map(({ view, angle, x, y, width, height, tiles }) => ({
      view,
      angle,
      x,
      y,
      width,
      height,
      tiles,
    })),
    [
      { view: "se", angle: 0, x: 0, y: 0, width: 4, height: 4, tiles: [{ x: 2, y: 3 }] },
      { view: "se", angle: 0, x: 0, y: 4, width: 4, height: 4, tiles: [{ x: 3, y: 4 }] },
      { view: "se", angle: 0, x: 252, y: 252, width: 4, height: 4, tiles: [{ x: 255, y: 255 }] },
    ],
  );
  const origin = earthTileBounds(plan, view, { x: 0, y: 0 }),
    last = earthTileBounds(plan, view, { x: 3, y: 3 });
  assert.deepEqual(blocks[0].bounds, [origin[0], origin[1], last[2], last[3]]);
  const clipped = blocksForPlan(plan, 3).at(-1);
  assert.deepEqual([clipped.x, clipped.y, clipped.width, clipped.height], [255, 255, 1, 1]);
  const shuffled = structuredClone(plan);
  shuffled.views[0].tiles.reverse();
  assert.deepEqual(blocksForPlan(shuffled), blocks);
  for (const invalid of [0, -1, 1.5, 257, Infinity])
    assert.throws(() => blocksForPlan(plan, invalid), /blockSize/);
});

test("sourcePoints covers every angular cell, including elevated footprints and one-cell padding", () => {
  for (const angle of [0, 45, 90, 180, 315])
    for (const level of [18, 19, 20]) {
      const [cx, cy] = projectStatePlane(
        ...proj4("EPSG:4326", STATE_PLANE, [-75.1636, 39.9526]),
        0,
        angle,
      );
      const bounds = [cx - 140, cy - 100, cx + 140, cy + 100];
      const points = sourcePoints(bounds, angle, level);
      for (const origin of [-120, 120])
        assert.deepEqual(
          sourcePoints(bounds, angle, level, 400, origin),
          sourcePoints(
            [bounds[0], bounds[1] - origin, bounds[2], bounds[3] - origin],
            angle,
            level,
          ),
        );
      const cell = 90 / 2 ** (level - 2);
      const key = (lon, lat) => `${Math.floor(lon / cell)}/${Math.floor(lat / cell)}`;
      const keys = new Set(points.map((p) => key(p.longitude, p.latitude)));
      assert.equal(points.length, keys.size);
      for (const { latitude, longitude } of points) {
        assert.equal((latitude / cell) % 1, 0.5);
        assert.equal((longitude / cell) % 1, -0.5);
      }
      const lonlat = [];
      for (const x of [bounds[0], (bounds[0] + bounds[2]) / 2, bounds[2]])
        for (const y of [bounds[1], bounds[3]])
          for (const h of [0, 200, 400]) {
            const [lon, lat] = proj4(STATE_PLANE, "EPSG:4326", inverseIsometric(x, y + h, angle));
            lonlat.push([lon, lat]);
            assert.ok(keys.has(key(lon, lat)));
          }
      const minX = Math.min(...lonlat.map(([lon]) => Math.floor(lon / cell)));
      const maxX = Math.max(...lonlat.map(([lon]) => Math.floor(lon / cell)));
      const minY = Math.min(...lonlat.map(([, lat]) => Math.floor(lat / cell)));
      const maxY = Math.max(...lonlat.map(([, lat]) => Math.floor(lat / cell)));
      assert.equal(points.length, (maxX - minX + 3) * (maxY - minY + 3));
    }
  assert.throws(() => sourcePoints([0, 0, 1, 1], 0, 18), /outside Philadelphia/);
  const [x, y] = projectStatePlane(...proj4("EPSG:4326", STATE_PLANE, [-75.16, 39.95]), 0);
  assert.throws(() => sourcePoints([x - 1500, y - 1500, x + 1500, y + 1500], 0, 20), /10000/);
});

test("validates bad grids, duplicate IDs/tiles, sizes, angles and pilot filters", async (t) => {
  const f = await fixture(t),
    plan = await createEarthPlan(f);
  for (const mutate of [
    (p) => {
      p.tile_size = 300;
    },
    (p) => {
      p.source_level = 17;
    },
    (p) => {
      p.height_origin = NaN;
    },
    (p) => {
      p.views.push(p.views[0]);
    },
    (p) => {
      p.views[0].tiles.push(p.views[0].tiles[0]);
    },
    (p) => {
      p.views[0].tiles[0].x = 256;
    },
    (p) => {
      p.views[0].tiles[0].y = 0.5;
    },
    (p) => {
      p.views[0].id = "../se";
    },
    (p) => {
      p.views[0].iso_bounds[0]++;
    },
  ]) {
    const bad = structuredClone(plan);
    mutate(bad);
    assert.throws(() => validateEarthPlan(bad));
  }
  for (const options of [
    { angles: [0, 0] },
    { angles: [0, 360] },
    { angles: [0, 44] },
    { angles: [90] },
    { angles: [0, 45], limit: 1 },
    { angles: [0, 45], center: [-75.16, 39.95] },
    { limit: 0 },
    { radius: -1 },
    { center: [0, 0] },
  ])
    await assert.rejects(createEarthPlan({ ...f, ...options }));
  const pilot = await fixture(t);
  const selected = await createEarthPlan({ ...pilot, limit: 1 });
  assert.deepEqual(selected.views[0].tiles, [{ x: 2, y: 3 }]);
  assert.deepEqual(selected.views[0].iso_bounds, pilot.sourceMeta.iso_bounds);
});

test("CLI help, status counts and exact manifest plan hash checks", async (t) => {
  const f = await fixture(t),
    plan = await createEarthPlan(f);
  const help = run("--help");
  assert.equal(help.status, 0);
  assert.match(help.stdout, /no API key required/);
  assert.notEqual(run("unknown").status, 0);
  let result = run("status", "--root", f.root);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /se: 0\/3 completed; 3 pending/);
  const manifest = {
    schema_version: 1,
    plan_sha256: hash(await readFile(join(f.root, "plan.json"))),
    images: plan.views[0].tiles
      .slice(0, 2)
      .map((tile) => ({ ...tile, view: "se", sha256: "a".repeat(64) })),
  };
  const file = join(f.root, "raw/se/8/2/3.png");
  await mkdir(join(f.root, "raw/se/8/2"), { recursive: true });
  await writeFile(file, "fixture");
  await writeFile(join(f.root, "manifest.json"), JSON.stringify(manifest));
  result = run("status", "--root", f.root);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /se: 1\/3 completed; 2 pending/);
  await writeFile(join(f.root, "plan.json"), `${JSON.stringify(plan)} `);
  result = run("status", "--root", f.root);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /plan_sha256/);
});

test("foreground static server serves MIME types and rejects traversal and escaping symlinks", async (t) => {
  const f = await fixture(t),
    site = join(f.root, "site");
  await mkdir(site);
  await writeFile(join(site, "index.html"), "<h1>Earth</h1>");
  await writeFile(join(site, "meta"), "{}");
  await writeFile(join(f.root, "private.txt"), "private");
  await symlink(join(f.root, "private.txt"), join(site, "escape"));
  const probe = createServer();
  await new Promise((accept) => probe.listen(0, "127.0.0.1", accept));
  const port = probe.address().port;
  await new Promise((accept) => probe.close(accept));
  const child = spawn(process.execPath, [cli, "serve", "--output", site, "--port", `${port}`], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  t.after(async () => {
    if (child.exitCode === null) {
      child.kill();
      await new Promise((accept) => child.once("exit", accept));
    }
  });
  await new Promise((accept, reject) => {
    const timer = setTimeout(() => reject(new Error("Server startup timed out")), 10000);
    child.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`Server exited ${code}`));
    });
    child.stdout.once("data", () => {
      clearTimeout(timer);
      accept();
    });
  });
  const get = (path) =>
    new Promise((accept, reject) => {
      request({ hostname: "127.0.0.1", port, path }, (res) => {
        let body = "";
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () =>
          accept({ status: res.statusCode, type: res.headers["content-type"], body }),
        );
      })
        .on("error", reject)
        .end();
    });
  assert.deepEqual(await get("/"), {
    status: 200,
    type: "text/html; charset=utf-8",
    body: "<h1>Earth</h1>",
  });
  assert.equal((await get("/meta")).type, "application/json");
  for (const path of ["/../private.txt", "/%2e%2e/private.txt", "/escape"])
    assert.equal((await get(path)).status, 403);
  assert.equal((await get("/%zz")).status, 400);
  assert.equal((await get("/missing")).status, 404);
});
