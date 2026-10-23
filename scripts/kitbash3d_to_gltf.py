"""A KitBash3D kit .blend -> one glTF per prop, stage A of `kitbash3d_import`.

A KitBash3D native .blend files its kit as top-level objects: an `_grp` empty per piece
(Enchanted Interiors, Lunar Base) or one joined mesh per piece (Atlantis), laid out in a showcase
grid. Every top-level object (minus lights and cameras) is one prop. Each exports on its own,
moved so its footprint centre at floor height is the origin, as `<Prop>.gltf` + `.bin` with the
textures shared in `textures/`. `<Prop>.json` records where the kit's own layout put that origin
(Y-up metres), which is how the importer rebuilds the showcase as one scene of instances.

One glTF per prop, not one per kit: Atlantis is 101 M triangles, past what a single GLB (4 GB)
holds, and per-prop files let shards run as parallel Blender processes.

Material repairs, in memory only (the .blend is never saved; the kits are hard-linked):
  * emission strength is scene-relative in the kits (1 = white in a Blender render); it is
    multiplied by --emissive-nits so the exporter writes KHR_materials_emissive_strength in nits
  * an opacity map wired straight into Alpha exports as BLEND, which the ray tracer treats as
    opaque; a Round node in between makes the exporter write MASK (cutoff 0.5)

    blender -b raw/atlantis/Atlantis.blend --python scripts/kitbash3d_to_gltf.py -- \
        --out raw/atlantis/_gltf [--only 'KB3D_ATL_Prop*'] [--shard 0/3]
"""

import argparse
import fnmatch
import glob
import json
import os
import sys

import bpy
from mathutils import Matrix, Vector


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser(prog="kitbash3d_to_gltf")
    p.add_argument("--out", required=True, help="output dir: <Prop>.gltf/.bin/.json + textures/")
    p.add_argument("--only", nargs="*", default=[], help="prop name globs (after `_grp` is dropped)")
    p.add_argument("--shard", default="0/1", help="k/n: export every n-th prop starting at k")
    p.add_argument("--emissive-nits", type=float, default=1000.0,
                   help="nits per unit of Blender emission strength")
    p.add_argument("--skip-existing", action="store_true", help="keep props whose .gltf exists here or in a sibling shard dir")
    return p.parse_args(argv)


def prop_name(ob):
    name = ob.name
    return name[:-4] if name.endswith("_grp") else name


def subtree(ob):
    out, stack = [], [ob]
    while stack:
        o = stack.pop()
        out.append(o)
        stack.extend(o.children)
    return out


def principled(mat):
    if not mat or not mat.node_tree:
        return None
    return next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)


def repair_materials(nits):
    emitters, cutouts = [], []
    for mat in bpy.data.materials:
        p = principled(mat)
        if p is None:
            continue
        color = p.inputs["Emission Color"]
        strength = p.inputs["Emission Strength"]
        lit = color.is_linked or any(c > 0.0 for c in color.default_value[:3])
        if lit and not strength.is_linked and strength.default_value > 0.0:
            strength.default_value *= nits
            emitters.append(f"{mat.name}={strength.default_value:g}")
        alpha = p.inputs["Alpha"]
        if alpha.is_linked:
            link = alpha.links[0]
            if link.from_node.type != "MATH":
                tree = mat.node_tree
                rnd = tree.nodes.new("ShaderNodeMath")
                rnd.operation = "ROUND"
                tree.links.new(link.from_socket, rnd.inputs[0])
                tree.links.new(rnd.outputs[0], alpha)
                cutouts.append(mat.name)
    print(f"  emissive nits: {', '.join(emitters) or 'none'}")
    print(f"  alpha -> mask: {', '.join(cutouts) or 'none'}")


def world_bounds(objs):
    lo = Vector((1e18, 1e18, 1e18))
    hi = Vector((-1e18, -1e18, -1e18))
    found = False
    for ob in objs:
        if ob.type != "MESH":
            continue
        for corner in ob.bound_box:
            p = ob.matrix_world @ Vector(corner)
            lo = Vector(map(min, lo, p))
            hi = Vector(map(max, hi, p))
            found = True
    return (lo, hi) if found else None


def triangles(objs):
    n = 0
    for ob in objs:
        if ob.type == "MESH":
            ob.data.calc_loop_triangles()
            n += len(ob.data.loop_triangles)
    return n


def export(root, out_dir):
    objs = subtree(root)
    bounds = world_bounds(objs)
    if bounds is None:
        print(f"  skip {root.name}: no meshes")
        return None
    lo, hi = bounds
    base = Vector(((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z))
    original = root.matrix_world.copy()
    root.matrix_world = Matrix.Translation(-base) @ original
    bpy.context.view_layer.update()

    bpy.ops.object.select_all(action="DESELECT")
    for ob in objs:
        ob.select_set(True)
    name = prop_name(root)
    path = os.path.join(out_dir, f"{name}.gltf")
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLTF_SEPARATE",
        export_texture_dir="textures",
        export_image_format="AUTO",
        use_selection=True,
        export_apply=True,
        export_materials="EXPORT",
        export_extras=False,
        export_cameras=False,
        export_lights=False,
        export_yup=True,
    )
    root.matrix_world = original
    bpy.context.view_layer.update()
    for img in bpy.data.images:
        if img.has_data:
            img.buffers_free()

    size = hi - lo
    meta = {
        "source": root.name,
        "place": [base.x, base.z, -base.y],
        "size": [size.x, size.z, size.y],
        "triangles": triangles(objs),
    }
    with open(os.path.join(out_dir, f"{name}.json"), "w") as f:
        json.dump(meta, f)
    print(f"  {name}: {meta['triangles']} tris, {size.x:.2f} x {size.z:.2f} x {size.y:.2f} m")
    return name


def main():
    args = parse_args()
    os.makedirs(args.out, exist_ok=True)
    k, n = (int(v) for v in args.shard.split("/"))

    scene = bpy.context.scene
    for other in [s for s in bpy.data.scenes if s != scene]:
        bpy.data.scenes.remove(other)
    for ob in bpy.data.objects:
        ob.hide_set(False)
        ob.hide_viewport = False
        ob.hide_select = False
    roots = sorted(
        (ob for ob in scene.objects if ob.parent is None and ob.type not in {"LIGHT", "CAMERA"}),
        key=lambda ob: ob.name,
    )
    if args.only:
        roots = [r for r in roots if any(fnmatch.fnmatch(prop_name(r), g) for g in args.only)]
    roots = roots[k::n]
    print(f"--- {scene.name}: {len(roots)} props (shard {k}/{n}) -> {args.out}")

    repair_materials(args.emissive_nits)
    for root in roots:
        name = prop_name(root)
        if args.skip_existing and (glob.glob(os.path.join(args.out, f"{name}.gltf"))
                                   or glob.glob(os.path.join(args.out, "..", "*", f"{name}.gltf"))):
            continue
        export(root, args.out)
    print("done")


main()
