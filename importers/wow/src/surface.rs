//! Surface response for classic WoW's colour-only materials, derived at bake time: a roughness
//! and a bump strength per kind of surface (by texture name: stone, wood, metal, cloth, leaves,
//! glass, water), and from each colour texture a height estimate (its luminance high-pass, dark
//! mortar and crevices low) turned into a normal map and a per-texel roughness, both off
//! (NORMAL_MAPS, ROUGHNESS_MAPS). The colours stay the originals. Maps bake to KTX2 (BC5 normals, linear BC7 data)
//! beside the colour PNG.

use std::fs;
use std::path::Path;

use aurora_bsn::bsn::Value;
use aurora_bsn::ktx::{Role, bake_texture};
use bevy::math::Vec3;
use image::{Rgba, RgbaImage};

/// How a kind of surface answers light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    /// Perceptual roughness where the texture is average.
    pub roughness: f32,
    /// Normal-map slope per unit of normalised height (0 = no normal map).
    pub bump: f32,
    /// Roughness added per unit of normalised depth (crevices rougher, raised paint smoother).
    pub variance: f32,
}

const fn surface(roughness: f32, bump: f32, variance: f32) -> Surface {
    Surface {
        roughness,
        bump,
        variance,
    }
}

/// How far WoW's gloss mask, where painted, lowers roughness at its peak.
const GLOSS_RANGE: f32 = 0.4;

/// Per-texel roughness maps (crevices rougher, the gloss mask smoother) and normal maps from the
/// height estimate. Off: a material keeps its colour and takes its kind's roughness as a factor.
const ROUGHNESS_MAPS: bool = false;
const NORMAL_MAPS: bool = false;

/// First match wins: specific kinds before the broad ones that share their words.
const KINDS: &[(&[&str], Surface)] = &[
    (&["water", "mist", "steam"], surface(0.08, 0.0, 0.0)),
    (
        &["glass", "bottle", "vial", "_wnd_", "window", "stained"],
        surface(0.2, 0.0, 0.0),
    ),
    (
        &[
            "treetallcanopy",
            "leaf",
            "flower",
            "grass",
            "shrub",
            "vine",
            "reed",
            "kelp",
            "lilypad",
            "seaweed",
            "cabbage",
        ],
        surface(0.6, 0.4, 0.08),
    ),
    (
        &[
            "metal",
            "iron",
            "chain",
            "anvil",
            "blade",
            "sword",
            "shield",
            "plate",
            "pick",
            "shovel",
            "cleaver",
            "brass",
            "silver",
            "lever",
            "rail",
            "pipe",
            "bars",
            "knob",
            "stein",
            "mug",
            "candel",
            "chandelier",
            "lantern",
            "lamp",
            "armor",
        ],
        surface(0.45, 0.6, 0.15),
    ),
    (
        &[
            "cloth", "banner", "rug", "quilt", "pillow", "tent", "leather", "rope", "sack", "hay",
            "canopy", "net", "web", "cocoon", "cork",
        ],
        surface(0.95, 0.5, 0.03),
    ),
    (&["roof", "shingle"], surface(0.85, 1.4, 0.1)),
    (
        &["tree", "trunk", "stump", "branch", "bark", "log"],
        surface(0.85, 1.5, 0.1),
    ),
    (
        &[
            "wood",
            "plank",
            "beam",
            "barrel",
            "crate",
            "cask",
            "door",
            "fence",
            "post",
            "wagon",
            "wheel",
            "furniture",
            "chair",
            "bench",
            "table",
            "support",
            "dock",
            "lumber",
            "planter",
            "keg",
        ],
        surface(0.75, 1.1, 0.12),
    ),
    (
        &[
            "brick", "stone", "rock", "cobble", "street", "sidewalk", "tomb", "monument", "statue",
            "bust", "wall", "floor", "cave", "border", "trim", "pillar", "dirt", "ash", "crop",
        ],
        surface(0.8, 1.8, 0.12),
    ),
];

const DEFAULT: Surface = surface(0.85, 1.0, 0.1);

/// Ground layers (the landscape's palette): tiled small, so gentler relief than a wall's.
const GROUND: &[(&[&str], Surface)] = &[
    (&["cobble", "road", "street"], surface(0.8, 1.2, 0.1)),
    (&["rock", "stone", "lava"], surface(0.8, 1.2, 0.1)),
    (
        &["grass", "flower", "scrub", "moss", "crop"],
        surface(0.92, 0.5, 0.05),
    ),
];

const GROUND_DEFAULT: Surface = surface(0.95, 0.6, 0.05);

/// The kind of ground a landscape layer paints, by its name.
pub fn classify_ground(texture: &str) -> Surface {
    let name = texture.to_ascii_lowercase();
    GROUND
        .iter()
        .find(|(words, _)| words.iter().any(|w| name.contains(w)))
        .map_or(GROUND_DEFAULT, |(_, s)| *s)
}

/// The kind of surface a texture paints, by its name.
pub fn classify(texture: &str) -> Surface {
    let name = texture.to_ascii_lowercase();
    KINDS
        .iter()
        .find(|(words, _)| words.iter().any(|w| name.contains(w)))
        .map_or(DEFAULT, |(_, s)| *s)
}

/// The derived maps of one colour texture, as file names in the textures dir.
pub struct Derived {
    pub normal: Option<String>,
    pub metallic_roughness: Option<String>,
}

/// Bakes (once per run: the output dir starts empty) the normal and roughness maps of the
/// colour texture `name` in `textures_dir`. `gloss`: the texture's alpha is WoW's specular
/// mask (an opaque material's), not opacity: roughness follows it and it shapes the relief
/// with the luminance.
pub fn derive(textures_dir: &Path, name: &str, surface: Surface, gloss: bool) -> Option<Derived> {
    let stem = name.strip_suffix(".png").unwrap_or(name);
    let normal_name = format!("{stem}_n.ktx2");
    let mr_name = format!("{stem}_mr.ktx2");
    let want_normal = NORMAL_MAPS && surface.bump > 0.0;
    let done = (!ROUGHNESS_MAPS || textures_dir.join(&mr_name).exists())
        && (!want_normal || textures_dir.join(&normal_name).exists());
    if !done {
        let colour = image::open(textures_dir.join(name)).ok()?.to_rgba8();
        let mask = gloss.then(|| gloss_mask(&colour)).flatten();
        let height = height_field(&colour, mask.as_deref());
        let (normal, mr) = maps(&colour, &height, mask.as_deref(), surface);
        let tmp = textures_dir.join(format!("{stem}.derived.png"));
        let bake = |image: &RgbaImage, role: Role, dst: &str| -> Option<()> {
            image.save(&tmp).ok()?;
            let baked = bake_texture(&tmp, role, &textures_dir.join(dst));
            let _ = fs::remove_file(&tmp);
            baked.map_err(|e| eprintln!("  {dst}: {e:#}")).ok()
        };
        if ROUGHNESS_MAPS {
            bake(&mr, Role::Data, &mr_name)?;
        }
        if want_normal {
            bake(&normal, Role::Normal, &normal_name)?;
        }
    }
    Some(Derived {
        normal: want_normal.then_some(normal_name),
        metallic_roughness: ROUGHNESS_MAPS.then_some(mr_name),
    })
}

/// Material fields for a colour-only WoW material: its kind's roughness factor (1 under a
/// roughness map, which carries the value) and the derived maps, replacing the matte default.
pub fn apply(fields: &mut Vec<(&'static str, Value)>, prefix: &str, textures_dir: &Path) {
    let Some(name) = fields
        .iter()
        .find_map(|(field, value)| match (field, value) {
            (&"base_color_texture", Value::Str(path)) => path.rsplit('/').next().map(str::to_owned),
            _ => None,
        })
    else {
        return;
    };
    let surface = classify(&name);
    // Cutouts and blends keep alpha as opacity; cloth, glass and water have no gloss to vary.
    let gloss = !fields.iter().any(|(field, _)| *field == "alpha_mode")
        && surface.bump > 0.0
        && surface.roughness < 0.9;
    fields.retain(|(field, _)| *field != "perceptual_roughness");
    let derived = derive(textures_dir, &name, surface, gloss);
    match derived.as_ref().and_then(|d| d.metallic_roughness.as_ref()) {
        Some(mr) => {
            fields.push(("perceptual_roughness", Value::Float(1.0)));
            fields.push((
                "metallic_roughness_texture",
                Value::Str(format!("{prefix}/textures/{mr}")),
            ));
        }
        None => fields.push(("perceptual_roughness", Value::Float(surface.roughness))),
    }
    if let Some(normal) = derived.and_then(|d| d.normal) {
        fields.push((
            "normal_map_texture",
            Value::Str(format!("{prefix}/textures/{normal}")),
        ));
    }
}

/// The alpha channel as a gloss mask, 0..1 over its own peak; `None` when it is flat (no mask
/// was painted).
fn gloss_mask(colour: &RgbaImage) -> Option<Vec<f32>> {
    let alpha: Vec<f32> = colour.pixels().map(|p| p.0[3] as f32).collect();
    let (lo, hi) = alpha
        .iter()
        .fold((f32::MAX, 0.0f32), |(lo, hi), &a| (lo.min(a), hi.max(a)));
    (hi - lo > 8.0).then(|| alpha.iter().map(|a| a / hi).collect())
}

/// The texture's relief, guessed from its painting: luminance (and the gloss mask, whose
/// mortar and grime read cleaner) minus its local average, so painted light gradients do not
/// tilt the surface, in units of its own spread. Tiles wrap.
fn height_field(colour: &RgbaImage, mask: Option<&[f32]>) -> Vec<f32> {
    let (w, h) = colour.dimensions();
    let (w, h) = (w as usize, h as usize);
    let lum: Vec<f32> = colour
        .pixels()
        .map(|Rgba([r, g, b, _])| {
            let c = |v: u8| (v as f32 / 255.0).powf(2.2);
            0.2126 * c(*r) + 0.7152 * c(*g) + 0.0722 * c(*b)
        })
        .map(|l| l.max(1.0e-4).sqrt())
        .collect();
    let lum = high_pass(&lum, w, h);
    match mask {
        Some(mask) => {
            let mask = high_pass(mask, w, h);
            let mixed: Vec<f32> = lum.iter().zip(&mask).map(|(l, m)| 0.5 * (l + m)).collect();
            normalised(mixed)
        }
        None => lum,
    }
}

/// `values` minus their local average, in units of their spread.
fn high_pass(values: &[f32], w: usize, h: usize) -> Vec<f32> {
    let fine = box_blur(values, w, h, 1);
    let coarse = box_blur(&fine, w, h, (w.min(h) / 16).max(2));
    normalised(fine.iter().zip(&coarse).map(|(f, c)| f - c).collect())
}

fn normalised(mut high: Vec<f32>) -> Vec<f32> {
    let spread = (high.iter().map(|v| v * v).sum::<f32>() / high.len() as f32)
        .sqrt()
        .max(1.0e-4);
    for v in &mut high {
        *v = (*v / spread).clamp(-3.0, 3.0);
    }
    high
}

/// Separable box blur of radius `r`, wrapping at the edges.
fn box_blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let n = (2 * r + 1) as f32;
    let mut tmp = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in 0..=2 * r {
                s += src[y * w + (x + w * 4 + k - r) % w];
            }
            tmp[y * w + x] = s / n;
        }
    }
    let mut out = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut s = 0.0;
            for k in 0..=2 * r {
                s += tmp[((y + h * 4 + k - r) % h) * w + x];
            }
            out[y * w + x] = s / n;
        }
    }
    out
}

/// The normal map (tangent space, +Y toward the top of the image as glTF and aurora's
/// tangent frame have it) and the metallic-roughness map (G roughness, B metallic 0).
fn maps(
    colour: &RgbaImage,
    height: &[f32],
    mask: Option<&[f32]>,
    s: Surface,
) -> (RgbaImage, RgbaImage) {
    let (w, h) = colour.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let at = |x: usize, y: usize| height[(y % hu) * wu + (x % wu)];
    let mut normal = RgbaImage::new(w, h);
    let mut mr = RgbaImage::new(w, h);
    for y in 0..hu {
        for x in 0..wu {
            let dx = (at(x + 1, y) - at(x + wu - 1, y)) * 0.5;
            // Up the image is row - 1.
            let dy = (at(x, y + hu - 1) - at(x, y + 1)) * 0.5;
            let n = Vec3::new(-s.bump * dx, -s.bump * dy, 1.0).normalize();
            let byte = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
            normal.put_pixel(
                x as u32,
                y as u32,
                Rgba([byte(n.x), byte(n.y), byte(n.z), 255]),
            );
            let rough = match mask {
                Some(mask) => {
                    let g = mask[y * wu + x];
                    s.roughness + s.variance - g * (GLOSS_RANGE + s.variance)
                }
                None => s.roughness - s.variance * at(x, y),
            }
            .clamp(0.04, 1.0);
            let g = (rough * 255.0).round() as u8;
            mr.put_pixel(x as u32, y as u32, Rgba([255, g, 0, 255]));
        }
    }
    (normal, mr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_name() {
        assert_eq!(classify("mm_abbey_brick_01.png").bump, 1.8);
        assert_eq!(classify("elwynntreetallcanopy_new01.png").roughness, 0.6);
        assert_eq!(classify("mm_canopy_opt.png").roughness, 0.95);
        assert_eq!(classify("glassbottle04.png").bump, 0.0);
        assert_eq!(classify("westfallwagonwood.png").roughness, 0.75);
        assert_eq!(classify("mm_abbey_combo_01.png"), DEFAULT);
        assert_eq!(
            classify_ground("elwynn_elwynndirtbase2_s.png"),
            GROUND_DEFAULT
        );
        assert_eq!(
            classify_ground("elwynn_elwynngrassbase_s.png").roughness,
            0.92
        );
    }
}
