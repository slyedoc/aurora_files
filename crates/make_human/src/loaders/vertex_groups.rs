//! Vertex groups loader - parses basemesh vertex group definitions

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};
use serde::Deserialize;
use thiserror::Error;

/// Vertex Groups - named vertex index ranges
///  Each [usize; 2] is a range: [start, end] where both are inclusive vertex
///  indices.

///  So vec![[0, 5], [10, 15]] means:
///  - Vertices 0,1,2,3,4,5
///  - Plus vertices 10,11,12,13,14,15
#[derive(Asset, TypePath, Debug, Default, Clone, Deref, DerefMut)]
pub struct VertexGroups(pub HashMap<String, Vec<[usize; 2]>>);

/// JSON format for vertex groups file
#[derive(Debug, Deserialize)]
struct VertexGroupsJson {
    #[serde(flatten)]
    groups: HashMap<String, Vec<[usize; 2]>>,
}

impl VertexGroups {
    /// Expand ranges to actual indices
    pub fn expand_ranges(ranges: &[[usize; 2]]) -> Vec<usize> {
        let mut indices = Vec::new();
        for &[start, end] in ranges {
            for idx in start..=end {
                indices.push(idx);
            }
        }
        indices
    }
}

/// Asset loader for vertex groups JSON files
#[derive(Default, TypePath)]
pub struct VertexGroupsLoader;

#[derive(Debug, Error)]
pub enum VertexGroupsLoaderError {
    #[error("Failed to load vertex groups: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse vertex groups JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl AssetLoader for VertexGroupsLoader {
    type Asset = VertexGroups;
    type Settings = ();
    type Error = VertexGroupsLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let data: VertexGroupsJson = serde_json::from_slice(&bytes)?;

        Ok(VertexGroups(data.groups))
    }

    fn extensions(&self) -> &[&str] {
        &["vertex_groups.json"]
    }
}
