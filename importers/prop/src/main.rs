//! Generic prop importer: `.glb` → `.cluster_mesh` + `.bsn`.
//!
//! Most importers here differ only by a few strings, so this takes them as flags. Reach for a
//! bespoke crate only when an asset needs real import LOGIC (san_miguel's submesh filters,
//! speedtree's alpha classification, zeroday's hierarchy, lunarbase's per-group split).
//!
//! Pairs with `scripts/asset_to_glb.py`, which does the Blender-side normalization and (for
//! procedural sources) flattens materials to texture-free scalars.
//!
//!   cargo run --release -p prop_import -- raw/mech/mech.glb assets/mech --scene-name Mech_A
//!   cargo run --release -p prop_import --bin load_test -- "$PWD/assets" mech/Mech_A.bsn

use std::path::PathBuf;

use clap::Parser;
use aurora_bsn::{bake_gltf_hierarchy, bake_gltf_per_group, bake_gltf_scene, GltfConfig};

#[derive(Parser)]
#[command(about = "Bake a normalized prop glb → .cluster_mesh + .bsn")]
struct Args {
    /// Source `.glb`/`.gltf`.
    glb: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn`).
    out_dir: PathBuf,
    /// `.bsn` scene name and filename stem. Defaults to the glb's file stem.
    #[arg(long)]
    scene_name: Option<String>,
    /// Asset-server-relative prefix the `.bsn` uses. Defaults to the out dir's name.
    #[arg(long)]
    asset_prefix: Option<String>,
    /// Emit one `.bsn` per top-level scene node plus a combined layout scene (kit-style sources).
    #[arg(long)]
    per_group: bool,
    /// Preserve the glTF node hierarchy: nested `Children[]` carrying LOCAL transforms, and
    /// meshless nodes kept as `Name` + `Transform` entities.
    ///
    /// Required for any RIGGED source. The flat path bakes every primitive to a world transform
    /// and emits nothing for a transform-only node, so a joint empty vanishes and its children
    /// freeze where they were baked - leaving the game nothing to rotate.
    #[arg(long)]
    hierarchy: bool,
    /// Meshless animation glb to transcode into `<scene_name>.animclip`, and stamp on the `.bsn`
    /// root as `bevy_animation::AnimatedScene` so the runtime loads, binds and plays it. Node names
    /// must match the geometry glb's — the clip binds by NAME-PATH. Implies `--hierarchy`.
    #[arg(long)]
    anim: Option<PathBuf>,
    /// Multiply every emissive that ships no `KHR_materials_emissive_strength` by this, in nits.
    /// The rescue hatch for factor-convention sources (glTF clamps `emissiveFactor` to [0,1], so
    /// pre-extension assets encode emitters in 0..1 and render ~1000x too dim). See `aurora_bsn::lint`.
    #[arg(long)]
    emissive_nits: Option<f32>,
    /// Re-bake `.cluster_mesh` files even if they already exist.
    #[arg(long)]
    replace: bool,
}

/// `emissive_nits` is a plain `fn` pointer in `GltfConfig` (no captures), so a CLI-provided scale
/// has to reach it through a global rather than a closure.
static EMISSIVE_NITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn emissive_nits_resolver(_material: &str) -> f32 {
    f32::from_bits(EMISSIVE_NITS.load(std::sync::atomic::Ordering::Relaxed))
}

fn main() {
    let args = Args::parse();

    let stem = args
        .glb
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "prop".to_string());
    let scene_name = args.scene_name.unwrap_or(stem);
    let asset_prefix = args.asset_prefix.unwrap_or_else(|| {
        args.out_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "prop".to_string())
    });

    let emissive_nits = args.emissive_nits.map(|n| {
        EMISSIVE_NITS.store(n.to_bits(), std::sync::atomic::Ordering::Relaxed);
        emissive_nits_resolver as fn(&str) -> f32
    });

    // An anim clip is only bindable against a preserved hierarchy — the flat path emits no entity
    // for a joint, so there would be nothing for the clip's name-paths to resolve to.
    let hierarchy = args.hierarchy || args.anim.is_some();
    if args.per_group && hierarchy {
        eprintln!("--per-group cannot be combined with --hierarchy/--anim");
        std::process::exit(2);
    }

    let root_components = match &args.anim {
        Some(_) => format!(
            "    bevy_animation::animclip::AnimatedScene(\"{asset_prefix}/{scene_name}.animclip\")\n"
        ),
        None => String::new(),
    };

    let clip_dst = args.out_dir.join(format!("{scene_name}.animclip"));
    let cfg = GltfConfig {
        gltf_path: args.glb,
        out_dir: args.out_dir,
        asset_prefix,
        scene_name,
        replace: args.replace,
        root_components,
        emissive_nits,
        erode_px: aurora_bsn::mesh::DEFAULT_ERODE_PX,
        omm_subdiv: aurora_bsn::mesh::DEFAULT_OMM_SUBDIV,
    };

    if args.per_group {
        bake_gltf_per_group(&cfg);
    } else if hierarchy {
        bake_gltf_hierarchy(&cfg);
    } else {
        bake_gltf_scene(&cfg);
    }

    if let Some(anim_src) = &args.anim {
        match aurora_bsn::transcode_gltf_to_animclip(anim_src, &clip_dst) {
            Ok(n) => println!(
                "transcoded {n} animation target(s) -> {}",
                clip_dst.display()
            ),
            Err(e) => {
                eprintln!(
                    "ERROR: could not transcode anim glb {} -> {}: {e}",
                    anim_src.display(),
                    clip_dst.display()
                );
                std::process::exit(1);
            }
        }
    }
}
