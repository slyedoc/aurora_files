//! Bake-time sanity checks on imported materials.
//!
//! These catch the failure class that is INVISIBLE at bake and only shows up in the viewer:
//! numbers that are individually well-formed but in the wrong unit system. The recurring one is
//! emissive.
//!
//! # The two emissive conventions
//!
//! glTF's `emissiveFactor` is a unitless `[0,1]` FACTOR. Before `KHR_materials_emissive_strength`
//! there was no way to express more, so a large body of content (Bistro, most Sketchfab exports,
//! any Blender Emission node left at strength 1) encodes emitters in 0..1. Solari is PHYSICAL —
//! radiance in cd/m², metered by the camera's EV100.
//!
//! At `Exposure::BLENDER` (EV100 9.7) the multiplier is `exp2(-9.7) / 1.2` ≈ `1/998`, so:
//!
//! | radiance | post-exposure | reads as        |
//! |----------|---------------|-----------------|
//! | 1.0      | 0.001         | black           |
//! | 100      | 0.10          | dim but visible |
//! | 180      | 0.18          | middle grey     |
//! | 998      | 1.00          | white point     |
//! | 20000    | 20.0          | hot, blooms     |
//!
//! A factor-convention asset is therefore ~1000x too dim and renders BLACK while looking correct
//! in the authoring tool. The check keys off post-exposure contribution rather than a magic
//! constant, so it tracks EV100 instead of drifting from it.

/// `Exposure::BLENDER` — bevy's `Exposure::default()`. Mirrors `bevy_camera`'s `EV100_BLENDER`.
pub const EV100_BLENDER: f32 = 9.7;

/// Fraction of the white point below which an emitter is considered suspiciously dim.
const DIM_FRACTION: f32 = 0.1;

/// bevy's `Exposure::exposure()`: `exp2(-ev100) / 1.2`.
pub fn exposure(ev100: f32) -> f32 {
    (-ev100).exp2() / 1.2
}

/// Rec.709 luminance — the same basis `prepare_light_sources` uses to weight emissive picking.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// What a material's emissive looks like once the camera exposes it.
pub struct EmissiveVerdict {
    pub luminance: f32,
    pub post_exposure: f32,
    pub warning: Option<String>,
}

/// Classify one material's emissive radiance. `None` for non-emitters.
pub fn check_emissive(name: &str, emissive: [f32; 3]) -> Option<EmissiveVerdict> {
    let lum = luminance(emissive);
    if lum <= 0.0 {
        return None;
    }
    let post = lum * exposure(EV100_BLENDER);
    let peak = emissive[0].max(emissive[1]).max(emissive[2]);

    // <= 1.0 is the glTF factor clamp: almost certainly authored in the factor convention.
    // Between there and the dim threshold is a dead zone — too big to be a deliberate factor,
    // too small to be a visible physical emitter.
    let warning = if post < DIM_FRACTION {
        let needed = DIM_FRACTION / exposure(EV100_BLENDER);
        Some(if peak <= 1.0 + f32::EPSILON {
            format!(
                "{name}: emissive peak {peak:.3} <= 1.0 — looks like a glTF [0,1] FACTOR, not \
                 radiance. Exposed at EV100 {EV100_BLENDER} it renders {post:.5} (black). Scale to \
                 nits (>= {needed:.0}) or set `emissive_nits` for this material."
            )
        } else {
            format!(
                "{name}: emissive luminance {lum:.2} nits exposes to {post:.4}, under {DIM_FRACTION} \
                 of the white point — visible as near-black. Authored in scene-relative units? \
                 (>= {needed:.0} nits reads as a dim emitter; ~1000 is white.)"
            )
        })
    } else {
        None
    };
    Some(EmissiveVerdict { luminance: lum, post_exposure: post, warning })
}

/// Whether a material would render as untextured default white — the signature of an import that
/// dropped its factors (or an asset that genuinely ships none).
pub fn is_default_white(base_color: [f32; 4], has_any_texture: bool) -> bool {
    !has_any_texture && base_color == [1.0, 1.0, 1.0, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The anchor the whole check hangs on: at Blender exposure ~1000 nits is the white point.
    #[test]
    fn white_point_is_about_1000_nits() {
        let e = exposure(EV100_BLENDER);
        assert!((1.0 / e - 998.0).abs() < 2.0, "white point was {}", 1.0 / e);
        assert!((0.18 / e - 179.6).abs() < 1.0, "middle grey was {}", 0.18 / e);
    }

    /// Bistro-style content: emissive is a glTF [0,1] FACTOR and renders black.
    #[test]
    fn factor_convention_warns() {
        let v = check_emissive("wine", [0.0, 0.42, 1.0]).expect("emitter");
        let w = v.warning.expect("should warn");
        assert!(w.contains("FACTOR"), "{w}");
        assert!(v.post_exposure < 0.001, "post exposure was {}", v.post_exposure);
    }

    /// The hoverboard bug: scene-relative Cycles radiance, too big to be a factor, too dim to see.
    #[test]
    fn scene_relative_radiance_warns() {
        let v = check_emissive("M_Neon_Cyan", [0.425, 8.075, 8.5]).expect("emitter");
        let w = v.warning.expect("should warn");
        assert!(w.contains("scene-relative"), "{w}");
    }

    /// Physical nits — the fixed hoverboard and the shipped assets — must stay silent.
    #[test]
    fn physical_nits_are_quiet() {
        for rgb in [[85.0, 1615.0, 1700.0], [100.0, 100.0, 100.0], [2000.0, 2000.0, 2000.0]] {
            let v = check_emissive("neon", rgb).expect("emitter");
            assert!(v.warning.is_none(), "unexpected warning for {rgb:?}: {:?}", v.warning);
        }
    }

    /// A deliberately faint indicator sits just over the line, so the check stays a warning about
    /// the CONVENTION rather than a ban on dim emitters.
    #[test]
    fn threshold_is_one_tenth_of_white() {
        assert!(check_emissive("dim", [120.0, 120.0, 120.0]).unwrap().warning.is_none());
        assert!(check_emissive("dimmer", [80.0, 80.0, 80.0]).unwrap().warning.is_some());
    }

    #[test]
    fn non_emitters_are_ignored() {
        assert!(check_emissive("plate", [0.0, 0.0, 0.0]).is_none());
    }
}

/// Summary of a bake's material lint, printed by the importers.
#[derive(Default)]
pub struct Report {
    pub emitters: usize,
    pub warnings: Vec<String>,
    pub white_plastic: Vec<String>,
}

impl Report {
    pub fn print(&self) {
        if self.emitters > 0 {
            println!("lint: {} emissive material(s)", self.emitters);
        }
        for w in &self.warnings {
            println!("  WARN {w}");
        }
        if !self.white_plastic.is_empty() {
            println!(
                "  WARN {} material(s) have no textures and default white base_color — they will \
                 render as flat white plastic: {}",
                self.white_plastic.len(),
                self.white_plastic.join(", "),
            );
        }
        if self.warnings.is_empty() && self.white_plastic.is_empty() {
            println!("lint: no material warnings");
        }
    }
}
