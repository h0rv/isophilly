#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scene="$project_root/data/tiles/current.json"
tile_version="$(node -e 'const fs=require("node:fs"); const scene=JSON.parse(fs.readFileSync(process.argv[1], "utf8")); process.stdout.write(scene.tile_version)' "$scene")"
tile_root="$project_root/data/tiles/$tile_version"
output_root="${1:-$project_root/artifacts/visual/tile-smoke-$tile_version}"

command -v magick >/dev/null || {
  echo "ImageMagick is required (missing: magick)" >&2
  exit 1
}
test -f "$tile_root/.complete" || {
  echo "active tile pyramid is incomplete: $tile_root" >&2
  exit 1
}
mkdir -p "$output_root"

render_mosaic() {
  local name="$1"
  local start_x="$2"
  local start_y="$3"
  local columns="$4"
  local rows="$5"
  local tiles=()
  local row column x y tile
  for ((row = 0; row < rows; row += 1)); do
    for ((column = 0; column < columns; column += 1)); do
      x=$((start_x + column))
      y=$((start_y + row))
      tile="$tile_root/8/$x/$y.webp"
      test -f "$tile" || {
        echo "missing smoke tile: $tile" >&2
        exit 1
      }
      tiles+=("$tile")
    done
  done
  magick montage "${tiles[@]}" -tile "${columns}x${rows}" -geometry +0+0 \
    "$output_root/$name.png"
  magick identify -format '%f %wx%h\n' "$output_root/$name.png"
  sha256sum "$output_root/$name.png"
}

render_mosaic rittenhouse 79 71 6 5
render_mosaic point-breeze 72 75 6 5
render_mosaic italian-market 83 76 6 5
render_mosaic east-passyunk 76 78 6 5
