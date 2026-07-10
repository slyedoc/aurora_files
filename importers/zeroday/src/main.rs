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
use solari_bsn::{bake_gltf_hierarchy, transcode_gltf_to_animclip, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake a Zero-Day geometry glb → .cluster_mesh + hierarchy .bsn (+ stage anim glb)")]
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
    /// Meshless animation glb to stage alongside the `.bsn`. Defaults to `<gltf-stem>_anim.glb`.
    #[arg(long)]
    anim: Option<PathBuf>,
    /// Re-bake `.cluster_mesh` files even if they already exist.
    #[arg(long)]
    replace: bool,
}

fn main() {
    let args = Args::parse();

    let anim_src = args.anim.clone().unwrap_or_else(|| {
        let stem = args.gltf.file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
        args.gltf.with_file_name(format!("{stem}_anim.glb"))
    });

    // Marker on the `.bsn` root: zero's `bsn_anim` crate sees it, loads the `.animclip`, builds the
    // AnimationGraph/Player, and binds the tree's nodes to the clip by name-path. `clip` is a
    // `Handle<AnimationClip>` resolved from the asset path by the `AnimClipLoader` (`.animclip`).
    let root_components = format!(
        "    bsn_anim::BsnAnimated(\"{}/{}.animclip\")\n",
        args.asset_prefix, args.scene_name,
    );

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
    let clip_dst = args.out_dir.join(format!("{}.animclip", args.scene_name));
    match transcode_gltf_to_animclip(&anim_src, &clip_dst) {
        Ok(n) => {
            let sz = std::fs::metadata(&clip_dst).map(|m| m.len()).unwrap_or(0);
            println!("transcoded {n} anim targets ({:.2} MB) -> {}", sz as f64 / 1e6, clip_dst.display());
        }
        Err(e) => eprintln!(
            "WARN: could not transcode anim glb {} -> {}: {e}\n      (bake the _anim.glb in stage A, or pass --anim)",
            anim_src.display(), clip_dst.display()
        ),
    }
}
