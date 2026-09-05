#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
base_url="${1:-https://isophilly.horv.co}"
base_url="${base_url%/}"
scene="$project_root/data/tiles/current.json"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

tile_version="$(node -e 'const fs=require("node:fs"); const scene=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); process.stdout.write(scene.tile_version)' "$scene")"
local_tile="$project_root/data/tiles/$tile_version/8/83/76.webp"
test -f "$local_tile" || {
  echo "missing fixed local smoke tile: $local_tile" >&2
  exit 1
}

curl --fail --silent --show-error --location --max-time 30 \
  "$base_url/meta" --output "$tmp_dir/meta.json"
node - "$scene" "$tmp_dir/meta.json" <<'NODE'
const fs = require("node:fs");
const local = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const live = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
for (const key of ["schema_version", "world_sha256", "land_cover_sha256", "tile_version"]) {
  if (live[key] !== local[key]) {
    throw new Error(`live ${key} mismatch: expected ${local[key]}, got ${live[key]}`);
  }
}
for (const key of ["buildings", "building_meshes", "street_trees"]) {
  if (live.counts?.[key] !== local.counts?.[key]) {
    throw new Error(`live count ${key} mismatch: expected ${local.counts?.[key]}, got ${live.counts?.[key]}`);
  }
}
console.log(`live manifest matches ${local.tile_version}`);
NODE

curl --fail --silent --show-error --location --max-time 30 \
  "$base_url/tiles/8/83/76.webp" --output "$tmp_dir/tile.webp"
local_hash="$(sha256sum "$local_tile" | cut -d' ' -f1)"
live_hash="$(sha256sum "$tmp_dir/tile.webp" | cut -d' ' -f1)"
test "$live_hash" = "$local_hash" || {
  echo "live fixed tile mismatch: expected $local_hash, got $live_hash" >&2
  exit 1
}
echo "live fixed tile matches $live_hash"
