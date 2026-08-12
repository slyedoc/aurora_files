//! BVH pose asset loader - parses MakeHuman pose BVH files
//!
//! BVH format:
//! - HIERARCHY section: defines skeleton structure
//! - MOTION section: frame data (typically 1 frame for static poses)
//!
//! Each joint has 6 channels: Xposition Yposition Zposition Xrotation Yrotation Zrotation

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    platform::collections::HashMap,
    prelude::*,
};
use std::io::{BufRead, BufReader};
use thiserror::Error;

/// Pose asset - bone rotations from BVH file
#[derive(Asset, TypePath, Debug, Clone)]
pub struct Pose {
    /// Bone rotations (Euler XYZ degrees) stored as Quat
    pub bone_rotations: HashMap<String, Quat>,
    /// Bone translations (only root typically has non-zero)
    pub bone_translations: HashMap<String, Vec3>,
}

impl Pose {
    /// Get rotation for a bone by name
    pub fn rotation(&self, bone_name: &str) -> Option<Quat> {
        self.bone_rotations.get(bone_name).copied()
    }

    /// Get translation for a bone by name
    pub fn translation(&self, bone_name: &str) -> Option<Vec3> {
        self.bone_translations.get(bone_name).copied()
    }
}

/// BVH joint definition during parsing
#[derive(Debug)]
struct BvhJoint {
    name: String,
    offset: Vec3,
    channels: Vec<String>,
}

#[derive(Default, TypePath)]
pub struct BvhPoseLoader;

#[derive(Debug, Error)]
pub enum BvhPoseLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

impl AssetLoader for BvhPoseLoader {
    type Asset = Pose;
    type Settings = ();
    type Error = BvhPoseLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let buf_reader = BufReader::new(&bytes[..]);
        let mut lines = buf_reader.lines();

        let mut joints: Vec<BvhJoint> = Vec::new();
        let mut in_hierarchy = false;
        let mut in_motion = false;
        let mut frame_data: Vec<f32> = Vec::new();

        while let Some(Ok(line)) = lines.next() {
            let line = line.trim();

            if line == "HIERARCHY" {
                in_hierarchy = true;
                continue;
            }

            if line == "MOTION" {
                in_hierarchy = false;
                in_motion = true;
                continue;
            }

            if in_hierarchy {
                // Parse joint definitions
                if line.starts_with("ROOT ") || line.starts_with("JOINT ") {
                    let name = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    joints.push(BvhJoint {
                        name,
                        offset: Vec3::ZERO,
                        channels: Vec::new(),
                    });
                } else if line.starts_with("OFFSET ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 4 {
                        if let Some(joint) = joints.last_mut() {
                            joint.offset = Vec3::new(
                                parts[1].parse().unwrap_or(0.0),
                                parts[2].parse().unwrap_or(0.0),
                                parts[3].parse().unwrap_or(0.0),
                            );
                        }
                    }
                } else if line.starts_with("CHANNELS ") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        let channel_count: usize = parts[1].parse().unwrap_or(0);
                        if let Some(joint) = joints.last_mut() {
                            joint.channels = parts[2..2 + channel_count]
                                .iter()
                                .map(|s| s.to_string())
                                .collect();
                        }
                    }
                }
            }

            if in_motion {
                // Skip "Frames:" and "Frame Time:" lines
                if line.starts_with("Frames:") || line.starts_with("Frame Time:") {
                    continue;
                }

                // Parse frame data (single line with all values)
                if !line.is_empty() && !line.starts_with("MOTION") {
                    frame_data = line
                        .split_whitespace()
                        .filter_map(|s| s.parse::<f32>().ok())
                        .collect();
                    break; // Only need first frame
                }
            }
        }

        // Apply frame data to joints
        let mut bone_rotations = HashMap::default();
        let mut bone_translations = HashMap::default();

        let mut data_idx = 0;
        for joint in &joints {
            let mut translation = Vec3::ZERO;
            let mut rotation_euler = Vec3::ZERO;

            for channel in &joint.channels {
                if data_idx >= frame_data.len() {
                    break;
                }
                let value = frame_data[data_idx];
                data_idx += 1;

                match channel.as_str() {
                    "Xposition" => translation.x = value,
                    "Yposition" => translation.y = value,
                    "Zposition" => translation.z = value,
                    "Xrotation" => rotation_euler.x = value.to_radians(),
                    "Yrotation" => rotation_euler.y = value.to_radians(),
                    "Zrotation" => rotation_euler.z = value.to_radians(),
                    _ => {}
                }
            }

            // Convert Euler to Quat
            // BVH specifies channels as Xrotation Yrotation Zrotation
            // MakeHuman uses ZXY order, negate all axes to match Bevy coordinate system
            let rotation = Quat::from_euler(
                EulerRot::ZXY,
                -rotation_euler.z,
                -rotation_euler.x,
                -rotation_euler.y,
            );

            // Store if non-identity
            if !rotation.is_near_identity() {
                bone_rotations.insert(joint.name.clone(), rotation);
            }

            // Store translation if non-zero (usually only root)
            if translation.length_squared() > 0.0001 {
                bone_translations.insert(joint.name.clone(), translation);
            }
        }

        Ok(Pose {
            bone_rotations,
            bone_translations,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["bvh"]
    }
}
