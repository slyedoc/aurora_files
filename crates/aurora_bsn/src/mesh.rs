//! Build a bevy [`Mesh`] from a tobj submesh.

use std::path::Path;

use aurora_cluster_mesh::ClusterMeshData;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use image::RgbaImage;

use crate::dedup::V3;
#[cfg(feature = "omm")]
use crate::img;
#[cfg(feature = "omm")]
use crate::omm;

/// Centroid (mean of vertex positions) of a tobj submesh, or `None` if it has none. The baked
/// geometry is shifted by `-centroid` so the submesh sits in local space; `centroid` is its world
/// `Transform.translation`. Centroid (not AABB center) so it matches the Kabsch fit's frame.
pub fn submesh_centroid(m: &tobj::Mesh) -> Option<V3> {
    if m.positions.is_empty() {
        return None;
    }
    let n = (m.positions.len() / 3) as f64;
    let mut c = [0.0f64; 3];
    for p in m.positions.chunks_exact(3) {
        c[0] += p[0] as f64;
        c[1] += p[1] as f64;
        c[2] += p[2] as f64;
    }
    Some([c[0] / n, c[1] / n, c[2] / n])
}

/// Build a `bevy` [`Mesh`] from a tobj submesh in LOCAL space (each vertex shifted by `-center`),
/// generating normals if the OBJ lacked them and flipping the V texcoord into Bevy's convention.
/// The caller guarantees non-empty positions (gated by [`submesh_centroid`]).
pub fn build_mesh(m: &tobj::Mesh, center: V3) -> Mesh {
    let positions: Vec<[f32; 3]> = m
        .positions
        .chunks_exact(3)
        .map(|c| {
            [
                (c[0] as f64 - center[0]) as f32,
                (c[1] as f64 - center[1]) as f32,
                (c[2] as f64 - center[2]) as f32,
            ]
        })
        .collect();
    let vertex_count = positions.len();

    let uvs: Vec<[f32; 2]> = if m.texcoords.len() == vertex_count * 2 {
        m.texcoords
            .chunks_exact(2)
            .map(|c| [c[0], 1.0 - c[1]])
            .collect()
    } else {
        vec![[0.0, 0.0]; vertex_count]
    };

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(m.indices.clone()));

    if m.normals.len() == vertex_count * 3 {
        let normals: Vec<[f32; 3]> = m
            .normals
            .chunks_exact(3)
            .map(|c| [c[0], c[1], c[2]])
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_normals();
    }

    mesh
}

// ---- Opacity micromaps ---------------------------------------------------------------------
//
// Alpha-cutout meshes get a baked opacity micromap (NVIDIA OMM SDK CPU baker, `omm.rs`) stored
// in the `.cluster_mesh` v3 slices; aurora attaches it to the mesh's BLAS so the RT cores resolve
// known opaque / transparent micro-regions without the any-hit shader.

/// Default cap on OMM subdivision (`OMM_SUBDIV` overrides). The baker picks per-triangle from
/// texel area up to this (4^6 = 4096 micro-tris max per triangle), so only detail-heavy triangles
/// reach the cap. 4 swallowed thin features (leaf tips), 9 was huge; 6 keeps thin silhouettes
/// while staying compact.
pub const DEFAULT_OMM_SUBDIV: u32 = 6;

/// Despeckle: after binarizing the alpha at the cutoff, drop every opaque island smaller than this
/// fraction of the LARGEST island. Kills source-PNG noise (stray opaque pixels, antialiasing crud)
/// that the hard cutoff would otherwise turn into spurious micro-regions, without removing real
/// leaves (each a substantial island). `0.0` disables.
const MIN_ISLAND_FRACTION: f32 = 0.02;

/// Default erosion radius in texels (`OMM_ERODE` overrides). The 2-state OMM is conservative at
/// the silhouette (a micro-triangle touching any opaque texel resolves opaque), so the mask bleeds
/// slightly outward past the real leaf edge; eroding pulls it back in and clears opaque texels
/// hugging the texture border. `0` disables; 1-3 is the useful range.
pub const DEFAULT_ERODE_PX: u32 = 2;

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Bake an opacity micromap from the material's base-colour alpha and attach it to `cm` (the
/// OBJ/MTL path). Returns `(omm count, array bytes)`, or `None` if the texture is unreadable, the
/// mesh has no UVs, or the bake produced nothing useful.
#[cfg(feature = "omm")]
pub fn attach_omm(
    cm: &mut ClusterMeshData,
    obj_dir: &Path,
    material: &tobj::Material,
) -> Option<(usize, usize)> {
    let tex = material.diffuse_texture.as_deref()?;
    let img = image::open(obj_dir.join(tex.replace('\\', "/"))).ok()?;
    attach_omm_rgba(cm, &img.into_rgba8(), img::MASK_CUTOFF, tex)
}

#[cfg(not(feature = "omm"))]
pub fn attach_omm(
    _cm: &mut ClusterMeshData,
    _obj_dir: &Path,
    _material: &tobj::Material,
) -> Option<(usize, usize)> {
    None
}

/// Bake an OMM directly from a decoded base-colour image (the glTF / SpeedTree paths, where the
/// alpha lives in an already-extracted PNG). `cutoff` is the material's alpha-test threshold;
/// `label` only tags the despeckle log line.
#[cfg(feature = "omm")]
pub fn attach_omm_rgba(
    cm: &mut ClusterMeshData,
    rgba: &RgbaImage,
    cutoff: f32,
    label: &str,
) -> Option<(usize, usize)> {
    use aurora_cluster_mesh::{OmmDesc, OmmUsage};

    let subdiv = env_u32("OMM_SUBDIV", DEFAULT_OMM_SUBDIV);
    let erode_px = env_u32("OMM_ERODE", DEFAULT_ERODE_PX);
    // 2-state by default: no "unknown" micro-triangles, so the any-hit never runs on the cutout
    // (a micro-tri-quantized edge, hidden by DLSS). `OMM_FORMAT=4` keeps an exact any-hit edge
    // band instead.
    let format = if env_u32("OMM_FORMAT", 2) == 4 {
        omm::OMM_FORMAT_OC1_4_STATE
    } else {
        omm::OMM_FORMAT_OC1_2_STATE
    };

    let (w, h) = (rgba.width(), rgba.height());
    // Alpha row-0-first, no V flip: the runtime samples the GPU texture with the same UVs, so
    // the unflipped alpha makes the bake sample exactly what the renderer does. (The SDK's
    // debug image dump draws its overlay mirrored -- trust the render, not the dump.)
    let mut alpha: Vec<f32> = rgba.pixels().map(|p| p.0[3] as f32 / 255.0).collect();
    despeckle_alpha(&mut alpha, w, h, cutoff, label);
    erode_alpha(&mut alpha, w, h, cutoff, erode_px);

    let uvs: Vec<f32> = cm.vertex_uvs.iter().flat_map(|uv| [uv.x, uv.y]).collect();
    if uvs.is_empty() || cm.vertex_uvs.len() != cm.vertex_positions.len() {
        return None;
    }
    // `cm.indices` are cluster-local (each cluster's triangles index its own vertex slice at
    // `vertex_offset`) while `vertex_uvs` is the global array: globalise them, walking clusters
    // in order so the flat triangle order (and so the per-triangle OMM index) matches the
    // renderer's `index_offset / 3` slicing.
    let mut indices: Vec<u32> = Vec::with_capacity(cm.indices.len());
    for cluster in cm.clusters.iter() {
        let start = cluster.index_offset as usize;
        let end = start + cluster.triangle_count as usize * 3;
        indices.extend(
            cm.indices[start..end]
                .iter()
                .map(|&i| cluster.vertex_offset + i),
        );
    }

    // REPEAT to match the runtime sampler.
    let bake = omm::bake(&alpha, w, h, &uvs, &indices, cutoff, format, subdiv, true).ok()?;
    if bake.descs.is_empty() {
        return None; // wholly uniform (all opaque / transparent): special indices only
    }
    if bake.indices.len() * 3 != indices.len() {
        eprintln!(
            "  OMM {label}: {} indices for {} triangles; skipped",
            bake.indices.len(),
            indices.len() / 3
        );
        return None;
    }

    let known = bake.stat_opaque + bake.stat_transparent;
    let unknown = bake.stat_unknown_opaque + bake.stat_unknown_transparent;
    let total = (known + unknown).max(1);
    println!(
        "  OMM {label}: {} omms, {} B, opaque={} transparent={} unknown={} ({:.0}% known, area_metric={:.3})",
        bake.descs.len(),
        bake.array_data.len(),
        bake.stat_opaque,
        bake.stat_transparent,
        unknown,
        100.0 * known as f64 / total as f64,
        bake.known_area_metric,
    );

    let stats = (bake.descs.len(), bake.array_data.len());
    let usage = |u: &omm::OmmUsage| OmmUsage {
        count: u.count,
        subdivision_level: u.subdivision_level,
        format: u.format,
    };
    cm.omm_descs = bake
        .descs
        .iter()
        .map(|&(offset, subdivision_level, format)| OmmDesc {
            offset,
            subdivision_level,
            format,
        })
        .collect();
    cm.omm_usage = bake.desc_histogram.iter().map(usage).collect();
    cm.omm_index_usage = bake.index_histogram.iter().map(usage).collect();
    cm.omm_array_data = bake.array_data.into();
    cm.omm_index = bake.indices.into();
    Some(stats)
}

#[cfg(not(feature = "omm"))]
pub fn attach_omm_rgba(
    _cm: &mut ClusterMeshData,
    _rgba: &RgbaImage,
    _cutoff: f32,
    _label: &str,
) -> Option<(usize, usize)> {
    None
}

/// Erode the opaque mask inward by `erode_px` texels (in place): an opaque texel within that
/// Chebyshev distance of any transparent texel (or the texture border) becomes transparent.
#[cfg(feature = "omm")]
fn erode_alpha(alpha: &mut [f32], w: u32, h: u32, cutoff: f32, erode_px: u32) {
    if erode_px == 0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    debug_assert_eq!(alpha.len(), n);

    let mut mask: Vec<bool> = alpha.iter().map(|&a| a >= cutoff).collect();
    for _ in 0..erode_px {
        let prev = mask.clone();
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if !prev[i] {
                    continue;
                }
                let edge = x == 0 || y == 0 || x + 1 == w || y + 1 == h;
                let eroded = edge
                    || !prev[i - 1]
                    || !prev[i + 1]
                    || !prev[i - w]
                    || !prev[i + w]
                    || !prev[i - w - 1]
                    || !prev[i - w + 1]
                    || !prev[i + w - 1]
                    || !prev[i + w + 1];
                if eroded {
                    mask[i] = false;
                }
            }
        }
    }
    for i in 0..n {
        if alpha[i] >= cutoff && !mask[i] {
            alpha[i] = 0.0;
        }
    }
}

/// Remove small opaque islands from the alpha mask (in place) before the bake: binarise at
/// `cutoff`, label 8-connected components (union-find), zero every island below
/// [`MIN_ISLAND_FRACTION`] of the largest. With a 2-state OMM the micromap *is* the cutout, so a
/// dropped island also leaves the render.
#[cfg(feature = "omm")]
fn despeckle_alpha(alpha: &mut [f32], w: u32, h: u32, cutoff: f32, tex: &str) {
    if MIN_ISLAND_FRACTION <= 0.0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    debug_assert_eq!(alpha.len(), n);

    let opaque = |a: &[f32], i: usize| a[i] >= cutoff;

    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    let union = |parent: &mut [u32], a: u32, b: u32| {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra as usize] = rb;
        }
    };

    // Forward-only neighbour scan (right, down, down-right, down-left) covers every
    // 8-connected pair exactly once.
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !opaque(alpha, i) {
                continue;
            }
            if x + 1 < w && opaque(alpha, i + 1) {
                union(&mut parent, i as u32, (i + 1) as u32);
            }
            if y + 1 < h {
                if opaque(alpha, i + w) {
                    union(&mut parent, i as u32, (i + w) as u32);
                }
                if x + 1 < w && opaque(alpha, i + w + 1) {
                    union(&mut parent, i as u32, (i + w + 1) as u32);
                }
                if x >= 1 && opaque(alpha, i + w - 1) {
                    union(&mut parent, i as u32, (i + w - 1) as u32);
                }
            }
        }
    }

    let mut area = vec![0u32; n];
    for i in 0..n {
        if opaque(alpha, i) {
            let r = find(&mut parent, i as u32) as usize;
            area[r] += 1;
        }
    }
    let max_area = area.iter().copied().max().unwrap_or(0);
    if max_area == 0 {
        return;
    }
    let threshold = (MIN_ISLAND_FRACTION * max_area as f32).ceil() as u32;

    let (mut removed_islands, mut kept_islands, mut removed_px) = (0u32, 0u32, 0u64);
    for &a in &area {
        if a > 0 {
            if a < threshold {
                removed_islands += 1;
            } else {
                kept_islands += 1;
            }
        }
    }
    for i in 0..n {
        if opaque(alpha, i) {
            let r = find(&mut parent, i as u32) as usize;
            if area[r] < threshold {
                alpha[i] = 0.0;
                removed_px += 1;
            }
        }
    }
    if removed_islands > 0 {
        println!(
            "  despeckle {tex}: kept {kept_islands} island(s), dropped {removed_islands} \
             (<{threshold}px) = {removed_px} texels"
        );
    }
}
