# Run inside the Winston UE editor (ue_python, exec this file): UAL1's clips retargeted onto
# Winston by UE's own retargeter, dumped for ue_to_animclip.py. The target is the imported
# winston.glb (not the MetaHuman template body), so UE and aurora share one bind exactly.
#   RETARGET_UAL=<UAL1_Standard.glb>  RETARGET_GLB=<winston.glb>  RETARGET_OUT=<json>  RETARGET_CLIPS=Walk_Loop,...
import os

import unreal

ual_glb = os.environ.get("RETARGET_UAL", "/mnt/code/p/aurora_files/raw/ual/Unreal-Godot/UAL1_Standard.glb")
winston_glb = os.environ.get("RETARGET_GLB", "/mnt/code/p/aurora_files/raw/winston/winston.glb")
out = os.environ.get("RETARGET_OUT", "/mnt/code/p/aurora_files/raw/winston/ue_ual.json")
clips = os.environ.get("RETARGET_CLIPS", "Walk_Loop,Idle_Loop,Jog_Fwd_Loop,Idle_Talking_Loop,Interact").split(",")
eal = unreal.EditorAssetLibrary
tools = unreal.AssetToolsHelpers.get_asset_tools()
S, T = unreal.RetargetSourceOrTarget.SOURCE, unreal.RetargetSourceOrTarget.TARGET


def import_glb(path, dest):
    task = unreal.AssetImportTask()
    task.filename, task.destination_path = path, dest
    task.automated = task.save = task.replace_existing = True
    tools.import_asset_tasks([task])


def fresh(path, name, folder, cls, factory):
    if eal.does_asset_exist(path):
        eal.delete_asset(path)
    return tools.create_asset(name, folder, cls, factory)


import_glb(ual_glb, "/Game/UAL")
import_glb(winston_glb, "/Game/WinstonGlb")
src_mesh = unreal.load_asset("/Game/UAL/UAL1_Standard/SkeletalMeshes/UAL1_Standard")
dst_mesh = unreal.load_asset("/Game/WinstonGlb/winston/SkeletalMeshes/winston")

src_rig = fresh("/Game/UAL/IK_UAL", "IK_UAL", "/Game/UAL", unreal.IKRigDefinition, unreal.IKRigDefinitionFactory())
rc = unreal.IKRigController.get_controller(src_rig)
rc.set_skeletal_mesh(src_mesh)
rc.apply_auto_generated_retarget_definition()
eal.save_asset("/Game/UAL/IK_UAL")

# Epic's MetaHuman body rig, re-pointed at Winston (same bone names).
if eal.does_asset_exist("/Game/WinstonGlb/IK_Winston"):
    eal.delete_asset("/Game/WinstonGlb/IK_Winston")
dst_rig = eal.duplicate_asset("/MetaHumanCharacter/Animation/Retargeting/IK_MH_IKRig", "/Game/WinstonGlb/IK_Winston")
unreal.IKRigController.get_controller(dst_rig).set_skeletal_mesh(dst_mesh)
eal.save_asset("/Game/WinstonGlb/IK_Winston")

rtg = fresh("/Game/WinstonGlb/RTG_UAL_Winston", "RTG_UAL_Winston", "/Game/WinstonGlb", unreal.IKRetargeter,
            unreal.IKRetargetFactory())
c = unreal.IKRetargeterController.get_controller(rtg)
c.set_ik_rig(S, src_rig)
c.set_ik_rig(T, dst_rig)
c.set_preview_mesh(S, src_mesh)
c.set_preview_mesh(T, dst_mesh)
if c.get_num_retarget_ops() == 0:
    c.add_default_ops()
c.assign_ik_rig_to_all_ops(S, src_rig)
c.assign_ik_rig_to_all_ops(T, dst_rig)
c.auto_map_chains(unreal.AutoMapChainType.FUZZY, True)
# FK only: in-place clips, and the runtime plants the feet itself.
for op in ["Retarget IK Goals", "Run IK Rig"]:
    c.set_retarget_op_enabled(c.get_index_of_op_by_name(op), False)
c.auto_align_all_bones(T, unreal.RetargetAutoAlignMethod.CHAIN_TO_CHAIN)
eal.save_asset("/Game/WinstonGlb/RTG_UAL_Winston")

for n in clips:
    if eal.does_asset_exist(f"/Game/W_{n}"):
        eal.delete_asset(f"/Game/W_{n}")
sources = [eal.find_asset_data(f"/Game/UAL/UAL1_Standard/SkeletalMeshes/UAL1_Standard{n}") for n in clips]
done = unreal.IKRetargetBatchOperation.duplicate_and_retarget(
    sources, src_mesh, dst_mesh, rtg, "UAL1_Standard", "", "W_", "", False, False)
print("retargeted", [str(a.package_name) for a in done])

os.environ.update(OUT=out, FPS="30", ANIMS=",".join(str(a.package_name) for a in done))
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)) if "__file__" in globals()
                       else "/mnt/code/p/aurora_files/tools/metahuman", "ue_dump_pose.py")) as fh:
    exec(fh.read())
