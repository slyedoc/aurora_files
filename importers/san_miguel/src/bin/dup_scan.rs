//! Scan the San Miguel OBJ for repeated geometry (instance candidates) and CONFIRM them with a
//! Kabsch (rigid + uniform scale) fit.
//!
//! Pass 1 — TOPOLOGY buckets by `(vertex count, triangle count, diffuse texture)`. Rotation /
//! scale / translation don't change topology, so copy-rotated furniture (the chairs/place-settings
//! around a table) lands in the same bucket even though the baked `.cluster_mesh` bytes differ.
//! That's an UPPER BOUND: a shared signature is necessary but not sufficient (coincidental
//! collisions, esp. untextured props).
//!
//! Pass 2 — KABSCH verify. Within a bucket, fit every member onto the reference under index
//! correspondence (instances exported from one source mesh keep vertex order), recovering the best
//! rotation (Horn's quaternion method — dominant eigenvector of a 4×4 via power iteration, no SVD
//! dep), uniform scale, and translation. The relative RMS residual gates it: ~0 ⇒ genuine instance
//! (and we'd have its exact `Transform`); large ⇒ coincidental, bake unique. Tiny residuals on an
//! obviously-identical group (plates ×84) also confirm the correspondence assumption holds.
//!
//!   cargo run --release --bin dup_scan -- raw/San_Miguel/san-miguel-low-poly.obj

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::path::PathBuf;

use aurora_bsn::dedup::{kabsch_rel_residual, positions};

/// Relative-RMS-residual gate below which a Kabsch fit counts as the same shape (fraction of the
/// object's RMS radius). Genuine instances sit at float noise (~1e-5); different shapes are >0.1.
const EPS: f64 = 1e-2;

fn main() {
    let obj = PathBuf::from(std::env::args().nth(1).expect("usage: <obj>"));
    println!("loading {}", obj.display());
    let (models, materials) = tobj::load_obj(
        &obj,
        &tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ignore_points: true,
            ignore_lines: true,
        },
    )
    .expect("load obj");
    let materials = materials.unwrap_or_default();
    println!("{} submeshes", models.len());

    // Pass 1: topology buckets → model indices.
    let mut buckets: BTreeMap<(usize, usize, String), Vec<usize>> = BTreeMap::new();
    for (i, m) in models.iter().enumerate() {
        let vtx = m.mesh.positions.len() / 3;
        let tris = m.mesh.indices.len() / 3;
        let tex = m
            .mesh
            .material_id
            .and_then(|id| materials.get(id))
            .and_then(|mat| mat.diffuse_texture.as_deref())
            .map(|t| t.rsplit(['/', '\\']).next().unwrap_or(t).to_string())
            .unwrap_or_else(|| "<none>".into());
        buckets.entry((vtx, tris, tex)).or_default().push(i);
    }

    let mut groups: Vec<_> = buckets.into_iter().filter(|(_, v)| v.len() > 1).collect();
    groups.sort_by_key(|(_, v)| Reverse(v.len()));

    let cand_involved: usize = groups.iter().map(|(_, v)| v.len()).sum();
    let cand_shareable: usize = groups.iter().map(|(_, v)| v.len() - 1).sum();
    println!(
        "\nPASS 1 (topology):  {} candidate groups | {} submeshes | {} shareable upper bound ({:.1}%)",
        groups.len(),
        cand_involved,
        cand_shareable,
        cand_shareable as f64 * 100.0 / models.len() as f64,
    );

    // Pass 2: Kabsch-verify every group; print the largest ones with confirmed counts + residuals.
    println!("\nPASS 2 (Kabsch verify, rigid+scale, index correspondence, eps={EPS:.0e}):");
    println!("  conf/cand  verts   tris   texture                        worstResid(conf) / minResid(rej)");

    let mut confirmed_shareable = 0usize;
    let mut confirmed_groups = 0usize;
    for (gi, ((vtx, tris, tex), idxs)) in groups.iter().enumerate() {
        let refp = positions(&models[idxs[0]]);
        let mut confirmed = 1usize; // the reference itself
        let mut worst_conf = 0.0f64;
        let mut min_rej = f64::INFINITY;
        for &j in &idxs[1..] {
            let res = kabsch_rel_residual(&refp, &positions(&models[j]));
            if res < EPS {
                confirmed += 1;
                worst_conf = worst_conf.max(res);
            } else {
                min_rej = min_rej.min(res);
            }
        }
        if confirmed >= 2 {
            confirmed_groups += 1;
            confirmed_shareable += confirmed - 1;
        }
        if gi < 45 {
            let rej = if min_rej.is_finite() {
                format!("{min_rej:.2e}")
            } else {
                "—".into()
            };
            println!(
                "  {:>3}/{:<3} {:>6} {:>6}   {:<30} {:.2e} / {}",
                confirmed,
                idxs.len(),
                vtx,
                tris,
                tex,
                worst_conf,
                rej,
            );
        }
    }

    println!(
        "\nCONFIRMED: {} groups, {} shareable submeshes ({:.1}% of {}) → ~{} unique meshes after dedup",
        confirmed_groups,
        confirmed_shareable,
        confirmed_shareable as f64 * 100.0 / models.len() as f64,
        models.len(),
        models.len() - confirmed_shareable,
    );
}
