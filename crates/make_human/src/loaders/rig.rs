//! Rig definition loader - parses MakeHuman rig JSON files

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};
use serde::Deserialize;
use thiserror::Error;

use crate::{
    VertexGroups,
    skeleton::{Bone, Skeleton},
};

/// Rig asset - bone definitions from MakeHuman JSON
#[derive(Asset, TypePath, Debug, Clone, Deref, DerefMut)]
pub struct RigBones(pub HashMap<String, BoneDefinition>);

/// Single bone definition from JSON
#[derive(Debug, Clone, Deserialize)]
pub struct BoneDefinition {
    pub head: PositionDefinition,
    pub tail: PositionDefinition,
    #[serde(default)]
    pub parent: Option<String>,
    pub roll: f32,
    #[serde(default)]
    pub use_connect: bool,
}

/// Position definition - how to calculate bone position
#[derive(Debug, Clone, Deserialize)]
pub struct PositionDefinition {
    pub strategy: String, // "CUBE", "VERTEX", "MEAN", "XYZ", "MH_DEFAULT"
    #[serde(default)]
    pub cube_name: Option<String>,
    #[serde(default)]
    pub vertex_index: Option<usize>,
    #[serde(default)]
    pub vertex_indices: Option<Vec<usize>>,
    #[serde(default)]
    pub default_position: Option<[f32; 3]>,
    #[serde(default)]
    pub offset: Option<[f32; 3]>,
}

impl PositionDefinition {
    /// Calculate position using strategy
    pub fn calculate(&self, mesh_vertices: &[Vec3], vertex_groups: &VertexGroups) -> Vec3 {
        let pos = match self.strategy.as_str() {
            "CUBE" => {
                // Average all vertices in named vertex group (joint cube)
                if let Some(cube_name) = &self.cube_name {
                    if let Some(ranges) = vertex_groups.get(cube_name) {
                        if !ranges.is_empty() {
                            let indices = VertexGroups::expand_ranges(ranges);
                            let sum: Vec3 = indices.iter().map(|&idx| mesh_vertices[idx]).sum();
                            sum / indices.len() as f32
                        } else {
                            warn!("Vertex group '{}' has empty ranges, using default_position (MH coords!)", cube_name);
                            self.default_position
                                .map(|p| Vec3::from(p))
                                .unwrap_or(Vec3::ZERO)
                        }
                    } else {
                        warn!("Vertex group '{}' not found, using default_position (MH coords!)", cube_name);
                        self.default_position
                            .map(|p| Vec3::from(p))
                            .unwrap_or(Vec3::ZERO)
                    }
                } else {
                    Vec3::ZERO
                }
            }
            "VERTEX" => {
                // Use specific vertex position
                if let Some(idx) = self.vertex_index {
                    if idx < mesh_vertices.len() {
                        mesh_vertices[idx]
                    } else {
                        warn!("Vertex index {} out of range", idx);
                        Vec3::ZERO
                    }
                } else {
                    Vec3::ZERO
                }
            }
            "MEAN" => {
                // Average multiple vertices
                if let Some(indices) = &self.vertex_indices {
                    if !indices.is_empty() {
                        let sum: Vec3 = indices
                            .iter()
                            .filter(|&&idx| idx < mesh_vertices.len())
                            .map(|&idx| mesh_vertices[idx])
                            .sum();
                        sum / indices.len() as f32
                    } else {
                        Vec3::ZERO
                    }
                } else {
                    Vec3::ZERO
                }
            }
            "XYZ" => {
                // Use different vertices for X, Y, Z (rare)
                self.default_position
                    .map(|p| Vec3::from(p))
                    .unwrap_or(Vec3::ZERO)
            }
            "MH_DEFAULT" => {
                // Use default_position directly, converting from MH internal
                // coords (X-left, Y-forward, Z-up) to Bevy space (X-right,
                // Y-up, Z-backward). mesh_vertices are already in Bevy space
                // (OBJ × 0.1 with X/Z negated), so we apply the same transform.
                // MH internal → Bevy: x' = -x, y' = z, z' = -y  (no scale
                // needed since MH internal × 10 = OBJ, OBJ × 0.1 = Bevy)
                self.default_position
                    .map(|p| Vec3::new(-p[0], p[2], -p[1]))
                    .unwrap_or(Vec3::ZERO)
            }
            _ => {
                warn!("Unknown position strategy: {}", self.strategy);
                Vec3::ZERO
            }
        };

        // Apply offset if specified
        if let Some(offset) = self.offset {
            pos + Vec3::from(offset)
        } else {
            pos
        }
    }
}

impl RigBones {
    /// Build skeleton from rig definition + morphed helpers + vertex groups
    pub fn build_skeleton(&self, mesh_vertices: &[Vec3], vertex_groups: &VertexGroups) -> Skeleton {
        let (bones, hierarchy) = self.build_bones_and_hierarchy(mesh_vertices, vertex_groups);
        Skeleton::new(bones, hierarchy)
    }

    fn build_bones_and_hierarchy(
        &self,
        mesh_vertices: &[Vec3],
        vertex_groups: &VertexGroups,
    ) -> (Vec<Bone>, Vec<Option<usize>>) {
        // Build bones in sorted order (ensure parents before children)
        let mut bone_names: Vec<String> = self.keys().cloned().collect();
        bone_names.sort();

        // Build bone index lookup
        let bone_indices: HashMap<String, usize> = bone_names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), i))
            .collect();

        // Create bones
        let mut bones = Vec::new();
        let mut hierarchy = Vec::new();

        for bone_name in &bone_names {
            let def = &self[bone_name];

            // Calculate head and tail positions
            let head = def.head.calculate(mesh_vertices, vertex_groups);
            let tail = def.tail.calculate(mesh_vertices, vertex_groups);

            bones.push(Bone {
                name: bone_name.clone(),
                head,
                tail,
                roll: def.roll,
            });

            // Build hierarchy
            let parent_idx = def
                .parent
                .as_ref()
                .and_then(|p| bone_indices.get(p).copied());
            hierarchy.push(parent_idx);
        }

        (bones, hierarchy)
    }
}

/// Asset loader for rig JSON files
#[derive(Default, TypePath)]
pub struct RigLoader;

#[derive(Debug, Error)]
pub enum RigLoaderError {
    #[error("Failed to load rig: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse rig JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl AssetLoader for RigLoader {
    type Asset = RigBones;
    type Settings = ();
    type Error = RigLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let bones: HashMap<String, BoneDefinition> = serde_json::from_slice(&bytes)?;

        Ok(RigBones(bones))
    }

    fn extensions(&self) -> &[&str] {
        &["rig.json"] // Filter by filename in load()
    }
}
