//! Kenney kit importer: `raw/kenney/<kit>/*.glb` → `assets/kenney/<kit>/meshes/<name>.cluster_mesh`
//! plus `assets/kenney/<kit>/textures/*.png`.
//!
//! Each glb's node tree is flattened and every primitive merged into one mesh in the prop's own
//! frame: a Kenney prop is a few primitives that all sample the kit colormap, so one mesh and one
//! material per prop loses nothing, and the city spawns one ray-traced instance per prop instead
//! of a scene of them. Materials are not baked: `bevy_city` builds them (colormap / variations /
//! flat colours) itself, exactly like the upstream bevy example.
//!
//!   scripts/fetch_kenney.sh
//!   cargo run --release -p kenney_import

use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use aurora_cluster_mesh::{ClusterMeshData, write_cluster_mesh_sync};
use bevy::{
    asset::RenderAssetUsages,
    math::{Mat4, Vec2, Vec3},
    mesh::{Indices, Mesh, PrimitiveTopology},
};
use clap::Parser;

const KITS: [&str; 4] = [
    "car-kit",
    "city-kit-commercial",
    "city-kit-roads",
    "city-kit-suburban",
];

#[derive(Parser)]
#[command(
    about = "Bake the Kenney kits (raw/kenney) into merged .cluster_mesh props (assets/kenney)"
)]
struct Args {
    /// Source directory holding the kits (see scripts/fetch_kenney.sh).
    #[arg(long, default_value = "raw/kenney")]
    raw: PathBuf,
    /// Output asset directory.
    #[arg(long, default_value = "assets/kenney")]
    out: PathBuf,
    /// Re-bake meshes that already exist.
    #[arg(long)]
    replace: bool,
}

fn main() {
    let args = Args::parse();
    let mut baked = 0;
    let mut skipped = 0;
    for kit in KITS {
        let src = args.raw.join(kit);
        if !src.is_dir() {
            eprintln!("missing {}: run scripts/fetch_kenney.sh", src.display());
            std::process::exit(2);
        }
        let meshes_dir = args.out.join(kit).join("meshes");
        let textures_dir = args.out.join(kit).join("textures");
        fs::create_dir_all(&meshes_dir).expect("create meshes dir");
        fs::create_dir_all(&textures_dir).expect("create textures dir");

        let mut glbs: Vec<PathBuf> = fs::read_dir(&src)
            .expect("read kit dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "glb"))
            .collect();
        glbs.sort();
        for glb in glbs {
            let stem = glb.file_stem().unwrap().to_string_lossy().to_string();
            let out = meshes_dir.join(format!("{stem}.cluster_mesh"));
            if out.exists() && !args.replace {
                skipped += 1;
                continue;
            }
            match bake(&glb) {
                Some(data) => {
                    let w = BufWriter::new(File::create(&out).expect("create .cluster_mesh"));
                    write_cluster_mesh_sync(&data, w).expect("write .cluster_mesh");
                    baked += 1;
                }
                None => eprintln!("{}: no triangles, skipped", glb.display()),
            }
        }

        let tex_src = src.join("Textures");
        if tex_src.is_dir() {
            for entry in fs::read_dir(&tex_src).expect("read Textures") {
                let path = entry.expect("entry").path();
                if path.extension().is_some_and(|e| e == "png") {
                    fs::copy(&path, textures_dir.join(path.file_name().unwrap()))
                        .expect("copy texture");
                }
            }
        }
    }
    println!(
        "kenney: {baked} props baked, {skipped} already present -> {}",
        args.out.display()
    );
}

/// One merged, world-space (prop-frame) mesh for the glb's default scene.
fn bake(glb: &Path) -> Option<ClusterMeshData> {
    let (document, buffers, _images) = gltf::import(glb).expect("import glb");
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())?;

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        for child in node.children() {
            stack.push((child, world));
        }
        let Some(mesh) = node.mesh() else { continue };
        let normal_matrix = world.inverse().transpose();
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|b| Some(&buffers[b.index()]));
            let Some(pos) = reader.read_positions() else {
                continue;
            };
            let base = positions.len() as u32;
            let prim_positions: Vec<Vec3> = pos.map(Vec3::from).collect();
            let prim_normals: Vec<Vec3> = match reader.read_normals() {
                Some(n) => n.map(Vec3::from).collect(),
                None => vec![Vec3::Y; prim_positions.len()],
            };
            let prim_uvs: Vec<Vec2> = match reader.read_tex_coords(0) {
                Some(t) => t.into_f32().map(Vec2::from).collect(),
                None => vec![Vec2::ZERO; prim_positions.len()],
            };
            for ((p, n), uv) in prim_positions.iter().zip(&prim_normals).zip(&prim_uvs) {
                positions.push(world.transform_point3(*p).to_array());
                normals.push(
                    normal_matrix
                        .transform_vector3(*n)
                        .normalize_or(Vec3::Y)
                        .to_array(),
                );
                uvs.push(uv.to_array());
            }
            match reader.read_indices() {
                Some(read) => indices.extend(read.into_u32().map(|i| base + i)),
                None => indices.extend(0..prim_positions.len() as u32),
            }
        }
    }
    if indices.len() < 3 {
        return None;
    }

    let mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices));
    Some(ClusterMeshData::from_mesh_flat(&mesh).expect("cluster mesh"))
}
