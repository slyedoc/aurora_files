"""Zero-Day FBX -> glb (stage A of the zeroday importer).

Blender is the only reliable FBX + DDS decoder here. It imports one Zero-Day set (a ~300 MB
FBX referencing DXT1/DXT5 `.dds` maps), rewires each material from its image FILENAMES (the
`_BaseColor/_Specular/_Normal/_Emissive` suffix is the only reliable classifier — FBX's Phong
slots don't map cleanly to a Principled ORM), and writes TWO self-contained glbs from the SAME
objects (so node names match byte-for-byte, which is what the runtime clip binds to):

  <out>.glb        geometry + materials (+ animation) — the Rust stage bakes cluster meshes
  <out>_anim.glb   meshless: node hierarchy + TRS animation + camera only (tiny; runtime clip)

Zero-Day maps (per README):
  BaseColor  RGB = base color, A = opacity            -> Base Color + Alpha
  Specular   R = occlusion, G = roughness, B = metal  -> Roughness(G) + Metallic(B)  (glTF ORM)
  Normal     DirectX (green points down)              -> Normal (green inverted -> OpenGL)
  Emissive   RGB = emissive                            -> Emission Color

    blender -b --python scripts/zeroday_to_glb.py -- <in.fbx> <out_stem> [--anim-only] [--quick]

`--anim-only` skips the heavy geometry glb (fast hierarchy/animation check); `--quick` exports
the geometry glb WITHOUT baking images (validates the material node graph without the multi-GB
texture pass).
"""

import bpy
import sys
import os


def argv_after_ddash():
    return sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []


def classify(image):
    """Zero-Day map type from an image's filename suffix, or None."""
    name = (image.filepath or image.name or "").lower()
    for tag in ("basecolor", "specular", "normal", "emissive"):
        if tag in name:
            return tag
    return None


def image_nodes(mat):
    return [n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image]


def rewire_material(mat):
    """Rebuild a material's Principled links from its image nodes' filename suffixes.
    Returns the set of map types found."""
    if not mat.use_nodes:
        return set()
    tree = mat.node_tree
    bsdf = next((n for n in tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    if bsdf is None:
        return set()

    found = {}
    for node in image_nodes(mat):
        tag = classify(node.image)
        if tag and tag not in found:
            found[tag] = node

    def link(from_socket, to_input):
        tree.links.new(from_socket, bsdf.inputs[to_input])

    # Force OPAQUE: FBX import pre-sets these materials to a blended state (base-color alpha =
    # opacity), which the glTF exporter turns into alphaMode=BLEND on ~all materials and wrecks the
    # RT path. Reset the Alpha socket + blend state so the exporter writes OPAQUE. (Opacity is a
    # later refinement — a per-material histogram classify like the OBJ cutmask pass.)
    if "Alpha" in bsdf.inputs and not bsdf.inputs["Alpha"].is_linked:
        bsdf.inputs["Alpha"].default_value = 1.0
    for attr, val in (("blend_method", "OPAQUE"), ("surface_render_method", "DITHERED")):
        try:
            setattr(mat, attr, val)
        except (AttributeError, TypeError):
            pass

    if "basecolor" in found:
        img = found["basecolor"]
        img.image.colorspace_settings.name = "sRGB"
        link(img.outputs["Color"], "Base Color")
        # NOTE: base-color alpha is OPACITY. Left unlinked (opaque) for the first cut — blanket
        # alpha-linking exports every material as glTF BLEND, which wrecks the RT path. Opacity is
        # a later refinement (per-material histogram classify, like the OBJ cutmask pass).

    if "specular" in found:
        img = found["specular"]
        img.image.colorspace_settings.name = "Non-Color"
        sep = tree.nodes.new("ShaderNodeSeparateColor")
        tree.links.new(img.outputs["Color"], sep.inputs["Color"])
        # glTF ORM packing: G -> roughness, B -> metallic (R occlusion handled by exporter/Rust).
        # The exporter recognizes Roughness+Metallic driven by one image's G/B and emits it as the
        # single metallicRoughnessTexture.
        link(sep.outputs["Green"], "Roughness")
        link(sep.outputs["Blue"], "Metallic")

    if "normal" in found:
        img = found["normal"]
        img.image.colorspace_settings.name = "Non-Color"
        # Straight image -> Normal Map -> Normal (keeps the exporter's normalTexture detection).
        # Zero-Day normals are DirectX (green down); the OpenGL green-flip is handled downstream
        # (Rust decode or shader), NOT via math nodes here (those defeat texture detection).
        nmap = tree.nodes.new("ShaderNodeNormalMap")
        tree.links.new(img.outputs["Color"], nmap.inputs["Color"])
        link(nmap.outputs["Normal"], "Normal")

    if "emissive" in found:
        img = found["emissive"]
        img.image.colorspace_settings.name = "sRGB"
        link(img.outputs["Color"], "Emission Color")
        if "Emission Strength" in bsdf.inputs:
            bsdf.inputs["Emission Strength"].default_value = 1.0

    return set(found.keys())


def scene_bbox():
    import mathutils
    lo = mathutils.Vector((1e18,) * 3); hi = mathutils.Vector((-1e18,) * 3)
    for o in bpy.data.objects:
        if o.type != "MESH":
            continue
        for c in o.bound_box:
            w = o.matrix_world @ mathutils.Vector(c)
            lo = mathutils.Vector(map(min, lo, w)); hi = mathutils.Vector(map(max, hi, w))
    return lo, hi


def export_geometry(out_glb, quick):
    bpy.ops.export_scene.gltf(
        filepath=out_glb,
        export_format="GLB",
        export_image_format="NONE" if quick else "AUTO",  # AUTO: PNG (alpha) / JPEG (opaque)
        export_yup=True,
        export_apply=False,          # NEVER bake modifiers here — it flattens animation
        export_animations=False,     # animation rides in the separate _anim.glb; keep geometry lean
        export_normals=True,
        export_tangents=False,       # Rust regenerates
        export_texcoords=True,
        export_cameras=True,
        use_visible=False,
    )


def export_anim_only(out_glb):
    """Meshless: swap every mesh's data for one shared trivial mesh (object name / transform /
    animation_data stay on the OBJECT), then export animation + cameras. Tiny glb, names intact."""
    stub = bpy.data.meshes.new("zeroday_stub")  # 0 verts
    for o in bpy.data.objects:
        if o.type == "MESH":
            o.data = stub
    for m in list(bpy.data.materials):
        bpy.data.materials.remove(m)
    bpy.ops.export_scene.gltf(
        filepath=out_glb,
        export_format="GLB",
        export_image_format="NONE",
        export_yup=True,
        export_apply=False,
        export_animations=True,
        export_animation_mode="SCENE",
        export_frame_range=True,
        export_bake_animation=True,
        export_cameras=True,
        use_visible=False,
    )


def main():
    args = argv_after_ddash()
    flags = {a for a in args if a.startswith("--")}
    pos = [a for a in args if not a.startswith("--")]
    if len(pos) < 2:
        print("usage: ... -- <in.fbx> <out_stem> [--anim-only] [--quick]"); sys.exit(2)
    in_fbx, out_stem = pos[0], pos[1]
    anim_only = "--anim-only" in flags
    quick = "--quick" in flags

    bpy.ops.wm.read_factory_settings(use_empty=True)
    print(f"zeroday: importing {in_fbx} ...", flush=True)
    bpy.ops.import_scene.fbx(filepath=in_fbx)

    from collections import Counter
    tally = Counter()
    for m in bpy.data.materials:
        for t in rewire_material(m):
            tally[t] += 1
    print(f"zeroday: {len(bpy.data.materials)} materials — maps: {dict(tally)}")

    lo, hi = scene_bbox()
    dim = [hi[i] - lo[i] for i in range(3)]
    print(f"zeroday: bbox min={tuple(round(x,2) for x in lo)} max={tuple(round(x,2) for x in hi)}")
    print(f"zeroday: dims={tuple(round(x,2) for x in dim)}  (glTF is Y-up after export)")

    os.makedirs(os.path.dirname(os.path.abspath(out_stem)), exist_ok=True)

    if not anim_only:
        geo = f"{out_stem}.glb"
        print(f"zeroday: exporting geometry -> {geo} (quick={quick}) ...", flush=True)
        export_geometry(geo, quick)
        print(f"zeroday: wrote {geo} ({os.path.getsize(geo)/1e6:.1f} MB)")

    anim = f"{out_stem}_anim.glb"
    print(f"zeroday: exporting anim-only -> {anim} ...", flush=True)
    export_anim_only(anim)
    print(f"zeroday: wrote {anim} ({os.path.getsize(anim)/1e6:.2f} MB)")
    print("zeroday: done", flush=True)


main()
