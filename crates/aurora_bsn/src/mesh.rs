//! Build a bevy [`Mesh`] from a tobj submesh.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};

use crate::dedup::V3;

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
