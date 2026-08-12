use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};
use serde::Deserialize;
use thiserror::Error;

/// Skinning weights asset - maps bones to vertex weights
#[derive(Asset, TypePath, Debug, Clone)]
pub struct SkinningWeights {
    /// Sparse weights: bone_name → [(vertex_idx, weight), ...]
    pub weights: HashMap<String, Vec<(usize, f32)>>,
}

/// JSON format for weights file
#[derive(Debug, Deserialize)]
struct WeightsJson {
    #[serde(default)]
    #[allow(dead_code)]
    copyright: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    description: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    license: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    version: Option<u32>,
    weights: HashMap<String, Vec<[f64; 2]>>, // bone → [[vert_idx, weight], ...]
}

impl SkinningWeights {
    /// Convert sparse weights to dense per-vertex format
    /// Returns: vertex_weights[vertex_idx] = [(bone_idx, weight), ...]
    pub fn to_vertex_weights(
        &self,
        bone_indices: &HashMap<String, usize>,
        vertex_count: usize,
    ) -> Vec<Vec<(usize, f32)>> {
        let mut vertex_weights = vec![Vec::new(); vertex_count];

        // For each bone
        for (bone_name, weights) in &self.weights {
            if let Some(&bone_idx) = bone_indices.get(bone_name) {
                // For each weighted vertex
                for &(vert_idx, weight) in weights {
                    if vert_idx < vertex_count && weight > 1e-6 {
                        vertex_weights[vert_idx].push((bone_idx, weight));
                    }
                }
            }
        }

        // Normalize weights for each vertex
        for weights in &mut vertex_weights {
            if !weights.is_empty() {
                let sum: f32 = weights.iter().map(|(_, w)| w).sum();
                if sum > 1e-6 {
                    for (_, weight) in weights.iter_mut() {
                        *weight /= sum;
                    }
                }
            }
        }

        vertex_weights
    }

    /// Get the maximum vertex index referenced in weights
    pub fn max_vertex_index(&self) -> usize {
        self.weights
            .values()
            .flat_map(|v| v.iter().map(|(idx, _)| *idx))
            .max()
            .unwrap_or(0)
    }

    /// Get weights for a specific bone
    pub fn bone_weights(&self, bone_name: &str) -> Option<&Vec<(usize, f32)>> {
        self.weights.get(bone_name)
    }
}

/// Asset loader for weight JSON files
#[derive(Default, TypePath)]
pub struct SkinningWeightsLoader;

#[derive(Debug, Error)]
pub enum SkinningWeightsLoadError {
    #[error("Failed to load weights: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse weights JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl AssetLoader for SkinningWeightsLoader {
    type Asset = SkinningWeights;
    type Settings = ();
    type Error = SkinningWeightsLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let json: WeightsJson = serde_json::from_slice(&bytes)?;

        // Convert from [[idx, weight], ...] to Vec<(idx, weight)>
        let weights: HashMap<String, Vec<(usize, f32)>> = json
            .weights
            .into_iter()
            .map(|(bone_name, weights_array)| {
                let weights_vec = weights_array
                    .into_iter()
                    .map(|[idx, weight]| (idx as usize, weight as f32))
                    .filter(|(_, weight)| *weight > 1e-6) // Skip tiny weights
                    .collect();
                (bone_name, weights_vec)
            })
            .collect();

        Ok(SkinningWeights { weights })
    }

    fn extensions(&self) -> &[&str] {
        &["mhw", "weights.json"]
    }
}
