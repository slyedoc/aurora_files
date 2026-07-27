//! Offline importer core: OBJ/MTL → `bevy_solari` `.cluster_mesh` assets + a `.bsn` scene.
//!
//! Sidesteps glTF: each OBJ submesh is baked to a `bevy_solari` `.cluster_mesh` and the scene is
//! emitted as Bevy Scene Notation referencing those meshes plus inline `SolariMaterial`s (which,
//! unlike glTF, can carry a displacement/depth texture). Per-asset importer binaries fill a
//! [`SceneConfig`] and call [`bake_scene`]; everything reusable lives in the submodules.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use bevy::solari::geometry::{write_cluster_mesh_sync, ClusterMesh};

pub mod animclip;
pub mod bsn;
pub mod dedup;
pub mod discovery;
pub mod gltf;
pub mod img;
pub mod lint;
pub mod mesh;
pub mod omm;
pub mod speedtree;

pub use animclip::transcode_gltf_to_animclip;
pub use gltf::{bake_gltf_hierarchy, bake_gltf_per_group, bake_gltf_scene, GltfConfig};
pub use speedtree::{bake_speedtree, SpeedTreeConfig};

/// Per-submesh facts a [`SubmeshFilter`] can decide on (e.g. keep only floor surfaces, or only
/// alpha-cutout objects that carry a baked OMM).
pub struct SubmeshInfo<'a> {
    /// The OBJ object name.
    pub name: &'a str,
    /// Basename of the material's diffuse texture, if any.
    pub diffuse_basename: Option<&'a str>,
    /// Whether this submesh's material is a genuine alpha cutout (gets `AlphaMode::Mask` + an OMM).
    pub is_cutmask: bool,
}

/// Predicate selecting which submeshes to emit. `None` in the config keeps everything.
pub type SubmeshFilter = Box<dyn Fn(&SubmeshInfo) -> bool>;

/// Everything an importer binary varies per asset.
pub struct SceneConfig {
    /// Source `.obj` (its `.mtl` and textures are resolved relative to it).
    pub obj_path: PathBuf,
    /// Output asset directory (`meshes/`, `textures/`, and the `.bsn` are written under it).
    pub out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference baked meshes/textures.
    pub asset_prefix: String,
    /// `.bsn` scene name and output filename stem (e.g. `SanMiguel` → `SanMiguel.bsn`).
    pub scene_name: String,
    /// Optional submesh selector; `None` emits every submesh.
    pub submesh_filter: Option<SubmeshFilter>,
    /// Share one baked mesh across rigid+uniform-scale instances (chairs around a table, the
    /// colonnade) instead of baking each submesh's geometry separately. Confirmed copies are placed
    /// by a recovered `Transform`; see the dedup pass in [`bake_scene`].
    pub dedup: bool,
    /// Re-bake `.cluster_mesh` files even when they already exist (overwrite). When `false`, an
    /// existing mesh file is left as-is and only the `.bsn` is re-emitted.
    pub replace: bool,
    /// Erosion radius (texels) applied to the cutout alpha mask before OMM baking; pulls the
    /// conservative 2-state silhouette back in. See [`mesh::DEFAULT_ERODE_PX`]; `0` disables.
    pub erode_px: u32,
    /// Max OMM subdivision level (the baker caps per-triangle subdivision at this). See
    /// [`mesh::DEFAULT_OMM_SUBDIV`].
    pub omm_subdiv: u32,
}

type V3 = dedup::V3;

/// Bake the scene described by `cfg`: copy + normalize textures, bake each submesh's
/// `.cluster_mesh` (with an OMM for alpha cutouts), and write the `.bsn`.
pub fn bake_scene(cfg: &SceneConfig) {
    let meshes_dir = cfg.out_dir.join("meshes");
    fs::create_dir_all(&meshes_dir).expect("create meshes dir");

    println!("loading {}", cfg.obj_path.display());
    let (models, materials) = tobj::load_obj(
        &cfg.obj_path,
        &tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ignore_points: true,
            ignore_lines: true,
        },
    )
    .expect("load obj");
    let materials = materials.unwrap_or_default();
    println!("{} submeshes, {} materials", models.len(), materials.len());

    let textures_dir = cfg.out_dir.join("textures");
    discovery::copy_textures(&cfg.obj_path, &materials, &models, &textures_dir);

    // The OBJ/MTL carries no alpha mode — foliage cutouts are signaled only by the base-color PNG's
    // alpha channel. Classify each material from its alpha histogram so the `.bsn` can emit
    // `AlphaMode::Mask` for genuine cutouts; everything else stays opaque-traced.
    let obj_dir = cfg
        .obj_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut tex_cutmask: HashMap<String, bool> = HashMap::new();
    let cutmask: Vec<bool> = materials
        .iter()
        .map(|m| discovery::material_is_cutmask(&obj_dir, m, &mut tex_cutmask))
        .collect();
    println!(
        "{}/{} materials classified as alpha cutouts (Mask)",
        cutmask.iter().filter(|&&c| c).count(),
        materials.len(),
    );

    // Displacement (depth) maps: matched to materials by name and carried on the inline
    // `SolariMaterial`. Source textures live alongside the OBJ under the MTL's relative dir; scan it
    // for the depth maps (the MTL never names them) and normalize each to the depth convention.
    let tex_src_dir = materials
        .iter()
        .filter_map(|m| m.diffuse_texture.as_deref())
        .find_map(|t| {
            let rel = t.replace('\\', "/");
            Path::new(&rel).parent().map(|p| obj_dir.join(p))
        })
        .unwrap_or_else(|| obj_dir.clone());
    let displacement_maps = discovery::displacement_index(&tex_src_dir, &textures_dir);
    let disp_matched = materials
        .iter()
        .filter_map(|m| m.diffuse_texture.as_deref())
        .filter(|d| img::match_displacement(d, &displacement_maps).is_some())
        .count();
    println!(
        "{} displacement maps -> {} materials matched",
        displacement_maps.len(),
        disp_matched,
    );

    // INSTANCE DEDUP. The geometry is centered (local-space), so rotated/scaled copies (chairs
    // around tables, the colonnade) are comparable: bucket submeshes by topology (vertex/triangle
    // count + diffuse texture), Kabsch-verify each bucket, and let confirmed copies SHARE one baked
    // mesh placed by a recovered rigid+scale `Transform`. `centroids[i]` is both the local pivot and
    // the world translation; `owner[i]` points each submesh at the mesh it reuses (itself when
    // unique); `rot`/`scale` carry the fit of the owner's geometry onto this submesh.
    let centroids: Vec<Option<V3>> =
        models.iter().map(|m| mesh::submesh_centroid(&m.mesh)).collect();
    let mut owner: Vec<usize> = (0..models.len()).collect();
    let mut rot: Vec<[f64; 4]> = vec![[1.0, 0.0, 0.0, 0.0]; models.len()]; // quaternion (w,x,y,z)
    let mut scale: Vec<f64> = vec![1.0; models.len()];

    if cfg.dedup {
        // Bucket by (vertex count, triangle count, diffuse texture). tobj reports a nearly
        // per-submesh material id (~317k for ~280 textures), so the *texture* — not `material_id` —
        // is what groups instances; the texture also implies a shared UV layout (verified per-pair).
        let mut buckets: HashMap<(usize, usize, String), Vec<usize>> = HashMap::new();
        for (i, m) in models.iter().enumerate() {
            if centroids[i].is_some() {
                let vtx = m.mesh.positions.len() / 3;
                let tris = m.mesh.indices.len() / 3;
                buckets
                    .entry((vtx, tris, dedup::diffuse_key(m, &materials)))
                    .or_default()
                    .push(i);
            }
        }
        let mut shared = 0usize;
        for idxs in buckets.values() {
            if idxs.len() < 2 {
                continue;
            }
            let r = idxs[0];
            let refp = dedup::positions(&models[r]);
            for &j in &idxs[1..] {
                let fit = dedup::kabsch_fit(&refp, &dedup::positions(&models[j]));
                // Same shape AND same UV layout: the shared mesh carries the owner's texcoords, so
                // an instance with different UVs would sample its texture wrong. A passing fit
                // reproduces the instance to within `EPS * rms_radius` (correct by construction).
                if fit.residual < dedup::EPS && dedup::uvs_match(&models[r].mesh, &models[j].mesh) {
                    owner[j] = r;
                    rot[j] = fit.rotation;
                    scale[j] = fit.scale;
                    shared += 1;
                }
            }
        }
        println!(
            "instance dedup: {} of {} submeshes reuse geometry -> {} unique meshes",
            shared,
            models.len(),
            models.len() - shared,
        );
    } else {
        println!(
            "instance dedup: DISABLED — {} submeshes each get their own mesh",
            models.len()
        );
    }

    // Which submeshes the filter keeps — this decides BOTH what gets emitted (pass 2) and what needs
    // baking (pass 1), so `--floor-only` / `--cutout-only` don't bake the whole scene. A submesh
    // with no geometry is dropped here too.
    let keep: Vec<bool> = (0..models.len())
        .map(|i| centroids[i].is_some() && submesh_kept(cfg, &models, &materials, &cutmask, i))
        .collect();
    // Owners referenced by at least one kept submesh — only these need a baked mesh.
    let mut needed_owner = vec![false; models.len()];
    for i in 0..models.len() {
        if keep[i] {
            needed_owner[owner[i]] = true;
        }
    }

    // Pass 1 — bake each NEEDED owner's centroid-centered geometry once (instances reuse it). A bake
    // that fails (e.g. missing UVs) marks the owner so its entities are dropped below.
    let mut baked = 0usize;
    let mut omm_baked = 0usize;
    let mut failed: HashSet<usize> = HashSet::new();
    for i in 0..models.len() {
        if owner[i] != i || !needed_owner[i] {
            continue; // not an owner, or no kept submesh references it
        }
        let Some(centroid) = centroids[i] else { continue };
        let mesh_file =
            meshes_dir.join(format!("{}_{i}.cluster_mesh", discovery::sanitize(&models[i].name)));
        if mesh_file.exists() && !cfg.replace {
            continue; // re-runs only re-emit the `.bsn` (use `replace` to overwrite)
        }
        let m = mesh::build_mesh(&models[i].mesh, centroid);
        match ClusterMesh::try_from(&m) {
            Ok(mut cm) => {
                // Alpha-cutout owners get a baked opacity micro-map so the RT cores resolve known
                // opaque/transparent micro-regions without invoking the any-hit shader. Baked
                // against the FINAL (post-cluster) triangle order.
                let is_cutmask = models[i]
                    .mesh
                    .material_id
                    .is_some_and(|id| cutmask.get(id).copied().unwrap_or(false));
                if is_cutmask {
                    if let Some(material) =
                        models[i].mesh.material_id.and_then(|id| materials.get(id))
                    {
                        if let Some((omms, bytes)) =
                            mesh::attach_omm(&mut cm, &obj_dir, material, cfg.erode_px, cfg.omm_subdiv)
                        {
                            omm_baked += 1;
                            if omm_baked % 20 == 0 {
                                println!("  OMM baked {omm_baked} (latest: {omms} omms, {bytes} B)");
                            }
                        }
                    }
                }
                let w = BufWriter::new(File::create(&mesh_file).expect("create .cluster_mesh"));
                write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
                baked += 1;
                if baked % 50 == 0 {
                    println!("  baked {baked}");
                }
            }
            Err(err) => {
                eprintln!("  bake failed {i} ({}): {err:?}", models[i].name);
                failed.insert(i);
            }
        }
    }

    // Pass 2 — one `.bsn` entity per submesh, referencing its owner's mesh, placed by its own
    // `Transform` (translation = centroid; rotation/scale from the Kabsch fit, for instances).
    let mut entities = String::new();
    let mut emitted = 0usize;
    for i in 0..models.len() {
        if !keep[i] {
            continue; // filtered out (or no geometry)
        }
        let centroid = centroids[i].expect("keep implies geometry");
        let o = owner[i];
        if failed.contains(&o) {
            continue; // owner mesh never baked → drop its instances too
        }
        let material = models[i].mesh.material_id.and_then(|id| materials.get(id));
        let is_cutmask = models[i]
            .mesh
            .material_id
            .is_some_and(|id| cutmask.get(id).copied().unwrap_or(false));
        let owner_stem = format!("{}_{o}", discovery::sanitize(&models[o].name));
        bsn::write_entity(
            &mut entities,
            &cfg.asset_prefix,
            &owner_stem,
            material,
            is_cutmask,
            &displacement_maps,
            centroid,
            rot[i],
            scale[i],
        );
        emitted += 1;
    }

    let bsn = bsn::scene(&cfg.scene_name, &entities);
    let bsn_path = cfg.out_dir.join(format!("{}.bsn", cfg.scene_name));
    fs::write(&bsn_path, bsn).expect("write .bsn");

    println!("baked {baked} meshes ({omm_baked} with OMM) -> {}", meshes_dir.display());
    println!("wrote {emitted} entities -> {}", bsn_path.display());
}

/// Whether submesh `i` passes the config's filter (`true` when there is no filter). Builds the
/// [`SubmeshInfo`] the filter decides on.
fn submesh_kept(
    cfg: &SceneConfig,
    models: &[tobj::Model],
    materials: &[tobj::Material],
    cutmask: &[bool],
    i: usize,
) -> bool {
    let Some(filter) = &cfg.submesh_filter else {
        return true;
    };
    let material = models[i].mesh.material_id.and_then(|id| materials.get(id));
    let is_cutmask = models[i]
        .mesh
        .material_id
        .is_some_and(|id| cutmask.get(id).copied().unwrap_or(false));
    let info = SubmeshInfo {
        name: &models[i].name,
        diffuse_basename: material
            .and_then(|m| m.diffuse_texture.as_deref())
            .map(img::basename),
        is_cutmask,
    };
    filter(&info)
}
