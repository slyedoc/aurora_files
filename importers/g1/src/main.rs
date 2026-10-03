//! G1 rig importer: bake each MotionBricks G1 link `.glb` → a named `.aurora_mesh`, and emit
//! `g1.bsn` — a reusable rig template (a `G1Rig` root with no transform of its own,
//! one `G1Link` child per geom with its baked mesh + flat material). zero's `BodyPlugin`
//! drives it: `drive_links` writes each child's `Transform` from the FK'd qpos every frame, so the
//! `.bsn` only seeds the prefab (offsets live on `G1Link`, not the Transform).
//!
//!   cargo run --release -p g1_import -- /mnt/code/p/zero/assets ai/motionbricks/g1

use std::fs;
use std::path::PathBuf;

use clap::Parser;
use serde::Deserialize;
use aurora_bsn::bsn::{self, Entity, Value};
use aurora_bsn::gltf::bake_glb_primitive;

#[derive(Parser)]
#[command(about = "Bake the G1 rig glbs → .aurora_mesh + g1.bsn template")]
struct Args {
    /// zero's asset root — the dir the asset-server paths are relative to.
    assets_root: PathBuf,
    /// Asset-relative dir holding `g1_links.json` + the link glbs (outputs land here too).
    #[arg(default_value = "ai/motionbricks/g1")]
    prefix: String,
    /// Reuse meshes and files that already exist instead of re-baking them.
    #[arg(long)]
    keep: bool,
}

#[derive(Deserialize)]
struct Link {
    index: usize,
    name: String,
    geoms: Vec<Geom>,
}

#[derive(Deserialize)]
struct Geom {
    /// Asset-relative glb, e.g. `ai/motionbricks/g1/pelvis.glb`.
    glb: String,
    pos: [f32; 3],
    /// wxyz.
    quat: [f32; 4],
    rgba: [f32; 4],
}

fn main() {
    let args = Args::parse();
    let dir = args.assets_root.join(&args.prefix);
    let manifest: Vec<Link> = serde_json::from_str(
        &fs::read_to_string(dir.join("g1_links.json")).expect("read g1_links.json"),
    )
    .expect("parse g1_links.json");

    let meshes_dir = dir.join("meshes");
    let mut baked = 0usize;
    let mut children = Vec::new();

    for link in &manifest {
        for (gi, geom) in link.geoms.iter().enumerate() {
            let stem = PathBuf::from(&geom.glb)
                .file_stem()
                .expect("glb has a stem")
                .to_string_lossy()
                .into_owned();
            let out = meshes_dir.join(format!("{stem}.aurora_mesh"));
            match bake_glb_primitive(&args.assets_root.join(&geom.glb), &out, !args.keep) {
                Ok(true) => baked += 1,
                Ok(false) => {}
                Err(e) => panic!("bake {stem}: {e}"),
            }

            // manifest quat is wxyz → glam xyzw; rgba is sRGB.
            let (qx, qy, qz, qw) = (geom.quat[1], geom.quat[2], geom.quat[3], geom.quat[0]);
            let [r, g, b, a] = geom.rgba;
            let mut components = vec![
                bsn::name(&format!("{}#{gi}", link.name)),
                bsn::transform([0.0; 3], Some([0.0, 0.0, 0.0, 1.0]), Some([1.0; 3])),
                Value::Struct(
                    "ai::model::body::plugin::G1Link",
                    vec![
                        ("index", Value::Int(link.index as i64)),
                        ("offset_pos", bsn::vec3(geom.pos)),
                        ("offset_rot", bsn::quat([qx, qy, qz, qw])),
                    ],
                ),
            ];
            // camera anchor: the torso's primary geom (the G1 has no separate head bone)
            if link.name == "torso_link" && gi == 0 {
                components.push(Value::Path("ai::model::body::plugin::G1Head"));
            }
            components.push(bsn::mesh(&args.prefix, &stem));
            components.push(bsn::material(vec![
                ("base_color", bsn::color_srgba([r, g, b, a])),
                ("metallic", Value::Float(0.5)),
                ("perceptual_roughness", Value::Float(0.55)),
            ]));
            children.push(Entity {
                components,
                children: Vec::new(),
            });
        }
    }

    // No root Transform: whoever places the rig owns it. The MuJoCo-to-bevy frame change is the
    // body plugin's, applied to the FK poses it writes onto the links.
    let bsn = bsn::scene("g1", vec![Value::Path("ai::model::body::plugin::G1Rig")], children);
    let bsn_path = dir.join("g1.bsn");
    fs::write(&bsn_path, bsn).expect("write g1.bsn");
    println!("baked {baked} meshes; wrote {}", bsn_path.display());
}
