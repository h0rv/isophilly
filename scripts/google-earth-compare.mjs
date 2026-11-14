// Prove that the optional water finish does not change any other atlas pixel.
import assert from "node:assert/strict";
import { readFile, realpath } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import sharp from "sharp";
import { validateEarthPlan } from "./google-earth-lib.mjs";
import { digest, writeJson } from "./google-earth-source.mjs";
import { loadWaterMasks } from "./google-earth-water-grade.mjs";

const { values } = parseArgs({
  options: { root: { type: "string", default: "data/google-earth" } },
});
const root = resolve(values.root);
const reference = JSON.parse(await readFile(join(root, "source-site.json")));
const before = await realpath(reference.generation),
  after = await realpath(join(root, "site"));
assert.notEqual(before, after, "Compare two different immutable generations");
const beforeMeta = await readFile(join(before, "meta")),
  afterMeta = await readFile(join(after, "meta"));
assert.equal(digest(beforeMeta), reference.meta_sha256, "Original atlas metadata changed");
assert.equal(JSON.parse(afterMeta).water_style, "city", "Build the City water treatment first");
const planBytes = await readFile(join(root, "plan.json"));
const plan = validateEarthPlan(JSON.parse(planBytes));
const masks = await loadWaterMasks(root, planBytes, await readFile(join(root, "manifest.json")));
assert.equal(JSON.parse(afterMeta).water_mask_sha256, masks.sha256);
const report = {
  schema_version: 1,
  plan_sha256: digest(planBytes),
  before_meta_sha256: digest(beforeMeta),
  after_meta_sha256: digest(afterMeta),
  water_mask_sha256: masks.sha256,
  leaves: 0,
  pixels: 0,
  selected_water_pixels: 0,
  changed_pixels: 0,
  unchanged_outside_water: true,
};
for (const view of plan.views)
  for (const tile of view.tiles) {
    const prefix = view.id === "se" ? [] : ["views", view.id];
    const parts = [...prefix, "tiles", "8", `${tile.x}`, `${tile.y}.webp`];
    const [left, right] = await Promise.all([
      sharp(await readFile(join(before, ...parts)))
        .ensureAlpha()
        .raw()
        .toBuffer(),
      sharp(await readFile(join(after, ...parts)))
        .ensureAlpha()
        .raw()
        .toBuffer(),
    ]);
    assert.equal(left.length, plan.tile_size ** 2 * 4);
    assert.equal(right.length, left.length);
    const record = masks.images.get(`${view.id}/${tile.x}/${tile.y}`);
    let mask;
    if (record) {
      const bytes = await readFile(record.file);
      assert.equal(digest(bytes), record.sha256);
      if (record.water_pixels) {
        mask = await sharp(bytes).raw().toBuffer();
        assert.equal(mask.length, left.length);
      }
    }
    let selected = 0;
    if (!mask)
      assert.deepEqual(left, right, `Unmasked leaf changed: ${view.id}/${tile.x}/${tile.y}`);
    for (let i = 0; mask && i < left.length; i += 4) {
      const water = mask?.[i] === 255;
      if (water) selected++;
      const changed =
        left[i] !== right[i] || left[i + 1] !== right[i + 1] || left[i + 2] !== right[i + 2];
      if (changed) {
        assert.ok(water, `Changed non-water pixel ${view.id}/${tile.x}/${tile.y}, offset ${i}`);
        report.changed_pixels++;
      }
      if (left[i + 3] !== right[i + 3]) assert.fail("Water treatment changed opacity");
    }
    assert.equal(selected, record?.water_pixels ?? 0);
    report.selected_water_pixels += selected;
    report.pixels += left.length / 4;
    report.leaves++;
    if (report.leaves % 1000 === 0) console.log(`Compared ${report.leaves} leaves`);
  }
report.finished_at = new Date().toISOString();
await writeJson(join(root, "compare.json"), report);
console.log(JSON.stringify(report));
