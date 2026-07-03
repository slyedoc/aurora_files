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
use bevy::solari::geometry::{write_cluster_mesh_sync, ClusterMesh};

use crate::gltf::build_primitive_mesh;
use crate::{bsn, img, mesh};

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
    /// Erosion radius (texels) on the cutout mask before OMM baking. See [`mesh::DEFAULT_ERODE_PX`].
    pub erode_px: u32,
    /// Max OMM subdivision level. See [`mesh::DEFAULT_OMM_SUBDIV`].
    pub omm_subdiv: u32,
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
        textures_dir,
        meshes_dir,
        cutmask: &cutmask,
        cfg,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        omm_baked: 0,
        aabb: None,
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
        "  {stem}: {} entities, {} OMM cutouts, ~{height:.2}m tall",
        ctx.emitted, ctx.omm_baked,
    );
}

struct Tree<'a> {
    stem: &'a str,
    buffers: &'a [gltf::buffer::Data],
    image_files: &'a HashMap<usize, String>,
    textures_dir: &'a Path,
    meshes_dir: &'a Path,
    cutmask: &'a HashMap<usize, bool>,
    cfg: &'a SpeedTreeConfig,
    /// `(mesh, prim) → owner stem` so instanced primitives bake once.
    baked: HashMap<(usize, usize), Option<String>>,
    entities: String,
    emitted: usize,
    omm_baked: usize,
    aabb: Option<([f32; 3], [f32; 3])>,
}

/// Accumulate the world transform and emit one entity per triangle primitive.
fn walk(node: &gltf::Node, parent: Mat4, ctx: &mut Tree) {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let cut = base_color_image(&prim.material())
                .and_then(|i| ctx.cutmask.get(&i))
                .copied()
                .unwrap_or(false);
            let key = (mesh.index(), prim.index());
            let stem = match ctx.baked.get(&key) {
                Some(cached) => cached.clone(),
                None => {
                    let r = bake_primitive(&mesh, &prim, cut, ctx);
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
                translation.to_array(),
                rotation.to_array(),
                scale.to_array(),
            );
            ctx.emitted += 1;
        }
    }
    for child in node.children() {
        walk(&child, world, ctx);
    }
}

/// Bake one primitive to `<tree>_m<mi>p<pi>.cluster_mesh`, attaching an OMM when `cut`.
fn bake_primitive(mesh: &gltf::Mesh, prim: &gltf::Primitive, cut: bool, ctx: &mut Tree) -> Option<String> {
    let stem = format!("{}_m{}p{}", ctx.stem, mesh.index(), prim.index());
    let file = ctx.meshes_dir.join(format!("{stem}.cluster_mesh"));

    let bevy_mesh = build_primitive_mesh(prim, ctx.buffers)?;
    if file.exists() && !ctx.cfg.replace {
        return Some(stem); // re-runs only re-emit the `.bsn`
    }

    let mut cm = match ClusterMesh::try_from(&bevy_mesh) {
        Ok(cm) => cm,
        Err(err) => {
            eprintln!("  bake failed {stem}: {err:?}");
            return None;
        }
    };
    if cut {
        if let Some(rgba) = base_color_image(&prim.material())
            .and_then(|i| ctx.image_files.get(&i))
            .and_then(|f| image::open(ctx.textures_dir.join(f)).ok())
        {
            if mesh::attach_omm_rgba(&mut cm, &rgba.into_rgba8(), ctx.cfg.erode_px, ctx.cfg.omm_subdiv, &stem)
                .is_some()
            {
                ctx.omm_baked += 1;
            }
        }
    }
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
    if let Some(p) = material.normal_texture().and_then(|t| t.texture().source()).map(|s| s.index()).and_then(|i| ctx.image_files.get(&i)) {
        let _ = write!(f, " normal_map_texture: \"{}/textures/{p}\",", ctx.cfg.asset_prefix);
    }
    if cut {
        let _ = write!(f, " alpha_mode: bevy_material::alpha::AlphaMode::Mask({}),", bsn::f(img::MASK_CUTOFF));
    }
    f
}

/// The `image` index a material samples for base color, if any.
fn base_color_image(material: &gltf::Material) -> Option<usize> {
    Some(material.pbr_metallic_roughness().base_color_texture()?.texture().source()?.index())
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
        if let Some(s) = mat.normal_texture().and_then(|t| t.texture().source()) {
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
