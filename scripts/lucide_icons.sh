#!/usr/bin/env bash
# Lucide's icons (ISC, github.com/lucide-icons/lucide) as white PNGs for the game's UI.
#
#   git clone --depth 1 --filter=blob:none --sparse https://github.com/lucide-icons/lucide.git /mnt/code/f/lucide
#   git -C /mnt/code/f/lucide sparse-checkout set icons
#   cargo install resvg --locked
#   scripts/lucide_icons.sh /mnt/code/f/lucide /home/slyedoc/code/p/zero/assets/icons/lucide [SIZE]
set -euo pipefail

src="${1:?lucide checkout}"
out="${2:?output dir}"
size="${3:-128}"
mkdir -p "$out"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
count=0
for svg in "$src"/icons/*.svg; do
  name="$(basename "$svg" .svg)"
  sed 's/currentColor/#ffffff/g' "$svg" > "$tmp/$name.svg"
  resvg -w "$size" -h "$size" "$tmp/$name.svg" "$out/$name.png"
  count=$((count + 1))
done
cp "$src/LICENSE" "$out/LICENSE.txt"
echo "$count icons at ${size}px into $out"
