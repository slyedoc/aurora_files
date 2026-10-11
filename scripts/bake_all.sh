#!/usr/bin/env bash
# Re-bake every asset aurora_files produces, from scratch: each importer cleans its own output
# (prop_import its meshes/ and textures/) and bakes everything. Run from the repo root.
#   scripts/bake_all.sh            all of it
#   scripts/bake_all.sh hex wow    just those steps
# Then mirror what zero uses (zero's docs/assets.md lists each dir).
set -euo pipefail
cd "$(dirname "$0")/.."

ZERO=../zero
cargo build --release --workspace
bin=target/release

ONLY="${*:-}"
run() { local name=$1; shift; if [ -z "$ONLY" ] || [[ " $ONLY " == *" $name "* ]]; then echo "=== $name"; "$@"; fi; }

run wow $bin/wow_import
run lunarbase $bin/lunarbase_import raw/lunarbase/kb3d_lunarbase.glb assets/lunarbase
run speedtree $bin/speedtree_import raw/SpeedTree_v2/_glb assets/speedtree --clump 7
run kenney $bin/kenney_import
run san_miguel $bin/san_miguel_import raw/San_Miguel/san-miguel.obj assets/san_miguel san_miguel
run bistro $bin/bistro_import raw/Bistro/Bistro.glb assets/bistro bistro
run zeroday $bin/zeroday_import raw/ZeroDay_v1/_glb/measure_one.glb assets/zeroday MeasureOne
run namaqualand $bin/namaqualand_boulder_import \
  raw/namaqualand_boulder_02_8k/namaqualand_boulder_02_8k.gltf assets/namaqualand_boulder
run g1 $bin/g1_import "$ZERO/assets" ai/motionbricks/g1

run hex $bin/prop_import raw/hex/hex.glb assets/hex \
  --hierarchy --scene-name hex --asset-prefix hex --colliders
run playground $bin/prop_import raw/playground/playground.glb assets/playground \
  --scene-name playground --colliders
for p in console core door gangway pad platform; do
  run sim $bin/prop_import raw/sim/$p.glb assets/sim/$p \
    --hierarchy --scene-name $p --asset-prefix sim/$p --colliders
done
run props $bin/prop_import /mnt/code/p/assets/fantacy_city/04_Props/04_Prompts.glb assets/props \
  --scene-name fantasy_props --per-group --group-depth 2 --colliders \
  --fixups kits/fantasy_city.fixups.ron --textures /mnt/code/p/assets/fantacy_city/2d
run hoverboard $bin/prop_import raw/hoverboard/hoverboard.glb assets/hoverboard --scene-name hoverboard
for s in phalanx:PhalanxCorvette praetor_gunship:PraetorGunship \
         rhodes_battlecruiser:RhodesBattlecruiser templar_frigate:TemplarFrigate; do
  dir=${s%%:*}; name=${s##*:}
  run ships $bin/prop_import raw/$dir/$dir.glb assets/ships/$dir --scene-name $name \
    --asset-prefix ships/$dir
done
run mech $bin/prop_import raw/mech/mech.glb assets/mech \
  --hierarchy --scene-name drv4_drover --anim raw/mech/mech_anim.glb
run fox $bin/prop_import raw/fox/Fox.glb assets/fox --hierarchy --scale 0.01
run ual $bin/prop_import raw/ual/Unreal-Godot/UAL1_Standard.glb assets/ual \
  --scene-name Mannequin --hierarchy
run ual_f $bin/prop_import raw/ual2/Mannequin_F.glb assets/ual_f --scene-name Mannequin_F --hierarchy
run claire $bin/prop_import raw/claire/claire.glb assets/characters/claire \
  --scene-name claire --asset-prefix characters/claire
run human_male $bin/prop_import raw/wow_human/humanmale.glb assets/characters/human_male \
  --hierarchy --scene-name human_male --asset-prefix characters/human_male --yaw 90
run human_male python3 scripts/foot_contacts.py raw/wow_human/humanmale.glb \
  assets/characters/human_male --feet bone_FootL=footstep_l,bone_FootR=footstep_r \
  --clips "Walk,Run,Walkbackwards,Sprint,JumpEnd,JumpLandRun,StealthWalk"

echo "baked."
