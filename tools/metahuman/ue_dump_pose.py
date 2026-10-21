# Run inside the Winston UE editor (ue_python): every bone's component-space pose of UE's own
# retarget at a few times, plus the ref pose, for compare_pose.py.
#   OUT=<json path>  ANIMS=<asset paths, comma-separated>  FRACTIONS=0,0.25,0.5,0.75
#   FPS=30 samples every frame instead (what ue_to_animclip.py wants).
import json
import os

import unreal

out_path = os.environ.get("OUT", "/tmp/ue_pose.json")
anims = os.environ.get("ANIMS", "/Game/MH_UAL1_StandardWalk_Loop").split(",")
fractions = [float(f) for f in os.environ.get("FRACTIONS", "0,0.25,0.5,0.75").split(",")]
opts = unreal.AnimPoseEvaluationOptions()
lib = unreal.AnimPoseExtensions
world = unreal.AnimPoseSpaces.WORLD


def pack(x):
    return [x.translation.x, x.translation.y, x.translation.z, x.rotation.x, x.rotation.y, x.rotation.z, x.rotation.w]


out = {}
for path in anims:
    anim = unreal.load_asset(path)
    length = anim.get_play_length()
    clip = {"length": length, "frames": {}}
    fps = float(os.environ.get("FPS", "0"))
    steps = [min(k / fps, length) for k in range(round(length * fps) + 1)] if fps else [length * f for f in fractions]
    for t in steps:
        pose = lib.get_anim_pose_at_time(anim, t, opts)
        names = [str(n) for n in lib.get_bone_names(pose)]
        if "ref" not in clip:
            clip["ref"] = {n: pack(lib.get_ref_bone_pose(pose, n, world)) for n in names}
        clip["frames"][f"{t:.4f}"] ={n: pack(lib.get_bone_pose(pose, n, world)) for n in names}
    out[path.rsplit("/", 1)[-1]] = clip
with open(out_path, "w") as fh:
    json.dump(out, fh)
print(out_path, {k: len(v["ref"]) for k, v in out.items()})
