#!/usr/bin/env bash
# Bake a KitBash3D kit end to end: stage A (Blender, one glTF per prop, in parallel shards) then
# stage B (kitbash3d_import -> assets/<kit>). Extra args go to stage A (--only, --emissive-nits).
# Stage A resumes: props whose glTF exists are kept (rm -rf raw/<kit>/_gltf for a fresh export).
# Each shard runs in a memory-capped scope: the exporter holds a whole interior's meshes and 4K
# images at once, and four uncapped shards once ran the machine out of memory.
#
#   scripts/kitbash3d.sh atlantis raw/atlantis/Atlantis.blend 2
#   scripts/kitbash3d.sh enchanted_interiors raw/enchanted_interiors/kb3d_enchantedinteriors-native.blend 3
#   rsync -a --delete --exclude '*.png' --exclude '*.jpg' assets/<kit>/ /mnt/code/p/zero/assets/<kit>/
set -euo pipefail
cd "$(dirname "$0")/.."
kit=$1 blend=$2 shards=${3:-2}
shift $(( $# < 3 ? $# : 3 ))
out=raw/$kit/_gltf
mkdir -p "$out"
cap=$(( $(awk '/MemAvailable/ {print int($2 / 1048576)}' /proc/meminfo) * 3 / 4 / shards ))
for ((k = 0; k < shards; k++)); do
  systemd-run --user --scope -q -p MemoryMax="${cap}G" -p MemorySwapMax=0 \
    blender -b "$blend" --python scripts/kitbash3d_to_gltf.py -- \
    --out "$out/shard$k" --shard "$k/$shards" --skip-existing "$@" > "$out/shard$k.log" 2>&1 &
done
fail=0
for job in $(jobs -p); do wait "$job" || fail=1; done
grep -h '^  KB3D\|^  skip' "$out"/shard*.log | sort
[ "$fail" = 0 ] || { echo "a shard failed (killed at its ${cap}G cap?): rerun to resume, with fewer shards" >&2; exit 1; }
cargo run --release -p kitbash3d_import -- "$out" "assets/$kit" --colliders
