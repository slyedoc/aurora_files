use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};
use std::io::{BufRead, BufReader};
use thiserror::Error;

/// Morph target data - sparse vertex offsets
#[derive(Asset, TypePath, Debug, Clone)]
pub struct MorphTargetData {
    /// Sparse map: vertex_index -> offset
    pub offsets: HashMap<u32, Vec3>,
}

#[derive(Default, TypePath)]
pub struct MorphTargetLoader;

#[derive(Debug, Error)]
pub enum MorphTargetLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error on line {line}: {msg}")]
    Parse { line: usize, msg: String },
}

impl AssetLoader for MorphTargetLoader {
    type Asset = MorphTargetData;
    type Settings = ();
    type Error = MorphTargetLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let mut offsets = HashMap::default();
        let buf_reader = BufReader::new(bytes.as_slice());

        for (line_num, line) in buf_reader.lines().enumerate() {
            let line = line?;
            let line = line.trim();

            // Skip empty lines and comments
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();

            if parts.len() != 4 {
                return Err(MorphTargetLoaderError::Parse {
                    line: line_num + 1,
                    msg: format!("Expected 4 values, got {}", parts.len()),
                });
            }

            let vertex_idx: u32 = parts[0]
                .parse()
                .map_err(|e| MorphTargetLoaderError::Parse {
                    line: line_num + 1,
                    msg: format!("Invalid vertex index: {}", e),
                })?;

            let x: f32 = parts[1]
                .parse()
                .map_err(|e| MorphTargetLoaderError::Parse {
                    line: line_num + 1,
                    msg: format!("Invalid x offset: {}", e),
                })?;

            let y: f32 = parts[2]
                .parse()
                .map_err(|e| MorphTargetLoaderError::Parse {
                    line: line_num + 1,
                    msg: format!("Invalid y offset: {}", e),
                })?;

            let z: f32 = parts[3]
                .parse()
                .map_err(|e| MorphTargetLoaderError::Parse {
                    line: line_num + 1,
                    msg: format!("Invalid z offset: {}", e),
                })?;

            // Scale to meters (0.1x), negate X and Z to match base mesh
            const SCALE: f32 = 0.1;
            offsets.insert(vertex_idx, Vec3::new(x * -SCALE, y * SCALE, z * -SCALE));
        }

        Ok(MorphTargetData { offsets })
    }

    fn extensions(&self) -> &[&str] {
        &["target"]
    }
}
