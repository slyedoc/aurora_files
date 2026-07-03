//! Pure image-processing helpers for the `.bsn` import pipeline.
//!
//! Two concerns live here, both depending only on `image` (no bevy / OMM):
//!   - **depth-map normalization** — fixing height maps into bevy_solari's `depth_map` convention.
//!   - **alpha-cutmask classification** — deciding which base-color textures are genuine foliage
//!     cutouts (mirrors the retired `scripts/reclassify_alpha.py`).
//! Plus the small filename helpers (`basename`, `match_displacement`) the importers use to wire
//! displacement maps to their diffuse textures.

use image::{DynamicImage, RgbaImage};

// --- Depth-map normalization --------------------------------------------------------------------
// bevy_solari's displacement pass treats a BRIGHTER texel as DEEPER (pushed inward along -normal):
// see `crates/bevy_solari/src/geometry/tess_displace.wgsl` ("a brighter texel is DEEPER"). A raw
// height map (white = raised) is the opposite and renders inverted, so it must be flipped to this
// convention before it ships into the scene.

/// Invert a depth/height map: `max - v` per colour channel, leaving alpha untouched. Bit-depth
/// aware (luma maps are often 16-bit) — delegates to `image`'s channel-correct invert. This is the
/// core of the "black where it should be white" fix.
pub fn invert_depth(img: &DynamicImage) -> DynamicImage {
    let mut out = img.clone();
    out.invert();
    out
}

/// Canonicalize a displacement map to the brighter-is-deeper convention: invert it when the source
/// is a raw height map, otherwise pass it through unchanged. Operates source → returned image
/// (never mutates the source), so re-running an importer is idempotent.
pub fn normalize_depth_map(src: &DynamicImage, looks_like_height: bool) -> DynamicImage {
    if looks_like_height {
        invert_depth(src)
    } else {
        src.clone()
    }
}

/// Default policy for whether a displacement map's *filename* denotes a raw height map needing
/// inversion: everything is treated as a height map UNLESS the name already says it's inverted
/// (`inv`), e.g. `columna_b_displacement_3_inv.png` passes through but `piso_rustico_displace2.png`
/// gets flipped. Importers can override this per asset.
pub fn looks_like_height_name(filename: &str) -> bool {
    !filename.to_lowercase().contains("inv")
}

// --- Alpha-cutmask classification ---------------------------------------------------------------
// A base-color texture is a genuine binary cutmask when a meaningful fraction of texels are cut
// away (alpha < ALPHA_HOLE) AND a meaningful fraction stay solid (alpha >= ALPHA_SOLID). The solid
// floor rejects an unused/packed alpha that's uniformly transparent (would vanish if masked at 0.5).

/// Min fraction of texels cut away (alpha < [`ALPHA_HOLE`]) for a cutmask.
pub const CUTMASK_MIN_HOLE_FRACTION: f64 = 0.01;
/// Min fraction of texels solid (alpha >= [`ALPHA_SOLID`]) for a cutmask.
pub const CUTMASK_MIN_SOLID_FRACTION: f64 = 0.05;
/// Alpha below this counts as a hole (cut away).
pub const ALPHA_HOLE: u8 = 128;
/// Alpha at/above this counts as solid.
pub const ALPHA_SOLID: u8 = 200;
/// Alpha-test cutoff emitted for `AlphaMode::Mask` and used for OMM baking.
pub const MASK_CUTOFF: f32 = 0.5;

/// Whether a decoded base-color image is a genuine binary alpha cutmask (the foliage heuristic).
pub fn classify_cutmask(rgba: &RgbaImage) -> bool {
    let total = (rgba.width() as u64 * rgba.height() as u64).max(1);
    let (mut holes, mut solid) = (0u64, 0u64);
    for px in rgba.pixels() {
        let a = px.0[3];
        if a < ALPHA_HOLE {
            holes += 1;
        }
        if a >= ALPHA_SOLID {
            solid += 1;
        }
    }
    (holes as f64 / total as f64) >= CUTMASK_MIN_HOLE_FRACTION
        && (solid as f64 / total as f64) >= CUTMASK_MIN_SOLID_FRACTION
}

// --- Filename helpers ---------------------------------------------------------------------------

/// Last path component of an MTL texture reference (MTL uses `\` separators).
pub fn basename(tex: &str) -> &str {
    tex.rsplit(['/', '\\']).next().unwrap_or(tex)
}

/// The displacement map (if any) whose base key prefixes this diffuse texture's name — e.g. diffuse
/// `columna_b_color.png` matches height base `columna_b`. `maps` holds `(base_key, filename)`
/// pairs; longest base wins (most specific).
pub fn match_displacement<'a>(diffuse: &str, maps: &'a [(String, String)]) -> Option<&'a str> {
    let stem = basename(diffuse).to_lowercase();
    maps.iter()
        .filter(|(base, _)| !base.is_empty() && stem.starts_with(base.as_str()))
        .max_by_key(|(base, _)| base.len())
        .map(|(_, file)| file.as_str())
}
