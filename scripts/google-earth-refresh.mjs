import { createHash } from "node:crypto";
import { constants } from "node:fs";
import {
  copyFile,
  cp,
  lstat,
  mkdtemp,
  readdir,
  readFile,
  realpath,
  rename,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const STATIC = [
  "app.js",
  "city-overlay.js",
  "map-presentation.js",
  "map-search.js",
  "map-projection.js",
  "presentation-config.js",
  "index.html",
  "neighborhoods.json",
  "_headers",
];
const REPO = fileURLToPath(new URL("../", import.meta.url));
function requireValid(condition, message) {
  if (!condition) throw new Error(message);
}
async function regular(file) {
  requireValid((await lstat(file)).isFile(), `Expected a regular file: ${file}`);
}
async function load(file) {
  await regular(file);
  return JSON.parse(await readFile(file, "utf8"));
}
const save = (file, value) => writeFile(file, `${JSON.stringify(value)}\n`);
function validateMeta(meta) {
  requireValid(
    meta?.schema_version === 2 &&
      meta.imagery_source === "google-earth-public" &&
      Array.isArray(meta.landmarks),
    "Expected schema 2 Google Earth metadata with landmarks",
  );
}
async function updateMeta(file, homeZoom) {
  const meta = await load(file);
  validateMeta(meta);
  await save(file, {
    ...meta,
    home_zoom: homeZoom,
    landmarks: meta.landmarks.filter((landmark) => landmark.name !== "Rocky"),
  });
}

/** Refresh only the viewer of a completed generated atlas, keeping every tile byte. */
export async function refreshGoogleEarth({ root, output = join(root, "site"), homeZoom = 7 }) {
  requireValid(typeof root === "string" && root.length > 0, "Expected a workspace root");
  requireValid(typeof output === "string" && output.length > 0, "Expected an output path");
  requireValid(
    Number.isInteger(homeZoom) && homeZoom >= 0,
    "Expected a nonnegative integer homeZoom",
  );
  root = await realpath(root);
  output = resolve(output);
  requireValid(
    (await realpath(dirname(output))) === root &&
      !["raw", "cache"].includes(basename(output)) &&
      /^[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(basename(output)),
    "Output must be a generated symlink directly inside the workspace, not raw captures",
  );
  output = join(root, basename(output));
  requireValid(
    (await lstat(output)).isSymbolicLink(),
    "Output must be an existing generated symlink",
  );
  const source = await realpath(output); // Pin this generation, not the mutable output link.
  const prefix = `.${basename(output)}-`;
  requireValid(
    dirname(source) === root &&
      ["building-", "viewer-"].some((kind) => basename(source).startsWith(`${prefix}${kind}`)) &&
      (await lstat(source)).isDirectory(),
    "Output must resolve to a generated directory inside the workspace",
  );
  const report = await load(join(source, "build.json"));
  requireValid(report?.complete === true, "Only complete generated atlases can be refreshed");
  validateMeta(await load(join(source, "meta")));
  const stage = await mkdtemp(join(root, `${prefix}viewer-`));
  const pointer = `${stage}.link`;
  try {
    await cp(source, stage, {
      recursive: true,
      mode: constants.COPYFILE_FICLONE, // Reflink where supported; copy fallback elsewhere.
      filter: async (file) => {
        const stat = await lstat(file);
        requireValid(stat.isFile() || stat.isDirectory(), `Unsafe atlas entry: ${file}`);
        return true;
      },
    });
    const revision = createHash("sha256");
    for (const name of STATIC) {
      const file = join(REPO, "static", name);
      await regular(file);
      revision.update(name).update(await readFile(file));
      await copyFile(file, join(stage, name));
    }
    await updateMeta(join(stage, "meta"), homeZoom);
    let views = [];
    try {
      views = await readdir(join(stage, "views"), { withFileTypes: true });
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
    }
    for (const view of views) {
      if (!view.isDirectory()) continue;
      const file = join(stage, "views", view.name, "meta");
      try {
        await updateMeta(file, homeZoom);
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
    }
    const updated = { ...report, viewer_revision: revision.digest("hex").slice(0, 16) };
    await save(join(stage, "build.json"), updated);
    await symlink(basename(stage), pointer, "dir");
    requireValid(
      (await lstat(output)).isSymbolicLink() && (await realpath(output)) === source,
      "Output changed during refresh; refusing to replace it",
    );
    await rename(pointer, output);
    return { generation: stage, report: updated };
  } catch (error) {
    await rm(pointer, { force: true });
    await rm(stage, { recursive: true, force: true });
    throw error;
  }
}

async function main() {
  const { values } = parseArgs({
    options: {
      root: { type: "string", default: "data/google-earth" },
      output: { type: "string" },
    },
  });
  console.log(JSON.stringify(await refreshGoogleEarth(values)));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
