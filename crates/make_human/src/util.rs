//! Skinning weights loader - parses MakeHuman weight JSON files

use bevy::{
    asset::RenderAssetUsages,
    mesh::{PrimitiveTopology, VertexAttributeValues},
    platform::collections::HashMap,
    prelude::*,
};

use crate::{loaders::*, skeleton::Skeleton};

/// Apply skinning weights to proxy mesh via barycentric interpolation
/// Proxy vertices map to base mesh triangles, so we blend weights from 3 base verts
pub fn apply_skinning_weights_to_proxy(
    mut mesh: Mesh,
    proxy: &ProxyAsset,
    mhid_lookup: &[u16],
    skeleton: &Skeleton,
    skinning_weights: &SkinningWeights,
) -> Mesh {
    // Allocate for all base mesh vertices the weights file references
    let max_weight_vertex = skinning_weights.max_vertex_index();
    let vertex_count = max_weight_vertex + 1;

    // Convert sparse weights to per-vertex format for base mesh
    let base_vertex_weights =
        skinning_weights.to_vertex_weights(&skeleton.bone_indices, vertex_count);

    let mesh_vert_count = mhid_lookup.len();
    let mut indices = vec![[0u16; 4]; mesh_vert_count];
    let mut weights = vec![[0.0f32; 4]; mesh_vert_count];

    // For each mesh vertex, look up the proxy binding (via mhid_lookup -> obj_idx -> binding)
    for (mesh_idx, &obj_idx) in mhid_lookup.iter().enumerate() {
        if (obj_idx as usize) >= proxy.bindings.len() {
            // No binding - default to bone 0
            indices[mesh_idx][0] = 0;
            weights[mesh_idx][0] = 1.0;
            continue;
        }

        let binding = &proxy.bindings[obj_idx as usize];

        // Blend bone weights from the 3 base mesh vertices using barycentric weights
        let mut bone_weights: HashMap<usize, f32> = HashMap::default();

        for (i, &base_vert_idx) in binding.triangle.iter().enumerate() {
            let bary_weight = binding.weights[i];
            if bary_weight < 1e-6 {
                continue;
            }

            // Get weights for this base mesh vertex
            if (base_vert_idx as usize) < base_vertex_weights.len() {
                for &(bone_idx, bone_weight) in &base_vertex_weights[base_vert_idx as usize] {
                    *bone_weights.entry(bone_idx).or_insert(0.0) += bone_weight * bary_weight;
                }
            }
        }

        // Convert to sorted vec and take top 4
        let sorted: Vec<_> = bone_weights.into_iter().collect();
        apply_top4_weights(&sorted, &mut indices[mesh_idx], &mut weights[mesh_idx]);
    }

    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(indices),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(weights),
    );

    mesh
}

/// Average normals for vertices at the same position (fixes UV seam artifacts)
fn average_normals_at_seams(mesh: &mut Mesh) {
    let Some(positions) = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|a| a.as_float3())
    else {
        return;
    };
    let Some(normals) = mesh
        .attribute(Mesh::ATTRIBUTE_NORMAL)
        .and_then(|a| a.as_float3())
    else {
        return;
    };

    // Group vertices by position (quantized to avoid float precision issues)
    let mut pos_to_indices: HashMap<[i32; 3], Vec<usize>> = HashMap::default();
    for (i, pos) in positions.iter().enumerate() {
        // Quantize to ~0.0001 precision
        let key = [
            (pos[0] * 10000.0) as i32,
            (pos[1] * 10000.0) as i32,
            (pos[2] * 10000.0) as i32,
        ];
        pos_to_indices.entry(key).or_default().push(i);
    }

    // Average normals for vertices at same position
    let mut new_normals: Vec<[f32; 3]> = normals.to_vec();
    for indices in pos_to_indices.values() {
        if indices.len() <= 1 {
            continue;
        }

        // Sum all normals at this position
        let mut sum = Vec3::ZERO;
        for &i in indices {
            sum += Vec3::from_array(normals[i]);
        }
        let avg = sum.normalize_or_zero();

        // Apply averaged normal to all vertices at this position
        for &i in indices {
            new_normals[i] = avg.to_array();
        }
    }

    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, new_normals);
}

/// Build fitted mesh from final verts
fn build_fitted_mesh(verts: &[Vec3], original_mesh: &Mesh) -> Mesh {
    let start = std::time::Instant::now();
    let mut new_mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(
        Mesh::ATTRIBUTE_POSITION,
        verts.iter().map(|v| [v.x, v.y, v.z]).collect::<Vec<_>>(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, get_vertex_uvs(original_mesh));

    if let Some(indices) = original_mesh.indices() {
        new_mesh = new_mesh.with_inserted_indices(indices.clone());
    }

    // Compute normals then average at UV seams
    new_mesh = new_mesh.with_computed_area_weighted_normals();
    average_normals_at_seams(&mut new_mesh);

    let result = new_mesh.with_generated_tangents().unwrap_or_else(|e| {
        warn!("Failed tangent gen: {:?}", e);
        // Recreate without tangents
        let mut m = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            verts.iter().map(|v| [v.x, v.y, v.z]).collect::<Vec<_>>(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, get_vertex_uvs(original_mesh));
        if let Some(idx) = original_mesh.indices() {
            m = m.with_inserted_indices(idx.clone());
        }
        m = m.with_computed_area_weighted_normals();
        average_normals_at_seams(&mut m);
        m
    });
    debug!(
        "build_fitted_mesh: {} verts took {:?}",
        verts.len(),
        start.elapsed()
    );
    result
}

/// Apply proxy fitting to mesh (fit proxy body mesh to morphed helpers)
pub fn apply_proxy_fitting(
    mesh: &Mesh,
    proxy: &ProxyAsset,
    base_vertices: &[Vec3],
    obj_verts: &[Vec3],
) -> Mesh {
    if !proxy.bindings.is_empty() {
        let mesh_verts = get_vertex_positions(mesh);

        // Build mhid_lookup
        let vertex_map = generate_vertex_map(&obj_verts, &mesh_verts);
        let mhid_lookup = generate_mhid_lookup(&vertex_map);

        // Each binding describes how to fit one obj vertex from base mesh
        // bindings[i] = how to compute position for obj vertex i
        let transformed = proxy.compute_proxy_vertices(base_vertices);

        if transformed.len() != obj_verts.len() {
            warn!(
                "Proxy binding count mismatch: {} bindings vs {} obj verts",
                transformed.len(),
                obj_verts.len()
            );
        }

        // Apply to mesh verts via mhid_lookup
        let mut final_verts = mesh_verts.clone();
        for (mesh_i, &obj_i) in mhid_lookup.iter().enumerate() {
            if (obj_i as usize) < transformed.len() {
                final_verts[mesh_i] = transformed[obj_i as usize];
            }
        }

        build_fitted_mesh(&final_verts, mesh)
    } else {
        mesh.clone()
    }
}

/// Apply mhclo fitting to mesh
/// `normal_offset` pushes verts outward along surface normal (prevents skin poke-through)
pub fn apply_mhclo_fitting(
    mesh: &Mesh,
    mhclo: &MhcloAsset,
    mhid_lookup: &[u16],
    base_vertices: &[Vec3],
    normal_offset: f32,
) -> Mesh {
    if !mhclo.bindings.is_empty() {
        // Get mesh verts
        let mesh_verts = get_vertex_positions(mesh);

        // Compute transformed verts from bindings (obj space)
        let mut transformed = vec![Vec3::ZERO; mhclo.bindings.len()];
        mhclo.apply_to_base(base_vertices, &mut transformed, normal_offset);

        // Apply to ALL mesh verts via mhid_lookup
        let mut final_verts = mesh_verts.clone();
        for (mesh_i, &obj_i) in mhid_lookup.iter().enumerate() {
            if (obj_i as usize) < transformed.len() {
                final_verts[mesh_i] = transformed[obj_i as usize];
            }
        }

        // Build mesh with all verts transformed
        build_fitted_mesh(&final_verts, mesh)
    } else if mhclo.has_vertex_mapping() {
        // Simple vertex mapping (eyes/teeth) - use mhid_lookup like barycentric path
        let mesh_verts = get_vertex_positions(mesh);
        let mapped = mhclo.apply_vertex_mapping(base_vertices);

        // Apply via mhid_lookup to handle UV seam vertex duplication
        let mut final_verts = mesh_verts.clone();
        for (mesh_i, &obj_i) in mhid_lookup.iter().enumerate() {
            if (obj_i as usize) < mapped.len() {
                final_verts[mesh_i] = mapped[obj_i as usize];
            }
        }

        build_fitted_mesh(&final_verts, mesh)
    } else {
        mesh.clone()
    }
}

// Maps bevy vertex ids to mh id
pub(crate) fn generate_mhid_lookup(map: &HashMap<u16, Vec<u16>>) -> Vec<u16> {
    let max_vert = map
        .values()
        .flat_map(|v| v.iter())
        .max()
        .copied()
        .unwrap_or(0);

    let mut lkup: Vec<u16> = vec![0; max_vert as usize + 1];
    for (&mhv, verts) in map.iter() {
        for &vert in verts.iter() {
            lkup[vert as usize] = mhv;
        }
    }
    lkup
}

// Helper functions for mesh vertex mapping
pub fn get_vertex_positions(mesh: &Mesh) -> Vec<Vec3> {
    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|attr| attr.as_float3())
        .map(|positions| positions.iter().map(|p| Vec3::from_array(*p)).collect())
        .expect("Mesh missing position attribute")
}

pub fn get_vertex_uvs(mesh: &Mesh) -> Vec<[f32; 2]> {
    mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        .and_then(|attr| match attr {
            VertexAttributeValues::Float32x2(v) => Some(v.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn generate_vertex_map(
    obj_vertices: &[Vec3],
    mesh_vertices: &[Vec3],
) -> HashMap<u16, Vec<u16>> {
    let mut vertex_map: HashMap<u16, Vec<u16>> = HashMap::default();

    for (mesh_idx, mesh_vert) in mesh_vertices.iter().enumerate() {
        for (obj_idx, obj_vert) in obj_vertices.iter().enumerate() {
            if (mesh_vert - obj_vert).length() < 0.0001 {
                vertex_map
                    .entry(obj_idx as u16)
                    .or_default()
                    .push(mesh_idx as u16);
                break;
            }
        }
    }
    vertex_map
}

/// Apply skinning weights to accessory mesh via MHCLO bindings
/// Uses mhid_lookup from fitting to map mesh verts -> obj verts -> helper weights
pub fn apply_skinning_weights_via_mhclo(
    mut mesh: Mesh,
    mhclo: &MhcloAsset,
    mhid_lookup: &[u16],
    skeleton: &Skeleton,
    skinning_weights: &SkinningWeights,
) -> Mesh {
    let mesh_vert_count = get_vertex_positions(&mesh).len();

    // Convert sparse weights to per-vertex format for helpers
    // Prefer bindings (complete coverage) over vertex_mapping (may be partial)
    let max_helper_idx = if !mhclo.bindings.is_empty() {
        mhclo
            .bindings
            .iter()
            .flat_map(|b| b.triangle.iter())
            .max()
            .map(|&v| v as usize + 1)
            .unwrap_or(0)
    } else {
        mhclo
            .vertex_mapping
            .iter()
            .max()
            .map(|&v| v as usize + 1)
            .unwrap_or(0)
    };

    let helper_vertex_weights =
        skinning_weights.to_vertex_weights(&skeleton.bone_indices, max_helper_idx);

    let mut indices = vec![[0u16; 4]; mesh_vert_count];
    let mut weights = vec![[0.0f32; 4]; mesh_vert_count];

    // Use bindings if available (covers all verts), otherwise fall back to vertex_mapping
    // Mixed format files (eyelashes02) have both but bindings is complete
    if !mhclo.bindings.is_empty() {
        // Barycentric bindings (clothing/eyebrows/eyelashes)
        for (mesh_idx, &obj_idx) in mhid_lookup.iter().enumerate() {
            if (obj_idx as usize) >= mhclo.bindings.len() {
                continue;
            }
            let binding = &mhclo.bindings[obj_idx as usize];

            // Blend bone weights from 3 helper vertices using barycentric weights
            let mut bone_weights: HashMap<usize, f32> = HashMap::default();

            for (i, &helper_idx) in binding.triangle.iter().enumerate() {
                let bary_weight = binding.weights[i];
                if bary_weight < 1e-6 {
                    continue;
                }

                if (helper_idx as usize) < helper_vertex_weights.len() {
                    for &(bone_idx, bone_weight) in &helper_vertex_weights[helper_idx as usize] {
                        *bone_weights.entry(bone_idx).or_insert(0.0) += bone_weight * bary_weight;
                    }
                }
            }

            // Convert to sorted vec and take top 4
            let sorted: Vec<_> = bone_weights.into_iter().collect();
            apply_top4_weights(&sorted, &mut indices[mesh_idx], &mut weights[mesh_idx]);
        }
    }

    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(indices),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(weights),
    );

    mesh
}

/// Apply morphed vertices to base mesh and add skinning weights
/// Used when no proxy mesh is specified
pub fn apply_morphed_base_mesh(
    base_mesh: &Mesh,
    mhid_lookup: &[u16],
    morphed_vertices: &[Vec3],
    skeleton: &Skeleton,
    skinning_weights: &SkinningWeights,
) -> Mesh {
    let mesh_verts = get_vertex_positions(base_mesh);
    let mesh_vert_count = mesh_verts.len();

    // Apply morphed vertices via mhid_lookup
    let mut final_verts = mesh_verts.clone();
    for (mesh_i, &obj_i) in mhid_lookup.iter().enumerate() {
        if (obj_i as usize) < morphed_vertices.len() {
            final_verts[mesh_i] = morphed_vertices[obj_i as usize];
        }
    }

    // Build mesh with morphed positions
    let mut mesh = build_fitted_mesh(&final_verts, base_mesh);

    // Add skinning weights - base mesh vertices map directly via mhid_lookup
    let max_weight_vertex = skinning_weights.max_vertex_index();
    let vertex_count = max_weight_vertex + 1;
    let base_vertex_weights =
        skinning_weights.to_vertex_weights(&skeleton.bone_indices, vertex_count);

    let mut indices = vec![[0u16; 4]; mesh_vert_count];
    let mut weights = vec![[0.0f32; 4]; mesh_vert_count];

    for (mesh_idx, &obj_idx) in mhid_lookup.iter().enumerate() {
        let base_idx = obj_idx as usize;
        if base_idx < base_vertex_weights.len() {
            let bone_weights: Vec<_> = base_vertex_weights[base_idx].clone();
            apply_top4_weights(
                &bone_weights,
                &mut indices[mesh_idx],
                &mut weights[mesh_idx],
            );
        } else {
            // Default to bone 0
            indices[mesh_idx][0] = 0;
            weights[mesh_idx][0] = 1.0;
        }
    }

    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_INDEX,
        VertexAttributeValues::Uint16x4(indices),
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_JOINT_WEIGHT,
        VertexAttributeValues::Float32x4(weights),
    );

    mesh
}

/// Helper to apply top 4 bone weights to mesh vertex
fn apply_top4_weights(
    bone_weights: &[(usize, f32)],
    indices: &mut [u16; 4],
    weights: &mut [f32; 4],
) {
    let mut sorted = bone_weights.to_vec();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    sorted.truncate(4);

    let sum: f32 = sorted.iter().map(|(_, w)| w).sum();

    if sum > 1e-6 {
        for (i, &(bone_idx, weight)) in sorted.iter().enumerate() {
            indices[i] = bone_idx as u16;
            weights[i] = weight / sum;
        }
        // Fill remaining with first bone (zero weight)
        for i in sorted.len()..4 {
            indices[i] = indices[0];
            weights[i] = 0.0;
        }
    } else {
        // No weights - default to bone 0
        indices[0] = 0;
        weights[0] = 1.0;
    }
}

/// Transfer weights from helpers to proxy/accessory mesh vertices
/// Uses proximity-based weight blending from nearby helper vertices
pub fn transfer_weights_from_helpers(
    asset_vertices: &[Vec3],
    morphed_helpers: &[Vec3],
    helper_weights: &SkinningWeights,
    bone_indices: &HashMap<String, usize>,
) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
    let mut vertex_indices = Vec::with_capacity(asset_vertices.len());
    let mut vertex_weights = Vec::with_capacity(asset_vertices.len());

    // Convert helper weights to per-vertex format
    let helper_vertex_weights =
        helper_weights.to_vertex_weights(bone_indices, morphed_helpers.len());

    for asset_vert in asset_vertices {
        // Find 3 closest helpers for interpolation
        let influences = find_closest_helpers(asset_vert, morphed_helpers, 3);

        // Blend bone weights from nearby helpers
        let mut bone_weights: HashMap<usize, f32> = HashMap::new();

        for (helper_id, influence_weight) in influences {
            if helper_id < helper_vertex_weights.len() {
                for &(bone_idx, bone_weight) in &helper_vertex_weights[helper_id] {
                    *bone_weights.entry(bone_idx).or_insert(0.0) += bone_weight * influence_weight;
                }
            }
        }

        // Get top 4 bones (Bevy limit)
        let mut sorted: Vec<_> = bone_weights.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        sorted.truncate(4);

        // Normalize weights
        let sum: f32 = sorted.iter().map(|(_, w)| w).sum();

        let mut indices = [0u16; 4];
        let mut weights = [0f32; 4];

        if sum > 1e-6 {
            for (i, (bone_idx, weight)) in sorted.iter().enumerate() {
                indices[i] = *bone_idx as u16;
                weights[i] = weight / sum;
            }

            // Fill remaining with first bone (zero weight)
            for i in sorted.len()..4 {
                indices[i] = indices[0];
                weights[i] = 0.0;
            }
        } else {
            // No weights found - use default (bone 0)
            indices[0] = 0;
            weights[0] = 1.0;
        }

        vertex_indices.push(indices);
        vertex_weights.push(weights);
    }

    (vertex_indices, vertex_weights)
}

/// Find N closest helper vertices using distance
fn find_closest_helpers(point: &Vec3, helpers: &[Vec3], count: usize) -> Vec<(usize, f32)> {
    // Calculate distances to all helpers
    let mut distances: Vec<(usize, f32)> = helpers
        .iter()
        .enumerate()
        .map(|(idx, helper)| (idx, point.distance_squared(*helper)))
        .collect();

    // Sort by distance
    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    // Take N closest
    distances.truncate(count);

    // Convert to influence weights using inverse distance
    let total_inv_dist: f32 = distances
        .iter()
        .map(|(_, dist)| {
            if *dist < 1e-6 {
                1e6 // Very close = very high weight
            } else {
                1.0 / dist.sqrt()
            }
        })
        .sum();

    distances
        .into_iter()
        .map(|(idx, dist)| {
            let inv_dist = if dist < 1e-6 { 1e6 } else { 1.0 / dist.sqrt() };
            let weight = inv_dist / total_inv_dist;
            (idx, weight)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vertex_weights_normalization() {
        let mut weights = HashMap::new();
        weights.insert("bone_a".to_string(), vec![(0, 0.5), (1, 0.3)]);
        weights.insert("bone_b".to_string(), vec![(0, 0.5), (2, 1.0)]);

        let skinning = SkinningWeights { weights };

        let mut bone_indices = HashMap::new();
        bone_indices.insert("bone_a".to_string(), 0);
        bone_indices.insert("bone_b".to_string(), 1);

        let vertex_weights = skinning.to_vertex_weights(&bone_indices, 3);

        // Vertex 0 should have weights from both bones, normalized
        assert_eq!(vertex_weights[0].len(), 2);
        let sum: f32 = vertex_weights[0].iter().map(|(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-6, "Weights should sum to 1.0");
    }
}
