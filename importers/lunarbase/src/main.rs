//! Lunarbase importer: bake the KitBash3D Lunarbase glTF to `.cluster_mesh` + **one `.bsn` per
//! building**. Lunarbase is a kit: 78 top-level `KB3D_LNB_*_grp` nodes, each a self-contained
//! building or prop. Unlike Bistro (one flat scene), this emits a separate `.bsn` per group,
//! centered at its own origin, so each piece is a reusable prop you compose with `bsn!`. Meshes +
//! textures are baked/extracted once into shared `meshes/`/`textures/` and deduplicated across
//! groups.
//!
//! Uses a self-contained PNG-textured `.glb`. Geometry + textures + alpha-mode + glass transmission
//! are imported; OMM baking and scalar/colour material factors are TODO (see `solari_bsn::gltf`).
//! Two source `.glb`s ship under `raw/lunarbase/`: `kb3d_lunarbase.glb` (decimated) and
//! `kb3d_lunarbase-native.glb` (full-res); pass whichever you want.
//!
//!   cargo run --release -p lunarbase_import -- \
//!     raw/lunarbase/kb3d_lunarbase-native.glb assets/lunarbase

use std::path::PathBuf;

use clap::Parser;
use solari_bsn::{bake_gltf_per_group, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake Lunarbase glTF/GLB → .cluster_mesh + one .bsn per building")]
struct Args {
    /// Source `.glb`/`.gltf` (self-contained GLB, or `.gltf` with sibling `.bin`/textures).
    gltf: PathBuf,
    /// Output asset directory (shared `meshes/`, `textures/`, and the per-building `.bsn` files).
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(default_value = "lunarbase")]
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
        // Unused in per-group mode — each top-level group names its own `.bsn`.
        scene_name: "lunarbase".to_string(),
        replace: args.replace,
        root_components: String::new(),
        emissive_nits: None,
    };
    bake_gltf_per_group(&cfg);
}
