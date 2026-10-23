#!/usr/bin/env bash
# The MakeHuman library zero's make_human crate generates people from (its build.rs scans the
# copy for the asset enums, the game loads it as make_human/...).
#
#   scripts/make_human_library.sh [SRC] [DEST]
#     SRC   raw/make_human (default)
#     DEST  ../zero/assets/make_human (default)
#
# Adding an asset: drop its folder into raw/make_human/<kind>/, rerun this, rebuild zero.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
src="${1:-$here/raw/make_human}"
dest="${2:-$here/../zero/assets/make_human}"
kinds=(3dobjs mesh_metadata targets skins hair clothes eyes eyebrows eyelashes teeth tongue proxymeshes rigs poses)

mkdir -p "$dest"
for kind in "${kinds[@]}"; do
  rsync -a --delete --exclude '_images/' --exclude '*.meta' "$src/$kind/" "$dest/$kind/"
done
echo "$(du -sh "$dest" | cut -f1) of MakeHuman library in $dest"
