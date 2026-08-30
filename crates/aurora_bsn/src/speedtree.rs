//! SpeedTree import path: one `.glb` per tree (staged by `scripts/speedtree_to_glb.py`) →
//! one `<Tree>.bsn` + shared `meshes/`/`textures/`, WITH an OMM baked on every leaf/frond
//! cutout. Unlike the Bistro glTF path this classifies cutouts from the base-color alpha
//! histogram (Blender exports foliage as `BLEND`; we re-decide `Mask`) and bakes the OMM so the
//! RT cores never invoke the any-hit on the millions of leaf micro-triangles.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use bevy::math::Mat4;
use aurora_cluster_mesh::{write_cluster_mesh_sync, ClusterMeshData};

use crate::gltf::build_primitive_mesh;
use crate::{bsn, img};

/// Per-run knobs for the SpeedTree importer.
pub struct SpeedTreeConfig {
    /// Directory of staged `.glb` trees (each `<Tree>.glb` becomes `<Tree>.bsn`).
    pub glb_dir: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the per-tree `.bsn`).
    pub out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    pub asset_prefix: String,
    /// Re-bake `.cluster_mesh` files even when they already exist.
    pub replace: bool,
    /// Uniform scale applied to every tree (SpeedTree FBX come in at author units).
    pub scale: f32,
    /// Trees per merged clump asset (0/1 = no clump baking). A clump merges K
    /// deterministic placements of the whole tree into ONE ClusterMesh per
    /// material — one BLAS per clump instead of K overlapping ones, so rays
    /// traverse the canopy through a real BVH instead of K AABB entries.
    pub clump: u32,
    /// Clump footprint radius in meters.
    pub clump_radius: f32,
}

/// Bake every `<Tree>.glb` under `cfg.glb_dir` to its own `.bsn`.
pub fn bake_speedtree(cfg: &SpeedTreeConfig) {
    let meshes_dir = cfg.out_dir.join("meshes");
    let textures_dir = cfg.out_dir.join("textures");
    fs::create_dir_all(&meshes_dir).expect("create meshes dir");
    fs::create_dir_all(&textures_dir).expect("create textures dir");

    let mut trees: Vec<PathBuf> = fs::read_dir(&cfg.glb_dir)
        .expect("read glb dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("glb")))
        .collect();
    trees.sort();
    println!("{} tree(s) in {}", trees.len(), cfg.glb_dir.display());

    for glb in &trees {
        let stem = crate::discovery::sanitize(glb.file_stem().unwrap().to_string_lossy().as_ref());
        bake_tree(&stem, glb, &meshes_dir, &textures_dir, cfg);
    }
}

/// Bake one tree glb: extract its textures, bake each primitive (+OMM on cutouts), write `<stem>.bsn`.
fn bake_tree(stem: &str, glb: &Path, meshes_dir: &Path, textures_dir: &Path, cfg: &SpeedTreeConfig) {
    let bytes = fs::read(glb).expect("read glb");
    let gltf = gltf::Gltf::from_slice(&bytes).expect("parse glb");
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, None, gltf.blob.clone()).expect("import buffers");

    // Extract only the images some material references as base-color/normal, tree-prefixed so trees
    // never clobber each other's textures in the shared dir.
    let image_files = extract_referenced(doc, &buffers, textures_dir, stem);

    // Cutmask per base-color image (foliage alpha histogram); drives both `AlphaMode::Mask` and OMM.
    let mut cutmask: HashMap<usize, bool> = HashMap::new();
    for mat in doc.materials() {
        if let Some(idx) = base_color_image(&mat) {
            cutmask.entry(idx).or_insert_with(|| {
                image_files
                    .get(&idx)
                    .and_then(|f| image::open(textures_dir.join(f)).ok())
                    .is_some_and(|im| img::classify_cutmask(&im.into_rgba8()))
            });
        }
    }

    let mut ctx = Tree {
        stem,
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir,
        cutmask: &cutmask,
        cfg,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        aabb: None,
        clump_sources: Vec::new(),
    };

    let scene = doc.default_scene().or_else(|| doc.scenes().next()).expect("glb has no scene");
    let root = Mat4::from_scale(bevy::math::Vec3::splat(cfg.scale));
    for node in scene.nodes() {
        walk(&node, root, &mut ctx);
    }

    let bsn = bsn::scene(stem, &ctx.entities);
    fs::write(cfg.out_dir.join(format!("{stem}.bsn")), bsn).expect("write .bsn");
    let height = ctx.aabb.map(|(mn, mx)| mx[1] - mn[1]).unwrap_or(0.0);
    println!(
        "  {stem}: {} entities, ~{height:.2}m tall",
        ctx.emitted,
    );
    if cfg.clump > 1 {
        bake_clump(stem, &ctx, cfg);
    }
}

/// Bake `<tree>_clump<K>.bsn`: every captured primitive merged across K
/// deterministic placements (golden-spiral spread + yaw/scale jitter).
fn bake_clump(tree: &str, ctx: &Tree, cfg: &SpeedTreeConfig) {
    let k = cfg.clump;
    let placements = clump_placements(tree, k, cfg.clump_radius);
    let mut entities = String::new();
    let mut emitted = 0;
    for (i, src) in ctx.clump_sources.iter().enumerate() {
        let stem = format!("{tree}_clump{k}_p{i}");
        let file = ctx.meshes_dir.join(format!("{stem}.cluster_mesh"));
        if cfg.replace || !file.exists() {
            let merged = merge_placements(&src.mesh, &placements);
            let cm = match ClusterMeshData::from_mesh_flat(&merged) {
                Ok(cm) => cm,
                Err(err) => {
                    eprintln!("  clump bake failed {stem}: {err:?}");
                    continue;
                }
            };
            let w = BufWriter::new(File::create(&file).expect("create .cluster_mesh"));
            write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
        }
        bsn::write_entity_trs(
            &mut entities,
            &cfg.asset_prefix,
            &stem,
            &src.fields,
            &src.name,
            [0.0; 3],
            [0.0, 0.0, 0.0, 1.0],
            [1.0; 3],
        );
        emitted += 1;
    }
    let name = format!("{tree}_clump{k}");
    let bsn = bsn::scene(&name, &entities);
    fs::write(cfg.out_dir.join(format!("{name}.bsn")), bsn).expect("write clump .bsn");
    println!("  {name}: {emitted} entities ({k} trees merged)");
}

/// K deterministic placements: golden-spiral spread over the footprint disc,
/// hashed yaw + scale jitter — stable per tree name across bakes.
fn clump_placements(seed_name: &str, k: u32, radius: f32) -> Vec<Mat4> {
    use bevy::math::{Quat, Vec3};
    let mut h: u32 = 2166136261;
    for b in seed_name.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let mut rand01 = move || {
        h = h.wrapping_mul(747796405).wrapping_add(2891336453);
        let w = ((h >> ((h >> 28) + 4)) ^ h).wrapping_mul(277803737);
        ((w >> 22) ^ w) as f32 / u32::MAX as f32
    };
    (0..k)
        .map(|i| {
            let r = radius * (((i as f32) + 0.5) / k as f32).sqrt();
            let theta = i as f32 * 2.399_963 + rand01() * 0.6;
            let yaw = rand01() * core::f32::consts::TAU;
            let scale = 0.85 + rand01() * 0.45;
            Mat4::from_scale_rotation_translation(
                Vec3::splat(scale),
                Quat::from_rotation_y(yaw),
                Vec3::new(r * theta.cos(), 0.0, r * theta.sin()),
            )
        })
        .collect()
}

/// Concatenate `src` under each placement into one mesh (positions by the full
/// transform, normals/tangents rotation-only — placements are uniform-scale).
fn merge_placements(
    src: &bevy::mesh::Mesh,
    placements: &[Mat4],
) -> bevy::mesh::Mesh {
    use bevy::math::Vec3;
    use bevy::mesh::{Indices, Mesh, VertexAttributeValues};

    let positions = match src.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(v)) => v.clone(),
        _ => panic!("clump source mesh has no positions"),
    };
    let normals = match src.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(v)) => Some(v.clone()),
        _ => None,
    };
    let uvs = match src.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(v)) => Some(v.clone()),
        _ => None,
    };
    let tangents = match src.attribute(Mesh::ATTRIBUTE_TANGENT) {
        Some(VertexAttributeValues::Float32x4(v)) => Some(v.clone()),
        _ => None,
    };
    let indices: Vec<u32> = match src.indices() {
        Some(Indices::U32(v)) => v.clone(),
        Some(Indices::U16(v)) => v.iter().map(|&i| i as u32).collect(),
        None => (0..positions.len() as u32).collect(),
    };

    let n = positions.len();
    let k = placements.len();
    let mut mp: Vec<[f32; 3]> = Vec::with_capacity(n * k);
    let mut mn: Vec<[f32; 3]> = Vec::with_capacity(if normals.is_some() { n * k } else { 0 });
    let mut mu: Vec<[f32; 2]> = Vec::with_capacity(if uvs.is_some() { n * k } else { 0 });
    let mut mt: Vec<[f32; 4]> = Vec::with_capacity(if tangents.is_some() { n * k } else { 0 });
    let mut mi: Vec<u32> = Vec::with_capacity(indices.len() * k);
    for m in placements {
        let (_, rot, _) = m.to_scale_rotation_translation();
        let base = mp.len() as u32;
        for p in &positions {
            mp.push(m.transform_point3(Vec3::from(*p)).to_array());
        }
        if let Some(v) = &normals {
            for x in v {
                mn.push((rot * Vec3::from(*x)).normalize_or_zero().to_array());
            }
        }
        if let Some(v) = &uvs {
            mu.extend_from_slice(v);
        }
        if let Some(v) = &tangents {
            for t in v {
                let r = (rot * Vec3::new(t[0], t[1], t[2])).normalize_or_zero();
                mt.push([r.x, r.y, r.z, t[3]]);
            }
        }
        mi.extend(indices.iter().map(|i| i + base));
    }

    let mut out = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::RENDER_WORLD,
    );
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, mp);
    if normals.is_some() {
        out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, mn);
    }
    if uvs.is_some() {
        out.insert_attribute(Mesh::ATTRIBUTE_UV_0, mu);
    }
    if tangents.is_some() {
        out.insert_attribute(Mesh::ATTRIBUTE_TANGENT, mt);
    }
    out.insert_indices(Indices::U32(mi));
    out
}

/// A primitive captured for clump merging: the world-baked mesh + everything
/// needed to re-emit its material entity and OMM.
struct ClumpSource {
    fields: String,
    name: String,
    mesh: bevy::mesh::Mesh,
}

struct Tree<'a> {
    stem: &'a str,
    buffers: &'a [gltf::buffer::Data],
    image_files: &'a HashMap<usize, String>,
    meshes_dir: &'a Path,
    cutmask: &'a HashMap<usize, bool>,
    cfg: &'a SpeedTreeConfig,
    /// `(mesh, prim, world) → owner stem` — identical instances bake once;
    /// distinct transforms bake distinct meshes (world is in the vertices).
    baked: HashMap<(usize, usize, [u32; 16]), Option<String>>,
    entities: String,
    emitted: usize,
    aabb: Option<([f32; 3], [f32; 3])>,
    clump_sources: Vec<ClumpSource>,
}

/// Accumulate the world transform and emit one entity per triangle primitive.
/// The FULL world transform (incl. the source's cm→m node scale) is baked
/// into the `.cluster_mesh` vertices — assets land at REAL-LIFE METERS with
/// identity `.bsn` transforms, so no consumer ever needs a unit factor.
fn walk(node: &gltf::Node, parent: Mat4, ctx: &mut Tree) {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let cut = base_color_image(&prim.material())
                .and_then(|i| ctx.cutmask.get(&i))
                .copied()
                .unwrap_or(false);
            // Dedup includes the world: with transforms baked into vertices,
            // a primitive instanced under two nodes is two distinct bakes.
            let world_bits: [u32; 16] = world.to_cols_array().map(f32::to_bits);
            let key = (mesh.index(), prim.index(), world_bits);
            let stem = match ctx.baked.get(&key) {
                Some(cached) => cached.clone(),
                None => {
                    let unique = ctx.baked.len();
                    let r = bake_primitive(&mesh, &prim, cut, world, unique, ctx);
                    ctx.baked.insert(key, r.clone());
                    r
                }
            };
            grow_aabb(&mut ctx.aabb, &prim, ctx.buffers, world);
            let Some(stem) = stem else { continue };
            let fields = material_fields(&prim.material(), cut, ctx);
            let name = format!("{}.{}", ctx.stem, prim.material().name().unwrap_or("part"));
            bsn::write_entity_trs(
                &mut ctx.entities,
                &ctx.cfg.asset_prefix,
                &stem,
                &fields,
                &name,
                [0.0; 3],
                [0.0, 0.0, 0.0, 1.0],
                [1.0; 3],
            );
            ctx.emitted += 1;
        }
    }
    for child in node.children() {
        walk(&child, world, ctx);
    }
}

/// Bake one primitive to `<tree>_m<mi>p<pi>[_wN].cluster_mesh`, attaching an
/// OMM when `cut`. `world` is baked into the vertices (real-life meters).
fn bake_primitive(
    mesh: &gltf::Mesh,
    prim: &gltf::Primitive,
    cut: bool,
    world: Mat4,
    unique: usize,
    ctx: &mut Tree,
) -> Option<String> {
    // First bake of a primitive keeps the legacy stem; further distinct
    // worlds get a suffix so they don't clobber each other's files.
    let base = format!("{}_m{}p{}", ctx.stem, mesh.index(), prim.index());
    let stem = if ctx.baked.keys().any(|(m, p, _)| (*m, *p) == (mesh.index(), prim.index())) {
        format!("{base}_w{unique}")
    } else {
        base
    };
    let file = ctx.meshes_dir.join(format!("{stem}.cluster_mesh"));

    let mut bevy_mesh = build_primitive_mesh(prim, ctx.buffers)?;
    bake_world_into_mesh(&mut bevy_mesh, world);
    if ctx.cfg.clump > 1 {
        let fields = material_fields(&prim.material(), cut, ctx);
        ctx.clump_sources.push(ClumpSource {
            fields,
            name: format!("{}.{}", ctx.stem, prim.material().name().unwrap_or("part")),
            mesh: bevy_mesh.clone(),
        });
    }
    if file.exists() && !ctx.cfg.replace {
        return Some(stem); // re-runs only re-emit the `.bsn`
    }

    let cm = match ClusterMeshData::from_mesh_flat(&bevy_mesh) {
        Ok(cm) => cm,
        Err(err) => {
            eprintln!("  bake failed {stem}: {err:?}");
            return None;
        }
    };
    let w = BufWriter::new(File::create(&file).expect("create .cluster_mesh"));
    write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
    Some(stem)
}

/// Inline `SolariMaterial` fields: base-color + normal textures and `Mask` for classified cutouts.
fn material_fields(material: &gltf::Material, cut: bool, ctx: &Tree) -> String {
    let mut f = String::new();
    if let Some(p) = base_color_image(material).and_then(|i| ctx.image_files.get(&i)) {
        let _ = write!(f, " base_color_texture: \"{}/textures/{p}\",", ctx.cfg.asset_prefix);
    }
    if let Some(p) = material.normal_texture().map(|t| t.texture().source().index()).and_then(|i| ctx.image_files.get(&i)) {
        let _ = write!(f, " normal_map_texture: \"{}/textures/{p}\",", ctx.cfg.asset_prefix);
    }
    if cut {
        let _ = write!(f, " alpha_mode: bevy_material::alpha::AlphaMode::Mask({}),", bsn::f(img::MASK_CUTOFF));
    }
    f
}

/// The `image` index a material samples for base color, if any.
fn base_color_image(material: &gltf::Material) -> Option<usize> {
    Some(material.pbr_metallic_roughness().base_color_texture()?.texture().source().index())
}

/// Bake `world` into the mesh attributes: positions by the full transform,
/// normals/tangents by rotation only (SpeedTree node scale is uniform).
fn bake_world_into_mesh(mesh: &mut bevy::mesh::Mesh, world: Mat4) {
    use bevy::mesh::{Mesh, VertexAttributeValues};
    let (_, rotation, _) = world.to_scale_rotation_translation();
    if let Some(VertexAttributeValues::Float32x3(positions)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_POSITION)
    {
        for p in positions.iter_mut() {
            *p = world.transform_point3(bevy::math::Vec3::from(*p)).to_array();
        }
    }
    if let Some(VertexAttributeValues::Float32x3(normals)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
    {
        for n in normals.iter_mut() {
            *n = (rotation * bevy::math::Vec3::from(*n)).normalize_or_zero().to_array();
        }
    }
    if let Some(VertexAttributeValues::Float32x4(tangents)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_TANGENT)
    {
        for t in tangents.iter_mut() {
            let rotated = rotation * bevy::math::Vec3::new(t[0], t[1], t[2]);
            let r = rotated.normalize_or_zero();
            *t = [r.x, r.y, r.z, t[3]];
        }
    }
}

/// Grow the running WORLD-space AABB from a primitive's positions (diagnostic height report);
/// `world` carries the node transform chain (incl. `cfg.scale`), so the reported size is true.
fn grow_aabb(aabb: &mut Option<([f32; 3], [f32; 3])>, prim: &gltf::Primitive, buffers: &[gltf::buffer::Data], world: Mat4) {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let Some(positions) = reader.read_positions() else { return };
    let (mut mn, mut mx) = aabb.unwrap_or(([f32::MAX; 3], [f32::MIN; 3]));
    for p in positions {
        let w = world.transform_point3(bevy::math::Vec3::from(p)).to_array();
        for k in 0..3 {
            mn[k] = mn[k].min(w[k]);
            mx[k] = mx[k].max(w[k]);
        }
    }
    *aabb = Some((mn, mx));
}

/// Extract every image referenced as a base-color or normal map to `<textures_dir>/<stem>_<name>.png`,
/// returning `image index → filename`. Embedded (GLB) images come from a buffer view.
fn extract_referenced(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    textures_dir: &Path,
    stem: &str,
) -> HashMap<usize, String> {
    let mut needed: HashSet<usize> = HashSet::new();
    for mat in doc.materials() {
        if let Some(i) = base_color_image(&mat) {
            needed.insert(i);
        }
        if let Some(s) = mat.normal_texture().map(|t| t.texture().source()) {
            needed.insert(s.index());
        }
    }
    let mut files = HashMap::new();
    for image in doc.images().filter(|im| needed.contains(&im.index())) {
        let idx = image.index();
        let name = format!("{stem}_{}.png", safe(image.name().unwrap_or(""), idx));
        if let gltf::image::Source::View { view, .. } = image.source() {
            let buf = &buffers[view.buffer().index()].0;
            let bytes = &buf[view.offset()..view.offset() + view.length()];
            let _ = fs::write(textures_dir.join(&name), bytes);
        }
        files.insert(idx, name);
    }
    files
}

/// A filesystem-safe image stem (SpeedTree's Blender names carry spaces / duplicated suffixes).
fn safe(name: &str, idx: usize) -> String {
    let clean: String = name
        .split(['(', '-']) // drop Blender's " (Image)-... (Image)" noise
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    let clean = clean.trim_matches('_').to_string();
    if clean.is_empty() { format!("img{idx}") } else { clean }
}
