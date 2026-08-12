//! .mhmat file parser - MakeHuman material definitions
//!
//! Format: ASCII key-value pairs
//! Common properties:
//!   diffuseColor 0.8 0.8 0.8
//!   specularColor 0.1 0.1 0.1
//!   emissiveColor 0.0 0.0 0.0
//!   shininess 0.5
//!   opacity 1.0
//!   diffuseTexture skin.png
//!   normalmapTexture skin_normal.png
//!   aomapTexture skin_ao.png
//!   backfaceCull true
//!   transparent false

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    image::ImageLoaderSettings,
    prelude::*,
};
use bevy_aurora::material::{AlphaMode, StandardAuroraMaterial};
use std::io::{BufRead, BufReader};
use thiserror::Error;

#[derive(Default, TypePath)]
pub struct MhmatLoader;

#[derive(Debug, Error)]
pub enum MhmatLoaderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(String),
}

fn parse_bool(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")
}

impl AssetLoader for MhmatLoader {
    type Asset = StandardAuroraMaterial;
    type Settings = ();
    type Error = MhmatLoaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        // TODO: Everything below here is make up by Claude, come back to this
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        // Colors
        let mut diffuse_color = Color::WHITE;
        let mut specular_color = Color::srgb(0.5, 0.5, 0.5);
        let mut emissive_color = Color::BLACK;

        // Scalars
        let mut opacity = 1.0f32;
        let mut shininess = 0.5f32;
        let mut metallic = 0.0f32;
        let mut roughness: Option<f32> = None;
        let mut ior = 1.5f32;
        let mut translucency = 0.0f32;
        let mut _normal_intensity = 1.0f32;

        // Textures
        let mut diffuse_texture: Option<Handle<Image>> = None;
        let mut normal_texture: Option<Handle<Image>> = None;
        let mut ao_texture: Option<Handle<Image>> = None;
        let mut bump_texture: Option<Handle<Image>> = None;

        // Flags
        let mut backface_cull = true;
        let mut transparent = false;
        let mut _shadeless = false;
        let mut alpha_to_coverage = false;

        let buf_reader = BufReader::new(&bytes[..]);

        for line in buf_reader.lines() {
            let line = line?;
            let line = line.trim();

            // Skip empty lines and comments (# or //)
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }

            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            match parts[0] {
                // Colors
                "diffuseColor" if parts.len() >= 4 => {
                    let r: f32 = parts[1].parse().unwrap_or(1.0);
                    let g: f32 = parts[2].parse().unwrap_or(1.0);
                    let b: f32 = parts[3].parse().unwrap_or(1.0);
                    diffuse_color = Color::srgb(r, g, b);
                }
                "specularColor" if parts.len() >= 4 => {
                    let r: f32 = parts[1].parse().unwrap_or(0.5);
                    let g: f32 = parts[2].parse().unwrap_or(0.5);
                    let b: f32 = parts[3].parse().unwrap_or(0.5);
                    specular_color = Color::srgb(r, g, b);
                }
                "emissiveColor" if parts.len() >= 4 => {
                    let r: f32 = parts[1].parse().unwrap_or(0.0);
                    let g: f32 = parts[2].parse().unwrap_or(0.0);
                    let b: f32 = parts[3].parse().unwrap_or(0.0);
                    emissive_color = Color::srgb(r, g, b);
                }

                // Scalars
                "opacity" if parts.len() >= 2 => {
                    opacity = parts[1].parse().unwrap_or(1.0);
                }
                "shininess" if parts.len() >= 2 => {
                    shininess = parts[1].parse().unwrap_or(0.5);
                }
                "metallic" if parts.len() >= 2 => {
                    metallic = parts[1].parse().unwrap_or(0.0);
                }
                "roughness" if parts.len() >= 2 => {
                    roughness = Some(parts[1].parse().unwrap_or(0.5));
                }
                "ior" if parts.len() >= 2 => {
                    ior = parts[1].parse().unwrap_or(1.5);
                }
                "translucency" if parts.len() >= 2 => {
                    translucency = parts[1].parse().unwrap_or(0.0);
                }
                "normalmapIntensity" if parts.len() >= 2 => {
                    _normal_intensity = parts[1].parse().unwrap_or(1.0);
                }

                // Textures
                "diffuseTexture" if parts.len() >= 2 => {
                    diffuse_texture = Some(load_texture(load_context, parts[1]));
                }
                "normalmapTexture" if parts.len() >= 2 => {
                    normal_texture = Some(load_texture_linear(load_context, parts[1]));
                }
                "aomapTexture" if parts.len() >= 2 => {
                    ao_texture = Some(load_texture_linear(load_context, parts[1]));
                }
                "bumpTexture" if parts.len() >= 2 => {
                    bump_texture = Some(load_texture_linear(load_context, parts[1]));
                }

                // Flags
                "backfaceCull" if parts.len() >= 2 => {
                    backface_cull = parse_bool(parts[1]);
                }
                "transparent" if parts.len() >= 2 => {
                    transparent = parse_bool(parts[1]);
                }
                "shadeless" if parts.len() >= 2 => {
                    _shadeless = parse_bool(parts[1]);
                }
                "alphaToCoverage" if parts.len() >= 2 => {
                    alpha_to_coverage = parse_bool(parts[1]);
                }

                // Ignored: name, tag, description, shader*, sss*, castShadows, receiveShadows, etc.
                _ => {}
            }
        }

        // Use explicit roughness if provided, otherwise derive from shininess
        // shininess in MH is 0-1, higher = shinier = lower roughness
        let perceptual_roughness = roughness.unwrap_or_else(|| 1.0 - shininess.clamp(0.0, 1.0));

        // If diffuse texture exists, use white base_color so texture shows properly
        // Otherwise tint with diffuse_color
        let base_color = if diffuse_texture.is_some() {
            Color::WHITE
        } else {
            diffuse_color
        };

        // Alpha mode
        let alpha_mode = if transparent || opacity < 1.0 {
            if alpha_to_coverage {
                AlphaMode::AlphaToCoverage
            } else {
                AlphaMode::Blend
            }
        } else {
            AlphaMode::Opaque
        };

        // Cull mode: no aurora counterpart — the ray tracer has no raster cull
        // state (backface behaviour belongs to BVH traversal).
        let _ = backface_cull;

        // Convert emissive to LinearRgba
        let emissive = emissive_color.to_linear();

        // Reflectance derived from specular color intensity (avg of RGB)
        let spec_linear = specular_color.to_linear();
        let reflectance = (spec_linear.red + spec_linear.green + spec_linear.blue) / 3.0;

        // // Warn if normal intensity != 1.0 (can't directly apply in StandardMaterial)
        // if (normal_intensity - 1.0).abs() > 0.01 && normal_texture.is_some() {
        //     // warn!(
        //     //     "mhmat normalmapIntensity={} ignored, StandardMaterial doesn't support intensity scaling",
        //     //     normal_intensity
        //     // );
        // }

        // StandardAuroraMaterial carries no specular_tint / diffuse_transmission /
        // occlusion_texture / cull_mode / double_sided / unlit — dropped in the
        // aurora migration (no BRDF counterparts in the ray tracer).
        let _ = (specular_color, translucency, ao_texture);

        Ok(StandardAuroraMaterial {
            base_color,
            base_color_texture: diffuse_texture,
            emissive,
            perceptual_roughness,
            metallic,
            reflectance: reflectance.clamp(0.0, 1.0),
            ior,
            normal_map_texture: normal_texture,
            depth_map: bump_texture,
            alpha_mode,
            ..default()
        })
    }

    fn extensions(&self) -> &[&str] {
        &["mhmat"]
    }
}

fn load_texture(load_context: &mut LoadContext, filename: &str) -> Handle<Image> {
    let parent = load_context.path().parent().unwrap();
    let full_path = format!("{}/{}", parent.path().display(), filename);
    load_context.load(full_path)
}

fn load_texture_linear(load_context: &mut LoadContext, filename: &str) -> Handle<Image> {
    let parent = load_context.path().parent().unwrap();
    let full_path = format!("{}/{}", parent.path().display(), filename);
    load_context
        .load_builder()
        .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
        .load(full_path)
}
