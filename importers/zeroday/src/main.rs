//! Zero-Day (Beeple ORCA) importer — the first ANIMATED `.bsn`. Two-stage, like SpeedTree: Blender
//! turns the ~300 MB FBX + DDS into glbs (`scripts/zeroday_to_glb.py`), this bakes them.
//!
//! Stage A emits two glbs from one Blender run so node names match byte-for-byte:
//!   <stem>.glb       geometry + materials (emissive/ORM/normal), animations stripped
//!   <stem>_anim.glb  meshless: node hierarchy + TRS animation + camera (tiny; the runtime clip)
//!
//! This stage bakes the geometry glb into a HIERARCHY-preserving `.bsn` (nested `Children[]`, per-node
//! local `Transform` + `Name`) via [`bake_gltf_hierarchy`], and copies the anim glb next to it. A
//! runtime crate (Stage C) loads the anim glb's `AnimationClip`s and binds them to the `.bsn` nodes by
//! name-path; the GPU transform table then propagates animated parents to their mesh children.
//!
//!   cargo run --release -p zeroday_import -- \
//!       raw/ZeroDay_v1/_glb/measure_one.glb assets/zeroday MeasureOne

use std::path::PathBuf;

use clap::Parser;
use aurora_bsn::{bake_gltf_hierarchy, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake a Zero-Day geometry glb → .cluster_mesh + hierarchy .bsn")]
struct Args {
    /// Source geometry `.glb` (from `scripts/zeroday_to_glb.py`, animations stripped).
    gltf: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, the `.bsn`, and the copied anim glb).
    out_dir: PathBuf,
    /// `.bsn` scene name + filename stem (e.g. `MeasureOne` → `MeasureOne.bsn`).
    #[arg(default_value = "MeasureOne")]
    scene_name: String,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(long, default_value = "zeroday")]
    asset_prefix: String,
    /// Re-bake `.cluster_mesh` files even if they already exist.
    #[arg(long)]
    replace: bool,
}

fn main() {
    let args = Args::parse();
    // No animation marker: the `.animclip` path lived in the old engine's bevy branch.
    let root_components = String::new();

    let cfg = GltfConfig {
        gltf_path: args.gltf,
        out_dir: args.out_dir.clone(),
        asset_prefix: args.asset_prefix,
        scene_name: args.scene_name.clone(),
        replace: args.replace,
        root_components,
        emissive_nits: None,
    };
    bake_gltf_hierarchy(&cfg);

    // Transcode the meshless anim glb's clips into a compact `.animclip` binary next to the `.bsn`
    // (no runtime glTF dependency). One target per animated node; bound at runtime by name-path.
}
