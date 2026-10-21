# Run inside the Winston UE editor (ue_python, exec this file): MetaHuman Creator's default outfit
# fitted to Winston by a Cinematic build, exported for assemble_glb.py as raw/winston/outfit.fbx
# plus the garment's textures as PNG. Never save the built outfit or open it in a viewport: its
# cloth material's PSO crashes NVIDIA 615's SPIR-V compiler (thumbnails on save count). The glTF
# exporter bakes materials the same way, hence FBX.
import os

import unreal

raw = "/mnt/code/p/aurora_files/raw/winston"
sub = unreal.get_editor_subsystem(unreal.MetaHumanCharacterEditorSubsystem)
ch = unreal.load_asset("/Game/Winston/Winston")
col = ch.get_editor_property("internal_collection")
inst = col.get_editor_property("default_instance")
sub.try_add_object_to_edit(ch)
# The selection does not always survive an editor restart; set it every time.
slots = [str(s.get_editor_property("selection").get_editor_property("slot_name")) for s in inst.get_slot_selection_data()]
if "Outfits" not in slots:
    key = col.try_add_item_from_wardrobe_item("Outfits", unreal.load_asset("/MetaHumanCharacter/Optional/Clothing/WI_DefaultGarment"))
    inst.set_single_slot_selection("Outfits", key)

params = unreal.MetaHumanCharacterEditorBuildParameters()
params.set_editor_property("pipeline_type", unreal.MetaHumanDefaultPipelineType.CINEMATIC)
params.set_editor_property("absolute_build_path", "/Game/MetaHumans")
params.set_editor_property("name_override", "WinstonUE")
sub.build_meta_human(ch, params)


def export(obj, filename, options=None):
    task = unreal.AssetExportTask()
    task.object, task.filename = obj, filename
    task.automated = task.replace_identical = True
    task.prompt = False
    if options:
        task.options = options
    return unreal.Exporter.run_asset_export_task(task)


outfit = unreal.load_asset("/Game/MetaHumans/WinstonUE/Clothing/Winston_Outfits")
fbx = unreal.FbxExportOption()
fbx.set_editor_property("collision", False)
fbx.set_editor_property("level_of_detail", False)
print("outfit.fbx", export(outfit, os.path.join(raw, "outfit.fbx"), fbx))

# The fit picks a body shape (D for Winston); its pieces' AO/normal/mask maps.
textures = os.path.join(raw, "outfit_tex")
os.makedirs(textures, exist_ok=True)
registry = unreal.AssetRegistryHelpers.get_asset_registry()
base = "/MetaHumanCharacter/Optional/Clothing/DefaultGarment/ClothAssets/bodyShapeD"
for a in registry.get_assets_by_path(base, recursive=True):
    if str(a.asset_class_path.asset_name) == "Texture2D":
        print(a.asset_name, export(a.get_asset(), os.path.join(textures, f"{a.asset_name}.png")))
