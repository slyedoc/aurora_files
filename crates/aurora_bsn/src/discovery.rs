//! Filesystem-facing asset discovery: copy referenced textures, find + normalize displacement
//! maps, and classify alpha cutouts. The pixel/string logic lives in the [`crate::img`] module;
//! this module is the I/O glue (decode, scan, copy, save).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::img;

/// Scan `obj_dir` for raw displacement (depth) maps — files containing `_displace` that are NOT the
/// `N_`-prefixed normal-map conversions the MTL already binds via `map_Bump`. Each is written into
/// `<textures_dir>` **in bevy_solari's depth convention** (brighter = deeper): a raw height map is
/// inverted via [`img::invert_depth`], an already-inverted (`_inv`) map is copied byte-for-byte.
/// Returns `(base_key, filename)` where `base_key` is the name before `_displace`
/// (`columna_b_displacement_3_inv.png` → `columna_b`), matched to a material's diffuse texture by
/// prefix. The MTL never references these, so `copy_textures` misses them.
///
/// Always rewrites the output (no exists-skip) so the normalization is idempotent and self-healing
/// across re-runs; source files under `obj_dir` are never mutated.
pub fn displacement_index(obj_dir: &Path, textures_dir: &Path) -> Vec<(String, String)> {
    let mut maps = Vec::new();
    let Ok(entries) = fs::read_dir(obj_dir) else {
        return maps;
    };
    let mut inverted = 0usize;
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().to_string();
        let lower = file.to_lowercase();
        if lower.starts_with("n_") {
            continue; // normal-map conversion, not a depth map
        }
        let Some(idx) = lower.find("_displace") else {
            continue;
        };
        let dst = textures_dir.join(&file);
        if img::looks_like_height_name(&file) {
            // Raw height map (white = raised): flip to brighter-is-deeper before it ships.
            match image::open(entry.path()) {
                Ok(src) => {
                    let _ = img::invert_depth(&src).save(&dst);
                    inverted += 1;
                }
                Err(_) => {
                    let _ = fs::copy(entry.path(), &dst); // undecodable — copy as-is
                }
            }
        } else {
            // Already in depth convention — copy exact bytes (no recompression).
            let _ = fs::copy(entry.path(), &dst);
        }
        maps.push((lower[..idx].to_string(), file));
    }
    if inverted > 0 {
        println!("  normalized {inverted} height map(s) to depth convention (inverted)");
    }
    maps
}

/// Whether a material is a genuine alpha cutout, caching the (expensive) per-texture decode by
/// diffuse-texture path. tobj reports ~317k material entries backed by only ~280 unique textures,
/// so without the cache the classify pass re-decodes each PNG hundreds of times. Transmissive
/// (glass, `illum 4`) materials are left opaque-traced.
pub fn material_is_cutmask(
    obj_dir: &Path,
    material: &tobj::Material,
    cache: &mut HashMap<String, bool>,
) -> bool {
    if material.illumination_model == Some(4) {
        return false; // glass/raytrace: alpha-blended, not a cutout
    }
    let Some(tex) = material.diffuse_texture.as_deref() else {
        return false; // no texture: opacity is the dissolve factor, not a cutmask
    };
    if let Some(&cached) = cache.get(tex) {
        return cached;
    }
    let is_cutmask = texture_is_cutmask(&obj_dir.join(tex.replace('\\', "/")));
    cache.insert(tex.to_string(), is_cutmask);
    is_cutmask
}

/// Decode a base-color texture and test its alpha histogram for a genuine binary cutmask
/// ([`img::classify_cutmask`]). Unreadable / unsupported formats are treated as opaque.
fn texture_is_cutmask(src: &Path) -> bool {
    let Ok(image) = image::open(src) else {
        return false;
    };
    img::classify_cutmask(&image.into_rgba8())
}

/// Copy the base-color/normal textures referenced by the used materials into `<out>/textures/`,
/// so the `.bsn`'s `"<prefix>/textures/<name>"` paths resolve under the asset root. MTL paths are
/// relative to the OBJ; dedup by destination so each file is copied once.
pub fn copy_textures(
    obj_path: &Path,
    materials: &[tobj::Material],
    models: &[tobj::Model],
    textures_dir: &Path,
) {
    fs::create_dir_all(textures_dir).expect("create textures dir");
    let obj_dir = obj_path.parent().unwrap_or_else(|| Path::new("."));
    let mut seen = HashSet::new();
    let (mut copied, mut missing) = (0u32, 0u32);

    for model in models {
        let Some(material) = model.mesh.material_id.and_then(|id| materials.get(id)) else {
            continue;
        };
        for tex in [
            material.diffuse_texture.as_deref(),
            material.normal_texture.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !seen.insert(tex.to_string()) {
                continue;
            }
            let src = obj_dir.join(tex.replace('\\', "/"));
            let dst = textures_dir.join(img::basename(tex));
            if dst.exists() {
                continue;
            }
            match fs::copy(&src, &dst) {
                Ok(_) => copied += 1,
                Err(_) => {
                    missing += 1;
                    if missing <= 10 {
                        eprintln!("  missing texture: {}", src.display());
                    }
                }
            }
        }
    }
    println!(
        "copied {copied} textures -> {} ({missing} missing)",
        textures_dir.display()
    );
}

/// Make an OBJ object name safe for a filename.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect();
    if s.is_empty() {
        "mesh".to_string()
    } else {
        s
    }
}
