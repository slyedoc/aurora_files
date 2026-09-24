//! glTF import path (Bistro): parse a `.glb`/`.gltf` with the `gltf` crate, walk the node
//! hierarchy for world transforms, bake each unique primitive `Mesh → .cluster_mesh`, extract the
//! embedded textures, and write a `.bsn` — the glTF analogue of [`crate::bake_scene`].
//!
//! Milestone: geometry + base-color/normal/metallic-roughness textures + alpha-mode + glass
//! transmission + OMM baking for alpha cutouts, plus the scalar/colour factors (base_color,
//! emissive, metallic, roughness, ior). Carrying the factors is what lets a fully UNTEXTURED
//! asset import correctly — see the hoverboard importer, which is texture-free by design so the
//! ray tracer shades it from the `GpuMaterial` struct with no sampler reads at all.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use aurora_cluster_mesh::{ClusterMeshData, write_cluster_mesh_sync};
use bevy::asset::RenderAssetUsages;
use bevy::math::{Mat4, Vec3};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

use crate::bsn;
use crate::mesh;

/// What an export lost, supplied per material because it cannot be derived from the file.
///
/// A custom shader's parameters have nowhere to go in glTF. The fantasy-city kit multiplies a
/// white mask by a URP `_BaseTexColorTint`; the export wrote `baseColorFactor: 0,0,0,0` and, for
/// one material, dropped the texture reference entirely. Where the texture is a plain mask the
/// tint was the ONLY colour information and no repair derived from the file can recover it, and a
/// material naming no texture has no alpha to cut with however the importer squints at it.
#[derive(Default, Clone, serde::Deserialize)]
#[serde(default)]
pub struct Fixups {
    /// Base-colour tint by material name, LINEAR, overriding the material's own factor.
    pub tints: HashMap<String, [f32; 4]>,
    /// Base-colour texture FILENAME by material name, for a material whose reference was lost.
    /// Resolved against the texture override directory, so the file ships with the bake.
    pub textures: HashMap<String, String>,
}

/// Per-asset knobs for the glTF importer.
pub struct GltfConfig {
    /// Source `.glb`/`.gltf` (self-contained GLB, or `.gltf` with sibling `.bin`/textures).
    pub gltf_path: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn` are written under it).
    pub out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference baked meshes/textures.
    pub asset_prefix: String,
    /// `.bsn` scene name and output filename stem.
    pub scene_name: String,
    /// Re-bake `.cluster_mesh` files even when they already exist (overwrite).
    pub replace: bool,
    /// Extra component patch lines emitted on the scene ROOT entity (before its `Children`). Used by
    /// [`bake_gltf_hierarchy`] to stamp e.g. an animation marker on the root. Empty for none.
    pub root_components: String,
    /// Per-scene fallback for emissive magnitude (nits), keyed on material name, used only when a
    /// material ships no `KHR_materials_emissive_strength`. `None` keeps the glTF value (×1).
    pub emissive_nits: Option<fn(&str) -> f32>,
    /// Per-material repairs for things the export did not carry. See [`Fixups`].
    pub fixups: Fixups,
    /// Directory of replacement textures, matched by the filename the embedded image would get.
    ///
    /// For a kit whose glb was exported lossily beside an intact texture pack. Anything not found
    /// there falls back to the embedded copy, so a partial directory is fine.
    pub textures: Option<PathBuf>,
    /// Also bake each primitive's collision to `meshes/<stem>.collider` and name it from the
    /// entity that carries the mesh. Off by default: collision is dead weight in a scene
    /// nothing walks around in, and the file is a second copy of the geometry.
    pub colliders: bool,
    /// [`bake_gltf_per_group`] only: how deep to descend before calling a node a "group".
    ///
    /// `1` (the default) splits on top-level scene nodes, which is right for a kit whose roots ARE
    /// the props. A kit filed by CATEGORY — `crafting`, `furniture`, each holding dozens of props —
    /// wants `2`, or every category bakes as one immovable blob. Only the author knows which level
    /// is a thing you would pick up and place, so it is a knob rather than a guess.
    pub group_depth: usize,
}

impl Default for GltfConfig {
    fn default() -> Self {
        Self {
            gltf_path: PathBuf::new(),
            out_dir: PathBuf::new(),
            asset_prefix: String::new(),
            scene_name: String::new(),
            replace: false,
            root_components: String::new(),
            emissive_nits: None,
            fixups: Fixups::default(),
            textures: None,
            colliders: false,
            group_depth: 1,
        }
    }
}

/// Bake the scene described by `cfg`: extract textures, bake each unique primitive's
/// `.cluster_mesh`, and write the `.bsn`.
pub fn bake_gltf_scene(cfg: &GltfConfig) {
    let meshes_dir = cfg.out_dir.join("meshes");
    let textures_dir = cfg.out_dir.join("textures");
    fs::create_dir_all(&meshes_dir).expect("create meshes dir");
    fs::create_dir_all(&textures_dir).expect("create textures dir");

    println!("loading {}", cfg.gltf_path.display());
    let bytes = fs::read(&cfg.gltf_path).expect("read gltf");
    let gltf = gltf::Gltf::from_slice(&bytes).expect("parse gltf");
    let base = cfg.gltf_path.parent();
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, base, gltf.blob.clone()).expect("import buffers");
    println!(
        "{} nodes, {} meshes, {} materials, {} textures",
        doc.nodes().count(),
        doc.meshes().count(),
        doc.materials().count(),
        doc.textures().count(),
    );
    lint_materials(doc, cfg.emissive_nits).print();

    // Extract every embedded/sourced image once (raw bytes, no re-encode) → `image index → file`.
    let image_files = extract_images(
        doc,
        &buffers,
        base,
        &textures_dir,
        cfg.textures.as_deref(),
        &cfg.fixups,
    );
    println!(
        "extracted {} textures -> {}",
        image_files.len(),
        textures_dir.display()
    );

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        textures_dir: &textures_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        fixups: &cfg.fixups,
        colliders: cfg.colliders,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        proxies: 0,
        collider_count: 0,
        collided: HashMap::new(),
        failed_count: 0,
        bounds: None,
        repairs: Default::default(),
    };

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .expect("gltf has no scene");
    for node in scene.nodes() {
        walk(&node, Mat4::IDENTITY, &mut ctx);
    }

    let bsn = bsn::scene(&cfg.scene_name, &ctx.entities);
    let bsn_path = cfg.out_dir.join(format!("{}.bsn", cfg.scene_name));
    fs::write(&bsn_path, bsn).expect("write .bsn");

    ctx.report_repairs();
    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count,
        ctx.failed_count,
        meshes_dir.display()
    );
    if ctx.colliders {
        println!(
            "baked {} colliders -> {}",
            ctx.collider_count,
            meshes_dir.display()
        );
    }
    println!("wrote {} entities -> {}", ctx.emitted, bsn_path.display());
}

/// Bake the glTF as **one `.bsn` per top-level scene node** PLUS a flat master layout scene — for a
/// KitBash-style kit where each root group is a self-contained building/prop. Textures are extracted
/// and `.cluster_mesh` files baked once into shared `meshes/`/`textures/` (deduplicated across groups
/// by mesh+primitive index, so pieces sharing geometry bake once). Each group's `.bsn` is centered at
/// its **own origin** — the root group's layout transform is dropped — so every building is a
/// reusable prop you place yourself. A combined `<scene_name>.bsn` is also written: the whole set
/// flat, every primitive at its authored world position, reconstructing the city.
pub fn bake_gltf_per_group(cfg: &GltfConfig) {
    let meshes_dir = cfg.out_dir.join("meshes");
    let textures_dir = cfg.out_dir.join("textures");
    fs::create_dir_all(&meshes_dir).expect("create meshes dir");
    fs::create_dir_all(&textures_dir).expect("create textures dir");

    println!("loading {}", cfg.gltf_path.display());
    let bytes = fs::read(&cfg.gltf_path).expect("read gltf");
    let gltf = gltf::Gltf::from_slice(&bytes).expect("parse gltf");
    let base = cfg.gltf_path.parent();
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, base, gltf.blob.clone()).expect("import buffers");
    println!(
        "{} nodes, {} meshes, {} materials, {} textures",
        doc.nodes().count(),
        doc.meshes().count(),
        doc.materials().count(),
        doc.textures().count(),
    );
    lint_materials(doc, cfg.emissive_nits).print();

    let image_files = extract_images(
        doc,
        &buffers,
        base,
        &textures_dir,
        cfg.textures.as_deref(),
        &cfg.fixups,
    );
    println!(
        "extracted {} textures -> {}",
        image_files.len(),
        textures_dir.display()
    );

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        textures_dir: &textures_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        fixups: &cfg.fixups,
        colliders: cfg.colliders,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        proxies: 0,
        collider_count: 0,
        collided: HashMap::new(),
        failed_count: 0,
        bounds: None,
        repairs: Default::default(),
    };

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .expect("gltf has no scene");

    let mut used_names: HashSet<String> = HashSet::new();
    let mut groups_written = 0usize;
    let mut manifest: Vec<(String, [f32; 3], [f32; 3])> = Vec::new();
    // Descend `group_depth` levels before treating a node as a placeable prop. Depth 1 is the
    // scene's own roots; a category-filed kit needs 2.
    let mut groups: Vec<gltf::Node> = scene.nodes().collect();
    for _ in 1..cfg.group_depth.max(1) {
        groups = groups
            .iter()
            .flat_map(|node| {
                let kids: Vec<_> = node.children().collect();
                // A node with no children IS the leaf; keep it rather than dropping it, so a kit
                // with uneven nesting does not silently lose its shallow props.
                if kids.is_empty() {
                    vec![node.clone()]
                } else {
                    kids
                }
            })
            .collect();
    }

    for node in &groups {
        // Center each group at its own origin: walk with `parent = inverse(local)` so the group's
        // own layout transform cancels and its geometry is emitted relative to the group origin — a
        // reusable prop rather than one pinned to its place in the assembled set.
        //
        // At depth > 1 that inverse only cancels the node's OWN transform, not its ancestors'.
        // That is deliberate: the ancestors are the category rows the kit was laid out in, and
        // their translation is exactly what a placeable prop must not inherit. Walking from the
        // node with `inverse(local)` drops the whole chain above it, because nothing above is ever
        // multiplied in.
        let local = Mat4::from_cols_array_2d(&node.transform().matrix());
        let parent = local.inverse();

        ctx.entities.clear();
        ctx.bounds = None;
        let before = ctx.emitted;
        walk(node, parent, &mut ctx);
        if ctx.emitted == before {
            continue; // group has no triangle primitives — nothing to write
        }

        let stem = group_scene_name(node.name(), node.index(), &mut used_names);
        let bsn = bsn::scene(&stem, &ctx.entities);
        let bsn_path = cfg.out_dir.join(format!("{stem}.bsn"));
        fs::write(&bsn_path, bsn).expect("write .bsn");
        if let Some((lo, hi)) = ctx.bounds {
            manifest.push((stem, lo.to_array(), hi.to_array()));
        }
        groups_written += 1;
    }

    // Master layout scene (`<scene_name>.bsn`): the whole set FLAT — every primitive a direct child
    // of one root, placed by its full WORLD transform (no group nodes kept). Reconstructs the city in
    // its authored positions. It must be flat, not a `root → group → parts` hierarchy: an empty group
    // node carries no mesh, so with `TransformPlugin` disabled it never gets a `GlobalTransform` (nor
    // a transform-table slot), and its children's transforms wouldn't propagate through it. The root
    // is the (parentless) scene-instance entity, which DOES get one — so a flat root → parts tree
    // works. Meshes/textures are already baked above; this walk only re-emits entities (cache hits).
    ctx.entities.clear();
    let before_master = ctx.emitted;
    for node in scene.nodes() {
        walk(&node, Mat4::IDENTITY, &mut ctx);
    }
    let master_entities = ctx.emitted - before_master;
    let master = bsn::scene(&cfg.scene_name, &ctx.entities);
    let master_path = cfg.out_dir.join(format!("{}.bsn", cfg.scene_name));
    fs::write(&master_path, master).expect("write master .bsn");

    ctx.report_repairs();
    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count,
        ctx.failed_count,
        meshes_dir.display()
    );
    if ctx.colliders {
        println!(
            "baked {} colliders -> {}",
            ctx.collider_count,
            meshes_dir.display()
        );
    }
    // The KIT MANIFEST. A bevy `AssetServer` cannot enumerate — there is no "list the .bsn files"
    // call, and a shipped build has no directory to scan anyway. So anything that wants to SHOW
    // what a kit contains (a palette, a placement tool, an XR shelf) needs an index, and bake time
    // is the only place that knows the answer for free.
    //
    // The extent is the part worth having. Laying out a shelf of live prop instances means scaling
    // each to a uniform cell, and without a size up front a palette has to instantiate every prop,
    // wait for it to stream, measure it, and only then lay out — which is the difference between a
    // shelf that appears and one that settles.
    let mut index = String::from(
        "// GENERATED by the prop importer. One entry per placeable .bsn in this kit.\n\
         // `min`/`max` are the prop's own AABB in metres, at its baked origin.\n(\n    props: [\n",
    );
    manifest.sort_by(|a, b| a.0.cmp(&b.0));
    for (stem, lo, hi) in &manifest {
        let _ = writeln!(
            index,
            "        (name: \"{stem}\", bsn: \"{}/{stem}.bsn\", min: ({:.3}, {:.3}, {:.3}), max: ({:.3}, {:.3}, {:.3})),",
            cfg.asset_prefix, lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]
        );
    }
    index.push_str("    ],\n)\n");
    let index_path = cfg.out_dir.join(format!("{}.kit.ron", cfg.scene_name));
    fs::write(&index_path, index).expect("write kit manifest");

    println!(
        "wrote {} group .bsn files + a {}-entity master {} + {} -> {}",
        groups_written,
        master_entities,
        master_path.display(),
        index_path.display(),
        cfg.out_dir.display()
    );
}

/// Scene name + filename stem for a top-level group node: the node name with a trailing `_grp`
/// dropped and non-identifier chars sanitized, made unique across the kit. Unnamed (or empty) nodes
/// fall back to `group_<index>`.
fn group_scene_name(name: Option<&str>, index: usize, used: &mut HashSet<String>) -> String {
    let raw = name.unwrap_or("");
    let raw = raw.strip_suffix("_grp").unwrap_or(raw);
    let mut base = crate::discovery::sanitize(raw);
    if base == "mesh" {
        base = format!("group_{index}"); // `sanitize` returns "mesh" for empty/all-invalid names
    }
    let mut stem = base.clone();
    let mut n = 1;
    while !used.insert(stem.clone()) {
        stem = format!("{base}_{n}");
        n += 1;
    }
    stem
}

/// Repairs for materials whose export does not say what the author meant.
///
/// Both cases below come from the same class of bug: a DCC material instance whose parameters
/// were left at their slot defaults, exported literally. They are worth correcting in the
/// importer rather than in the source, because the correction is decidable from the data — a
/// material cannot have meant either of these — and because every kit exported that way will
/// arrive the same shape.
#[derive(Default)]
struct Repairs {
    /// Base-colour texture image index → is its alpha a genuine binary cutmask. Decoding is
    /// expensive and a kit shares a handful of atlases across hundreds of materials, so the
    /// answer is cached per image the way the OBJ path caches it per texture path.
    cutmask: HashMap<String, bool>,
    /// Materials whose zeroed base-colour factor was ignored, and whose alpha mode was promoted.
    zeroed: Vec<String>,
    promoted: Vec<String>,
}

impl Ctx<'_> {
    /// The base-colour texture FILENAME for a material, fixup first.
    ///
    /// Every consumer goes through here — the emitted `.bsn`, the cutmask test and the OMM bake —
    /// so a material's texture, its alpha mode and its micromap cannot end up disagreeing about
    /// which image they are talking about.
    fn base_color_file(&self, material: &gltf::Material) -> Option<String> {
        if let Some(file) = material.name().and_then(|n| self.fixups.textures.get(n)) {
            return Some(file.clone());
        }
        let info = material.pbr_metallic_roughness().base_color_texture()?;
        self.image_files
            .get(&info.texture().source().index())
            .cloned()
    }

    /// The alpha cutoff this material should be treated as having, or `None` for opaque.
    ///
    /// `Mask` is taken at its word. `Opaque` is NOT, when the base-colour texture's alpha is a
    /// genuine binary cutmask: a texture that is a third holes and two thirds solid is a foliage
    /// card, and declaring it opaque renders the card as a black quad. Blend is left alone —
    /// that is a real choice with a different renderer path.
    fn alpha_cutout(&self, material: &gltf::Material) -> Option<f32> {
        match material.alpha_mode() {
            gltf::material::AlphaMode::Mask => {
                return Some(material.alpha_cutoff().unwrap_or(0.5));
            }
            gltf::material::AlphaMode::Blend => return None,
            gltf::material::AlphaMode::Opaque => {}
        }
        let file = self.base_color_file(material)?;
        if let Some(&known) = self.repairs.borrow().cutmask.get(&file) {
            return known.then_some(0.5);
        }
        let cutmask = image::open(self.textures_dir.join(&file))
            .map(|img| crate::img::classify_cutmask(&img.into_rgba8()))
            .unwrap_or(false);
        let mut repairs = self.repairs.borrow_mut();
        repairs.cutmask.insert(file, cutmask);
        if cutmask {
            let name = material.name().unwrap_or("<unnamed>").to_string();
            if !repairs.promoted.contains(&name) {
                repairs.promoted.push(name);
            }
        }
        cutmask.then_some(0.5)
    }

    /// Note a material whose base-colour factor was zeroed against a texture, once.
    fn note_zeroed(&self, material: &gltf::Material) {
        let name = material.name().unwrap_or("<unnamed>").to_string();
        let mut repairs = self.repairs.borrow_mut();
        if !repairs.zeroed.contains(&name) {
            repairs.zeroed.push(name);
        }
    }

    fn report_repairs(&self) {
        let repairs = self.repairs.borrow();
        if !repairs.zeroed.is_empty() {
            println!(
                "  repaired {} material(s) whose base colour factor was 0 beside a texture: {}",
                repairs.zeroed.len(),
                repairs.zeroed.join(", ")
            );
        }
        if !repairs.promoted.is_empty() {
            println!(
                "  promoted {} opaque material(s) to alpha Mask (cutmask texture): {}",
                repairs.promoted.len(),
                repairs.promoted.join(", ")
            );
        }
    }
}

struct Ctx<'a> {
    buffers: &'a [gltf::buffer::Data],
    image_files: &'a HashMap<usize, String>,
    meshes_dir: &'a Path,
    textures_dir: &'a Path,
    asset_prefix: &'a str,
    replace: bool,
    emissive_nits: Option<fn(&str) -> f32>,
    fixups: &'a Fixups,
    colliders: bool,
    /// `(mesh index, primitive index) → owner stem`, so shared meshes bake once and instance nodes
    /// reuse the baked `.cluster_mesh`. `None` marks a primitive whose bake failed (entities skipped).
    baked: HashMap<(usize, usize), Option<String>>,
    entities: String,
    emitted: usize,
    baked_count: usize,
    /// Nodes swapped for a `.bsn` reference (see [`bsn_proxy`]).
    proxies: usize,
    /// Per-material export repairs, behind a `RefCell` because the emit path only has `&Ctx`.
    repairs: core::cell::RefCell<Repairs>,
    /// `.collider` files written this run.
    collider_count: usize,
    /// `(mesh index, primitive index) → collider stem`, separate from `baked` on purpose:
    /// whether a primitive needs collision is a property of the NODE, and the first node to
    /// reference a shared mesh may be one that opts out.
    collided: HashMap<(usize, usize), Option<String>>,
    failed_count: usize,
    /// World-space AABB of everything emitted since the last reset, for the kit manifest.
    bounds: Option<(Vec3, Vec3)>,
}

impl Ctx<'_> {
    /// Grow the running AABB by a primitive's own bounds, placed by `world`.
    ///
    /// Taken from the POSITION accessor's `min`/`max`, which glTF REQUIRES for positions — so this
    /// costs no buffer reads at all, just eight corners through a matrix. Worth doing at bake time
    /// because the alternative is a palette that has to load every prop before it can lay them out.
    fn grow(&mut self, prim: &gltf::Primitive, world: Mat4) {
        let Some(accessor) = prim.get(&gltf::Semantic::Positions) else {
            return;
        };
        let read = |value: Option<gltf::json::Value>| -> Option<Vec3> {
            let array = value?;
            let array = array.as_array()?;
            if array.len() < 3 {
                return None;
            }
            Some(Vec3::new(
                array[0].as_f64()? as f32,
                array[1].as_f64()? as f32,
                array[2].as_f64()? as f32,
            ))
        };
        let (Some(lo), Some(hi)) = (read(accessor.min()), read(accessor.max())) else {
            return;
        };
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            let point = world.transform_point3(corner);
            self.bounds = Some(match self.bounds {
                None => (point, point),
                Some((lo, hi)) => (lo.min(point), hi.max(point)),
            });
        }
    }
}

/// Recurse the node hierarchy, accumulating the world transform; emit one `.bsn` entity per
/// triangle primitive, placed by the node's world `Transform`.
fn walk(node: &gltf::Node, parent: Mat4, ctx: &mut Ctx) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;

    if let Some(mesh) = node.mesh() {
        let (scale, rotation, translation) = world.to_scale_rotation_translation();
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let key = (mesh.index(), prim.index());
            // Bake the owner mesh once (cache by mesh+primitive index); instances reuse the stem.
            let stem = if let Some(cached) = ctx.baked.get(&key) {
                cached.clone()
            } else {
                let result = bake_primitive(&mesh, &prim, ctx);
                ctx.baked.insert(key, result.clone());
                result
            };
            let Some(stem) = stem else { continue };
            ctx.grow(&prim, world);

            let fields = material_fields(&prim.material(), ctx);
            // Entity name = `<node>.<material>` (bevy's glTF convention) so entities are identifiable
            // in viewers/tools and match the source nodes.
            let node_name = node
                .name()
                .map(str::to_string)
                .unwrap_or_else(|| format!("node{}", node.index()));
            let name = match prim.material().name() {
                Some(mat) => format!("{node_name}.{mat}"),
                None => format!("{node_name}#{}", prim.index()),
            };
            // Collision, when the asset is baked with it and this node has not opted out. The
            // flat path needs this as much as the hierarchy one: a kit of props is exactly the
            // thing a character walks into, and `--colliders` silently did nothing here.
            let collider = (ctx.colliders && collides(node))
                .then(|| ensure_collider(&mesh, &prim, ctx))
                .flatten();
            bsn::write_entity_trs(
                &mut ctx.entities,
                ctx.asset_prefix,
                &stem,
                &fields,
                &name,
                translation.to_array(),
                rotation.to_array(),
                scale.to_array(),
                collider.as_deref(),
            );
            ctx.emitted += 1;
        }
    }

    for child in node.children() {
        walk(&child, world, ctx);
    }
}

/// Bake the glTF **preserving the node hierarchy** — the animation-capable analogue of
/// [`bake_gltf_scene`]. Instead of flattening every primitive to a world transform, this emits a
/// nested `Children[]` tree: each node is one entity carrying its `Name` + LOCAL `Transform` (and,
/// if it has geometry, `Mesh3d` + inline `RaytracingMaterial3d`). Keeping the tree + the Names
/// is what lets a runtime `AnimationPlayer` bind clips to nodes by name-path (`AnimationTargetId`)
/// and drive them — the GPU transform table propagates an animated parent to its mesh children.
/// Meshes/textures bake into shared `meshes/`/`textures/`, deduped by (mesh, primitive) index.
pub fn bake_gltf_hierarchy(cfg: &GltfConfig) {
    let meshes_dir = cfg.out_dir.join("meshes");
    let textures_dir = cfg.out_dir.join("textures");
    fs::create_dir_all(&meshes_dir).expect("create meshes dir");
    fs::create_dir_all(&textures_dir).expect("create textures dir");

    println!("loading {}", cfg.gltf_path.display());
    let bytes = fs::read(&cfg.gltf_path).expect("read gltf");
    let gltf = gltf::Gltf::from_slice(&bytes).expect("parse gltf");
    let base = cfg.gltf_path.parent();
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, base, gltf.blob.clone()).expect("import buffers");
    println!(
        "{} nodes, {} meshes, {} materials, {} textures",
        doc.nodes().count(),
        doc.meshes().count(),
        doc.materials().count(),
        doc.textures().count(),
    );
    lint_materials(doc, cfg.emissive_nits).print();

    let image_files = extract_images(
        doc,
        &buffers,
        base,
        &textures_dir,
        cfg.textures.as_deref(),
        &cfg.fixups,
    );
    println!(
        "extracted {} textures -> {}",
        image_files.len(),
        textures_dir.display()
    );

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        textures_dir: &textures_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        fixups: &cfg.fixups,
        colliders: cfg.colliders,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        proxies: 0,
        collider_count: 0,
        collided: HashMap::new(),
        failed_count: 0,
        bounds: None,
        repairs: Default::default(),
    };

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .expect("gltf has no scene");

    let mut root_children = String::new();
    for node in scene.nodes() {
        emit_node(&node, &mut ctx, &mut root_children, 1);
    }

    let bsn = bsn::scene_with_root(&cfg.scene_name, &cfg.root_components, &root_children);
    let bsn_path = cfg.out_dir.join(format!("{}.bsn", cfg.scene_name));
    fs::write(&bsn_path, bsn).expect("write .bsn");

    ctx.report_repairs();
    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count,
        ctx.failed_count,
        meshes_dir.display()
    );
    if ctx.colliders {
        println!(
            "baked {} colliders -> {}",
            ctx.collider_count,
            meshes_dir.display()
        );
    }
    if ctx.colliders {
        println!(
            "baked {} colliders ({} primitives collide) -> {}",
            ctx.collider_count,
            ctx.collided.values().filter(|s| s.is_some()).count(),
            meshes_dir.display()
        );
    }
    println!(
        "wrote {} node entities (hierarchy) -> {}",
        ctx.emitted,
        bsn_path.display()
    );
}

/// Emit one node as a `.bsn` entity block (comma-terminated) at `depth`, recursing into children.
/// Every node carries a `Name` (the animation binds to it) + its LOCAL `Transform`. A single-primitive
/// mesh is inlined on the node; extra primitives and the node's glTF children become nested `Children`.
/// A node tagged in Blender with a `bsn` custom property is a PROXY: something
/// you can see and place in the viewport (an empty, a box, whatever) that stands
/// in for an asset baked elsewhere. The importer emits a scene reference at the
/// proxy's transform and drops its geometry.
///
/// ```text
/// empty.  ["bsn"] = "speedtree/White_Oak.bsn"
/// ```
///
/// Blender writes object custom properties into the glTF node's `extras`
/// (`export_extras=True`), so placement stays in the .blend where you can see it
/// while the asset itself is baked once and shared.
fn bsn_proxy(node: &gltf::Node) -> Option<String> {
    let extras = node.extras().as_ref()?;
    let value: serde_json::Value = serde_json::from_str(extras.get()).ok()?;
    let path = value.get("bsn")?.as_str()?;
    (!path.is_empty()).then(|| path.replace('"', "'"))
}

/// Does this node want collision? Default yes (with `--colliders`); a Blender object carrying
/// `collide = 0` opts out. Water, ceilings and light fixtures are the usual ones: a collider on
/// the pool surface has agents walking on water, and a coffered ceiling 11 m up is voxelisation
/// cost for a surface nothing can reach.
fn collides(node: &gltf::Node) -> bool {
    let Some(extras) = node.extras().as_ref() else {
        return true;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(extras.get()) else {
        return true;
    };
    match value.get("collide") {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(1.0) != 0.0,
        _ => true,
    }
}

/// `.collider`: `ACOL`, version, vertex count, triangle count (u32 LE), then the positions
/// (f32 LE x 3) and the triangles (u32 LE x 3). Read by `bevy_aurora::collision`.
fn write_collider(mesh: &Mesh, path: &Path) -> Option<usize> {
    let bevy::mesh::VertexAttributeValues::Float32x3(positions) =
        mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    // Non-indexed primitives are legal glTF: the triangles are consecutive vertices.
    let indices: Vec<u32> = match mesh.indices() {
        Some(Indices::U16(i)) => i.iter().map(|&i| i as u32).collect(),
        Some(Indices::U32(i)) => i.clone(),
        _ => (0..positions.len() as u32).collect(),
    };
    let triangles = indices.len() / 3;
    if triangles == 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(16 + positions.len() * 12 + triangles * 12);
    bytes.extend_from_slice(b"ACOL");
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(triangles as u32).to_le_bytes());
    for v in positions.iter().flatten() {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    for i in indices.chunks_exact(3).flatten() {
        bytes.extend_from_slice(&i.to_le_bytes());
    }
    fs::write(path, bytes).ok()?;
    Some(triangles)
}

/// Bake this primitive's collision if it is not on disk already, and return its stem.
///
/// Positions come from the same `build_primitive_mesh` the render bake uses, so collision is
/// the render geometry in the node's own local space — the entity's `Transform` places both.
/// That is only the right shape because these are ARCHITECTURAL bakes; a foliage model wants a
/// separate collision hull, which is why the WoW importer reads `.phys.obj` instead.
fn ensure_collider(mesh: &gltf::Mesh, prim: &gltf::Primitive, ctx: &mut Ctx) -> Option<String> {
    let key = (mesh.index(), prim.index());
    if let Some(cached) = ctx.collided.get(&key) {
        return cached.clone();
    }
    let stem = format!("mesh{}_{}", mesh.index(), prim.index());
    let file = ctx.meshes_dir.join(format!("{stem}.collider"));
    let result = if file.exists() && !ctx.replace {
        Some(stem)
    } else if build_primitive_mesh(prim, ctx.buffers)
        .and_then(|m| write_collider(&m, &file))
        .is_some()
    {
        ctx.collider_count += 1;
        Some(stem)
    } else {
        None
    };
    ctx.collided.insert(key, result.clone());
    result
}

fn emit_node(node: &gltf::Node, ctx: &mut Ctx, out: &mut String, depth: usize) {
    let pad = "    ".repeat(depth);
    let (t, r, s) = node.transform().decomposed();

    // Name — MUST match the anim glb's node name (same Blender export) so the runtime's
    // `AnimationTargetId::from_names` resolves this entity. Unnamed nodes fall back to `node<idx>`.
    let name = node
        .name()
        .map(|n| n.replace('"', "'"))
        .unwrap_or_else(|| format!("node{}", node.index()));

    // Triangle primitives of this node's mesh (baked once, cached by (mesh, prim) index).
    let mut prims: Vec<String> = Vec::new();
    if let Some(mesh) = node.mesh() {
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let key = (mesh.index(), prim.index());
            let stem = if let Some(cached) = ctx.baked.get(&key) {
                cached.clone()
            } else {
                let result = bake_primitive(&mesh, &prim, ctx);
                ctx.baked.insert(key, result.clone());
                result
            };
            if let Some(stem) = stem {
                prims.push(stem);
            }
        }
    }
    let mat_fields = node
        .mesh()
        .and_then(|m| {
            m.primitives()
                .find(|p| p.mode() == gltf::mesh::Mode::Triangles)
        })
        .map(|p| material_fields(&p.material(), ctx))
        .unwrap_or_default();

    let _ = write!(out, "{pad}bevy_ecs::name::Name(\"{name}\")\n");
    let _ = write!(
        out,
        "{pad}bevy_transform::components::transform::Transform {{ \
         translation: glam::Vec3 {{ x: {}, y: {}, z: {} }}, \
         rotation: glam::Quat {{ x: {}, y: {}, z: {}, w: {} }}, \
         scale: glam::Vec3 {{ x: {}, y: {}, z: {} }} }}\n",
        bsn::f(t[0]),
        bsn::f(t[1]),
        bsn::f(t[2]),
        bsn::f(r[0]),
        bsn::f(r[1]),
        bsn::f(r[2]),
        bsn::f(r[3]),
        bsn::f(s[0]),
        bsn::f(s[1]),
        bsn::f(s[2]),
    );

    // A proxy stands in for a separately baked scene: the name and transform above
    // are exactly what it needs, so emit the reference and stop -- its own geometry
    // is only a viewport stand-in and nothing of it is baked.
    if let Some(bsn_path) = bsn_proxy(node) {
        let _ = write!(
            out,
            "{pad}bevy_scene::scene_patch::ScenePatchInstance(\"{bsn_path}\"),\n\n"
        );
        ctx.proxies += 1;
        return;
    }
    // A skinned node names its joint palette; aurora's `resolve_skin_joints` turns the names
    // into a real `SkinnedMesh` once they resolve in the spawned subtree, taking the inverse
    // bind poses from the joints' own bind transforms. Order must match the glTF skin's joint
    // list, because that is what JOINT_INDEX indexes.
    let skin_joints = node.skin().map(|skin| {
        skin.joints()
            .map(|j| {
                j.name()
                    .map(|n| n.replace('"', "'"))
                    .unwrap_or_else(|| format!("node{}", j.index()))
            })
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ")
    });

    // Collision, when the asset is baked with it and this node has not opted out. The
    // component names the geometry only: `bevy_aurora::collision` has no physics engine, and a
    // scene that named one could not be opened by anything that did not have it.
    let collide = ctx.colliders && collides(node);

    // Single-primitive mesh: inline it on the node (the common case). Extra primitives drop to
    // identity-transform children below so this entity keeps the node's Name for animation.
    if let Some(stem) = prims.first() {
        let _ = write!(
            out,
            "{pad}bevy_mesh::components::Mesh3d(\"{}/meshes/{stem}.cluster_mesh\")\n\
             {pad}bevy_aurora::material::AuroraMaterial3d(bevy_aurora::material::AuroraMaterial {{{mat_fields}}})\n",
            ctx.asset_prefix,
        );
        if collide
            && let Some(mesh) = node.mesh()
            && let Some(prim) = mesh
                .primitives()
                .find(|p| p.mode() == gltf::mesh::Mode::Triangles)
            && let Some(col) = ensure_collider(&mesh, &prim, ctx)
        {
            let _ = write!(
                out,
                "{pad}bevy_aurora::collision::CollisionMesh(\"{}/meshes/{col}.collider\")\n",
                ctx.asset_prefix,
            );
        }
        if let Some(joints) = &skin_joints {
            let _ = write!(
                out,
                "{pad}bevy_aurora::skinning::SkinJointsByName([{joints}])\n"
            );
        }
        ctx.emitted += 1;
    } else {
        ctx.emitted += 1; // empty/camera anchor node (Name + Transform only)
    }

    // Nested children: extra primitives (index > 0) as identity entities, then the glTF child nodes.
    let mut kids = String::new();
    for (i, stem) in prims.iter().enumerate().skip(1) {
        let cpad = "    ".repeat(depth + 1);
        let mat = node
            .mesh()
            .and_then(|m| m.primitives().nth(i))
            .map(|p| material_fields(&p.material(), ctx))
            .unwrap_or_default();
        // Extra primitives of a skinned mesh share the node's skin.
        let skin = match &skin_joints {
            Some(joints) => {
                format!("{cpad}bevy_aurora::skinning::SkinJointsByName([{joints}])\n")
            }
            None => String::new(),
        };
        let col = match collide {
            true => node
                .mesh()
                .and_then(|m| m.primitives().nth(i).map(|p| (m, p)))
                .and_then(|(m, p)| ensure_collider(&m, &p, ctx))
                .map(|col| {
                    format!(
                        "{cpad}bevy_aurora::collision::CollisionMesh(\"{}/meshes/{col}.collider\")\n",
                        ctx.asset_prefix,
                    )
                })
                .unwrap_or_default(),
            false => String::new(),
        };
        let _ = write!(
            kids,
            "{cpad}bevy_ecs::name::Name(\"{name}#{i}\")\n\
             {cpad}bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: 0.0, y: 0.0, z: 0.0 }}, rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }}, scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }} }}\n\
             {cpad}bevy_mesh::components::Mesh3d(\"{}/meshes/{stem}.cluster_mesh\")\n\
             {cpad}bevy_aurora::material::AuroraMaterial3d(bevy_aurora::material::AuroraMaterial {{{mat}}})\n\
             {col}{skin}{cpad},\n",
            ctx.asset_prefix,
        );
        ctx.emitted += 1;
    }
    for child in node.children() {
        emit_node(&child, ctx, &mut kids, depth + 1);
    }
    if !kids.is_empty() {
        let _ = write!(out, "{pad}bevy_ecs::hierarchy::Children [\n{kids}{pad}]\n");
    }
    let _ = write!(out, "{pad},\n");
}

/// Bake one primitive into a `.cluster_mesh` (skipping the bake if the file already exists from a
/// prior run). Returns the owner stem, or `None` if the mesh has no positions or the bake fails.
fn bake_primitive(mesh: &gltf::Mesh, prim: &gltf::Primitive, ctx: &mut Ctx) -> Option<String> {
    let stem = format!("mesh{}_{}", mesh.index(), prim.index());
    let mesh_file = ctx.meshes_dir.join(format!("{stem}.cluster_mesh"));
    if mesh_file.exists() && !ctx.replace {
        ctx.baked_count += 1;
        return Some(stem); // re-runs only re-emit the `.bsn` (use `replace` to overwrite)
    }

    let bevy_mesh = build_primitive_mesh(prim, ctx.buffers)?;
    match ClusterMeshData::from_mesh_flat(&bevy_mesh) {
        Ok(mut cm) => {
            // Alpha-cutout primitives get a baked opacity micromap against their base-colour
            // alpha (the material's own cutoff).
            let material = prim.material();
            if let Some(cutoff) = ctx.alpha_cutout(&material)
                && let Some(file) = ctx.base_color_file(&material)
                && let Ok(img) = image::open(ctx.textures_dir.join(&file))
            {
                mesh::attach_omm_rgba(
                    &mut cm,
                    &img.into_rgba8(),
                    cutoff,
                    &file,
                    &mesh::OmmOptions::from_env(),
                );
            }
            let w = BufWriter::new(File::create(&mesh_file).expect("create .cluster_mesh"));
            write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
            ctx.baked_count += 1;
            if ctx.baked_count % 200 == 0 {
                println!("  baked {}", ctx.baked_count);
            }
            Some(stem)
        }
        Err(err) => {
            eprintln!("  bake failed {stem}: {err:?}");
            ctx.failed_count += 1;
            None
        }
    }
}

/// Bake a single `.glb`'s first triangle primitive into a `.cluster_mesh` at `out_file` — the stem
/// is caller-controlled (unlike [`bake_gltf_scene`]'s `meshN_M`), so an articulated rig can name one
/// file per link. Skips if `out_file` exists unless `replace`. `Ok(true)` baked, `Ok(false)` skipped.
pub fn bake_glb_primitive(glb_path: &Path, out_file: &Path, replace: bool) -> Result<bool, String> {
    if out_file.exists() && !replace {
        return Ok(false);
    }
    let bytes = fs::read(glb_path).map_err(|e| format!("read {}: {e}", glb_path.display()))?;
    let gltf = gltf::Gltf::from_slice(&bytes).map_err(|e| format!("parse gltf: {e}"))?;
    let doc: &gltf::Document = &gltf;
    let buffers = gltf::import_buffers(doc, glb_path.parent(), gltf.blob.clone())
        .map_err(|e| format!("import buffers: {e}"))?;
    let prim = doc
        .meshes()
        .flat_map(|m| m.primitives())
        .find(|p| p.mode() == gltf::mesh::Mode::Triangles)
        .ok_or_else(|| "no triangle primitive".to_string())?;
    let mesh = build_primitive_mesh(&prim, &buffers).ok_or_else(|| "empty mesh".to_string())?;
    let cm = ClusterMeshData::from_mesh_flat(&mesh).map_err(|e| format!("cluster bake: {e:?}"))?;
    if let Some(parent) = out_file.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let w = BufWriter::new(File::create(out_file).map_err(|e| format!("create: {e}"))?);
    write_cluster_mesh_sync(&cm, w).map_err(|e| format!("write cluster: {e:?}"))?;
    Ok(true)
}

/// Build a bevy [`Mesh`] from a glTF primitive (positions in node-local space; the node's world
/// `Transform` places it). Loads only what the glTF carries — missing normals / UVs /
/// tangents default downstream in `ClusterMeshData::from_mesh_flat`, so bare CAD/URDF
/// primitives (POSITION only) still bake.
pub(crate) fn build_primitive_mesh(
    prim: &gltf::Primitive,
    buffers: &[gltf::buffer::Data],
) -> Option<Mesh> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
    if positions.is_empty() {
        return None;
    }
    let n = positions.len();

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);

    if let Some(normals) = reader.read_normals() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.collect::<Vec<[f32; 3]>>());
    }
    // Only accept UVs that cover every vertex; a partial/absent set is left to the bake
    // to fill (zero UV + identity tangent), which is correct for these untextured meshes.
    if let Some(uvs) = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect::<Vec<[f32; 2]>>())
    {
        if uvs.len() == n {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        }
    }

    // Skin, for a rigged source. Both streams or neither: the bake and the renderer both gate
    // on a palette that covers every vertex. The joint INDICES here are into the glTF skin's
    // joint list, which is the same order `SkinJointsByName` writes the names in.
    if let (Some(j), Some(w)) = (reader.read_joints(0), reader.read_weights(0)) {
        let joints: Vec<[u16; 4]> = j.into_u16().collect();
        let weights: Vec<[f32; 4]> = w.into_f32().collect();
        if joints.len() == n && weights.len() == n {
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_JOINT_INDEX,
                bevy::mesh::VertexAttributeValues::Uint16x4(joints),
            );
            mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights);
        }
    }

    match reader.read_indices() {
        Some(idx) => mesh.insert_indices(Indices::U32(idx.into_u32().collect())),
        None => mesh.insert_indices(Indices::U32((0..n as u32).collect())),
    }

    Some(mesh)
}

/// A material's emissive as linear radiance: `emissive_factor × KHR_materials_emissive_strength`.
/// When the asset ships no strength, falls back to the per-scene `emissive_nits` resolver (keyed on
/// material name) so relative emitter brightness bakes in physically.
///
/// Shared by the `.bsn` writer and [`crate::lint`] so the warning can never disagree with the value
/// actually written. `None` for non-emitters.
pub(crate) fn emissive_radiance(
    material: &gltf::Material,
    emissive_nits: Option<fn(&str) -> f32>,
    has_emissive_texture: bool,
) -> Option<[f32; 3]> {
    let ef = material.emissive_factor();
    if !has_emissive_texture && ef == [0.0, 0.0, 0.0] {
        return None;
    }
    let strength = material
        .emissive_strength()
        .unwrap_or_else(|| emissive_nits.map_or(1.0, |f| f(material.name().unwrap_or(""))));
    // A textured emissive with a zero factor would be invisible (emissive = factor × texture);
    // default such a material to unit factor so the texture shows.
    let [r, g, b] = if has_emissive_texture && ef == [0.0, 0.0, 0.0] {
        [1.0, 1.0, 1.0]
    } else {
        ef
    };
    Some([r * strength, g * strength, b * strength])
}

/// Lint every material in the document — unit-system mistakes that bake cleanly and only show up
/// in the viewer. See [`crate::lint`].
pub(crate) fn lint_materials(
    doc: &gltf::Document,
    emissive_nits: Option<fn(&str) -> f32>,
) -> crate::lint::Report {
    let mut report = crate::lint::Report::default();
    for material in doc.materials() {
        let name = material.name().unwrap_or("<unnamed>").to_string();
        let has_emissive_tex = material.emissive_texture().is_some();
        if let Some(rgb) = emissive_radiance(&material, emissive_nits, has_emissive_tex) {
            report.emitters += 1;
            // A textured emitter's factor is only a tint; the texture carries the magnitude, so a
            // low factor there is expected rather than suspicious.
            if !has_emissive_tex {
                if let Some(v) = crate::lint::check_emissive(&name, rgb) {
                    report.warnings.extend(v.warning);
                }
            }
        }
        let pbr = material.pbr_metallic_roughness();
        let textured = pbr.base_color_texture().is_some()
            || pbr.metallic_roughness_texture().is_some()
            || material.normal_texture().is_some();
        if crate::lint::is_default_white(pbr.base_color_factor(), textured) {
            report.white_plastic.push(name);
        }
    }
    report
}

/// Inline `AuroraMaterial` field list for a glTF material: the scalar/colour factors, textures (by
/// extracted file path), alpha-mode, and glass transmission/IOR.
fn material_fields(material: &gltf::Material, ctx: &Ctx) -> String {
    let mut fields = String::new();

    let pbr = material.pbr_metallic_roughness();

    // Scalar factors. Emitted only when they differ from `StandardSolariMaterial::default()`
    // (white / 0.5 rough / 0 metallic), so pre-existing textured imports keep their exact `.bsn`.
    // These are what make an UNTEXTURED material render correctly — without them a texture-free
    // asset falls back to default white plastic. glTF's base color factor is linear, and
    // `base_color` is a `Color` ENUM, so it needs the tuple-variant form (unlike `emissive`,
    // which is a plain `LinearRgba` struct).
    let mut bc = pbr.base_color_factor();
    // A supplied tint wins over whatever the file says: it exists precisely because the file is
    // wrong, and it is the only colour some of these materials have.
    if let Some(tint) = material.name().and_then(|n| ctx.fixups.tints.get(n)) {
        bc = *tint;
    } else
    // An all-zero base colour factor beside a base-colour TEXTURE is an export bug, not a black
    // material: multiplying the texture by zero would make referencing it pointless. The
    // fantasy-city kit proves it is a slip rather than a convention -- its foliage materials
    // export 0,0,0,0 while one of them, `..._Static_pot`, exports 1,1,1,1 off the same atlas.
    // Drop the factor and let the texture speak.
    if pbr.base_color_texture().is_some() && bc[0] == 0.0 && bc[1] == 0.0 && bc[2] == 0.0 {
        ctx.note_zeroed(material);
        bc = [1.0, 1.0, 1.0, 1.0];
    }
    if bc[0] != 1.0 || bc[1] != 1.0 || bc[2] != 1.0 || bc[3] != 1.0 {
        let _ = write!(
            fields,
            " base_color: bevy_color::color::Color::LinearRgba(bevy_color::linear_rgba::LinearRgba \
             {{ red: {}, green: {}, blue: {}, alpha: {} }}),",
            bsn::f(bc[0]),
            bsn::f(bc[1]),
            bsn::f(bc[2]),
            bsn::f(bc[3]),
        );
    }
    // glTF's metallicFactor defaults to 1.0 and is meant to scale a metallic-roughness texture.
    // aurora samples a white fallback where the texture is missing, so a bare factor would make
    // the whole material a mirror: only carry it when there is a texture to scale.
    let metallic = if pbr.metallic_roughness_texture().is_some() {
        pbr.metallic_factor()
    } else {
        0.0
    };
    if metallic != 0.0 {
        let _ = write!(fields, " metallic: {},", bsn::f(metallic));
    }
    let roughness = pbr.roughness_factor();
    if roughness != 0.5 {
        let _ = write!(fields, " perceptual_roughness: {},", bsn::f(roughness));
    }
    if let Some(ior) = material.ior() {
        // Only meaningful for transmissive surfaces, but harmless and cheap to carry.
        if ior != 1.5 {
            let _ = write!(fields, " ior: {},", bsn::f(ior));
        }
    }

    if let Some(file) = ctx.base_color_file(material) {
        let _ = write!(
            fields,
            " base_color_texture: \"{}/textures/{file}\",",
            ctx.asset_prefix
        );
    }
    if let Some(info) = pbr.metallic_roughness_texture() {
        if let Some(p) = tex_file(info.texture(), ctx) {
            let _ = write!(fields, " metallic_roughness_texture: \"{p}\",");
        }
    }
    if let Some(nt) = material.normal_texture() {
        if let Some(p) = tex_file(nt.texture(), ctx) {
            let _ = write!(fields, " normal_map_texture: \"{p}\",");
        }
    }

    // Emissive: factor × KHR_materials_emissive_strength as linear radiance. When the asset ships no
    // strength, fall back to the per-scene `emissive_nits` resolver (keyed on material name) so
    // relative emitter brightness bakes in physically. `None` keeps the glTF value. View with exposure.
    let emissive_tex = material
        .emissive_texture()
        .and_then(|info| tex_file(info.texture(), ctx));
    let radiance = emissive_radiance(material, ctx.emissive_nits, emissive_tex.is_some());
    if let Some([r, g, b]) = radiance {
        let _ = write!(
            fields,
            " emissive: bevy_color::linear_rgba::LinearRgba {{ red: {}, green: {}, blue: {}, alpha: 1.0 }},",
            bsn::f(r),
            bsn::f(g),
            bsn::f(b),
        );
        if let Some(p) = emissive_tex {
            let _ = write!(fields, " emissive_texture: \"{p}\",");
        }
    }

    // Alpha cutout (foliage, fences): emit `AlphaMode::Mask` so the ray tracer any-hit-tests it.
    // Via `alpha_cutout`, so a material the exporter wrongly called opaque still gets its cutout.
    if let Some(cutoff) = ctx.alpha_cutout(material) {
        let _ = write!(
            fields,
            " alpha_mode: bevy_aurora::material::AlphaMode::Mask({}),",
            bsn::f(cutoff)
        );
    }

    // Glass/liquids: KHR_materials_transmission → specular transmission + IOR (refraction owns it).
    if let Some(t) = material.transmission() {
        let factor = t.transmission_factor();
        if factor > 0.0 {
            let ior = material.ior().unwrap_or(1.5);
            let _ = write!(
                fields,
                " specular_transmission: {}, ior: {},",
                bsn::f(factor),
                bsn::f(ior)
            );
        }
    }

    fields
}

/// A texture's extracted file as an asset-prefixed path: texture → source image → filename.
/// `None` if the texture has no image source or the image wasn't extracted.
fn tex_file(tex: gltf::Texture, ctx: &Ctx) -> Option<String> {
    let img_idx = tex.source().index();
    let image = ctx.image_files.get(&img_idx)?;
    Some(format!("{}/textures/{}", ctx.asset_prefix, image))
}

/// Write every glTF image to `<textures_dir>` as raw bytes (no re-encode), returning
/// `image index → filename`. Embedded images come from a buffer view; external ones are copied.
fn extract_images(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
    textures_dir: &Path,
    overrides: Option<&Path>,
    fixups: &Fixups,
) -> HashMap<usize, String> {
    let mut files = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    let mut overridden = 0usize;
    for image in doc.images() {
        let idx = image.index();
        match image.source() {
            gltf::image::Source::View { view, mime_type } => {
                let ext = ext_for_mime(mime_type);
                let name = unique_name(image.name(), idx, ext, &mut used_names);
                // Prefer a same-named file from the override directory over the embedded copy.
                //
                // A kit often ships a glb beside the texture pack it was built from, and the glb
                // is the LOSSIER of the two: the fantasy-city export flattened every foliage
                // texture from RGBA to RGB, discarding a cutout alpha that is 40% holes. Nothing
                // downstream can recover that -- no alpha means no cutmask, so no `AlphaMode::Mask`
                // and no opacity micromap, and the leaf cards trace as solid quads.
                if let Some(dir) = overrides
                    && let Ok(bytes) = fs::read(dir.join(&name))
                {
                    let _ = fs::write(textures_dir.join(&name), bytes);
                    overridden += 1;
                    files.insert(idx, name);
                    continue;
                }
                let buf = &buffers[view.buffer().index()].0;
                let start = view.offset();
                let bytes = &buf[start..start + view.length()];
                let _ = fs::write(textures_dir.join(&name), bytes);
                files.insert(idx, name);
            }
            gltf::image::Source::Uri { uri, mime_type } => {
                // External file (e.g. a `.gltf` + `Textures/`): copy it in, keeping its basename.
                let decoded = percent_decode(uri);
                let src_name = Path::new(&decoded)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        let ext = mime_type.map(ext_for_mime).unwrap_or("bin");
                        format!("image_{idx}.{ext}")
                    });
                used_names.insert(src_name.clone());
                // `uri` is relative to the glTF's directory.
                if let Some(base) = base {
                    let _ = fs::copy(base.join(&decoded), textures_dir.join(&src_name));
                }
                files.insert(idx, src_name);
            }
        }
    }
    if overridden > 0 {
        println!("  took {overridden} texture(s) from the override directory");
    }
    // A fixup names a texture the glTF never referenced, so nothing above copied it in. Do that
    // here or the `.bsn` points at a file that is not there.
    for file in fixups.textures.values() {
        if textures_dir.join(file).exists() {
            continue;
        }
        match overrides.map(|dir| fs::copy(dir.join(file), textures_dir.join(file))) {
            Some(Ok(_)) => println!("  added {file} for a material that referenced none"),
            _ => eprintln!("  fixup texture {file} not found in the override directory"),
        }
    }
    files
}

fn ext_for_mime(mime: &str) -> &'static str {
    if mime.contains("png") {
        "png"
    } else if mime.contains("jpeg") || mime.contains("jpg") {
        "jpg"
    } else if mime.contains("ktx") {
        "ktx2"
    } else {
        "bin"
    }
}

/// A filesystem-safe, collision-free filename for an extracted image.
fn unique_name(name: Option<&str>, idx: usize, ext: &str, used: &mut HashSet<String>) -> String {
    let stem = name
        .map(|n| {
            n.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("image_{idx}"));
    let mut candidate = format!("{stem}.{ext}");
    let mut n = 1;
    while !used.insert(candidate.clone()) {
        candidate = format!("{stem}_{n}.{ext}");
        n += 1;
    }
    candidate
}

/// Minimal percent-decoding for glTF image URIs (`%20` → space, etc.).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
