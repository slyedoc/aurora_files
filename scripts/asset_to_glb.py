"""Any asset -> a normalized glb the Rust importers can bake. Stage A, shared.

Handles the two things every incoming asset needs and no importer should reinvent:

  NORMALIZE  author units -> metres, chosen up-axis, origin where you want it, and a printed
             bounding box so a scale mistake is caught by reading a number instead of by noticing
             the mech is the size of a mug next to a character.

  FLATTEN    (optional) collapse material graphs to the scalar factors `StandardSolariMaterial`
             carries, so the asset renders with ZERO texture reads. Procedural graphs are averaged
             by BAKING them — no assumptions about what the node tree does.

Flatten decides per material:
  * socket unlinked            -> take its default_value (most third-party assets)
  * socket driven by an image  -> leave the material alone; the textured import path handles it
  * socket driven by a graph   -> bake it small and average the texels (the general case)

A material may also carry `flat_base_color` / `flat_metallic` / `flat_roughness` / `flat_emissive`
custom properties — a generator's DECLARED intent. Those are advisory; the bake wins. Measuring
beat declaring in practice: the hoverboard's analytic metallic claimed 0.04 where the bake found
~0.6, because "a flat face reads 0.5 pointiness" stops holding on low-poly beveled geometry where
most vertices sit on a bevel. `--verify-flat` prints both side by side.

Emissive is converted to NITS on the way out. Blender-authored radiance is scene-relative; Solari
is physical and exposes at EV100 9.7, where ~1000 nits is white — so factor-convention values
render black. See `solari_bsn::lint`.

    blender -b [file.blend] --python scripts/asset_to_glb.py -- --out raw/x/x.glb [options]
    blender -b --python scripts/asset_to_glb.py -- --input mech.fbx --out raw/mech/mech.glb \
        --flatten --scale 0.01 --origin base
"""

import bpy
import sys
import os
import argparse
from mathutils import Vector

CHANNELS = ("Base Color", "Metallic", "Roughness")


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    p = argparse.ArgumentParser(prog="asset_to_glb")
    p.add_argument("--out", required=True, help="output .glb")
    p.add_argument("--input", help="source to import (.fbx/.glb/.gltf/.obj/.dae); omit to use the open .blend")
    p.add_argument("--collection", help="export only this collection (others are deleted)")
    p.add_argument("--scale", type=float, default=1.0, help="author units -> metres (FBX cm = 0.01)")
    p.add_argument("--origin", choices=("keep", "center", "base"), default="keep",
                   help="'base' puts the origin at the footprint centre, floor height")
    p.add_argument("--up", choices=("z", "y"), default="z", help="source up-axis (y rotates +90 X)")
    p.add_argument("--flatten", action="store_true", help="collapse materials to texture-free scalars")
    p.add_argument("--emissive-scale", type=float, default=1.0,
                   help="multiply emissive into nits (scene-relative -> physical)")
    p.add_argument("--bake-res", type=int, default=32, help="resolution for averaging procedural graphs")
    p.add_argument("--verify-flat", action="store_true",
                   help="bake even when flat_* props exist, and print both for comparison")
    p.add_argument("--keep-lights", action="store_true", help="export lights/cameras too")
    p.add_argument("--retex", metavar="DIR", nargs="?", const="", default=None,
                   help="relink broken image paths and REWIRE materials from texture filenames. "
                        "For purchased assets whose DCC exporter guessed the wrong sockets. "
                        "Defaults to searching beside --input.")
    p.add_argument("--variant", help="texture variant token to prefer, e.g. Weathered / Pristine")
    p.add_argument("--color", help="colourway token to prefer, e.g. Blue / Red")
    p.add_argument("--emissive-strength", type=float, default=1.0,
                   help="Emission Strength for retextured materials, in NITS. An emissive MAP with "
                        "no strength is the [0,1] factor convention and exposes to near-black.")
    p.add_argument("--neutralize-root", metavar="EMPTY",
                   help="zero this object's transform before export — for sources posed for a "
                        "hero render, so the asset ships at rest and the game places it")
    p.add_argument("--rest-pose", action="store_true",
                   help="zero the ROTATION of every empty before export. For a RIGGED source "
                        "whose hero pose lives on joint empties, so it ships at its bind pose and "
                        "joint rotation 0 means rest. Locations are left alone - those are pivots.")
    return p.parse_args(argv)


# ---------------- import ----------------

def import_source(path):
    ext = os.path.splitext(path)[1].lower()
    bpy.ops.wm.read_homefile(use_empty=True)
    if ext == ".fbx":
        bpy.ops.import_scene.fbx(filepath=path)
    elif ext in (".glb", ".gltf"):
        bpy.ops.import_scene.gltf(filepath=path)
    elif ext == ".obj":
        bpy.ops.wm.obj_import(filepath=path)
    elif ext == ".dae":
        bpy.ops.wm.collada_import(filepath=path)
    elif ext == ".blend":
        bpy.ops.wm.open_mainfile(filepath=path)
    else:
        raise SystemExit(f"unsupported input: {ext}")


# ---------------- normalize ----------------

def mesh_objects():
    return [o for o in bpy.data.objects if o.type == "MESH"]


def world_bbox(objs):
    lo = Vector((1e18, 1e18, 1e18))
    hi = Vector((-1e18, -1e18, -1e18))
    found = False
    for ob in objs:
        for corner in ob.bound_box:
            p = ob.matrix_world @ Vector(corner)
            lo = Vector((min(lo[i], p[i]) for i in range(3)))
            hi = Vector((max(hi[i], p[i]) for i in range(3)))
            found = True
    return (lo, hi) if found else (Vector(), Vector())


def report_bbox(tag):
    objs = mesh_objects()
    lo, hi = world_bbox(objs)
    d = hi - lo
    print(f"  {tag:9s} dims {d.x:.3f} x {d.y:.3f} x {d.z:.3f} m   "
          f"min ({lo.x:.3f}, {lo.y:.3f}, {lo.z:.3f})")
    return lo, hi


def convert_non_meshes():
    conv = [o for o in bpy.data.objects if o.type in {"CURVE", "FONT", "SURFACE", "META"}]
    if not conv:
        return
    bpy.ops.object.select_all(action="DESELECT")
    for o in conv:
        o.select_set(True)
    bpy.context.view_layer.objects.active = conv[0]
    bpy.ops.object.convert(target="MESH")
    print(f"  converted {len(conv)} curve/text objects to mesh")


def normalize(args):
    print("--- normalize")
    report_bbox("source")

    if args.neutralize_root:
        rig = bpy.data.objects.get(args.neutralize_root)
        if rig is None:
            raise SystemExit(f"no object named {args.neutralize_root!r} to neutralize")
        rig.rotation_euler = (0.0, 0.0, 0.0)
        rig.location = (0.0, 0.0, 0.0)
        bpy.context.view_layer.update()
        print(f"  neutralized {args.neutralize_root} (pose -> rest)")

    if args.rest_pose:
        n = 0
        for o in bpy.data.objects:
            if o.type == "EMPTY" and tuple(o.rotation_euler) != (0.0, 0.0, 0.0):
                o.rotation_euler = (0.0, 0.0, 0.0)
                n += 1
        bpy.context.view_layer.update()
        print(f"  rest-pose: zeroed rotation on {n} empties (joints -> bind)")

    roots = [o for o in bpy.data.objects if o.parent is None]
    if args.up == "y":
        import math
        for o in roots:
            o.rotation_euler.rotate_axis("X", math.radians(90))
        print("  rotated +90 X (source was Y-up)")
    if args.scale != 1.0:
        for o in roots:
            o.scale *= args.scale
            o.location *= args.scale
        print(f"  scaled x{args.scale}")

    bpy.context.view_layer.update()
    if args.origin != "keep":
        lo, hi = world_bbox(mesh_objects())
        mid = (lo + hi) * 0.5
        shift = Vector((-mid.x, -mid.y, -(lo.z if args.origin == "base" else mid.z)))
        for o in roots:
            o.location += shift
        bpy.context.view_layer.update()
        print(f"  origin -> {args.origin}")

    report_bbox("final")


# ---------------- retexture ----------------
#
# Purchased assets routinely arrive with materials that can't be trusted: absolute authoring-machine
# paths (`D:\Work\...`), textures that resolve for one part and not another, and DCC exporters that
# guess a Unity-style map into the wrong socket (a Smoothness map landing on Specular IOR Level).
# The filenames, however, are reliable. So relink by basename and rewire by suffix.

MAP_TOKENS = {
    "albedo": "base", "basecolor": "base", "diffuse": "base",
    "metallic": "metallic", "metalness": "metallic",
    "roughness": "roughness",
    "smoothness": "smoothness",   # Unity: roughness = 1 - smoothness
    "normal": "normal",
    "emissive": "emissive", "emission": "emissive",
    "ao": "ao", "occlusion": "ao",
}
# Longest-first so 'metallicsmoothness' is never matched as 'metallic'.
_TOKEN_ORDER = sorted(MAP_TOKENS, key=len, reverse=True)


def index_textures(root):
    idx = {}
    for dirpath, _, files in os.walk(root):
        for f in files:
            if os.path.splitext(f)[1].lower() in (".png", ".jpg", ".jpeg", ".tga", ".tif", ".tiff"):
                idx.setdefault(f.lower(), os.path.join(dirpath, f))
    return idx


def classify_map(filename):
    """(maptype, is_packed) from a filename, or (None, _)."""
    import re
    stem = os.path.splitext(os.path.basename(filename))[0].lower()
    # 'orm' must be a whole token — a bare substring test also matches n-ORM-al, which silently
    # dropped every normal map.
    packed = "metallicsmoothness" in stem or re.search(r"(^|[_\-.])orm([_\-.]|$)", stem) is not None
    for tok in _TOKEN_ORDER:
        if tok in stem:
            return MAP_TOKENS[tok], packed
    return None, packed


def relink_images(root):
    """Repoint every unresolved image at a file with the same basename under `root`."""
    idx = index_textures(root)
    fixed, broken = 0, []
    for img in bpy.data.images:
        if img.has_data and tuple(img.size) != (0, 0):
            continue
        base = os.path.basename(img.filepath.replace("\\", "/")).lower()
        hit = idx.get(base)
        if hit:
            img.filepath = hit
            try:
                img.reload()
                fixed += 1
            except Exception:
                broken.append(base)
        elif base:
            broken.append(base)
    print(f"  relinked {fixed} images" + (f", {len(broken)} still unresolved: {broken[:4]}" if broken else ""))
    return idx


def covered_names(tex_dirs):
    """Model names each texture dir serves.

    Kits share one texture set across variants and name the folder for the pair — `Turret_M1_S1`
    holds the maps for both Turret_M1 and Turret_S1. Expand `<Family>_<V1>_<V2>` to both.
    """
    cover = {}
    for d in tex_dirs:
        base = os.path.basename(d)
        parts = base.split("_")
        cover.setdefault(base.lower(), d)
        if len(parts) >= 3:
            family = parts[0]
            for v in parts[1:]:
                cover.setdefault(f"{family}_{v}".lower(), d)
    return cover


def dir_by_name(name, cover):
    """Match an object/material name to a texture dir by decreasing prefix."""
    import re
    n = re.sub(r"\.\d+$", "", name)                    # Blender's .001 suffix
    n = re.sub(r"\d*\s*-\s*Default$", "", n)           # 3ds Max '01 - Default'
    parts = [p for p in n.strip().replace(" ", "_").split("_") if p]
    for k in range(len(parts), 0, -1):
        hit = cover.get("_".join(parts[:k]).lower())
        if hit:
            return hit
    return None


def material_texture_dir(mat, idx, cover=None, obj_names=()):
    """Where this material's maps live: first via its (possibly broken) image refs, then by name.

    The name fallback matters because kit sub-parts often ship 3ds Max PLACEHOLDER materials
    ('01 - Default', 'Turret1') that reference no image at all — nothing for the relink to follow,
    even though the maps sit on disk.
    """
    from collections import Counter
    dirs = Counter()
    if mat.node_tree:
        for n in mat.node_tree.nodes:
            if n.type == "TEX_IMAGE" and n.image and n.image.filepath:
                base = os.path.basename(n.image.filepath.replace("\\", "/")).lower()
                hit = idx.get(base)
                if hit:
                    dirs[os.path.dirname(hit)] += 1
    if dirs:
        return dirs.most_common(1)[0][0]
    if cover:
        for nm in (mat.name, *obj_names):
            hit = dir_by_name(nm, cover)
            if hit:
                return hit
    return None


def pick_maps(tex_dir, variant, color):
    """Choose one file per map type, honouring variant (Pristine/Weathered) and colourway."""
    chosen = {}
    for f in sorted(os.listdir(tex_dir)):
        kind, packed = classify_map(f)
        if kind is None or packed:
            continue  # packed Unity maps need channel surgery the glTF exporter can't express
        low = f.lower()
        if variant and variant.lower() not in low and any(
                v in low for v in ("pristine", "weathered")):
            continue
        if color and kind == "base" and color.lower() not in low and any(
                c in low for c in ("_blue", "_red", "_green")):
            continue
        # A shipped roughness map beats deriving it from smoothness: an Invert node between image
        # and socket is a graph the glTF exporter won't recognise, so the texture would be dropped.
        if kind == "smoothness" and "roughness" in chosen:
            continue
        chosen.setdefault(kind, os.path.join(tex_dir, f))
    if "roughness" not in chosen and "smoothness" in chosen:
        print("    only Smoothness shipped — inverting (exporter may drop it; prefer a Roughness map)")
    return chosen


def rewire_material(mat, maps, emissive_strength):
    nt = mat.node_tree
    for n in list(nt.nodes):
        nt.nodes.remove(n)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    p = nt.nodes.new("ShaderNodeBsdfPrincipled")
    nt.links.new(p.outputs[0], out.inputs["Surface"])

    def img_node(path, non_color):
        im = bpy.data.images.load(path, check_existing=True)
        if non_color:
            im.colorspace_settings.name = "Non-Color"
        n = nt.nodes.new("ShaderNodeTexImage")
        n.image = im
        return n

    wired = []
    if "base" in maps:
        nt.links.new(img_node(maps["base"], False).outputs["Color"], p.inputs["Base Color"])
        wired.append("base")
    if "metallic" in maps:
        nt.links.new(img_node(maps["metallic"], True).outputs["Color"], p.inputs["Metallic"])
        wired.append("metallic")
    if "roughness" in maps:
        nt.links.new(img_node(maps["roughness"], True).outputs["Color"], p.inputs["Roughness"])
        wired.append("roughness")
    elif "smoothness" in maps:
        inv = nt.nodes.new("ShaderNodeInvert")
        nt.links.new(img_node(maps["smoothness"], True).outputs["Color"], inv.inputs["Color"])
        nt.links.new(inv.outputs["Color"], p.inputs["Roughness"])
        wired.append("roughness(1-smoothness)")
    if "normal" in maps:
        nm = nt.nodes.new("ShaderNodeNormalMap")
        nt.links.new(img_node(maps["normal"], True).outputs["Color"], nm.inputs["Color"])
        nt.links.new(nm.outputs["Normal"], p.inputs["Normal"])
        wired.append("normal")
    if "emissive" in maps:
        nt.links.new(img_node(maps["emissive"], False).outputs["Color"], p.inputs["Emission Color"])
        # Without a strength the factor stays <=1 — the glTF FACTOR convention, which exposes to
        # near-black in Solari. See solari_bsn::lint.
        p.inputs["Emission Strength"].default_value = emissive_strength
        wired.append(f"emissive x{emissive_strength:g}")
    return wired


def retexture_all(args):
    print("--- retexture from filenames")
    root = args.retex if os.path.isdir(args.retex) else os.path.dirname(os.path.abspath(args.input or "."))
    idx = relink_images(root)
    cover = covered_names(sorted({os.path.dirname(p) for p in idx.values()}))

    users = {}
    for ob in mesh_objects():
        for m in ob.data.materials:
            if m is not None:
                users.setdefault(m.name, []).append(ob.name)

    missed = []
    for mat in bpy.data.materials:
        tex_dir = material_texture_dir(mat, idx, cover, users.get(mat.name, ()))
        if tex_dir is None:
            print(f"  {mat.name:22s} NO TEXTURES — used by {users.get(mat.name, ['(unused)'])}; "
                  f"will render as a flat colour")
            missed.append(mat.name)
            continue
        maps = pick_maps(tex_dir, args.variant, args.color)
        wired = rewire_material(mat, maps, args.emissive_strength)
        print(f"  {mat.name:22s} {os.path.basename(tex_dir)}: {', '.join(wired) or 'nothing'}")
    if missed:
        print(f"  WARN {len(missed)} material(s) got no textures: {', '.join(missed)}")


# ---------------- flatten ----------------

def principled(mat):
    if not mat or not mat.node_tree:
        return None
    return next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)


def emission_node(mat):
    if not mat or not mat.node_tree:
        return None
    return next((n for n in mat.node_tree.nodes if n.type == "EMISSION"), None)


def source_of(socket):
    """'const' | 'image' | 'graph' for what drives a socket."""
    if not socket.is_linked:
        return "const"
    node = socket.links[0].from_node
    if node.type == "TEX_IMAGE":
        return "image"
    return "graph"


def ensure_uvs(ob):
    if ob.data.uv_layers:
        return
    bpy.ops.object.select_all(action="DESELECT")
    ob.select_set(True)
    bpy.context.view_layer.objects.active = ob
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=1.15, island_margin=0.02)
    bpy.ops.object.mode_set(mode="OBJECT")


def bake_socket_mean(ob, mat, socket, res):
    """Average of whatever drives `socket`, by routing it through Emission and baking EMIT.

    One trick covers every channel — Cycles has no 'metallic' bake target, but EMIT bakes any
    shader, so temporarily wiring the source into an Emission node measures it directly.
    """
    nt = mat.node_tree
    out = next((n for n in nt.nodes if n.type == "OUTPUT_MATERIAL"), None)
    if out is None or not socket.is_linked:
        return None
    saved = out.inputs["Surface"].links[0].from_socket if out.inputs["Surface"].is_linked else None

    emit = nt.nodes.new("ShaderNodeEmission")
    nt.links.new(socket.links[0].from_socket, emit.inputs["Color"])
    nt.links.new(emit.outputs[0], out.inputs["Surface"])

    # Alpha-masked: a UV unwrap never covers the whole image, and uncovered texels clear to black.
    # Averaging them too would scale every result down by the island coverage fraction — which
    # looks like a plausible number, not a bug.
    img = bpy.data.images.new("__bake", res, res, alpha=True, float_buffer=True)
    # Explicitly zero the buffer (setting `generated_color` does NOT re-fill an allocated one) and
    # bake with use_clear=False, so alpha stays 0 wherever no UV island landed.
    img.pixels = [0.0] * (res * res * 4)
    tex = nt.nodes.new("ShaderNodeTexImage")
    tex.image = img
    nt.nodes.active = tex

    ensure_uvs(ob)
    bpy.ops.object.select_all(action="DESELECT")
    ob.select_set(True)
    bpy.context.view_layer.objects.active = ob
    try:
        bpy.ops.object.bake(type="EMIT", use_clear=False, margin=0)
        px = list(img.pixels)
        acc, hits = [0.0, 0.0, 0.0], 0
        for i in range(res * res):
            if px[i * 4 + 3] > 0.5:
                hits += 1
                for c in range(3):
                    acc[c] += px[i * 4 + c]
        mean = [a / hits for a in acc] if hits else None
        if mean is None:
            print(f"    bake covered no texels for {mat.name}")
    except Exception as e:
        print(f"    bake failed for {mat.name}: {e}")
        mean = None

    nt.nodes.remove(tex)
    nt.nodes.remove(emit)
    bpy.data.images.remove(img)
    if saved is not None:
        nt.links.new(saved, out.inputs["Surface"])
    return mean


def measure_material(mat, ob, res, verify):
    """Scalar factors for a material, or None to leave it textured."""
    bsdf = principled(mat)
    emis_node = emission_node(mat)
    if bsdf is None and emis_node is None:
        return None

    # `flat_*` props are a generator's DECLARED intent. They're advisory only — measuring beat them
    # in practice: the hoverboard's analytic metallic said 0.04 where the bake measured ~0.6,
    # because "flat faces read 0.5 pointiness" stops being true on low-poly beveled geometry where
    # most vertices sit on a bevel. Bake wins; declared is kept for comparison.
    declared = None
    if mat.get("flat_base_color") is not None:
        declared = {
            "base_color": list(mat["flat_base_color"]),
            "metallic": float(mat["flat_metallic"]),
            "roughness": float(mat["flat_roughness"]),
            "emissive": list(mat.get("flat_emissive", (0.0, 0.0, 0.0))),
        }

    if bsdf is None:
        # Bare Emission shader (common for neon): nothing to measure but its colour x strength.
        col = list(emis_node.inputs["Color"].default_value)[:3]
        s = emis_node.inputs["Strength"].default_value
        out = {"base_color": [0.0, 0.0, 0.0], "metallic": 0.0, "roughness": 0.5,
               "emissive": [c * s for c in col]}
        return _reconcile(mat, declared, out, verify)

    vals = {}
    for name in CHANNELS:
        sock = bsdf.inputs[name]
        kind = source_of(sock)
        if kind == "image":
            return None  # textured: the existing import path is better than any average
        if kind == "const":
            v = sock.default_value
            vals[name] = list(v)[:3] if hasattr(v, "__len__") else float(v)
        else:
            m = bake_socket_mean(ob, mat, sock, res)
            if m is None:
                return None
            vals[name] = m if name == "Base Color" else sum(m) / 3.0

    ecol = bsdf.inputs["Emission Color"]
    estr = bsdf.inputs["Emission Strength"].default_value
    if source_of(ecol) == "image":
        return None
    ec = (list(ecol.default_value)[:3] if not ecol.is_linked
          else bake_socket_mean(ob, mat, ecol, res) or [0.0, 0.0, 0.0])

    out = {
        "base_color": vals["Base Color"],
        "metallic": vals["Metallic"],
        "roughness": vals["Roughness"],
        "emissive": [c * estr for c in ec],
    }
    return _reconcile(mat, declared, out, verify)


def _reconcile(mat, declared, measured, verify):
    if declared is None or not verify:
        return measured
    d, m = declared, measured
    print(f"    VERIFY {mat.name}")
    print(f"      declared base {[round(c,3) for c in d['base_color']]} "
          f"metal {d['metallic']:.3f} rough {d['roughness']:.3f}")
    print(f"      baked    base {[round(c,3) for c in m['base_color']]} "
          f"metal {m['metallic']:.3f} rough {m['roughness']:.3f}")
    return measured


def rewrite_material(mat, f, emissive_scale):
    nt = mat.node_tree
    for n in list(nt.nodes):
        nt.nodes.remove(n)
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    p = nt.nodes.new("ShaderNodeBsdfPrincipled")
    nt.links.new(p.outputs[0], out.inputs["Surface"])
    p.inputs["Base Color"].default_value = (*f["base_color"], 1.0)
    p.inputs["Metallic"].default_value = f["metallic"]
    p.inputs["Roughness"].default_value = f["roughness"]
    emis = [c * emissive_scale for c in f["emissive"]]
    peak = max(emis)
    if peak > 0.0:
        # glTF clamps emissiveFactor to [0,1] and carries magnitude in
        # KHR_materials_emissive_strength — split, or the value silently clamps.
        p.inputs["Emission Color"].default_value = (*[c / peak for c in emis], 1.0)
        p.inputs["Emission Strength"].default_value = peak
    else:
        p.inputs["Emission Strength"].default_value = 0.0
    return emis


def flatten_all(args):
    print("--- flatten materials")
    bpy.context.scene.render.engine = "CYCLES"
    bpy.context.scene.cycles.samples = 1
    bpy.context.scene.cycles.device = "CPU"  # tiny bakes; avoids GPU init in headless runs

    owner = {}
    for ob in mesh_objects():
        for m in ob.data.materials:
            if m is not None:
                owner.setdefault(m, ob)

    flattened, textured = 0, 0
    for mat, ob in owner.items():
        f = measure_material(mat, ob, args.bake_res, args.verify_flat)
        if f is None:
            textured += 1
            print(f"  {mat.name:24s} textured — left as-is")
            continue
        emis = rewrite_material(mat, f, args.emissive_scale)
        flattened += 1
        tail = f"  emissive {[round(c, 1) for c in emis]}" if max(emis) > 0 else ""
        print(f"  {mat.name:24s} base {[round(c, 3) for c in f['base_color']]} "
              f"metal {f['metallic']:.2f} rough {f['roughness']:.2f}{tail}")
    print(f"  {flattened} flattened, {textured} left textured")


# ---------------- export ----------------

def main():
    args = parse_args()
    if args.input:
        import_source(args.input)

    if args.collection:
        keep = bpy.data.collections.get(args.collection)
        if keep is None:
            raise SystemExit(f"no collection named {args.collection!r}")
        keep_objs = set(keep.all_objects)
        for ob in list(bpy.data.objects):
            if ob not in keep_objs:
                bpy.data.objects.remove(ob, do_unlink=True)

    if not args.keep_lights:
        for ob in list(bpy.data.objects):
            if ob.type in {"LIGHT", "CAMERA"}:
                bpy.data.objects.remove(ob, do_unlink=True)

    convert_non_meshes()
    normalize(args)

    dropped = 0
    for ob in mesh_objects():
        for i in reversed(range(len(ob.data.materials))):
            if ob.data.materials[i] is None:
                ob.data.materials.pop(index=i)
                dropped += 1
    if dropped:
        print(f"  removed {dropped} empty material slots")

    if args.retex is not None:
        retexture_all(args)

    if args.flatten:
        flatten_all(args)

    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    meshes = mesh_objects()
    tris = 0
    for ob in meshes:
        ob.data.calc_loop_triangles()
        tris += len(ob.data.loop_triangles)
    print(f"--- exporting {len(meshes)} meshes, {tris} tris -> {args.out}")

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.export_scene.gltf(
        filepath=args.out,
        export_format="GLB",
        use_selection=True,
        export_apply=True,
        export_materials="EXPORT",
        export_cameras=False,
        export_lights=False,
        export_yup=True,
    )
    print("wrote", args.out)


main()
