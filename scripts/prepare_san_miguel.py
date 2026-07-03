# Prepares Guillermo M. Leal Llaguno's "San Miguel" scene for the san_miguel
# example: imports the McGuire-archive OBJ and exports a single glTF (GLB).
#
# Download the scene from Morgan McGuire's Computer Graphics Archive:
#   https://casual-effects.com/data/  ("San Miguel")
# It ships as san-miguel.obj (+ .mtl + textures/). A lower-poly variant
# (san-miguel-low-poly.obj) is also included if the full ~10M-triangle mesh is
# too heavy for your GPU.
#
# Run headless:
#   blender --background --python prepare_san_miguel.py -- /path/to/san-miguel.obj /path/to/out/SanMiguel.glb
# or open it in Blender's Text Editor, edit the two paths below, and Run Script.
#
# What it does:
#   1. Imports the OBJ (with its materials/textures).
#   2. Adds planar UVs to any mesh that has none (tangent generation needs them).
#   3. Deletes cameras and lights (the example supplies its own sun + camera).
#   4. Exports everything as one .glb.
#
# After this, run the two post-processing passes from the README (texture
# compression to KTX2, then reclassify_alpha.py) to produce SanMiguel_ktx2.glb,
# which the example loads.

import bpy
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# Config (positional CLI args after `--` override these two paths)
# ---------------------------------------------------------------------------

OBJ_PATH = Path.home() / "Downloads/san-miguel/san-miguel.obj"
OUT_PATH = OBJ_PATH.with_name("SanMiguel.glb")

if "--" in sys.argv:
    cli = sys.argv[sys.argv.index("--") + 1 :]
    if len(cli) >= 1:
        OBJ_PATH = Path(cli[0])
    if len(cli) >= 2:
        OUT_PATH = Path(cli[1])

# ---------------------------------------------------------------------------
# Start from an empty scene
# ---------------------------------------------------------------------------

bpy.ops.wm.read_factory_settings(use_empty=True)

print(f"Importing {OBJ_PATH}")
# Blender 4.x native OBJ importer. San Miguel is modelled Y-up; the default
# import axes (forward=-Z, up=Y) keep it upright in Blender's Z-up world.
bpy.ops.wm.obj_import(filepath=str(OBJ_PATH))

# ---------------------------------------------------------------------------
# UV handling. The OBJ importer creates a `UVMap` layer but can leave it without
# an active/active-render flag; the glTF exporter only writes the active-render
# UV, so an unflagged layer is silently dropped (then bevy's mikktspace tangent
# bake fails with "missing Vertex_Uv"). Mark it. Meshes that arrived with no UVs
# at all get a planar projection (tangent space needs UVs).
# ---------------------------------------------------------------------------

for obj in bpy.data.objects:
    if obj.type != "MESH":
        continue
    mesh = obj.data
    if mesh.uv_layers:
        if mesh.uv_layers.active is None:
            mesh.uv_layers.active = mesh.uv_layers[0]
        mesh.uv_layers[0].active_render = True
    else:
        bpy.context.view_layer.objects.active = obj
        bpy.ops.object.select_all(action="DESELECT")
        obj.select_set(True)
        bpy.ops.object.mode_set(mode="EDIT")
        bpy.ops.mesh.select_all(action="SELECT")
        bpy.ops.uv.smart_project(angle_limit=1.15)  # ~66 degrees
        bpy.ops.object.mode_set(mode="OBJECT")

# ---------------------------------------------------------------------------
# Drop cameras and lights — the example owns lighting and the camera.
# ---------------------------------------------------------------------------

for obj in list(bpy.data.objects):
    if obj.type in {"CAMERA", "LIGHT"}:
        bpy.data.objects.remove(obj, do_unlink=True)

# ---------------------------------------------------------------------------
# Export a single GLB.
# ---------------------------------------------------------------------------

OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
print(f"Exporting {OUT_PATH}")
bpy.ops.export_scene.gltf(
    filepath=str(OUT_PATH),
    export_format="GLB",
    export_apply=True,            # apply modifiers
    export_yup=True,              # glTF is Y-up
    export_texcoords=True,
    export_normals=True,
    export_tangents=True,
    export_materials="EXPORT",
)
print("Done.")
