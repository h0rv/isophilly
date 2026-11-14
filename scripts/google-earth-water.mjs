import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { captureEarth } from "./google-earth-capture.mjs";
import { createWaterMaskPlan } from "./google-earth-water-lib.mjs";

const { values } = parseArgs({
  options: {
    root: { type: "string", default: "data/google-earth" },
    cache: { type: "string" },
    limit: { type: "string" },
    "block-size": { type: "string", default: "4" },
  },
});
const root = resolve(values.root);
const selected = await createWaterMaskPlan(root);
console.log(
  `Water-mask workspace: ${selected.root}; ${selected.plan.views.reduce((n, view) => n + view.tiles.length, 0)} potentially affected leaves.`,
);
const result = await captureEarth({
  root: selected.root,
  cacheRoot: resolve(values.cache ?? join(root, "cache")),
  blockSize: Number(values["block-size"]),
  limit: values.limit === undefined ? Infinity : Number(values.limit),
});
console.log(JSON.stringify(result));
if (result.rejected) process.exitCode = 1;
