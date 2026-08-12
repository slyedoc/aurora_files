//! Skeleton and bone structures for MakeHuman characters

use bevy::math::{ToPrecision, ToRender};
use bevy::{math::Affine3A, platform::collections::HashMap, prelude::*};

// TODO: Clean this up, shouldn't be storing all this and using strings for everything...
/// Component storing character skeleton - bones, hierarchy, bind pose
#[derive(Component, Clone)]
pub struct Skeleton {
    /// Bone definitions (name, head, tail, roll)
    pub bones: Vec<Bone>,
    /// Parent indices - hierarchy[i] = parent bone index for bone i
    pub hierarchy: Vec<Option<usize>>,
    /// Bind pose transforms (T-pose) - LOCAL space
    pub bind_pose: Vec<Transform>,
    /// Global bind rotations - for converting world-space to local-space animations
    pub global_bind_rotations: Vec<Quat>,
    /// Inverse bind pose matrices (for skinning)
    pub inverse_bind_matrices: Vec<Mat4>,
    /// Bone name → index lookup
    pub bone_indices: HashMap<String, usize>,
}

/// Single bone definition
#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub head: Vec3, // Start position in mesh space
    pub tail: Vec3, // End position in mesh space
    pub roll: f32,  // Twist rotation around bone axis (radians)
}

impl Bone {
    /// Get bone direction vector (normalized)
    pub fn direction(&self) -> Vec3 {
        (self.tail - self.head).normalize()
    }

    /// Get bone length
    pub fn length(&self) -> f32 {
        self.head.distance(self.tail)
    }

    /// Create transform for this bone in bind pose.
    ///
    /// Uses identity rotation — matches SMPL's convention where all joints
    /// have identity global rotation in the T-pose.
    pub fn bind_transform(&self) -> Transform {
        Transform::from_translation(self.head.to_precision())
    }
}

impl Skeleton {
    /// Create new skeleton from bones and hierarchy
    pub fn new(bones: Vec<Bone>, hierarchy: Vec<Option<usize>>) -> Self {
        assert_eq!(
            bones.len(),
            hierarchy.len(),
            "Bones and hierarchy must have same length"
        );

        // Build bone name → index lookup
        let bone_indices: HashMap<String, usize> = bones
            .iter()
            .enumerate()
            .map(|(i, bone)| (bone.name.clone(), i))
            .collect();

        // Calculate GLOBAL bind pose transforms (identity rotation, positioned at joint)
        let global_bind_pose: Vec<Transform> =
            bones.iter().map(|bone| bone.bind_transform()).collect();

        // Store global bind rotations for animation conversion
        let global_bind_rotations: Vec<Quat> =
            global_bind_pose.iter().map(|t| t.rotation.to_render()).collect();

        // Calculate LOCAL transforms relative to parent
        // For skinning entities in a hierarchy, we need local transforms
        let mut bind_pose = vec![Transform::IDENTITY; bones.len()];
        for (bone_idx, &parent_idx_opt) in hierarchy.iter().enumerate() {
            let global = global_bind_pose[bone_idx];
            if let Some(parent_idx) = parent_idx_opt {
                // Local = inverse(parent_global) * global
                let parent_global = global_bind_pose[parent_idx];
                let parent_inv = parent_global.compute_affine().inverse();
                let local_affine = parent_inv * global.compute_affine();
                bind_pose[bone_idx] = Transform::from_matrix(local_affine.into());
            } else {
                // Root bone - local == global
                bind_pose[bone_idx] = global;
            }
        }

        // Calculate inverse bind matrices from GLOBAL transforms
        // This is what GPU skinning needs: inverse of world-space bind pose
        let inverse_bind_matrices: Vec<Mat4> = global_bind_pose
            .iter()
            .map(|transform| {
                let affine = Affine3A::from_scale_rotation_translation(
                    transform.scale.to_render(),
                    transform.rotation.to_render(),
                    transform.translation.to_render(),
                );
                Mat4::from(affine.inverse())
            })
            .collect();

        Self {
            bones,
            hierarchy,
            bind_pose,
            global_bind_rotations,
            inverse_bind_matrices,
            bone_indices,
        }
    }

    /// Find bone index by name
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bone_indices.get(name).copied()
    }

    /// Get bone by name
    pub fn bone(&self, name: &str) -> Option<&Bone> {
        self.bone_index(name).map(|idx| &self.bones[idx])
    }

    /// Get global transform for bone (parent chain multiplied)
    pub fn global_transform(&self, bone_idx: usize, local_transforms: &[Transform]) -> Transform {
        let mut transform = local_transforms[bone_idx];
        let mut current = bone_idx;

        // Walk up parent chain, multiplying transforms
        while let Some(parent_idx) = self.hierarchy[current] {
            transform = local_transforms[parent_idx] * transform;
            current = parent_idx;
        }

        transform
    }

    /// Apply pose (local bone transforms) and compute global transforms
    pub fn compute_global_transforms(&self, local_transforms: &[Transform]) -> Vec<Mat4> {
        let mut global_matrices = vec![Mat4::IDENTITY; self.bones.len()];

        for bone_idx in 0..self.bones.len() {
            let global_transform = self.global_transform(bone_idx, local_transforms);
            let affine = Affine3A::from_scale_rotation_translation(
                global_transform.scale.to_render(),
                global_transform.rotation.to_render(),
                global_transform.translation.to_render(),
            );
            global_matrices[bone_idx] = Mat4::from(affine);
        }

        global_matrices
    }

    /// Apply pose and compute skinning matrices (global * inverse_bind)
    pub fn compute_skinning_matrices(&self, local_transforms: &[Transform]) -> Vec<Mat4> {
        let global_matrices = self.compute_global_transforms(local_transforms);

        global_matrices
            .iter()
            .zip(&self.inverse_bind_matrices)
            .map(|(global, inv_bind)| *global * *inv_bind)
            .collect()
    }

    /// Apply skinning to mesh vertices (CPU implementation)
    ///
    /// vertex_weights: per-vertex list of (bone_idx, weight) pairs
    /// bind_vertices: original vertex positions in bind pose
    /// local_transforms: current bone transforms (pose)
    ///
    /// Returns: deformed vertex positions
    pub fn apply_skinning(
        &self,
        vertex_weights: &[Vec<(usize, f32)>],
        bind_vertices: &[Vec3],
        local_transforms: &[Transform],
    ) -> Vec<Vec3> {
        let skinning_matrices = self.compute_skinning_matrices(local_transforms);

        bind_vertices
            .iter()
            .zip(vertex_weights)
            .map(|(&bind_pos, weights)| {
                if weights.is_empty() {
                    // No skinning weights, use bind position
                    return bind_pos;
                }

                // Apply weighted blend of bone transforms
                let mut skinned_pos = Vec3::ZERO;
                for &(bone_idx, weight) in weights {
                    if bone_idx < skinning_matrices.len() && weight > 1e-6 {
                        // Transform vertex by bone matrix
                        let transformed = skinning_matrices[bone_idx].transform_point3(bind_pos);
                        skinned_pos += transformed * weight;
                    }
                }

                skinned_pos
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bone_direction() {
        let bone = Bone {
            name: "test".to_string(),
            head: Vec3::ZERO,
            tail: Vec3::new(0.0, 1.0, 0.0),
            roll: 0.0,
        };

        assert_eq!(bone.direction(), Vec3::Y);
        assert_eq!(bone.length(), 1.0);
    }

    #[test]
    fn test_skeleton_hierarchy() {
        let bones = vec![
            Bone {
                name: "root".to_string(),
                head: Vec3::ZERO,
                tail: Vec3::Y,
                roll: 0.0,
            },
            Bone {
                name: "child".to_string(),
                head: Vec3::Y,
                tail: Vec3::new(0.0, 2.0, 0.0),
                roll: 0.0,
            },
        ];

        let hierarchy = vec![None, Some(0)]; // child's parent is root
        let skeleton = Skeleton::new(bones, hierarchy);

        assert_eq!(skeleton.bones.len(), 2);
        assert_eq!(skeleton.bone_index("root"), Some(0));
        assert_eq!(skeleton.bone_index("child"), Some(1));
        assert_eq!(skeleton.hierarchy[1], Some(0));
    }
}
