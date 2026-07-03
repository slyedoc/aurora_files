"""SpeedTree FBX -> glb (stage A of the SpeedTree importer).

Blender is the only reliable FBX + DDS decoder in this pipeline; it imports one tree's
`.fbx`, guarantees each leaf/frond material's base-color ALPHA survives into the glb (the
Rust stage classifies cutouts + bakes the OMM from that alpha), and writes a self-contained
`.glb` with PNG textures. Alpha *mode* need not be exact — only the channel must be kept.

    blender --background --python scripts/speedtree_to_glb.py -- <in.fbx> <out.glb>
"""

import bpy
import sys


def argv_after_ddash():
    return sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []


def base_color_image_node(mat):
    """The image node feeding the material's Principled Base Color, if any."""
    if not mat.use_nodes:
        return None
    bsdf = next((n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    if bsdf is None:
        return None
    link = next((l for l in mat.node_tree.links
                 if l.to_node == bsdf and l.to_socket.name == "Base Color"), None)
    if link and link.from_node.type == "TEX_IMAGE" and link.from_node.image:
        return bsdf, link.from_node
    return bsdf, None


def keep_alpha(mat):
    """Route base-color alpha into Principled.Alpha so the glTF exporter retains the channel."""
    got = base_color_image_node(mat)
    if not got:
        return False
    bsdf, img_node = got
    if img_node is None or img_node.image.depth != 32:  # 32 bpp == RGBA
        return False
    tree = mat.node_tree
    alpha_in = bsdf.inputs["Alpha"]
    if not alpha_in.is_linked:
        tree.links.new(img_node.outputs["Alpha"], alpha_in)
    # Alpha-clip hints (harmless / ignored on versions that dropped them); Rust re-decides the cutoff.
    for attr, val in (("blend_method", "CLIP"), ("alpha_threshold", 0.5), ("surface_render_method", "DITHERED")):
        try:
            setattr(mat, attr, val)
        except (AttributeError, TypeError):
            pass
    return True


def main():
    args = argv_after_ddash()
    if len(args) < 2:
        print("usage: ... -- <in.fbx> <out.glb>")
        sys.exit(2)
    in_fbx, out_glb = args[0], args[1]

    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.fbx(filepath=in_fbx)

    cut = sum(keep_alpha(m) for m in bpy.data.materials)
    print(f"speedtree: {len(bpy.data.materials)} materials, {cut} kept alpha")

    bpy.ops.export_scene.gltf(
        filepath=out_glb,
        export_format="GLB",
        export_image_format="AUTO",   # keep PNG (alpha) for RGBA, JPEG for opaque
        export_yup=True,
        export_apply=True,            # bake modifiers
        export_normals=True,
        export_tangents=False,        # Rust regenerates / cutouts don't need them
        export_texcoords=True,
        use_visible=False,
    )
    print(f"speedtree: wrote {out_glb}")


main()
