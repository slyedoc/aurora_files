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

use bevy::asset::RenderAssetUsages;
use bevy::math::Mat4;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use aurora_cluster_mesh::{write_cluster_mesh_sync, ClusterMeshData};

use crate::bsn;

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
    let image_files = extract_images(doc, &buffers, base, &textures_dir);
    println!("extracted {} textures -> {}", image_files.len(), textures_dir.display());

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        failed_count: 0,
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

    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count,
        ctx.failed_count,
        meshes_dir.display()
    );
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

    let image_files = extract_images(doc, &buffers, base, &textures_dir);
    println!("extracted {} textures -> {}", image_files.len(), textures_dir.display());

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        failed_count: 0,
    };

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .expect("gltf has no scene");

    let mut used_names: HashSet<String> = HashSet::new();
    let mut groups_written = 0usize;
    for node in scene.nodes() {
        // Center each group at its own origin: walk with `parent = inverse(local)` so the root
        // group's own (layout) transform cancels and the building's geometry is emitted relative to
        // the group origin — a reusable prop rather than one pinned to its place in the assembled set.
        let local = Mat4::from_cols_array_2d(&node.transform().matrix());
        let parent = local.inverse();

        ctx.entities.clear();
        let before = ctx.emitted;
        walk(&node, parent, &mut ctx);
        if ctx.emitted == before {
            continue; // group has no triangle primitives — nothing to write
        }

        let stem = group_scene_name(node.name(), node.index(), &mut used_names);
        let bsn = bsn::scene(&stem, &ctx.entities);
        let bsn_path = cfg.out_dir.join(format!("{stem}.bsn"));
        fs::write(&bsn_path, bsn).expect("write .bsn");
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

    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count,
        ctx.failed_count,
        meshes_dir.display()
    );
    println!(
        "wrote {} group .bsn files + a {}-entity master {} -> {}",
        groups_written,
        master_entities,
        master_path.display(),
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

struct Ctx<'a> {
    buffers: &'a [gltf::buffer::Data],
    image_files: &'a HashMap<usize, String>,
    meshes_dir: &'a Path,
    asset_prefix: &'a str,
    replace: bool,
    emissive_nits: Option<fn(&str) -> f32>,
    /// `(mesh index, primitive index) → owner stem`, so shared meshes bake once and instance nodes
    /// reuse the baked `.cluster_mesh`. `None` marks a primitive whose bake failed (entities skipped).
    baked: HashMap<(usize, usize), Option<String>>,
    entities: String,
    emitted: usize,
    baked_count: usize,
    failed_count: usize,
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
            bsn::write_entity_trs(
                &mut ctx.entities,
                ctx.asset_prefix,
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

    let image_files = extract_images(doc, &buffers, base, &textures_dir);
    println!("extracted {} textures -> {}", image_files.len(), textures_dir.display());

    let mut ctx = Ctx {
        buffers: &buffers,
        image_files: &image_files,
        meshes_dir: &meshes_dir,
        asset_prefix: &cfg.asset_prefix,
        replace: cfg.replace,
        emissive_nits: cfg.emissive_nits,
        baked: HashMap::new(),
        entities: String::new(),
        emitted: 0,
        baked_count: 0,
        failed_count: 0,
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

    println!(
        "baked {} meshes ({} failed) -> {}",
        ctx.baked_count, ctx.failed_count, meshes_dir.display()
    );
    println!("wrote {} node entities (hierarchy) -> {}", ctx.emitted, bsn_path.display());
}

/// Emit one node as a `.bsn` entity block (comma-terminated) at `depth`, recursing into children.
/// Every node carries a `Name` (the animation binds to it) + its LOCAL `Transform`. A single-primitive
/// mesh is inlined on the node; extra primitives and the node's glTF children become nested `Children`.
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
        .and_then(|m| m.primitives().find(|p| p.mode() == gltf::mesh::Mode::Triangles))
        .map(|p| material_fields(&p.material(), ctx))
        .unwrap_or_default();

    let _ = write!(out, "{pad}bevy_ecs::name::Name(\"{name}\")\n");
    let _ = write!(
        out,
        "{pad}bevy_transform::components::transform::Transform {{ \
         translation: glam::Vec3 {{ x: {}, y: {}, z: {} }}, \
         rotation: glam::Quat {{ x: {}, y: {}, z: {}, w: {} }}, \
         scale: glam::Vec3 {{ x: {}, y: {}, z: {} }} }}\n",
        bsn::f(t[0]), bsn::f(t[1]), bsn::f(t[2]),
        bsn::f(r[0]), bsn::f(r[1]), bsn::f(r[2]), bsn::f(r[3]),
        bsn::f(s[0]), bsn::f(s[1]), bsn::f(s[2]),
    );
    // Single-primitive mesh: inline it on the node (the common case). Extra primitives drop to
    // identity-transform children below so this entity keeps the node's Name for animation.
    if let Some(stem) = prims.first() {
        let _ = write!(
            out,
            "{pad}bevy_mesh::components::Mesh3d(\"{}/meshes/{stem}.cluster_mesh\")\n\
             {pad}bevy_aurora::bsn::RaytracingMaterial3d(bevy_pbr::pbr_material::StandardMaterial {{{mat_fields}}})\n",
            ctx.asset_prefix,
        );
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
        let _ = write!(
            kids,
            "{cpad}bevy_ecs::name::Name(\"{name}#{i}\")\n\
             {cpad}bevy_transform::components::transform::Transform {{ translation: glam::Vec3 {{ x: 0.0, y: 0.0, z: 0.0 }}, rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }}, scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }} }}\n\
             {cpad}bevy_mesh::components::Mesh3d(\"{}/meshes/{stem}.cluster_mesh\")\n\
             {cpad}bevy_aurora::bsn::RaytracingMaterial3d(bevy_pbr::pbr_material::StandardMaterial {{{mat}}}),\n",
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
        Ok(cm) => {
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
pub(crate) fn build_primitive_mesh(prim: &gltf::Primitive, buffers: &[gltf::buffer::Data]) -> Option<Mesh> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let positions: Vec<[f32; 3]> = reader.read_positions()?.collect();
    if positions.is_empty() {
        return None;
    }
    let n = positions.len();

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);

    if let Some(normals) = reader.read_normals() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals.collect::<Vec<[f32; 3]>>());
    }
    // Only accept UVs that cover every vertex; a partial/absent set is left to the bake
    // to fill (zero UV + identity tangent), which is correct for these untextured meshes.
    if let Some(uvs) = reader.read_tex_coords(0).map(|t| t.into_f32().collect::<Vec<[f32; 2]>>()) {
        if uvs.len() == n {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
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
    let [r, g, b] = if has_emissive_texture && ef == [0.0, 0.0, 0.0] { [1.0, 1.0, 1.0] } else { ef };
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

/// Inline `SolariMaterial` field list for a glTF material: the scalar/colour factors, textures (by
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
    let bc = pbr.base_color_factor();
    if bc[0] != 1.0 || bc[1] != 1.0 || bc[2] != 1.0 || bc[3] != 1.0 {
        let _ = write!(
            fields,
            " base_color: bevy_color::color::Color::LinearRgba(bevy_color::linear_rgba::LinearRgba \
             {{ red: {}, green: {}, blue: {}, alpha: {} }}),",
            bsn::f(bc[0]), bsn::f(bc[1]), bsn::f(bc[2]), bsn::f(bc[3]),
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

    if let Some(info) = pbr.base_color_texture() {
        if let Some(p) = tex_file(info.texture(), ctx) {
            let _ = write!(fields, " base_color_texture: \"{p}\",");
        }
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
    let emissive_tex = material.emissive_texture().and_then(|info| tex_file(info.texture(), ctx));
    let radiance = emissive_radiance(material, ctx.emissive_nits, emissive_tex.is_some());
    if let Some([r, g, b]) = radiance {
        let _ = write!(
            fields,
            " emissive: bevy_color::linear_rgba::LinearRgba {{ red: {}, green: {}, blue: {}, alpha: 1.0 }},",
            bsn::f(r), bsn::f(g), bsn::f(b),
        );
        if let Some(p) = emissive_tex {
            let _ = write!(fields, " emissive_texture: \"{p}\",");
        }
    }

    // Alpha cutout (foliage, fences): emit `AlphaMode::Mask` so the ray tracer any-hit-tests it.
    if material.alpha_mode() == gltf::material::AlphaMode::Mask {
        let cutoff = material.alpha_cutoff().unwrap_or(0.5);
        let _ = write!(
            fields,
            " alpha_mode: bevy_material::alpha::AlphaMode::Mask({}),",
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
) -> HashMap<usize, String> {
    let mut files = HashMap::new();
    let mut used_names: HashSet<String> = HashSet::new();
    for image in doc.images() {
        let idx = image.index();
        match image.source() {
            gltf::image::Source::View { view, mime_type } => {
                let ext = ext_for_mime(mime_type);
                let name = unique_name(image.name(), idx, ext, &mut used_names);
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
fn unique_name(
    name: Option<&str>,
    idx: usize,
    ext: &str,
    used: &mut HashSet<String>,
) -> String {
    let stem = name
        .map(|n| {
            n.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
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
