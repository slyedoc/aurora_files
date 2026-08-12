//! .proxy file parser - maps proxy mesh vertices to full-res base mesh
//!
//! Format: Text-based vertex binding (same as mhclo)
//! Each line after "verts 0": v0 v1 v2 w0 w1 w2 x_offset y_offset z_offset
//! - v0,v1,v2: base mesh (hm08) triangle vertex indices
//! - w0,w1,w2: barycentric weights
//! - x,y,z: position offsets

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};
use std::io::{BufRead, BufReader};
use thiserror::Error;

use super::mhclo::VertexBinding;

#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct ProxyAsset {
    /// Base mesh identifier (e.g. "hm08")
    pub basemesh: String,
    /// Proxy name
    pub name: String,
    /// Associated .obj file
    pub obj_file: Option<String>,
    /// Vertex bindings - maps proxy vert idx → base mesh triangle
    pub bindings: Vec<VertexBinding>,
    /// Scale factors
    pub x_scale: Option<(u32, u32, f32)>,
    pub y_scale: Option<(u32, u32, f32)>,
    pub z_scale: Option<(u32, u32, f32)>,
}
#[derive(Default, TypePath)]
pub struct ProxyLoader;

#[derive(Debug, Error)]
pub enum ProxyLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl AssetLoader for ProxyLoader {
    type Asset = ProxyAsset;
    type Settings = ();
    type Error = ProxyLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let mut asset = ProxyAsset::default();
        let buf_reader = BufReader::new(&bytes[..]);
        let mut in_vertex_bindings = false;

        for line_result in buf_reader.lines() {
            let line = line_result?;
            let line_trim = line.trim();

            if line_trim.is_empty() || line_trim.starts_with('#') {
                continue;
            }

            // Check for vertex binding section
            if line_trim == "verts 0" {
                in_vertex_bindings = true;
                continue;
            }

            // Parse vertex bindings
            if in_vertex_bindings {
                let parts: Vec<&str> = line_trim.split_whitespace().collect();

                if parts.len() == 9 {
                    // Triangle binding: v0 v1 v2 w0 w1 w2 x y z
                    let v0 = parts[0].parse().unwrap_or(0);
                    let v1 = parts[1].parse().unwrap_or(0);
                    let v2 = parts[2].parse().unwrap_or(0);
                    let w0 = parts[3].parse().unwrap_or(0.0);
                    let w1 = parts[4].parse().unwrap_or(0.0);
                    let w2 = parts[5].parse().unwrap_or(0.0);
                    let x = parts[6].parse().unwrap_or(0.0);
                    let y = parts[7].parse().unwrap_or(0.0);
                    let z = parts[8].parse().unwrap_or(0.0);

                    asset.bindings.push(VertexBinding {
                        triangle: [v0, v1, v2],
                        weights: [w0, w1, w2],
                        offset: Vec3::new(x, y, z),
                    });
                    continue;
                } else if parts.len() == 1 {
                    // Direct vertex reference: single base mesh vertex index
                    let v = parts[0].parse().unwrap_or(0);
                    asset.bindings.push(VertexBinding {
                        triangle: [v, v, v],
                        weights: [1.0, 0.0, 0.0],
                        offset: Vec3::ZERO,
                    });
                    continue;
                }
            }

            // Parse metadata
            if let Some((key, value)) = line_trim.split_once(' ') {
                match key {
                    "basemesh" => asset.basemesh = value.to_string(),
                    "name" => asset.name = value.to_string(),
                    "obj_file" => asset.obj_file = Some(value.to_string()),
                    "x_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.x_scale = Some((
                                parts[0].parse().unwrap_or(0),
                                parts[1].parse().unwrap_or(0),
                                parts[2].parse().unwrap_or(1.0),
                            ));
                        }
                    }
                    "y_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.y_scale = Some((
                                parts[0].parse().unwrap_or(0),
                                parts[1].parse().unwrap_or(0),
                                parts[2].parse().unwrap_or(1.0),
                            ));
                        }
                    }
                    "z_scale" => {
                        let parts: Vec<&str> = value.split_whitespace().collect();
                        if parts.len() == 3 {
                            asset.z_scale = Some((
                                parts[0].parse().unwrap_or(0),
                                parts[1].parse().unwrap_or(0),
                                parts[2].parse().unwrap_or(1.0),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(asset)
    }

    fn extensions(&self) -> &[&str] {
        &["proxy"]
    }
}

impl ProxyAsset {
    /// Given base mesh vertices, compute proxy mesh vertex positions
    /// NOTE: base_vertices must already be transformed (scaled to meters, Z-flipped)
    pub fn compute_proxy_vertices(&self, base_vertices: &[Vec3]) -> Vec<Vec3> {
        // Offset transform: proxy files store offsets in decimeters with +Z
        // Base vertices are in meters with -Z, so scale and flip offset
        const SCALE: f32 = 0.1;

        self.bindings
            .iter()
            .map(|binding| {
                // Get base triangle vertices (already transformed)
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

                // Transform offset to match base vertex coordinate system (negate X and Z)
                let offset = Vec3::new(
                    binding.offset.x * -SCALE,
                    binding.offset.y * SCALE,
                    binding.offset.z * -SCALE,
                );

                // Barycentric interpolation + transformed offset
                v0 * binding.weights[0] + v1 * binding.weights[1] + v2 * binding.weights[2] + offset
            })
            .collect()
    }

    /// Inverse operation: given proxy vertex index, get base mesh triangle info
    /// This is useful for attachments that reference base mesh indices
    pub fn get_base_binding(&self, proxy_vertex_idx: usize) -> Option<&VertexBinding> {
        self.bindings.get(proxy_vertex_idx)
    }

    /// Reconstruct hm08 basemesh vertex positions from proxy
    /// For each hm08 vertex, finds proxy verts that reference it and averages their positions
    pub fn reconstruct_basemesh_vertices(
        &self,
        proxy_vertices: &[Vec3],
        max_basemesh_index: usize,
    ) -> Vec<Vec3> {
        let mut basemesh_positions: Vec<Option<Vec3>> = vec![None; max_basemesh_index + 1];
        let mut basemesh_counts: Vec<u32> = vec![0; max_basemesh_index + 1];

        // For each proxy vertex, distribute its position to referenced basemesh vertices
        for (proxy_idx, binding) in self.bindings.iter().enumerate() {
            if proxy_idx >= proxy_vertices.len() {
                continue;
            }

            let proxy_pos = proxy_vertices[proxy_idx];

            // Each basemesh vertex in triangle gets the proxy position weighted inversely
            // This is approximate - ideally we'd solve the barycentric system
            for &base_idx in &binding.triangle {
                let idx = base_idx as usize;
                if idx < basemesh_positions.len() {
                    basemesh_positions[idx] =
                        Some(basemesh_positions[idx].unwrap_or(Vec3::ZERO) + proxy_pos);
                    basemesh_counts[idx] += 1;
                }
            }
        }

        // Average positions
        basemesh_positions
            .iter()
            .zip(basemesh_counts.iter())
            .map(|(pos, &count)| {
                if count > 0 {
                    pos.unwrap() / count as f32
                } else {
                    Vec3::ZERO
                }
            })
            .collect()
    }
}
