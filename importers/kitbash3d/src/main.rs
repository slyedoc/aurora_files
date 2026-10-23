//! KitBash3D kit importer: a dir of per-prop glTFs (`scripts/kitbash3d_to_gltf.py`) -> one `.bsn`
//! prop each plus the kit's showcase layout `<kit>.bsn`, which instances them where the kit put them.
//!
//!   cargo run --release -p kitbash3d_import -- raw/atlantis/_gltf assets/atlantis

use std::path::{Path, PathBuf};

use aurora_bsn::{GltfConfig, bake_gltf_prop, bsn};
use clap::Parser;
use serde::Deserialize;

#[derive(Parser)]
#[command(about = "Bake a KitBash3D kit's per-prop glTFs -> one .bsn prop each + the kit layout")]
struct Args {
    /// Stage-A output: `<Prop>.gltf` + `<Prop>.json` per prop, here or in a subdir per shard.
    gltf_dir: PathBuf,
    /// Output dir; cleaned first.
    out_dir: PathBuf,
    /// Layout scene name. Defaults to the out dir's name.
    #[arg(long)]
    kit: Option<String>,
    /// Asset-server-relative prefix the `.bsn` files use. Defaults to the out dir's name.
    #[arg(long)]
    asset_prefix: Option<String>,
    /// Also bake a `.collider` per mesh.
    #[arg(long)]
    colliders: bool,
}

#[derive(Deserialize)]
struct Meta {
    place: [f32; 3],
}

fn dir_name(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "kit".to_string())
}

/// `*.gltf` in `dir` and one level down (one subdir per stage-A shard).
fn gltfs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for path in std::fs::read_dir(dir).expect("read gltf dir").flatten().map(|e| e.path()) {
        if path.is_dir() && depth == 0 {
            gltfs(&path, 1, out);
        } else if path.extension().is_some_and(|e| e == "gltf") {
            out.push(path);
        }
    }
}

fn main() {
    let args = Args::parse();
    let kit = args.kit.unwrap_or_else(|| dir_name(&args.out_dir));
    let prefix = args.asset_prefix.unwrap_or_else(|| dir_name(&args.out_dir));
    aurora_bsn::clean_dir(&args.out_dir);
    std::fs::create_dir_all(&args.out_dir).expect("create out dir");

    let mut sources = Vec::new();
    gltfs(&args.gltf_dir, 0, &mut sources);
    sources.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    println!("{} props in {}", sources.len(), args.gltf_dir.display());

    let mut layout = Vec::new();
    for (i, gltf) in sources.iter().enumerate() {
        let stem = gltf.file_stem().unwrap().to_string_lossy().to_string();
        print!("[{}/{}] ", i + 1, sources.len());
        let cfg = GltfConfig {
            gltf_path: gltf.clone(),
            out_dir: args.out_dir.clone(),
            asset_prefix: prefix.clone(),
            scene_name: stem.clone(),
            colliders: args.colliders,
            ..Default::default()
        };
        if bake_gltf_prop(&cfg).is_none() {
            eprintln!("  {stem}: no geometry, skipped");
            continue;
        }
        let place = std::fs::read_to_string(gltf.with_extension("json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Meta>(&t).ok())
            .map_or([0.0; 3], |m| m.place);
        layout.push(bsn::Entity {
            components: vec![
                bsn::name(&stem),
                bsn::transform(place, None, None),
                bsn::scene_ref(&format!("{prefix}/{stem}.bsn")),
            ],
            children: Vec::new(),
        });
    }

    let count = layout.len();
    let path = args.out_dir.join(format!("{kit}.bsn"));
    std::fs::write(&path, bsn::scene(&kit, Vec::new(), layout)).expect("write layout");
    println!("wrote {count} props + layout {}", path.display());
    aurora_bsn::finish_textures(&args.out_dir, &prefix);
}
