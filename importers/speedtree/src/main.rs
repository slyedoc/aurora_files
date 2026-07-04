//! SpeedTree importer: bake every staged `<Tree>.glb` to `<Tree>.bsn` + shared meshes/textures,
//! with an OMM on each leaf/frond cutout.
//!
//! Stage A first turns the source FBX into glb (Blender is the FBX/DDS decoder):
//!   for d in raw/SpeedTree_v2/*/HighPoly; do
//!     blender -b --python scripts/speedtree_to_glb.py -- "$d"/*.fbx raw/SpeedTree_v2/_glb/"$(basename $(dirname $d))".glb
//!   done
//! then:
//!   cargo run --release -p speedtree_import -- raw/SpeedTree_v2/_glb assets/speedtree

use std::path::PathBuf;

use clap::Parser;
use solari_bsn::{bake_speedtree, SpeedTreeConfig};

#[derive(Parser)]
#[command(about = "Bake staged SpeedTree .glb trees → per-tree .bsn (+OMM cutouts)")]
struct Args {
    /// Directory of stage-A `.glb` trees.
    glb_dir: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and one `.bsn` per tree).
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(default_value = "speedtree")]
    asset_prefix: String,
    /// Uniform scale applied to every tree (SpeedTree FBX author units → metres).
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
    /// Re-bake `.cluster_mesh` files even if they already exist.
    #[arg(long)]
    replace: bool,
    /// Erosion radius (texels) on the cutout mask before OMM baking.
    #[arg(long, default_value_t = 2)]
    erode: u32,
    /// Max OMM subdivision level.
    #[arg(long, default_value_t = 6)]
    level: u32,

    /// Also bake `<Tree>_clump<K>.bsn`: K trees merged into one BLAS per material.
    #[arg(long, default_value_t = 0)]
    clump: u32,

    /// Clump footprint radius in meters.
    #[arg(long, default_value_t = 9.0)]
    clump_radius: f32,
}

fn main() {
    let args = Args::parse();
    bake_speedtree(&SpeedTreeConfig {
        glb_dir: args.glb_dir,
        out_dir: args.out_dir,
        asset_prefix: args.asset_prefix,
        replace: args.replace,
        scale: args.scale,
        erode_px: args.erode,
        omm_subdiv: args.level,
        clump: args.clump,
        clump_radius: args.clump_radius,
    });
}
