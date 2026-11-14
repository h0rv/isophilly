import assert from "node:assert/strict";
import {
  lstat,
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { refreshGoogleEarth } from "../scripts/google-earth-refresh.mjs";

const repo = fileURLToPath(new URL("../", import.meta.url));
const load = async (file) => JSON.parse(await readFile(file, "utf8"));
const save = (file, value) => writeFile(file, JSON.stringify(value));
async function fixture(t) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "earth-refresh-")));
  t.after(() => rm(root, { recursive: true, force: true }));
  const generation = join(root, ".site-building-original");
  await mkdir(join(generation, "tiles", "0", "0"), { recursive: true });
  const meta = {
    schema_version: 2,
    imagery_source: "google-earth-public",
    tile_version: "unchanged-pixels",
    source_plan_sha256: "plan",
    reference_world_sha256: "reference",
    home_zoom: 5,
    landmarks: [{ name: "Rocky" }, { name: "City Hall", point: [1, 2] }],
  };
  const report = {
    complete: true,
    water_style: "city",
    plan_sha256: "plan",
    views: [{ id: "se" }],
  };
  await save(join(generation, "meta"), meta);
  await save(join(generation, "build.json"), report);
  await save(join(generation, "coverage.json"), {
    tile_version: meta.tile_version,
    tiles: ["0/0/0"],
  });
  await writeFile(join(generation, "tiles", "0", "0", "0.webp"), "fake untouched WebP bytes");
  await writeFile(join(generation, "app.js"), "old viewer");
  await mkdir(join(root, "raw"));
  await writeFile(join(root, "raw", "private.png"), "not atlas data");
  const output = join(root, "site");
  await symlink(".site-building-original", output, "dir");
  return { root, output, generation, meta, report };
}
async function unchanged(f) {
  assert.equal(await realpath(f.output), f.generation);
  assert.deepEqual(await load(join(f.generation, "meta")), f.meta);
  assert.deepEqual(await load(join(f.generation, "build.json")), f.report);
  assert.deepEqual((await readdir(f.root)).sort(), [".site-building-original", "raw", "site"]);
}

test("refresh swaps a cloned generation, preserves tiles/provenance, and copies current static assets", async (t) => {
  const f = await fixture(t);
  await mkdir(join(f.generation, "views", "nw"), { recursive: true });
  await save(join(f.generation, "views", "nw", "meta"), f.meta);
  const result = await refreshGoogleEarth({ root: f.root });
  assert.equal(await realpath(f.output), result.generation);
  assert.ok((await lstat(f.output)).isSymbolicLink());
  assert.notEqual(result.generation, f.generation);
  assert.match(result.report.viewer_revision, /^[a-f0-9]{16}$/);
  assert.deepEqual(result.report, { ...f.report, viewer_revision: result.report.viewer_revision });
  assert.deepEqual(await load(join(result.generation, "build.json")), result.report);
  for (const base of [result.generation, join(result.generation, "views", "nw")]) {
    assert.deepEqual(await load(join(base, "meta")), {
      ...f.meta,
      home_zoom: 7,
      landmarks: [f.meta.landmarks[1]],
    });
  }
  for (const file of ["tiles/0/0/0.webp", "coverage.json"]) {
    assert.deepEqual(
      await readFile(join(result.generation, file)),
      await readFile(join(f.generation, file)),
    );
  }
  for (const file of [
    "app.js",
    "city-overlay.js",
    "map-presentation.js",
    "index.html",
    "neighborhoods.json",
    "_headers",
  ]) {
    assert.deepEqual(
      await readFile(join(result.generation, file)),
      await readFile(join(repo, "static", file)),
    );
  }
  assert.deepEqual(await load(join(f.generation, "meta")), f.meta);
  assert.deepEqual(await load(join(f.generation, "build.json")), f.report);
  assert.equal(await readFile(join(f.generation, "app.js"), "utf8"), "old viewer");
  await assert.rejects(lstat(join(result.generation, "raw")), { code: "ENOENT" });
  const second = await refreshGoogleEarth({ root: f.root, homeZoom: 6 });
  assert.equal((await load(join(second.generation, "meta"))).home_zoom, 6);
  assert.equal((await load(join(result.generation, "meta"))).home_zoom, 7);
});

test("rejects incomplete, foreign or unsupported metadata before creating a stage", async (t) => {
  for (const change of ["incomplete", "schema", "source"]) {
    await t.test(change, async (t) => {
      const f = await fixture(t);
      if (change === "incomplete") {
        f.report.complete = false;
        await save(join(f.generation, "build.json"), f.report);
      } else {
        f.meta[change === "schema" ? "schema_version" : "imagery_source"] = "invalid";
        await save(join(f.generation, "meta"), f.meta);
      }
      await assert.rejects(refreshGoogleEarth({ root: f.root }));
      await unchanged(f);
    });
  }
});

test("rejects symlink entries and cleans only its own failed stage", async (t) => {
  const f = await fixture(t);
  await symlink(join(f.root, "raw"), join(f.generation, "escape"), "dir");
  await assert.rejects(refreshGoogleEarth({ root: f.root }), /Unsafe atlas entry/);
  await unchanged(f);
  assert.equal(await readFile(join(f.root, "raw", "private.png"), "utf8"), "not atlas data");
});

test("rejects non-generated outputs, directories and outside-workspace generations", async (t) => {
  const f = await fixture(t);
  await assert.rejects(refreshGoogleEarth({ root: f.root, output: f.generation }));
  await symlink("raw", join(f.root, "other"), "dir");
  await assert.rejects(refreshGoogleEarth({ root: f.root, output: join(f.root, "other") }));
  const outside = await mkdtemp(join(tmpdir(), ".site-building-outside-"));
  t.after(() => rm(outside, { recursive: true, force: true }));
  await symlink(outside, join(f.root, "outside"), "dir");
  await assert.rejects(refreshGoogleEarth({ root: f.root, output: join(f.root, "outside") }));
  await assert.rejects(refreshGoogleEarth({ root: f.root, output: join(outside, "site") }));
  assert.equal(await realpath(f.output), f.generation);
});
