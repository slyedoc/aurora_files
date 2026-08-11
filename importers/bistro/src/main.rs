//! Bistro importer: bake the Amazon Lumberyard Bistro glTF to `.cluster_mesh` + a `.bsn`.
//!
//! Uses the PNG-textured `Bistro.glb`. Geometry + textures + alpha-mode + glass transmission are
//! imported; OMM baking and scalar/colour material factors are TODO (see `aurora_bsn::gltf`).
//!
//!   cargo run --release -p bistro_import -- raw/Bistro/Bistro.glb assets/bistro bistro

use std::path::PathBuf;

use clap::Parser;
use aurora_bsn::{bake_gltf_scene, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake Bistro glTF/GLB → .cluster_mesh + .bsn")]
struct Args {
    /// Source `.glb`/`.gltf` (self-contained GLB, or `.gltf` with sibling `.bin`/textures).
    gltf: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn`).
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(default_value = "bistro")]
    asset_prefix: String,
    /// Re-bake `.cluster_mesh` files even if they already exist (no more `rm -rf meshes`).
    #[arg(long)]
    replace: bool,
    /// OMM cutout alpha-mask erosion radius in texels (pulls the conservative silhouette in;
    /// `0` disables, 1-3 useful).
    #[arg(long, default_value_t = aurora_bsn::mesh::DEFAULT_ERODE_PX)]
    erode: u32,
    /// Max OMM subdivision level (per-triangle cap; higher = finer cutout edge, larger data).
    #[arg(long, default_value_t = aurora_bsn::mesh::DEFAULT_OMM_SUBDIV)]
    level: u32,
}

/// Bistro emitters ship no `KHR_materials_emissive_strength`; assign physical nits by material name
/// so signage, screens, lamps and firelight bake at correct relative brightness. View with exposure.
fn emissive_nits_default(name: &str) -> f32 {
    let n = name.to_ascii_lowercase();
    if n.contains("sign") || n.contains("neon") || n.contains("letter") { 2000.0 }
    else if n.contains("screen") || n.contains("monitor") || n.contains("display") || n.contains("tv") { 350.0 }
    else if n.contains("lamp") || n.contains("bulb") || n.contains("light") { 20000.0 }
    else if n.contains("candle") || n.contains("fire") || n.contains("ember") || n.contains("flame") { 6000.0 }
    else { 100.0 }
}

fn main() {
    let args = Args::parse();

    let cfg = GltfConfig {
        gltf_path: args.gltf,
        out_dir: args.out_dir,
        asset_prefix: args.asset_prefix,
        // Lowercase so the output is `bistro.bsn` (the path the bevy bistro example loads).
        scene_name: "bistro".to_string(),
        replace: args.replace,
        root_components: String::new(),
        emissive_nits: Some(emissive_nits_default),
        erode_px: args.erode,
        omm_subdiv: args.level,
    };
    bake_gltf_scene(&cfg);
}
