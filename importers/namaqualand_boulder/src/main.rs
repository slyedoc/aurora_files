//! Namaqualand Boulder importer: bake the Poly Haven scanned-boulder glTF to `.cluster_mesh` + a
//! `.bsn`. A single high-detail prop — geometry + base-color/normal/ARM textures are imported; the
//! relief is baked into the mesh, so there is no displacement/depth map for the tessellation pass.
//!
//!   cargo run --release -p namaqualand_boulder_import -- \
//!     raw/namaqualand_boulder_02_8k/namaqualand_boulder_02_8k.gltf assets/namaqualand_boulder

use std::path::PathBuf;

use clap::Parser;
use aurora_bsn::{bake_gltf_scene, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake Namaqualand Boulder glTF → .cluster_mesh + .bsn")]
struct Args {
    /// Source `.gltf` (with sibling `.bin`/textures), or a self-contained `.glb`.
    gltf: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn`).
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(default_value = "namaqualand_boulder")]
    asset_prefix: String,
    /// Re-bake `.cluster_mesh` files even if they already exist (no more `rm -rf meshes`).
    #[arg(long)]
    replace: bool,
}

fn main() {
    let args = Args::parse();

    let cfg = GltfConfig {
        gltf_path: args.gltf,
        out_dir: args.out_dir,
        asset_prefix: args.asset_prefix,
        // Lowercase so the output is `namaqualand_boulder.bsn`.
        scene_name: "namaqualand_boulder".to_string(),
        replace: args.replace,
        root_components: String::new(),
        emissive_nits: None,
        erode_px: aurora_bsn::mesh::DEFAULT_ERODE_PX,
        omm_subdiv: aurora_bsn::mesh::DEFAULT_OMM_SUBDIV,
    };
    bake_gltf_scene(&cfg);
}
