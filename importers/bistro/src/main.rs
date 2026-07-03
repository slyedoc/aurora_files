//! Bistro importer: bake the Amazon Lumberyard Bistro glTF to `.cluster_mesh` + a `.bsn`.
//!
//! Uses the PNG-textured `Bistro.glb`. Geometry + textures + alpha-mode + glass transmission are
//! imported; OMM baking and scalar/colour material factors are TODO (see `solari_bsn::gltf`).
//!
//!   cargo run --release -p bistro_import -- raw/Bistro/Bistro.glb assets/bistro bistro

use std::path::PathBuf;

use clap::Parser;
use solari_bsn::{bake_gltf_scene, GltfConfig};

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
    };
    bake_gltf_scene(&cfg);
}
