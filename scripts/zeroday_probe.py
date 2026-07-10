"""Probe a Zero-Day FBX for ANIMATION content (what, if anything, is keyframed).

    blender -b --python scripts/zeroday_probe.py -- <in.fbx>

Prints: scene fps + frame range, cameras (+ whether animated), armatures (skinned),
and a per-object tally of which have animation_data/actions and on which TRS channels.
Loads the whole FBX (slow, ~minutes for the 300 MB sets) — animation-only import isn't
a thing in the FBX importer.
"""

import bpy
import sys
from collections import Counter


def argv_after_ddash():
    return sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []


def action_fcurves(act):
    """Blender 4.4+/5.x layered-action fcurves (legacy `act.fcurves` was removed)."""
    if hasattr(act, "fcurves") and len(getattr(act, "fcurves")):
        yield from act.fcurves
        return
    for layer in getattr(act, "layers", []):
        for strip in layer.strips:
            for cbag in getattr(strip, "channelbags", []):
                yield from cbag.fcurves


def channels_of(obj):
    ad = obj.animation_data
    if not ad or not ad.action:
        return set()
    return {fc.data_path.split('.')[-1] for fc in action_fcurves(ad.action)}


def main():
    args = argv_after_ddash()
    if not args:
        print("usage: ... -- <in.fbx>"); sys.exit(2)
    in_fbx = args[0]

    bpy.ops.wm.read_factory_settings(use_empty=True)
    print(f"probe: importing {in_fbx} ...", flush=True)
    bpy.ops.import_scene.fbx(filepath=in_fbx)

    sc = bpy.context.scene
    print(f"\n=== SCENE ===")
    print(f"fps={sc.render.fps}/{sc.render.fps_base}  frames={sc.frame_start}..{sc.frame_end}")
    print(f"objects={len(bpy.data.objects)}  meshes={len(bpy.data.meshes)}  "
          f"materials={len(bpy.data.materials)}  actions={len(bpy.data.actions)}  "
          f"cameras={len(bpy.data.cameras)}  armatures={len(bpy.data.armatures)}")

    # Cameras
    print(f"\n=== CAMERAS ===")
    for o in bpy.data.objects:
        if o.type == 'CAMERA':
            ch = channels_of(o)
            print(f"  {o.name!r}  animated_channels={sorted(ch) or 'STATIC'}")

    # Armatures (skinned)
    print(f"\n=== ARMATURES (skinned) ===")
    arm = [o for o in bpy.data.objects if o.type == 'ARMATURE']
    if not arm:
        print("  none")
    for o in arm:
        bones = len(o.data.bones)
        ch = channels_of(o)
        print(f"  {o.name!r}  bones={bones}  animated={sorted(ch) or 'STATIC'}")

    # Object-level TRS animation tally
    print(f"\n=== OBJECT TRS ANIMATION ===")
    animated = []
    chan_tally = Counter()
    for o in bpy.data.objects:
        ch = channels_of(o)
        if ch:
            animated.append((o.name, o.type, sorted(ch)))
            for c in ch:
                chan_tally[c] += 1
    print(f"  {len(animated)} objects carry an action")
    for c, n in chan_tally.most_common():
        print(f"    channel {c!r}: {n} objects")
    print("  first 30 animated objects:")
    for name, typ, ch in animated[:30]:
        print(f"    [{typ}] {name!r}  {ch}")

    # NLA / multiple actions hint
    print(f"\n=== ACTIONS ===")
    for a in list(bpy.data.actions)[:20]:
        fr = a.frame_range
        print(f"  {a.name!r}  frames={fr[0]:.0f}..{fr[1]:.0f}  fcurves={len(a.fcurves)}")

    print("\nprobe: done", flush=True)


main()
