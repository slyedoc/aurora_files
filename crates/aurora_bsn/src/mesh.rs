//! Build a bevy [`Mesh`] from a tobj submesh and bake/attach its opacity micro-map.

use std::path::Path;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy_aurora::geometry::{ClusterMesh, OmmDesc, OmmUsage};
use image::RgbaImage;

use crate::dedup::V3;
use crate::img;
use crate::omm;

/// Default cap on OMM subdivision when an importer doesn't override `--level`. The baker picks
/// per-triangle from texel area up to this (4^6 = 4096 micro-tris max per triangle), scaling
/// subdivision per-triangle by texel area so only detail-heavy triangles reach the cap. 4 swallowed
/// thin features (leaf tips), 9 was huge; 6 keeps thin silhouettes while staying compact.
pub const DEFAULT_OMM_SUBDIV: u32 = 6;

/// Despeckle: after binarizing the alpha at the cutoff, drop every opaque island
/// smaller than this fraction of the LARGEST island. Kills source-PNG noise (stray
/// opaque pixels, antialiasing crud) that the hard cutoff would otherwise turn into
/// spurious micro-regions, without removing real leaves (each a substantial island).
/// `0.0` disables; raise toward `1.0` to keep only the biggest island(s).
const MIN_ISLAND_FRACTION: f32 = 0.02;

/// Default erosion radius (texels) when an importer doesn't override `--erode`. The 2-state OMM is
/// conservative at the silhouette (a micro-triangle touching any opaque texel resolves opaque), so
/// the mask bleeds slightly *outward* past the real leaf edge. Eroding pulls it back in — also
/// clears opaque texels hugging the texture border (out-of-bounds neighbors count as transparent).
/// `0` disables; 1-3 is the useful range.
pub const DEFAULT_ERODE_PX: u32 = 2;

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
        m.texcoords.chunks_exact(2).map(|c| [c[0], 1.0 - c[1]]).collect()
    } else {
        vec![[0.0, 0.0]; vertex_count]
    };

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(m.indices.clone()));

    if m.normals.len() == vertex_count * 3 {
        let normals: Vec<[f32; 3]> =
            m.normals.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    } else {
        mesh.compute_normals();
    }

    mesh
}

/// Bake an opacity micro-map from the material's base-color alpha and attach it to `cm`.
/// Returns `(omm count, array bytes)` baked, or `None` if the texture is unreadable, the
/// mesh has no UVs, or the bake produced nothing useful. The per-triangle index is baked
/// against `cm.indices()` so it stays aligned to the cluster triangle order on the GPU.
pub fn attach_omm(
    cm: &mut ClusterMesh,
    obj_dir: &Path,
    material: &tobj::Material,
    erode_px: u32,
    subdiv: u32,
) -> Option<(usize, usize)> {
    let tex = material.diffuse_texture.as_deref()?;
    let img = image::open(obj_dir.join(tex.replace('\\', "/"))).ok()?;
    attach_omm_rgba(cm, &img.into_rgba8(), erode_px, subdiv, tex)
}

/// Bake an OMM directly from a decoded base-color image (the glTF/SpeedTree path, where the alpha
/// lives in an already-extracted PNG rather than an MTL `diffuse_texture`). Same core as
/// [`attach_omm`]; `label` only tags the despeckle log line.
pub fn attach_omm_rgba(
    cm: &mut ClusterMesh,
    rgba: &RgbaImage,
    erode_px: u32,
    subdiv: u32,
    label: &str,
) -> Option<(usize, usize)> {
    let (w, h) = (rgba.width(), rgba.height());
    // Alpha row-0-first, matching how Bevy uploads the PNG (no flip): the SDK samples
    // `textureData` row-major with UV(0,0)→texel(0,0), and the runtime samples the GPU
    // texture with the SAME cm UVs (`build_mesh`'s `1.0 - v`), so feeding the unflipped
    // alpha makes the bake sample exactly what the renderer does.
    let mut alpha: Vec<f32> = rgba.pixels().map(|p| p.0[3] as f32 / 255.0).collect();
    despeckle_alpha(&mut alpha, w, h, img::MASK_CUTOFF, label);
    erode_alpha(&mut alpha, w, h, img::MASK_CUTOFF, erode_px);

    // NO V flip — verified against the RENDER (ground truth): flipping V makes the
    // cutout render upside-down; the un-flipped cm UVs sample the same texel the
    // runtime does. The OMM SDK's `ommDebugSaveAsImages` debug dump draws its overlay
    // mirrored from the texture it displays, so a correct bake LOOKS mirrored in the
    // dump (green opposite the white) — the dump is not trustworthy for V alignment;
    // the render is.
    let uvs: Vec<f32> = cm.vertex_uvs().iter().flat_map(|uv| [uv.x, uv.y]).collect();
    if uvs.is_empty() {
        return None;
    }
    // `cm.indices()` are CLUSTER-LOCAL (each cluster's triangles index into its own
    // vertex slice starting at `vertex_offset`), but `cm.vertex_uvs()` is the GLOBAL
    // per-cluster-duplicated array. Feeding local indices into the global UVs makes
    // every cluster but the first read cluster-0's UVs → the mask piles onto the
    // wrong texture region ("green on the sides"). Rebuild GLOBAL indices by adding
    // each cluster's `vertex_offset`, walking clusters in `index_offset` order so the
    // flat triangle order (and thus the per-triangle OMM index) still matches the
    // GPU's `index_offset/3` cluster slicing.
    let src = cm.indices();
    let mut indices: Vec<u32> = Vec::with_capacity(src.len());
    for cluster in cm.clusters() {
        let base = cluster.vertex_offset;
        let start = cluster.index_offset as usize;
        let end = start + cluster.triangle_count as usize * 3;
        for &local in &src[start..end] {
            indices.push(base + local);
        }
    }

    // REPEAT to match the runtime sampler the `.bsn` sets on these images.
    // 2-state (not 4-state): the micromap has NO "unknown" state, so every
    // micro-triangle resolves to opaque/transparent at bake time (promotion) and
    // the GPU never invokes the any-hit — zero any-hit on the cutout. Trades the
    // 4-state edge band's per-pixel alpha test for a micro-tri-quantized edge
    // (fine at subdiv 9, hidden by DLSS).
    let bake = omm::bake(
        &alpha, w, h, &uvs, &indices,
        img::MASK_CUTOFF,
        omm::OMM_FORMAT_OC1_2_STATE,
        subdiv,
        true,
    )
    .ok()?;

    if bake.descs.is_empty() {
        return None; // wholly uniform (all opaque/transparent) — special indices only
    }

    // Bake-quality: pure opaque/transparent micro-tris skip the any-hit; "unknown"
    // still invokes it. A high unknown fraction means the OMM gives little accel.
    let known = bake.stat_opaque + bake.stat_transparent;
    let unknown = bake.stat_unknown_opaque + bake.stat_unknown_transparent;
    let total = (known + unknown).max(1);
    println!(
        "  OMM stats: opaque={} transparent={} unknown={} ({:.0}% known, area_metric={:.3})",
        bake.stat_opaque,
        bake.stat_transparent,
        unknown,
        100.0 * known as f64 / total as f64,
        bake.known_area_metric,
    );

    let stats = (bake.descs.len(), bake.array_data.len());
    let descs = bake
        .descs
        .iter()
        .map(|&(offset, subdivision_level, format)| OmmDesc { offset, subdivision_level, format })
        .collect();
    let usage = bake.desc_histogram.iter().map(to_bevy_usage).collect();
    let index_usage = bake.index_histogram.iter().map(to_bevy_usage).collect();
    cm.set_opacity_micromap(bake.array_data, descs, bake.indices, usage, index_usage);
    Some(stats)
}

fn to_bevy_usage(u: &omm::OmmUsage) -> OmmUsage {
    OmmUsage {
        count: u.count,
        subdivision_level: u.subdivision_level,
        format: u.format,
    }
}

/// Erode the opaque mask inward by `erode_px` texels (in place). An opaque texel within that
/// Chebyshev distance of any transparent texel (or the texture border) is set transparent —
/// counters the conservative 2-state silhouette bleed and clears edge-hugging opaque strips.
/// Operates as N single-texel 8-neighbor erosion passes; cheap for the small radii used.
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
                // Border or any transparent 8-neighbor → erode this texel.
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

/// Remove small opaque islands from the alpha mask (in place) before the OMM bake.
/// Binarize at `cutoff`, connected-component label the opaque texels (8-connectivity,
/// union-find), then zero every island below [`MIN_ISLAND_FRACTION`] of the largest.
/// Because the 2-state OMM *is* the cutout, dropping an island here also removes it
/// from the render. No-op when the fraction is `0.0` or the mask is empty.
fn despeckle_alpha(alpha: &mut [f32], w: u32, h: u32, cutoff: f32, tex: &str) {
    if MIN_ISLAND_FRACTION <= 0.0 {
        return;
    }
    let (w, h) = (w as usize, h as usize);
    let n = w * h;
    debug_assert_eq!(alpha.len(), n);

    let opaque = |a: &[f32], i: usize| a[i] >= cutoff;

    // Union-find with path halving.
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

    // Forward-only neighbor scan (right, down, down-right, down-left) covers all
    // 8-connected pairs exactly once.
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

    // Area per root.
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

    // Zero texels in islands below the threshold.
    let (mut removed_islands, mut kept_islands, mut removed_px) = (0u32, 0u32, 0u64);
    for r in 0..n {
        if area[r] > 0 {
            if area[r] < threshold {
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
