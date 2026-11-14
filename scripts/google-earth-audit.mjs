import assert from "node:assert/strict";
import { mkdir, readFile, realpath, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import sharp from "sharp";
import { earthCoverageScope, validateEarthPlan } from "./google-earth-lib.mjs";
import { digest } from "./google-earth-source.mjs";

const { values } = parseArgs({
  options: { root: { type: "string", default: "data/google-earth" } },
});
const root = resolve(values.root);
const planBytes = await readFile(join(root, "plan.json"));
const plan = validateEarthPlan(JSON.parse(planBytes));
const manifest = JSON.parse(await readFile(join(root, "manifest.json"), "utf8"));
const site = await realpath(join(root, "site"));
const build = JSON.parse(await readFile(join(site, "build.json"), "utf8"));
const metaBytes = await readFile(join(site, "meta"));
const meta = JSON.parse(metaBytes);
assert.equal(manifest.plan_sha256, digest(planBytes));
assert.equal(build.plan_sha256, digest(planBytes));
assert.equal(build.complete, true, "Cannot audit an incomplete atlas as a release candidate");
assert.equal(meta.source_plan_sha256, digest(planBytes));
if (meta.water_style === "city")
  assert.equal(
    meta.water_mask_sha256,
    digest(await readFile(join(root, "water", "manifest.json"))),
  );
const coverage = JSON.parse(await readFile(join(root, "reference/coverage.json"), "utf8"));
const referenceMeta = JSON.parse(await readFile(join(root, "reference/meta.json"), "utf8"));
const scope = earthCoverageScope(plan, referenceMeta, coverage);
const output = join(root, "audit");
await mkdir(output, { recursive: true });
const images = new Map(manifest.images.map((tile) => [`${tile.view}/${tile.x}/${tile.y}`, tile]));
const sheets = [];
let total = 0;
const report = {
  schema_version: 1,
  plan_sha256: digest(planBytes),
  atlas_tile_version: meta.tile_version,
  atlas_meta_sha256: digest(metaBytes),
  water_style: meta.water_style ?? "source",
  scope: scope.scope,
  source_city_leaf_count: scope.referenceLeafCount,
  expected_capture_leaf_count: scope.expectedLeafCount,
  checked: 0,
  sheets,
  tiles: [],
};
for (const view of plan.views) {
  const groups = new Map();
  for (const tile of view.tiles) {
    const id = `${Math.floor(tile.x / 16)}/${Math.floor(tile.y / 16)}`;
    if (!groups.has(id)) groups.set(id, []);
    groups.get(id).push(tile);
  }
  for (const [group, tiles] of groups) {
    const [gx, gy] = group.split("/").map(Number);
    const inputs = [];
    for (const tile of tiles) {
      const record = images.get(`${view.id}/${tile.x}/${tile.y}`);
      assert.ok(record, `Missing capture ${view.id}/${tile.x}/${tile.y}`);
      const raw = await readFile(join(root, "raw", view.id, "8", `${tile.x}`, `${tile.y}.png`));
      assert.equal(digest(raw), record.sha256, "Raw capture changed after build");
      const base = view.id === "se" ? site : join(site, "views", view.id);
      const file = join(base, "tiles/8", `${tile.x}`, `${tile.y}.webp`);
      const webp = await readFile(file);
      const metadata = await sharp(webp).metadata();
      assert.equal(metadata.width, plan.tile_size);
      assert.equal(metadata.height, plan.tile_size);
      const stats = await sharp(webp).stats();
      assert.ok(stats.isOpaque, `Transparent finished tile ${file}`);
      const thumb = await sharp(webp).resize(96, 96).png().toBuffer();
      inputs.push({ input: thumb, left: (tile.x % 16) * 96, top: (tile.y % 16) * 96 + 32 });
      report.tiles.push({
        view: view.id,
        x: tile.x,
        y: tile.y,
        bytes: webp.length,
        sha256: digest(webp),
        entropy: stats.entropy,
        sharpness: stats.sharpness,
      });
      total++;
    }
    const heading = `<svg width="1536" height="32"><rect width="1536" height="32" fill="#191714"/><text x="12" y="22" fill="white" font-size="17">${view.id}: x ${gx * 16} to ${gx * 16 + 15}, y ${gy * 16} to ${gy * 16 + 15}. Beige cells are outside the planned coverage.</text></svg>`;
    inputs.push({ input: Buffer.from(heading), left: 0, top: 0 });
    const name = `${view.id}-${gx}-${gy}.jpg`;
    await sharp({ create: { width: 1536, height: 1568, channels: 3, background: "#ded4c4" } })
      .composite(inputs)
      .jpeg({ quality: 90 })
      .toFile(join(output, name));
    sheets.push({ view: view.id, x: gx, y: gy, file: name, checked: tiles.length });
    console.log(`Audited ${total} leaves; contact sheet ${name}`);
  }
}
report.checked = total;
report.finished_at = new Date().toISOString();
await writeFile(join(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
await writeFile(
  join(output, "index.html"),
  `<!doctype html><meta charset="utf-8"><title>Google Earth atlas audit</title><style>body{background:#191714;color:white;font:16px system-ui}img{width:min(100%,1536px);image-rendering:pixelated}figure{margin:20px}</style><h1>Atlas contact sheets</h1><p>Scope: ${report.scope}. Checked every one of ${total} planned leaves. These sheets assist visual review; they do not certify artwork quality.</p>${sheets.map((s) => `<figure><figcaption>${s.file}, ${s.checked} tiles</figcaption><img loading="lazy" src="${s.file}"></figure>`).join("\n")}`,
);
console.log(`All ${total} leaves audited. Review ${output}/index.html.`);
