//! Material textures baked to block-compressed KTX2, the last step of every bake.
//!
//! A texture's role comes from the material field that names it, never from the file:
//! colour (`*base_color_texture`, `*emissive_texture`) is BC7 sRGB, a normal map BC5 (XY,
//! the shader rebuilds Z), packed linear data (`metallic_roughness_texture`,
//! `occlusion_texture`) BC7 UNORM, a single channel (`depth_map`) BC4. Mips are built here,
//! each in its own space: colour averaged in linear light, normals renormalised, data raw.
//! Levels are zstd-supercompressed; the file states its transfer function, which the
//! loader honours.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use intel_tex_2::{RSurface, RgSurface, RgbaSurface, bc4, bc5, bc7};
use rayon::prelude::*;
use regex::Regex;

/// What a texture holds, from the material field that references it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Color,
    Normal,
    Data,
    Mask,
}

impl Role {
    /// The role a material field gives the texture it names.
    pub fn of_field(field: &str) -> Option<Self> {
        if field.ends_with("normal_map_texture") {
            Some(Self::Normal)
        } else if field.ends_with("base_color_texture") || field.ends_with("emissive_texture") {
            Some(Self::Color)
        } else if field.ends_with("metallic_roughness_texture")
            || field.ends_with("occlusion_texture")
        {
            Some(Self::Data)
        } else if field.ends_with("depth_map") {
            Some(Self::Mask)
        } else {
            None
        }
    }

    fn vk_format(self) -> u32 {
        match self {
            Self::Color => 146,  // BC7_SRGB_BLOCK
            Self::Normal => 141, // BC5_UNORM_BLOCK
            Self::Data => 145,   // BC7_UNORM_BLOCK
            Self::Mask => 139,   // BC4_UNORM_BLOCK
        }
    }

    fn block_bytes(self) -> usize {
        match self {
            Self::Mask => 8,
            _ => 16,
        }
    }
}

/// One mip level, RGBA8.
struct Level {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Bake the image at `src` to a mipped, block-compressed KTX2 at `dst`.
pub fn bake_texture(src: &Path, role: Role, dst: &Path) -> Result<()> {
    let image = image::open(src)
        .with_context(|| format!("reading {}", src.display()))?
        .to_rgba8();
    let (width, height) = image.dimensions();
    let levels = mip_chain(
        Level {
            width,
            height,
            rgba: image.into_raw(),
        },
        role,
    );
    let encoded: Vec<Vec<u8>> = levels.iter().map(|level| encode(level, role)).collect();
    let bytes = write_ktx2(role, width, height, &encoded)?;
    if let Some(dir) = dst.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(dst, bytes).with_context(|| format!("writing {}", dst.display()))?;
    Ok(())
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

/// Every level down to 1x1, each a 2x2 box of the one above in the role's own space.
fn mip_chain(base: Level, role: Role) -> Vec<Level> {
    let decode: [f32; 256] = std::array::from_fn(|v| srgb_to_linear(v as u8));
    let mut levels = vec![base];
    loop {
        let above = levels.last().unwrap();
        if above.width == 1 && above.height == 1 {
            break;
        }
        let (w, h) = ((above.width / 2).max(1), (above.height / 2).max(1));
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(above.width - 1);
                    let sy = (y * 2 + dy).min(above.height - 1);
                    let i = ((sy * above.width + sx) * 4) as usize;
                    let px = &above.rgba[i..i + 4];
                    match role {
                        Role::Color => {
                            for c in 0..3 {
                                sum[c] += decode[px[c] as usize];
                            }
                            sum[3] += px[3] as f32 / 255.0;
                        }
                        Role::Normal => {
                            for c in 0..3 {
                                sum[c] += px[c] as f32 / 255.0 * 2.0 - 1.0;
                            }
                            sum[3] += px[3] as f32 / 255.0;
                        }
                        Role::Data | Role::Mask => {
                            for c in 0..4 {
                                sum[c] += px[c] as f32 / 255.0;
                            }
                        }
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                let out = &mut rgba[o..o + 4];
                match role {
                    Role::Color => {
                        for c in 0..3 {
                            out[c] = linear_to_srgb(sum[c] / 4.0);
                        }
                        out[3] = (sum[3] / 4.0 * 255.0 + 0.5) as u8;
                    }
                    Role::Normal => {
                        let n = [sum[0], sum[1], sum[2]];
                        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                        for c in 0..3 {
                            out[c] = ((n[c] / len * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        }
                        out[3] = 255;
                    }
                    Role::Data | Role::Mask => {
                        for c in 0..4 {
                            out[c] = (sum[c] / 4.0 * 255.0 + 0.5) as u8;
                        }
                    }
                }
            }
        }
        levels.push(Level {
            width: w,
            height: h,
            rgba,
        });
    }
    levels
}

/// `level`'s channels the role keeps, edge-padded to whole 4x4 blocks.
fn padded(level: &Level, channels: usize) -> (Vec<u8>, u32, u32) {
    let (pw, ph) = (level.width.div_ceil(4) * 4, level.height.div_ceil(4) * 4);
    let mut out = Vec::with_capacity((pw * ph) as usize * channels);
    for y in 0..ph {
        let sy = y.min(level.height - 1);
        for x in 0..pw {
            let sx = x.min(level.width - 1);
            let i = ((sy * level.width + sx) * 4) as usize;
            out.extend_from_slice(&level.rgba[i..i + channels]);
        }
    }
    (out, pw, ph)
}

fn encode(level: &Level, role: Role) -> Vec<u8> {
    match role {
        Role::Color | Role::Data => {
            let (data, width, height) = padded(level, 4);
            let opaque = data.chunks_exact(4).all(|px| px[3] == 255);
            let settings = if opaque {
                bc7::opaque_basic_settings()
            } else {
                bc7::alpha_basic_settings()
            };
            bc7::compress_blocks(
                &settings,
                &RgbaSurface {
                    data: &data,
                    width,
                    height,
                    stride: width * 4,
                },
            )
        }
        Role::Normal => {
            let (data, width, height) = padded(level, 2);
            bc5::compress_blocks(&RgSurface {
                data: &data,
                width,
                height,
                stride: width * 2,
            })
        }
        Role::Mask => {
            let (data, width, height) = padded(level, 1);
            bc4::compress_blocks(&RSurface {
                data: &data,
                width,
                height,
                stride: width,
            })
        }
    }
}

const KTX2_MAGIC: [u8; 12] = [
    0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
];
const SUPERCOMPRESSION_ZSTD: u32 = 2;
const ZSTD_LEVEL: i32 = 19;

/// A KTX2 file of `levels` (level 0 first), zstd-supercompressed, stored smallest level
/// first as the spec requires.
fn write_ktx2(role: Role, width: u32, height: u32, levels: &[Vec<u8>]) -> Result<Vec<u8>> {
    let compressed: Vec<Vec<u8>> = levels
        .iter()
        .map(|level| zstd::bulk::compress(level, ZSTD_LEVEL))
        .collect::<std::io::Result<_>>()?;
    let level_count = levels.len() as u32;

    // Data format descriptor: one basic block, one sample per channel the format codes.
    let (model, samples): (u8, &[(u32, u32)]) = match role {
        Role::Color | Role::Data => (134, &[(0, 0)]), // BC7: one 128-bit colour sample
        Role::Normal => (132, &[(0, 0), (64, 1)]),    // BC5: red then green, 64 bits each
        Role::Mask => (131, &[(0, 0)]),               // BC4: red, 64 bits
    };
    let bits = role.block_bytes() as u32 * 8 / samples.len() as u32;
    let block_size = 24 + 16 * samples.len() as u32;
    let mut dfd = Vec::new();
    dfd.extend_from_slice(&(4 + block_size).to_le_bytes());
    dfd.extend_from_slice(&0u32.to_le_bytes()); // vendor 0, type 0
    dfd.extend_from_slice(&(2 | (block_size << 16)).to_le_bytes()); // version 2
    let transfer = if role == Role::Color { 2 } else { 1 }; // sRGB / linear
    dfd.extend_from_slice(&[model, 1, transfer, 0]); // BT.709 primaries, straight alpha
    dfd.extend_from_slice(&[3, 3, 0, 0]); // 4x4 texel blocks
    dfd.extend_from_slice(&[role.block_bytes() as u8, 0, 0, 0, 0, 0, 0, 0]);
    for &(offset, channel) in samples {
        dfd.extend_from_slice(&(offset | ((bits - 1) << 16) | (channel << 24)).to_le_bytes());
        dfd.extend_from_slice(&0u32.to_le_bytes()); // sample position
        dfd.extend_from_slice(&0u32.to_le_bytes()); // lower
        dfd.extend_from_slice(&u32::MAX.to_le_bytes()); // upper
    }

    let dfd_offset = 80 + level_count * 24;
    let mut file = Vec::new();
    file.extend_from_slice(&KTX2_MAGIC);
    // vkFormat, typeSize, width, height, depth, layers, faces, levels, supercompression
    for v in [
        role.vk_format(),
        1,
        width,
        height,
        0,
        0,
        1,
        level_count,
        SUPERCOMPRESSION_ZSTD,
    ] {
        file.extend_from_slice(&v.to_le_bytes());
    }
    for v in [dfd_offset, dfd.len() as u32, 0, 0] {
        file.extend_from_slice(&v.to_le_bytes()); // dfd offset/length, no key/values
    }
    file.extend_from_slice(&0u64.to_le_bytes()); // no supercompression global data
    file.extend_from_slice(&0u64.to_le_bytes());

    // Supercompressed levels need no alignment.
    let mut offset = (dfd_offset as usize + dfd.len()) as u64;
    let mut offsets = vec![0u64; levels.len()];
    for l in (0..levels.len()).rev() {
        offsets[l] = offset;
        offset += compressed[l].len() as u64;
    }
    for l in 0..levels.len() {
        for v in [offsets[l], compressed[l].len() as u64, levels[l].len() as u64] {
            file.extend_from_slice(&v.to_le_bytes());
        }
    }
    file.extend_from_slice(&dfd);
    for l in (0..levels.len()).rev() {
        if file.len() as u64 != offsets[l] {
            bail!("ktx2 writer: level {l} landed at {}, not {}", file.len(), offsets[l]);
        }
        file.extend_from_slice(&compressed[l]);
    }
    Ok(file)
}

/// A material field naming a texture by path: `base_color_texture: "a/b.png"`.
fn texture_field() -> Regex {
    Regex::new(r#"([A-Za-z_]*(?:texture|depth_map)): *"([^"]+\.(?:png|jpe?g|tga))""#).unwrap()
}

/// Every `.bsn` under `root`.
fn bsn_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "bsn") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// The baked path of a texture: same place, `.ktx2`.
pub fn baked_path(texture: &str) -> String {
    match texture.rfind('.') {
        Some(dot) => format!("{}.ktx2", &texture[..dot]),
        None => format!("{texture}.ktx2"),
    }
}

/// What [`bake_dir`] did.
#[derive(Debug, Default)]
pub struct Report {
    pub baked: Vec<PathBuf>,
    pub reused: usize,
    pub rewritten: Vec<PathBuf>,
    /// Sources a material now reaches only through their baked copy.
    pub replaced: Vec<PathBuf>,
}

/// Bake every texture the `.bsn` files under `out_dir` name in a material field and point the
/// fields at the baked files. Texture paths start with `asset_prefix`: `out_dir` is
/// `<asset root>/<asset_prefix>`. A texture reached through two roles takes the first by
/// [`Role`] order; up-to-date bakes are kept.
pub fn bake_dir(out_dir: &Path, asset_prefix: &str) -> Result<Report> {
    let mut root = out_dir.to_path_buf();
    if out_dir.ends_with(asset_prefix) {
        for _ in Path::new(asset_prefix).components() {
            root.pop();
        }
    }
    bake_files(&root, &bsn_files(out_dir), false)
}

/// Bake what `files` name, relative to the asset `root`, and rewrite `files`.
fn bake_files(root: &Path, files: &[PathBuf], force: bool) -> Result<Report> {
    let field = texture_field();
    let mut roles: BTreeMap<String, Role> = BTreeMap::new();
    for file in files {
        let text = fs::read_to_string(file)?;
        for cap in field.captures_iter(&text) {
            let Some(role) = Role::of_field(&cap[1]) else {
                continue;
            };
            let texture = cap[2].to_string();
            match roles.get(&texture) {
                Some(&seen) if seen != role => {
                    eprintln!(
                        "textures: {texture} is both {seen:?} and {role:?}; baking as {:?}",
                        seen.min(role)
                    );
                    roles.insert(texture, seen.min(role));
                }
                _ => {
                    roles.insert(texture, role);
                }
            }
        }
    }

    let mut report = Report::default();
    let jobs: Vec<(PathBuf, PathBuf, Role)> = roles
        .iter()
        .map(|(texture, role)| (root.join(texture), root.join(baked_path(texture)), *role))
        .filter(|(src, _, _)| src.exists())
        .collect();
    let results: Vec<Result<bool>> = jobs
        .par_iter()
        .map(|(src, dst, role)| {
            let fresh = !force
                && dst.exists()
                && fs::metadata(dst)?.modified()? >= fs::metadata(src)?.modified()?;
            if fresh {
                return Ok(false);
            }
            bake_texture(src, *role, dst)?;
            Ok(true)
        })
        .collect();
    for ((src, dst, _), result) in jobs.iter().zip(results) {
        if result.with_context(|| format!("baking {}", src.display()))? {
            report.baked.push(dst.clone());
        } else {
            report.reused += 1;
        }
        report.replaced.push(src.clone());
    }

    let baked: BTreeMap<&str, String> = roles
        .keys()
        .filter(|texture| root.join(texture).exists())
        .map(|texture| (texture.as_str(), baked_path(texture)))
        .collect();
    for file in files {
        let text = fs::read_to_string(file)?;
        let rewritten = field.replace_all(&text, |cap: &regex::Captures| {
            match (Role::of_field(&cap[1]), baked.get(&cap[2])) {
                (Some(_), Some(path)) => format!("{}: \"{path}\"", &cap[1]),
                _ => cap[0].to_string(),
            }
        });
        if rewritten != text {
            fs::write(file, rewritten.as_ref())?;
            report.rewritten.push(file.clone());
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_follow_the_field() {
        assert_eq!(Role::of_field("base_color_texture"), Some(Role::Color));
        assert_eq!(Role::of_field("detail_base_color_texture"), Some(Role::Color));
        assert_eq!(Role::of_field("layer_normal_map_texture"), Some(Role::Normal));
        assert_eq!(Role::of_field("metallic_roughness_texture"), Some(Role::Data));
        assert_eq!(Role::of_field("depth_map"), Some(Role::Mask));
        assert_eq!(Role::of_field("texture_array_a"), None);
    }

    #[test]
    fn colour_mips_average_in_linear_light() {
        // Black and white average to linear 0.5, which is sRGB 188, not 128.
        let base = Level {
            width: 2,
            height: 1,
            rgba: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let chain = mip_chain(base, Role::Color);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[1].rgba[0], 188);
    }

    #[test]
    fn written_files_parse_with_every_level() {
        for role in [Role::Color, Role::Normal, Role::Data, Role::Mask] {
            let base = Level {
                width: 6,
                height: 3,
                rgba: (0..6 * 3 * 4).map(|i| (i * 7) as u8).collect(),
            };
            let levels = mip_chain(base, role);
            let encoded: Vec<Vec<u8>> = levels.iter().map(|l| encode(l, role)).collect();
            let bytes = write_ktx2(role, 6, 3, &encoded).unwrap();
            let reader = ktx2::Reader::new(&bytes).unwrap();
            assert_eq!(reader.header().level_count, 3);
            assert_eq!(reader.header().format.map(|f| f.value()), Some(role.vk_format()));
            let transfer = reader.transfer_function().unwrap();
            assert_eq!(transfer == ktx2::TransferFunction::SRGB, role == Role::Color);
            let first = reader.levels().next().unwrap();
            assert_eq!(first.uncompressed_byte_length as usize, 2 * role.block_bytes());
        }
    }

    #[test]
    fn fields_point_at_the_baked_file() {
        let text = r#"M { base_color_texture: "a/b.png", normal_map_texture: "a/n.jpg", name: "x.png" }"#;
        let out = texture_field().replace_all(text, |cap: &regex::Captures| {
            format!("{}: \"{}\"", &cap[1], baked_path(&cap[2]))
        });
        assert_eq!(
            out,
            r#"M { base_color_texture: "a/b.ktx2", normal_map_texture: "a/n.ktx2", name: "x.png" }"#
        );
    }
}
