#!/usr/bin/env bash
# Fetch the Kenney kits bevy_city uses (car-kit, city-kit-commercial, city-kit-roads,
# city-kit-suburban) into raw/kenney/, from bevyengine/bevy_asset_files (CC0 assets).
#
# A local checkout is used when available ($BEVY_ASSET_FILES, or ~/code/f/bevy_asset_files);
# otherwise a sparse clone of just the kenney/ directory is made. raw/ is gitignored.
#
#   scripts/fetch_kenney.sh
#   cargo run --release -p kenney_import
set -euo pipefail

cd "$(dirname "$0")/.."
dest="raw/kenney"
if [ -d "$dest/city-kit-roads" ]; then
  echo "$dest already present"
  exit 0
fi

local_checkout="${BEVY_ASSET_FILES:-$HOME/code/f/bevy_asset_files}"
if [ -d "$local_checkout/kenney/city-kit-roads" ]; then
  echo "copying from $local_checkout/kenney"
  mkdir -p raw
  cp -r "$local_checkout/kenney" "$dest"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
echo "sparse-cloning bevyengine/bevy_asset_files (kenney/) ..."
git clone --depth 1 --filter=blob:none --sparse https://github.com/bevyengine/bevy_asset_files.git "$tmp/repo"
git -C "$tmp/repo" sparse-checkout set kenney
mkdir -p raw
cp -r "$tmp/repo/kenney" "$dest"
echo "fetched $dest"
