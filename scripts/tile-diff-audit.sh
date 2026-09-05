#!/usr/bin/env bash
set -euo pipefail

if [[ ${1:-} == -- ]]; then
  shift
fi
if [[ $# -lt 2 ]]; then
  echo "usage: $0 OLD_TILE_SMOKE_DIR NEW_TILE_SMOKE_DIR [IMAGE.png ...]" >&2
  exit 2
fi
if ! command -v magick >/dev/null 2>&1; then
  echo "tile diff audit requires ImageMagick's magick command" >&2
  exit 2
fi

old_dir=$1
new_dir=$2
shift 2
images=("$@")
if [[ ${#images[@]} -eq 0 ]]; then
  images=(rittenhouse.png point-breeze.png italian-market.png east-passyunk.png)
fi

printf 'image\tchanged_pixels\ttotal_pixels\tcoverage_percent\n'
for image in "${images[@]}"; do
  old_path="$old_dir/$image"
  new_path="$new_dir/$image"
  if [[ ! -f $old_path || ! -f $new_path ]]; then
    echo "missing matched image: $image" >&2
    exit 1
  fi
  old_size=$(magick identify -format '%w %h' "$old_path")
  new_size=$(magick identify -format '%w %h' "$new_path")
  if [[ $old_size != "$new_size" ]]; then
    echo "image dimensions differ for $image: $old_size versus $new_size" >&2
    exit 1
  fi
  read -r width height <<<"$new_size"
  total=$((width * height))
  changed=$(
    magick "$old_path" "$new_path" -compose difference -composite -threshold 0 \
      -format '%[fx:mean*w*h]' info:
  )
  changed=$(awk -v value="$changed" 'BEGIN { printf "%.0f", value }')
  coverage=$(awk -v changed="$changed" -v total="$total" \
    'BEGIN { printf "%.6f", 100 * changed / total }')
  printf '%s\t%s\t%s\t%s\n' "$image" "$changed" "$total" "$coverage"
done
