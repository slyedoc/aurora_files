#!/usr/bin/env bash
# Winston's MetaHuman DCC export -> zero's characters/winston. Run from anywhere.
#   tools/metahuman/bake_winston.sh [EXPORT_DIR]   (default /mnt/code/p/winston/Export/Winston)
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
EXPORT=${1:-/mnt/code/p/winston/Export/Winston}
RAW=$ROOT/raw/winston
MHDNA=$HERE/build/mhdna
mkdir -p "$RAW"
[ -x "$MHDNA" ] || { cmake -S "$HERE" -B "$HERE/build" -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=clang++ && cmake --build "$HERE/build" -j; }
cd "$RAW"
python3 -I - "$HERE/arkit_to_metahuman.json" > arkit_poses.txt <<'PY'
import json, sys
for p, ctrls in sorted(json.load(open(sys.argv[1]))["poses"].items()):
    for c, v in ctrls.items():
        print(p, c, v)
PY
: > empty_poses.txt
"$MHDNA" dump "$EXPORT/head.dna" head
"$MHDNA" dump "$EXPORT/body.dna" body
"$MHDNA" eval "$EXPORT/head.dna" arkit_poses.txt head_eval
"$MHDNA" eval "$EXPORT/body.dna" empty_poses.txt body_eval
python3 -I "$HERE/bake_arkit.py" head head_eval head_arkit.npz
# Clothes: raw/winston/outfit.fbx + outfit_tex/ from ue_export_outfit.py (run in the editor).
blender -b --python "$HERE/assemble_glb.py" -- "$RAW" "$EXPORT/Maps" "$RAW/winston.glb" 2>&1 | grep -E "wrote|Error|Traceback"
cd "$ROOT"
rm -rf assets/characters/winston
cargo run --release -q -p prop_import -- raw/winston/winston.glb assets/characters/winston \
    --hierarchy --scene-name winston --asset-prefix characters/winston --yaw 180 2>&1 | grep -v "^warning: patch\|^help:\|^ *|\|version\.\|lock file\|enabled\.$"
rsync -a --delete assets/characters/winston/ /mnt/code/p/zero/assets/characters/winston/
# UAL1's clips, retargeted by UE's own retargeter onto this winston.glb: run
# tools/metahuman/ue_retarget_ual.py in the Winston editor first (writes raw/winston/ue_ual.json).
# animlib_import --skeleton-only writes the rig's .skn.ron (the source is only parsed).
cargo run --release -q -p animlib_import -- raw/ual/Unreal-Godot/UAL1_Standard.glb \
    /mnt/code/p/zero/assets/characters/winston/winston.bsn /mnt/code/p/zero/assets/anim/winston \
    --skeleton-only 2>&1 | grep -E "^skeleton"
cp /mnt/code/p/zero/assets/characters/winston/winston.skn.ron assets/characters/winston/
UE_UAL=$RAW/ue_ual.json
[ "$UE_UAL" -nt "$RAW/winston.glb" ] || echo "warning: $UE_UAL is older than winston.glb; rerun ue_retarget_ual.py" >&2
python3 -I "$HERE/ue_to_animclip.py" "$UE_UAL" /mnt/code/p/zero/assets/characters/winston/winston.skn.ron \
    /mnt/code/p/zero/assets/anim/winston
