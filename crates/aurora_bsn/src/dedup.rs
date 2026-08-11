//! Instance recovery: verify topology-bucketed submeshes are genuine rigid + uniform-scale copies
//! and recover the placing transform. Rotation via Horn's unit-quaternion method (dominant
//! eigenvector of a 4×4 via shifted power iteration — no SVD dependency). Shared by the importer's
//! dedup pass (currently disabled) and the `dup_scan` measurement binary.

use crate::img;

pub type V3 = [f64; 3];

/// Relative-RMS-residual gate below which a Kabsch fit counts as the same shape (a fraction of the
/// object's RMS radius). Genuine instances sit at float noise (~1e-4); different shapes are >1e-2 —
/// a wide gap, so this isn't sensitive. A pass also guarantees the recovered transform reproduces
/// the instance to within `EPS * rms_radius`, so dedup placement is correct by construction.
pub const EPS: f64 = 1e-2;

/// Fit of reference points onto candidate points: relative RMS residual (≈0 ⇒ same shape) plus the
/// rotation (quaternion `[w,x,y,z]`) and uniform scale mapping reference → candidate.
pub struct KabschFit {
    pub residual: f64,
    pub rotation: [f64; 4],
    pub scale: f64,
}

/// Vertex positions of a tobj submesh as `f64` triples.
pub fn positions(m: &tobj::Model) -> Vec<V3> {
    m.mesh
        .positions
        .chunks_exact(3)
        .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
        .collect()
}

/// Diffuse-texture basename used in the dedup bucket key (`<none>` when untextured). tobj's
/// near-per-submesh material ids can't group instances; the texture does.
pub fn diffuse_key(m: &tobj::Model, materials: &[tobj::Material]) -> String {
    m.mesh
        .material_id
        .and_then(|id| materials.get(id))
        .and_then(|mat| mat.diffuse_texture.as_deref())
        .map(|t| img::basename(t).to_string())
        .unwrap_or_else(|| "<none>".into())
}

/// Whether two submeshes have matching texcoords under index correspondence (empty on both counts
/// as a match). Guards mesh sharing: an instance must reuse the owner's UVs exactly.
pub fn uvs_match(a: &tobj::Mesh, b: &tobj::Mesh) -> bool {
    a.texcoords.len() == b.texcoords.len()
        && a.texcoords.iter().zip(&b.texcoords).all(|(x, y)| (x - y).abs() < 1e-4)
}

pub fn centroid(p: &[V3]) -> V3 {
    let n = p.len() as f64;
    let mut c = [0.0; 3];
    for v in p {
        c[0] += v[0];
        c[1] += v[1];
        c[2] += v[2];
    }
    [c[0] / n, c[1] / n, c[2] / n]
}

pub fn v_sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn v_dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn mat_vec(m: &[[f64; 3]; 3], v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// Best similarity (rotation + uniform scale + translation) fit mapping `p` onto `q` under index
/// correspondence (instances export with matching vertex order). Rotation via Horn's unit-quaternion
/// method — the dominant eigenvector of a 4×4, found by shifted power iteration.
pub fn kabsch_fit(p: &[V3], q: &[V3]) -> KabschFit {
    let n = p.len();
    let bad = KabschFit {
        residual: f64::INFINITY,
        rotation: [1.0, 0.0, 0.0, 0.0],
        scale: 1.0,
    };
    if n == 0 || n != q.len() {
        return bad;
    }
    let (cp, cq) = (centroid(p), centroid(q));
    let pc: Vec<V3> = p.iter().map(|v| v_sub(*v, cp)).collect();
    let qc: Vec<V3> = q.iter().map(|v| v_sub(*v, cq)).collect();
    let mut h = [[0.0f64; 3]; 3];
    let (mut sp, mut sq) = (0.0, 0.0);
    for i in 0..n {
        let (a, b) = (pc[i], qc[i]);
        for r in 0..3 {
            for c in 0..3 {
                h[r][c] += a[r] * b[c];
            }
        }
        sp += v_dot(a, a);
        sq += v_dot(b, b);
    }
    if sp == 0.0 || sq == 0.0 {
        return bad;
    }
    let (sxx, sxy, sxz) = (h[0][0], h[0][1], h[0][2]);
    let (syx, syy, syz) = (h[1][0], h[1][1], h[1][2]);
    let (szx, szy, szz) = (h[2][0], h[2][1], h[2][2]);
    let nm = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let shift = 1.0
        + 2.0
            * (sxx.abs() + syy.abs() + szz.abs() + sxy.abs() + sxz.abs() + syx.abs() + syz.abs()
                + szx.abs()
                + szy.abs());
    let mut v = [1.0, 0.3, -0.2, 0.1];
    for _ in 0..160 {
        let mut w = [0.0f64; 4];
        for r in 0..4 {
            let mut acc = shift * v[r];
            for c in 0..4 {
                acc += nm[r][c] * v[c];
            }
            w[r] = acc;
        }
        let norm = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2] + w[3] * w[3]).sqrt();
        if norm == 0.0 {
            break;
        }
        for r in 0..4 {
            v[r] = w[r] / norm;
        }
    }
    let (qw, qx, qy, qz) = (v[0], v[1], v[2], v[3]);
    let rotm = [
        [1.0 - 2.0 * (qy * qy + qz * qz), 2.0 * (qx * qy - qw * qz), 2.0 * (qx * qz + qw * qy)],
        [2.0 * (qx * qy + qw * qz), 1.0 - 2.0 * (qx * qx + qz * qz), 2.0 * (qy * qz - qw * qx)],
        [2.0 * (qx * qz - qw * qy), 2.0 * (qy * qz + qw * qx), 1.0 - 2.0 * (qx * qx + qy * qy)],
    ];
    let mut num = 0.0;
    for i in 0..n {
        num += v_dot(qc[i], mat_vec(&rotm, pc[i]));
    }
    let s = num / sp;
    let mut err = 0.0;
    for i in 0..n {
        let ra = mat_vec(&rotm, pc[i]);
        let d = [s * ra[0] - qc[i][0], s * ra[1] - qc[i][1], s * ra[2] - qc[i][2]];
        err += v_dot(d, d);
    }
    let residual = (err / n as f64).sqrt() / (sq / n as f64).sqrt();
    KabschFit {
        residual,
        rotation: [qw, qx, qy, qz],
        scale: s,
    }
}

/// Relative RMS residual of the best similarity fit mapping `p` onto `q` (just the residual from
/// [`kabsch_fit`]). ~0 ⇒ congruent shapes (one is a rigid+scale copy of the other).
pub fn kabsch_rel_residual(p: &[V3], q: &[V3]) -> f64 {
    kabsch_fit(p, q).residual
}
