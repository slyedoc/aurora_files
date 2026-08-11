//! San Miguel importer: configure [`aurora_bsn::bake_scene`] for the San Miguel courtyard.
//!
//!   cargo run --release -p san_miguel_import -- raw/San_Miguel/san-miguel.obj assets/san_miguel san_miguel

use std::path::PathBuf;

use clap::Parser;
use aurora_bsn::{bake_scene, SceneConfig, SubmeshFilter, SubmeshInfo};

/// Default texture stem isolated by `--cutout-only` (a flower petal cutmask).
const DEFAULT_CUTOUT_TEX: &str = "FL11pet3";

#[derive(Parser)]
#[command(about = "Bake San Miguel OBJ → .cluster_mesh + .bsn")]
struct Args {
    /// Source `.obj` (its `.mtl` and textures resolve relative to it).
    obj: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn`).
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(default_value = "san_miguel")]
    asset_prefix: String,
    /// Emit only the `piso` floor submeshes (the displacement surface) → `Floor.bsn`.
    #[arg(long, conflicts_with = "cutout_only")]
    floor_only: bool,
    /// Emit only the cutout (cutmask) submeshes of one texture — a single OMM-bearing object →
    /// `Cutout.bsn`. Optional value is the diffuse texture stem to match (default `FL11pet3`).
    #[arg(long, num_args = 0..=1, default_missing_value = DEFAULT_CUTOUT_TEX)]
    cutout_only: Option<String>,
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

fn main() {
    let args = Args::parse();

    let (scene_name, submesh_filter): (&str, Option<SubmeshFilter>) = if args.floor_only {
        // `starts_with` (not `contains`) so the two real floors (`piso_rustico`,
        // `piso_patio_exterior`) match but `moldura2piso` (trim) does not.
        (
            "Floor",
            Some(Box::new(|i: &SubmeshInfo| {
                i.diffuse_basename.is_some_and(|d| d.starts_with("piso"))
            })),
        )
    } else if let Some(tex) = args.cutout_only.clone() {
        // Keep only the cutout (`is_cutmask`) submeshes of the chosen texture, so the scene is a
        // single OMM-bearing object — the OMM analogue of `--floor-only`.
        (
            "Cutout",
            Some(Box::new(move |i: &SubmeshInfo| {
                i.is_cutmask && i.diffuse_basename.is_some_and(|d| d.starts_with(&tex))
            })),
        )
    } else {
        ("SanMiguel", None)
    };

    let cfg = SceneConfig {
        obj_path: args.obj,
        out_dir: args.out_dir,
        asset_prefix: args.asset_prefix,
        scene_name: scene_name.to_string(),
        submesh_filter,
        // Share geometry across the courtyard's repeated props (chairs, plates, colonnade).
        dedup: true,
        replace: args.replace,
        erode_px: args.erode,
        omm_subdiv: args.level,
    };
    bake_scene(&cfg);
}
