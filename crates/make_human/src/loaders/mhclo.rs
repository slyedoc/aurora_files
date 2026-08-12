//! .mhclo file parser - clothing/hair asset binding
//!
//! Format: Text-based vertex binding
//! Each line: v0 v1 v2 w0 w1 w2 x_offset y_offset z_offset
//! - v0,v1,v2: base mesh triangle vertex indices
//! - w0,w1,w2: barycentric weights
//! - x,y,z: position offsets

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};
use std::io::{BufRead, BufReader};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct VertexBinding {
    /// Base mesh triangle indices
    pub triangle: [u32; 3],
    /// Barycentric weights
    pub weights: [f32; 3],
    /// Position offset
    pub offset: Vec3,
}

#[derive(Debug, Clone)]
pub struct ScaleRef {
    pub min_vert: u32,
    pub max_vert: u32,
    pub scale: f32,
}

#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct MhcloAsset {
    /// Vertex bindings for each asset vertex
    pub bindings: Vec<VertexBinding>,
    /// Simple vertex mapping (for eyes/teeth) - maps asset vert idx to base mesh vert idx
    pub vertex_mapping: Vec<u32>,
    /// Scale references for each axis
    pub x_scale: Option<ScaleRef>,
    pub y_scale: Option<ScaleRef>,
    pub z_scale: Option<ScaleRef>,
    /// Associated .obj mesh path (relative)
    pub obj_file: Option<String>,
    /// Material file path
    pub material: Option<String>,
    /// Asset name
    pub name: String,
    /// Z-depth ordering
    pub z_depth: u32,
}

#[derive(Default, TypePath)]
pub struct MhcloLoader;

#[derive(Debug, Error)]
pub enum MhcloLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl AssetLoader for MhcloLoader {
    type Asset = MhcloAsset;
    type Settings = ();
    type Error = MhcloLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let mut asset = MhcloAsset::default();
        let buf_reader = BufReader::new(&bytes[..]);
        let mut in_verts_section = false;

        for line_result in buf_reader.lines() {
            let line = line_result?;
            let line_trim = line.trim();

            if line_trim.is_empty() || line_trim.starts_with('#') {
                continue;
            }

            // Check if entering verts section
            if line_trim == "verts 0" {
                in_verts_section = true;
                continue;
            }

            // Parse verts section (can be 9-value barycentric OR single indices)
            // Mixed formats possible (e.g., eyelashes02) - unify into bindings
            if in_verts_section {
                let parts: Vec<&str> = line_trim.split_whitespace().collect();

                if parts.len() == 9 {
                    // Barycentric format: v0 v1 v2 w0 w1 w2 x y z
                    // OR special direct ref: base_idx 0 1 1.0 0.0 0.0 x y z
                    let v0: u32 = parts[0].parse().unwrap_or(0);
                    let v1: u32 = parts[1].parse().unwrap_or(0);
                    let v2: u32 = parts[2].parse().unwrap_or(0);
                    let mut w0: f32 = parts[3].parse().unwrap_or(0.0);
                    let mut w1: f32 = parts[4].parse().unwrap_or(0.0);
                    let mut w2: f32 = parts[5].parse().unwrap_or(0.0);

                    let x: f32 = parts[6].parse().unwrap_or(0.0);
                    let y: f32 = parts[7].parse().unwrap_or(0.0);
                    let z: f32 = parts[8].parse().unwrap_or(0.0);

                    // Detect special "direct vertex with offset" format: v0 0 1 1.0 0.0 0.0 x y z
                    // v1=0, v2=1 are dummy placeholders, v0 is actual base mesh index
                    if v1 == 0
                        && v2 == 1
                        && (w0 - 1.0).abs() < 0.001
                        && w1.abs() < 0.001
                        && w2.abs() < 0.001
                    {
                        asset.bindings.push(VertexBinding {
                            triangle: [v0, v0, v0],
                            weights: [1.0, 0.0, 0.0],
                            offset: Vec3::new(x, y, z),
                        });
                    } else {
                        // Normal barycentric - normalize weights
                        let sum = w0 + w1 + w2;
                        if sum > 0.0 {
                            w0 /= sum;
                            w1 /= sum;
                            w2 /= sum;
                        }

                        asset.bindings.push(VertexBinding {
                            triangle: [v0, v1, v2],
                            weights: [w0, w1, w2],
                            offset: Vec3::new(x, y, z),
                        });
                    }
                } else if parts.len() == 1 {
                    // Simple index mapping - convert to trivial binding for unified handling
                    if let Ok(idx) = parts[0].parse::<u32>() {
                        asset.bindings.push(VertexBinding {
                            triangle: [idx, idx, idx],
                            weights: [1.0, 0.0, 0.0],
                            offset: Vec3::ZERO,
                        });
                        asset.vertex_mapping.push(idx);
                    }
                }
                continue;
            }

            // Parse key-value metadata
            if let Some((key, value)) = line_trim.split_once(' ') {
                match key {
                    "name" => asset.name = value.to_string(),
                    "obj_file" => asset.obj_file = Some(value.to_string()),
                    "material" => asset.material = Some(value.to_string()),
                    "z_depth" => {
                        asset.z_depth = value.parse().unwrap_or(0);
                    }
                    "x_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.x_scale = Some(ScaleRef {
                                min_vert: parts[0].parse().unwrap_or(0),
                                max_vert: parts[1].parse().unwrap_or(0),
                                scale: parts[2].parse().unwrap_or(1.0),
                            });
                        }
                    }
                    "y_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.y_scale = Some(ScaleRef {
                                min_vert: parts[0].parse().unwrap_or(0),
                                max_vert: parts[1].parse().unwrap_or(0),
                                scale: parts[2].parse().unwrap_or(1.0),
                            });
                        }
                    }
                    "z_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.z_scale = Some(ScaleRef {
                                min_vert: parts[0].parse().unwrap_or(0),
                                max_vert: parts[1].parse().unwrap_or(0),
                                scale: parts[2].parse().unwrap_or(1.0),
                            });
                        }
                    }
                    _ if key.parse::<u32>().is_ok() => {
                        // Legacy numbered binding line: idx v0 v1 v2 w0 w1 w2 x y z
                        let parts: Vec<&str> = line_trim.split_whitespace().collect();
                        if parts.len() == 10 {
                            let v0 = parts[1].parse().unwrap_or(0);
                            let v1 = parts[2].parse().unwrap_or(0);
                            let v2 = parts[3].parse().unwrap_or(0);
                            let mut w0 = parts[4].parse().unwrap_or(0.0);
                            let mut w1 = parts[5].parse().unwrap_or(0.0);
                            let mut w2 = parts[6].parse().unwrap_or(0.0);

                            // Normalize weights
                            let sum = w0 + w1 + w2;
                            if sum > 0.0 {
                                w0 /= sum;
                                w1 /= sum;
                                w2 /= sum;
                            }

                            let x = parts[7].parse().unwrap_or(0.0);
                            let y = parts[8].parse().unwrap_or(0.0);
                            let z = parts[9].parse().unwrap_or(0.0);

                            asset.bindings.push(VertexBinding {
                                triangle: [v0, v1, v2],
                                weights: [w0, w1, w2],
                                offset: Vec3::new(x, y, z),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(asset)
    }

    fn extensions(&self) -> &[&str] {
        &["mhclo"]
    }
}

impl MhcloAsset {
    /// Check if this uses simple vertex mapping (eyes/teeth) vs full bindings (clothing)
    pub fn has_vertex_mapping(&self) -> bool {
        !self.vertex_mapping.is_empty()
    }

    /// Apply simple vertex mapping (for eyes/teeth)
    pub fn apply_vertex_mapping(&self, base_vertices: &[Vec3]) -> Vec<Vec3> {
        self.vertex_mapping
            .iter()
            .map(|&idx| {
                base_vertices
                    .get(idx as usize)
                    .copied()
                    .unwrap_or(Vec3::ZERO)
            })
            .collect()
    }

    /// Compute final positions for asset vertices bound to base mesh (for clothing/eyes)
    /// `normal_offset` pushes verts outward along triangle normal (useful to prevent skin poke-through)
    pub fn apply_to_base(
        &self,
        base_vertices: &[Vec3],
        asset_vertices: &mut [Vec3],
        normal_offset: f32,
    ) {
        // MakeHuman uses decimeters, we scale to meters (0.1x) in obj loader
        const SCALE: f32 = 0.1;

        // Calculate deformation scale factors from base mesh if refs exist
        // Scale refs are in original units, basemesh is already scaled
        let scale_x = self
            .x_scale
            .as_ref()
            .and_then(|s| {
                let min = base_vertices.get(s.min_vert as usize)?;
                let max = base_vertices.get(s.max_vert as usize)?;
                let current_dist = (max.x - min.x).abs();
                let original_dist = s.scale * SCALE; // Convert ref to same scale
                Some(current_dist / original_dist)
            })
            .unwrap_or(1.0);

        let scale_y = self
            .y_scale
            .as_ref()
            .and_then(|s| {
                let min = base_vertices.get(s.min_vert as usize)?;
                let max = base_vertices.get(s.max_vert as usize)?;
                let current_dist = (max.y - min.y).abs();
                let original_dist = s.scale * SCALE;
                Some(current_dist / original_dist)
            })
            .unwrap_or(1.0);

        let scale_z = self
            .z_scale
            .as_ref()
            .and_then(|s| {
                let min = base_vertices.get(s.min_vert as usize)?;
                let max = base_vertices.get(s.max_vert as usize)?;
                let current_dist = (max.z - min.z).abs();
                let original_dist = s.scale * SCALE;
                Some(current_dist / original_dist)
            })
            .unwrap_or(1.0);

        for (i, binding) in self.bindings.iter().enumerate() {
            if i >= asset_vertices.len() {
                break;
            }

            // Get base triangle vertices
            let v0 = base_vertices
                .get(binding.triangle[0] as usize)
                .copied()
                .unwrap_or(Vec3::ZERO);
            let v1 = base_vertices
                .get(binding.triangle[1] as usize)
                .copied()
                .unwrap_or(Vec3::ZERO);
            let v2 = base_vertices
                .get(binding.triangle[2] as usize)
                .copied()
                .unwrap_or(Vec3::ZERO);

            // Barycentric interpolation
            let pos = v0 * binding.weights[0] + v1 * binding.weights[1] + v2 * binding.weights[2];

            // Apply offset: scale to meters (0.1) + deformation scale + negate X/Z to match basemesh
            let scaled_offset = Vec3::new(
                -binding.offset.x * SCALE * scale_x,
                binding.offset.y * SCALE * scale_y,
                -binding.offset.z * SCALE * scale_z,
            );

            // Push outward along triangle normal to prevent skin poke-through
            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            let normal_push = edge1.cross(edge2).normalize_or_zero() * normal_offset;

            asset_vertices[i] = pos + scaled_offset + normal_push;
        }
    }
}
