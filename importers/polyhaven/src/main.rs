//! Poly Haven terrain importer: downloads material maps and bakes them into
//! mipped, uncompressed KTX2 `texture_2d_array`s (one layer per material).
//!
//! Layer order IS biome id order (`zero::planet::BiomeType`, 0..=10) when run
//! with no slugs; pass explicit slugs to bake a custom set.
//!
//!   cargo run -p polyhaven --release            # 11-biome set at 1k
//!   cargo run -p polyhaven --release -- --res 2k aerial_rocks_02 snow_02

use anyhow::{bail, Context, Result};
use clap::Parser;
use image::{imageops::FilterType, RgbaImage};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Default layer set: 0..=10 primaries index-aligned with `BiomeType`
/// (Ocean..Mountain), 11..=21 per-biome secondary variants (biome id + 11),
/// 22 scree (mid-slope band), 23 wet (drainage/flow lines).
const BIOME_SET: [(&str, &str); 24] = [
    ("Ocean", "gravelly_sand"),
    ("Beach", "aerial_beach_01"),
    ("Desert", "sandy_gravel_02"),
    ("Savanna", "mud_cracked_dry_03"),
    ("TropicalForest", "brown_mud_leaves_01"),
    ("Grassland", "aerial_grass_rock"),
    ("TemperateForest", "forrest_ground_01"),
    ("Taiga", "forest_ground_04"),
    ("Tundra", "rocky_terrain_02"),
    ("Snow", "snow_02"),
    ("Mountain", "aerial_rocks_02"),
    ("Ocean2", "coast_sand_rocks_02"),
    ("Beach2", "sand_01"),
    ("Desert2", "red_sand"),
    ("Savanna2", "red_laterite_soil_stones"),
    ("TropicalForest2", "leafy_grass"),
    ("Grassland2", "grass_path_2"),
    ("TemperateForest2", "forest_leaves_03"),
    ("Taiga2", "moss_wood"),
    ("Tundra2", "lichen_rock"),
    ("Snow2", "snow_03"),
    ("Mountain2", "rock_face_03"),
    ("Scree", "rocky_gravel"),
    ("Wet", "brown_mud_02"),
];

/// (map key in the Poly Haven files API, output array suffix, sRGB?).
const MAPS: [(&str, &str, bool); 3] = [
    ("Diffuse", "albedo", true),
    ("nor_gl", "normal", false),
    ("arm", "arm", false), // AO (r), roughness (g), metallic (b)
];

#[derive(Parser)]
#[command(about = "Bake Poly Haven materials into terrain KTX2 texture arrays")]
struct Args {
    /// Poly Haven asset slugs, one per layer. Empty = the 11-biome default set.
    slugs: Vec<String>,
    /// Resolution tag understood by the API: 1k, 2k, 4k, 8k.
    #[arg(long, default_value = "1k")]
    res: String,
    /// Output dir for the arrays (default: <repo>/assets/terrain).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Download cache (default: <repo>/raw/polyhaven).
    #[arg(long)]
    raw: Option<PathBuf>,
    /// Output file prefix: <name>_albedo_array.ktx2 etc.
    #[arg(long, default_value = "terrain")]
    name: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out_dir = args.out.clone().unwrap_or(repo.join("assets/terrain"));
    let raw_dir = args.raw.clone().unwrap_or(repo.join("raw/polyhaven"));
    fs::create_dir_all(&out_dir)?;

    let layers: Vec<(String, String)> = if args.slugs.is_empty() {
        BIOME_SET.iter().map(|(b, s)| (b.to_string(), s.to_string())).collect()
    } else {
        args.slugs.iter().map(|s| (s.clone(), s.clone())).collect()
    };

    // Download every map of every layer first (cached in raw_dir).
    let mut downloads: Vec<Vec<PathBuf>> = Vec::new();
    let mut sizes_m: Vec<f32> = Vec::new();
    for (_, slug) in &layers {
        downloads.push(download_material(slug, &args.res, &raw_dir)?);
        sizes_m.push(material_size_m(slug)?);
    }

    let mut manifest = Vec::new();
    for (mi, (_, suffix, srgb)) in MAPS.iter().enumerate() {
        let images: Vec<RgbaImage> = downloads
            .iter()
            .map(|maps| decode(&maps[mi]))
            .collect::<Result<_>>()?;
        let path = out_dir.join(format!("{}_{}_array.ktx2", args.name, suffix));
        bake_array(&path, &images, *srgb)?;
        verify_ktx2(&path, images[0].width(), layers.len() as u32, *srgb)?;
        manifest.push((suffix.to_string(), path));
    }

    // Layer manifest: which slug landed at which layer index + native size.
    let json = serde_json::json!({
        "res": args.res,
        "layers": layers.iter().enumerate().map(|(i, (biome, slug))| {
            serde_json::json!({ "layer": i, "biome": biome, "slug": slug, "size_m": sizes_m[i] })
        }).collect::<Vec<_>>(),
        "arrays": manifest.iter().map(|(s, p)| {
            serde_json::json!({ "map": s, "file": p.file_name().unwrap().to_str() })
        }).collect::<Vec<_>>(),
    });
    let manifest_path = out_dir.join(format!("{}_layers.json", args.name));
    fs::write(&manifest_path, serde_json::to_string_pretty(&json)?)?;
    println!("wrote {}", manifest_path.display());

    // Paste-ready per-layer tiling for the consumer shader (chit_planet.wgsl).
    let tiles: Vec<String> = sizes_m.iter().map(|s| format!("{s:.1}")).collect();
    println!("const LAYER_TILE_M = array<f32, {}>({});", tiles.len(), tiles.join(", "));
    Ok(())
}

/// Native physical coverage of the material, meters (the API's `dimensions`
/// metadata is millimeters). 2m fallback for the rare asset without it.
fn material_size_m(slug: &str) -> Result<f32> {
    let info: serde_json::Value = ureq::get(&format!("https://api.polyhaven.com/info/{slug}"))
        .call()
        .with_context(|| format!("info API for {slug}"))?
        .into_json()?;
    Ok(info
        .get("dimensions")
        .and_then(|d| d.get(0))
        .and_then(|v| v.as_f64())
        .map_or(2.0, |mm| (mm / 1000.0) as f32))
}

/// Fetch one material's maps at `res` via the files API; returns one cached
/// path per `MAPS` entry.
fn download_material(slug: &str, res: &str, raw_dir: &Path) -> Result<Vec<PathBuf>> {
    let dir = raw_dir.join(slug);
    fs::create_dir_all(&dir)?;
    let files: serde_json::Value = ureq::get(&format!("https://api.polyhaven.com/files/{slug}"))
        .call()
        .with_context(|| format!("files API for {slug}"))?
        .into_json()?;

    let mut out = Vec::new();
    for (key, _, _) in MAPS {
        let entry = files
            .get(key)
            .and_then(|m| m.get(res))
            .with_context(|| {
                let keys: Vec<_> = files.as_object().map(|o| o.keys().collect()).unwrap_or_default();
                format!("{slug}: no {key}@{res} (available: {keys:?})")
            })?;
        // Prefer png; jpg fallback (some huge maps are jpg-only).
        let (ext, file) = ["png", "jpg"]
            .iter()
            .find_map(|e| entry.get(e).map(|f| (*e, f)))
            .with_context(|| format!("{slug}: {key}@{res} has no png/jpg"))?;
        let url = file["url"].as_str().context("url")?;
        let size = file["size"].as_u64().unwrap_or(0);
        let path = dir.join(format!("{slug}_{key}_{res}.{ext}"));
        if path.metadata().map(|m| m.len()).ok() != Some(size) {
            println!("downloading {url}");
            let mut bytes = Vec::new();
            ureq::get(url).call()?.into_reader().take(1 << 31).read_to_end(&mut bytes)?;
            fs::write(&path, &bytes)?;
        }
        out.push(path);
    }
    Ok(out)
}

fn decode(path: &Path) -> Result<RgbaImage> {
    Ok(image::open(path)
        .with_context(|| format!("decode {}", path.display()))?
        .to_rgba8())
}

/// Assemble one RGBA8 KTX2 2D-array with a full mip chain, all layers resized
/// to the first layer's (square) dimensions.
fn bake_array(path: &Path, images: &[RgbaImage], srgb: bool) -> Result<()> {
    let size = images[0].width();
    if size != images[0].height() || !size.is_power_of_two() {
        bail!("layer 0 must be square power-of-two, got {}x{}", size, images[0].height());
    }
    let images: Vec<RgbaImage> = images
        .iter()
        .map(|img| {
            if img.width() == size && img.height() == size {
                img.clone()
            } else {
                image::imageops::resize(img, size, size, FilterType::Lanczos3)
            }
        })
        .collect();

    let level_count = size.ilog2() + 1;
    // level_data[l] = all layers' pixels at mip l, layer-major within the level.
    let mut level_data: Vec<Vec<u8>> = Vec::new();
    for l in 0..level_count {
        let s = (size >> l).max(1);
        let mut data = Vec::with_capacity((s * s * 4) as usize * images.len());
        for img in &images {
            if l == 0 {
                data.extend_from_slice(img.as_raw());
            } else {
                data.extend_from_slice(
                    image::imageops::resize(img, s, s, FilterType::Lanczos3).as_raw(),
                );
            }
        }
        level_data.push(data);
    }
    write_ktx2(path, size, images.len() as u32, &level_data, srgb)?;
    println!(
        "baked {} ({} layers, {}px, {} mips, {:.1} MiB)",
        path.display(),
        images.len(),
        size,
        level_count,
        fs::metadata(path)?.len() as f64 / (1024.0 * 1024.0)
    );
    Ok(())
}

const VK_FORMAT_R8G8B8A8_UNORM: u32 = 37;
const VK_FORMAT_R8G8B8A8_SRGB: u32 = 43;

/// Minimal spec-conforming KTX2 writer: RGBA8, no supercompression, no kvd,
/// levels stored smallest-first as the spec mandates.
fn write_ktx2(path: &Path, size: u32, layers: u32, level_data: &[Vec<u8>], srgb: bool) -> Result<()> {
    let level_count = level_data.len() as u32;
    let mut f = Vec::new();
    f.extend_from_slice(&[0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A]);
    let vk_format = if srgb { VK_FORMAT_R8G8B8A8_SRGB } else { VK_FORMAT_R8G8B8A8_UNORM };
    for v in [vk_format, 1, size, size, 0, layers, 1, level_count, 0] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    let dfd_offset = 80 + level_count * 24;
    let dfd_len = 4 + 24 + 4 * 16;
    for v in [dfd_offset, dfd_len, 0, 0] {
        f.extend_from_slice(&v.to_le_bytes()); // dfd off/len, kvd off/len (no kvd)
    }
    f.extend_from_slice(&0u64.to_le_bytes()); // sgd offset
    f.extend_from_slice(&0u64.to_le_bytes()); // sgd length

    // Level index: file order is smallest level first; offsets computed now.
    let mut offset = (dfd_offset + dfd_len) as u64;
    let mut level_offsets = vec![0u64; level_data.len()];
    for l in (0..level_data.len()).rev() {
        offset = (offset + 3) & !3; // mipPadding: lcm(texel block 4, 4)
        level_offsets[l] = offset;
        offset += level_data[l].len() as u64;
    }
    for (l, data) in level_data.iter().enumerate() {
        for v in [level_offsets[l], data.len() as u64, data.len() as u64] {
            f.extend_from_slice(&v.to_le_bytes());
        }
    }

    // DFD: one basic descriptor block, 4 unsigned 8-bit samples (R,G,B,A).
    f.extend_from_slice(&dfd_len.to_le_bytes());
    f.extend_from_slice(&0u32.to_le_bytes()); // vendor 0 | type 0
    f.extend_from_slice(&(2u32 | ((24 + 4 * 16) << 16)).to_le_bytes()); // version 2 | block size
    let transfer = if srgb { 2u8 } else { 1u8 }; // KHR_DF_TRANSFER_SRGB / _LINEAR
    f.extend_from_slice(&[1, 1, transfer, 0]); // model RGBSDA, primaries BT709, flags straight
    f.extend_from_slice(&[0, 0, 0, 0]); // texel block dimensions (1x1x1x1)
    f.extend_from_slice(&[4, 0, 0, 0, 0, 0, 0, 0]); // bytesPlane0 = 4
    for (i, channel) in [0u32, 1, 2, 15].iter().enumerate() {
        // sRGB formats still store alpha linearly (KHR_DF_SAMPLE_DATATYPE_LINEAR).
        let linear = if srgb && *channel == 15 { 1u32 << 28 } else { 0 };
        f.extend_from_slice(&((i as u32 * 8) | (7 << 16) | (channel << 24) | linear).to_le_bytes());
        f.extend_from_slice(&0u32.to_le_bytes()); // sample position
        f.extend_from_slice(&0u32.to_le_bytes()); // lower
        f.extend_from_slice(&255u32.to_le_bytes()); // upper
    }

    for l in (0..level_data.len()).rev() {
        while f.len() % 4 != 0 {
            f.push(0);
        }
        assert_eq!(f.len() as u64, level_offsets[l]);
        f.extend_from_slice(&level_data[l]);
    }
    fs::write(path, f)?;
    Ok(())
}

/// Re-parse the written file with the `ktx2` crate and sanity-check the header
/// + per-level byte sizes — catches writer bugs before bevy ever loads it.
fn verify_ktx2(path: &Path, size: u32, layers: u32, srgb: bool) -> Result<()> {
    let bytes = fs::read(path)?;
    let reader = ktx2::Reader::new(&bytes).context("ktx2 reparse")?;
    let h = reader.header();
    let want_format = if srgb { ktx2::Format::R8G8B8A8_SRGB } else { ktx2::Format::R8G8B8A8_UNORM };
    if h.format != Some(want_format)
        || h.pixel_width != size
        || h.pixel_height != size
        || h.layer_count != layers
        || h.level_count != size.ilog2() + 1
        || h.supercompression_scheme.is_some()
    {
        bail!("{}: header mismatch: {h:?}", path.display());
    }
    for (l, level) in reader.levels().enumerate() {
        let s = (size >> l).max(1);
        let want = (s * s * 4 * layers) as usize;
        if level.len() != want {
            bail!("{}: level {l} is {} bytes, want {want}", path.display(), level.len());
        }
    }
    Ok(())
}
