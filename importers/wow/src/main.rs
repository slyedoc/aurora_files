//! WoW zone importer: wow.export OBJ dumps → one `.bsn` per ADT tile (doodads + WMOs) plus
//! the zone's terrain as an `aurora_landscape` scene.
//!
//! Source layout (wow.export v0.2.6, see `~/code/p/bevy_wow/assets/wow`):
//!   maps/<map>/adt_X_Y.obj                          terrain (256 subchunk groups)
//!   maps/<map>/adt_X_Y_ModelPlacementInformation.csv  doodad (m2) + building (wmo) placements
//!   maps/<map>/tex_X_Y_<chunk>.{json,png}           per-subchunk splat layers + 64² alphas
//!   world/…/<model>.obj (+ .mtl + textures)         referenced models, shared across tiles
//!   world/wmo/…/<wmo>_ModelPlacementInformation.csv  interior doodads, local to the WMO
//!
//! Output:
//!   assets/wow/meshes/*.aurora_mesh + textures/*   baked models (shared)
//!   assets/wow/meshes/*.collider                     one collision mesh per model that has one
//!   assets/wow/models/<model>.bsn                    one scene per model: its submeshes and
//!                                                    collision body, in model space
//!   assets/wow/<map>_X_Y.bsn                        one instance of a model scene per doodad/WMO
//!                                                    placement (+ glow lights), TILE-LOCAL coords
//!   assets/wow/<map>_zone.bsn                       the Landscape: palette, one height + splat
//!                                                    layer per ADT, a sculpt and a paint layer
//!   assets/wow/map/<map>_X_Y_height.png             129² 16-bit height grid (range in the zone)
//!   assets/wow/map/<map>_X_Y_splat_ids.png          1024² palette ids, a byte per slot (RGBA)
//!   assets/wow/map/<map>_X_Y_splat_weights.png      1024² their weights, same layout
//!   assets/wow/map/tileset/*.png                    the shared splat palette's textures
//!   assets/wow/<map>_clutter.bsn                    the ground clutter: one ScatterSpecies per
//!                                                    plant part (rest mesh, wind poses,
//!                                                    palette densities), regenerated each run
//!
//! Terrain is NOT baked to a mesh: the zone scene is a landscape whose layers are the tiles'
//! heights and splatmaps; edits go on its sculpt and paint layers (jackdaw), never into the
//! baked files. The zone's origin is the centre of the range's middle tile. Every run cleans the
//! output dir and writes everything, the zone included; its `.pages` bake comes from saving the
//! zone in jackdaw.
//!
//! Coordinate frames: the wow.export OBJs live in `obj = (C − wow_x, height, C − wow_z)` where
//! `C = 32 · 533.333` and `wow_*` are the CSV's coords. The proven placement math from
//! `core/tools/import_wow` works in the `+wow_x/+wow_z` tile-local frame; our frame is that
//! rotated 180° about Y, so placements get `t → (−t.x, t.y, −t.z)` and `R → RotY(π) · R`.
//!
//! Emissives: wow.export writes a `.json` beside each model. For an M2 it carries the
//! materials (flags, blend mode) and the skin's texture units, which map an OBJ `Geoset<i>`
//! group to its material; for a WMO the materials (flags, self-illuminated colour) and each
//! group's render batches, which map an OBJ `<GroupName><batch>` group. An additive-blended
//! M2 material (blend 3/4/7) is a glow card: the base-colour texture becomes the emissive
//! texture at `--emissive-nits`. An unlit material (flag 1) is WoW's fullbright: lantern
//! glass and lit windows, but also campfire logs and lava, so it glows at the lower
//! `--unlit-nits`. A WMO material with a self-illuminated (sidn) colour glows in that colour
//! at `--sidn-nits`, day and night alike (one value, as WoW shows the texture as is).
//!
//! What emits is baked into derived textures under `textures/`. A glow CARD (any emissive
//! material whose texture has real alpha -- the shape lives there, the RGB is bright
//! everywhere) is WoW's additive billboard halo, which a flat emitter cannot be: a path
//! tracer shows it as a disc. So a card becomes two things: its CORE (`<tex>_card.png`,
//! black RGB with the alpha, cut at 0.5, emitting `<tex>_glow.png` = RGB x alpha) -- the
//! visible flame -- and a `PointLight` at the card's centre, the halo's illumination without the
//! halo. A FULLBRIGHT material (opaque alpha) keeps its base colour and emits `<tex>_glow.png` =
//! RGB^`--unlit-contrast` (1: its own colour, frame and glass alike, as WoW draws it unlit); a
//! lamp whose only emitter is such a material gets its point light there.
//!
//! Emitters glow only a little (they have to read, not light the scene); the light is the
//! point light's, by what the lamp is (`lamp_lumens`: a lantern 800 lm, a candle 40, a firefly
//! 2), times `--light-scale`. WoW's volumetric light shafts are glow cards that light nothing.
//!
//! Collision: a model with collision has a child in its scene carrying
//! `bevy_aurora::collision::CollisionMesh`, which shares one built collider between all of the
//! model's instances (zero turns it into an avian static body). An M2's comes from wow.export's `<model>.phys.obj` -- WoW's own
//! collision mesh, so a tree is its trunk and bushes are walked through; a WMO's from its
//! `.phys.obj` (its BSP faces, written by the local wow.export fork with "export collision" on),
//! else from all its render triangles (an older dump has no collision-only ramps or caps, so
//! stairs collide as steps). Ground clutter never collides.
//!
//! Ground clutter: each splat layer's `effectID` (wow.export tex json) -> GroundEffectTexture
//! (density, up to four doodads with weights) -> GroundEffectDoodad (model file id) -> the
//! community listfile (path) -> `maps/<map>/foliage/<basename>.obj`. The plants are baked
//! in plant space (cutmask -> Mask + opacity micromap, plus baked wind poses for what WoW
//! animates) and written as `ScatterSpecies` of the zone's landscape (`<map>_clutter.bsn`,
//! instanced by the zone's `Clutter` child); aurora_landscape grows them on the GPU.
//!
//! Deferred: liquids, creatures.
//!
//!   cargo run --release -p wow_import                        # Northshire (31-33 × 47-49)

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f32::consts::PI;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use aurora_bsn::bsn::{self, Entity, Value, material_fields, mesh_entity, scene};
use aurora_bsn::discovery::{copy_textures, material_is_cutmask, sanitize};
use aurora_bsn::mesh::{OmmOptions, attach_omm, build_mesh, submesh_centroid};
use aurora_mesh::{AuroraMesh, write_aurora_mesh};
use bevy::math::{Quat, Vec3};
use clap::Parser;

mod surface;

/// One WoW ADT tile, in yards (= world units): `1600/3`.
const TILE: f32 = 1600.0 / 3.0;
/// wow.export's map-space origin constant: `32 · TILE` (maps span ADT 0..64 centered at 32).
const C: f32 = 32.0 * TILE;
/// Height grid edge: 16 subchunks × 8 cells + 1.
const HEIGHT_RES: u32 = 129;
/// Splatmap edge (16 chunk cells of 64px).
const ALPHA_ATLAS: u32 = 1024;
/// `aurora_landscape::HOLE`: the palette id that is no ground. A splat texel no ADT triangle
/// covers (WoW's terrain holes: the mine mouths, cave entrances) is all hole.
const HOLE: u8 = 255;
const ALPHA_CELL: u32 = 64;
/// No texture in a chunk-layer slot.
const NO_LAYER: u32 = u32::MAX;
/// Texture repeats per chunk edge (WoW ground textures tile ~every 4.2 yd).
const REPEATS: f32 = 8.0;

#[derive(Parser)]
#[command(about = "Bake wow.export ADT tiles → per-tile .bsn + editable terrain map data")]
struct Args {
    /// wow.export dump root (contains `maps/` and `world/`).
    #[arg(long, default_value = "/home/slyedoc/code/p/bevy_wow/assets/wow")]
    wow_root: PathBuf,
    /// Map name under `maps/`.
    #[arg(long, default_value = "azeroth")]
    map: String,
    /// ADT tile range, inclusive. Default = the Northshire Abbey 3×3 ("abby").
    #[arg(long, default_value_t = 31)]
    x0: i32,
    #[arg(long, default_value_t = 33)]
    x1: i32,
    #[arg(long, default_value_t = 47)]
    y0: i32,
    #[arg(long, default_value_t = 49)]
    y1: i32,
    /// Output asset directory.
    #[arg(long, default_value = "assets/wow")]
    out_dir: PathBuf,
    /// Asset-server-relative prefix the `.bsn` uses to reference meshes/textures.
    #[arg(long, default_value = "wow")]
    asset_prefix: String,
    /// The community listfile (`<file id>;<path>` per line): resolves GroundEffectDoodad
    /// model ids to plant model names.
    #[arg(
        long,
        default_value = "/home/slyedoc/code/p/core/assets/terrain/dbc/listfile.csv"
    )]
    listfile: PathBuf,
    /// Radiance (nits) of additive-blended materials: a glow card's visible core (the flame).
    /// The light a lamp gives is its point light's (`lamp_lumens`), so this only has to read.
    #[arg(long, default_value_t = 500.0)]
    emissive_nits: f32,
    /// Radiance (nits) of unlit (fullbright) materials: lantern glass, lit logs.
    #[arg(long, default_value_t = 40.0)]
    unlit_nits: f32,
    /// Radiance (nits) of WMO self-illuminated colours (lit windows), day and night alike: WoW
    /// shows its texture as is, and 200 reads by day beside shaded stone (~5-30 nits).
    #[arg(long, default_value_t = 200.0)]
    sidn_nits: f32,
    /// Multiplies every lamp's point light (`lamp_lumens`).
    #[arg(long, default_value_t = 1.0)]
    light_scale: f32,
    /// Exponent on a fullbright material's linear colour for its glow map. 1 is WoW's: the
    /// texture shows as it is, frame and glass alike (higher left lantern frames black).
    #[arg(long, default_value_t = 1.0)]
    unlit_contrast: f32,
}

// ---- emissive materials from the wow.export json sidecars ----------------------------------

/// What a submesh emits, from its model's json: a tint (linear, 1 = white) and whether the
/// material is additive-blended (a glow card whose alpha is coverage).
#[derive(Clone, Copy, Debug)]
struct Emissive {
    tint: [f32; 3],
    nits: f32,
    additive: bool,
}

/// Per OBJ submesh (tobj model order), the emissive its material carries.
fn model_emissives(args: &Args, obj_path: &Path, models: &[tobj::Model]) -> Vec<Option<Emissive>> {
    let json_path = obj_path.with_extension("json");
    let Ok(text) = fs::read_to_string(&json_path) else {
        return vec![None; models.len()];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![None; models.len()];
    };
    match json["fileType"].as_str() {
        Some("m2") => m2_emissives(args, &json, models),
        Some("wmo") => wmo_emissives(args, &json, models),
        _ => vec![None; models.len()],
    }
}

fn m2_emissives(
    args: &Args,
    json: &serde_json::Value,
    models: &[tobj::Model],
) -> Vec<Option<Emissive>> {
    let materials = json["materials"].as_array();
    let units = json["skin"]["textureUnits"].as_array();
    let (Some(materials), Some(units)) = (materials, units) else {
        return vec![None; models.len()];
    };
    models
        .iter()
        .map(|m| {
            // `Geoset<i>` = skin section i; its first texture unit names the material.
            let section: u64 = m.name.strip_prefix("Geoset")?.parse().ok()?;
            let unit = units
                .iter()
                .find(|u| u["skinSectionIndex"].as_u64() == Some(section))?;
            let material = materials.get(unit["materialIndex"].as_u64()? as usize)?;
            let flags = material["flags"].as_u64().unwrap_or(0);
            let blend = material["blendingMode"].as_u64().unwrap_or(0);
            let unlit = flags & 1 != 0;
            let additive = matches!(blend, 3 | 4 | 7);
            (unlit || additive).then_some(Emissive {
                tint: [1.0; 3],
                nits: if additive {
                    args.emissive_nits
                } else {
                    args.unlit_nits
                },
                additive,
            })
        })
        .collect()
}

fn wmo_emissives(
    args: &Args,
    json: &serde_json::Value,
    models: &[tobj::Model],
) -> Vec<Option<Emissive>> {
    let (Some(materials), Some(groups)) = (json["materials"].as_array(), json["groups"].as_array())
    else {
        return vec![None; models.len()];
    };
    // OBJ group `<GroupName><batch>` -> WMO material id, from each group's render batches.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for group in groups {
        let Some(name) = group["groupName"].as_str() else {
            continue;
        };
        for (batch, info) in group["renderBatches"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            if let Some(id) = info["materialID"].as_u64() {
                by_name
                    .entry(format!("{name}{batch}"))
                    .or_insert(id as usize);
            }
        }
    }
    models
        .iter()
        .map(|m| {
            let material = materials.get(*by_name.get(&m.name)?)?;
            let flags = material["flags"].as_u64().unwrap_or(0);
            let unlit = flags & 1 != 0;
            // Self-illuminated day/night colour, 0xAARRGGBB.
            let sidn = material["color1"].as_u64().unwrap_or(0) & 0x00ff_ffff;
            if !unlit && sidn == 0 {
                return None;
            }
            let (tint, nits) = if sidn != 0 {
                let c = |shift: u32| srgb_to_linear(((sidn >> shift) & 0xff) as f32 / 255.0);
                ([c(16), c(8), c(0)], args.sidn_nits)
            } else {
                ([1.0; 3], args.unlit_nits)
            };
            Some(Emissive {
                tint,
                nits,
                additive: false,
            })
        })
        .collect()
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The `.bsn` material fields for an emissive submesh, baking its derived glow (and card)
/// textures on the way. `None` (no texture / unreadable) leaves the material as it was.
fn emissive_fields(
    args: &Args,
    obj_dir: &Path,
    textures_dir: &Path,
    material: Option<&tobj::Material>,
    is_cutmask: bool,
    emissive: &Emissive,
    derived: &mut DerivedGlows,
) -> Option<(Vec<(&'static str, Value)>, bool, [f32; 3])> {
    let tex = material?.diffuse_texture.as_deref()?;
    let name = aurora_bsn::img::basename(tex);
    let stem = Path::new(name).file_stem()?.to_string_lossy().into_owned();
    let glow_name = format!("{stem}_glow.png");
    let card_name = format!("{stem}_card.png");
    let prefix = &args.asset_prefix;

    // Decide (and bake) once per source texture.
    let (card, mean) = if let Some(&d) = derived.get(&stem) {
        d
    } else {
        let img = image::open(obj_dir.join(tex.replace('\\', "/")))
            .ok()?
            .into_rgba8();
        // A card keeps its shape in alpha; a fullbright texture is opaque throughout.
        let card = img.pixels().any(|p| p.0[3] < 128);
        let lin = |v: u8| srgb_to_linear(v as f32 / 255.0);
        let enc = |v: f32| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
        let mut glow = image::RgbaImage::new(img.width(), img.height());
        let mut sum = [0.0f64; 3];
        for (x, y, p) in img.enumerate_pixels() {
            let a = p.0[3] as f32 / 255.0;
            let rgb = if card {
                [lin(p.0[0]) * a, lin(p.0[1]) * a, lin(p.0[2]) * a]
            } else {
                [lin(p.0[0]), lin(p.0[1]), lin(p.0[2])].map(|c| c.powf(args.unlit_contrast))
            };
            for c in 0..3 {
                sum[c] += rgb[c] as f64;
            }
            glow.put_pixel(
                x,
                y,
                image::Rgba([enc(rgb[0]), enc(rgb[1]), enc(rgb[2]), 255]),
            );
        }
        glow.save(textures_dir.join(&glow_name)).ok()?;
        let n = (img.width() * img.height()).max(1) as f64;
        let mean = sum.map(|s| (s / n) as f32);
        if card {
            let mut base = image::RgbaImage::new(img.width(), img.height());
            for (x, y, p) in img.enumerate_pixels() {
                base.put_pixel(x, y, image::Rgba([0, 0, 0, p.0[3]]));
            }
            base.save(textures_dir.join(&card_name)).ok()?;
        }
        derived.insert(stem.clone(), (card, mean));
        (card, mean)
    };

    let mut fields = if card {
        // The card's core only: a black cutout that emits; the halo is the point light's.
        vec![
            (
                "base_color_texture",
                Value::Str(format!("{prefix}/textures/{card_name}")),
            ),
            ("alpha_mode", bsn::alpha_mask(0.5)),
        ]
    } else {
        let mut fields = material_fields(prefix, material, is_cutmask, &[]);
        surface::apply(&mut fields, prefix, textures_dir);
        fields
    };
    let [r, g, b] = emissive.tint.map(|t| t * emissive.nits);
    fields.push(("emissive", bsn::linear_rgba([r, g, b, 1.0])));
    fields.push((
        "emissive_texture",
        Value::Str(format!("{prefix}/textures/{glow_name}")),
    ));
    Some((fields, card, mean))
}

/// Area (model units squared) of a submesh.
fn submesh_area(m: &tobj::Mesh) -> f32 {
    let p = |i: u32| {
        let i = i as usize * 3;
        Vec3::new(m.positions[i], m.positions[i + 1], m.positions[i + 2])
    };
    m.indices
        .chunks_exact(3)
        .map(|t| 0.5 * (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0])).length())
        .sum()
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// One model placement in OUR tile-local frame (tile center at origin, +Y up, heights absolute).
struct Placement {
    /// Model path relative to the wow root (e.g. `world/azeroth/…/elwynntreecanopy04.obj`).
    model: String,
    translation: Vec3,
    rotation: Quat,
    scale: f32,
}

/// A `wmo`-typed CSV row, kept in the CSV's own coords for the interior-doodad pass.
struct WmoPlacement {
    rel_path: String,
    wow_position: Vec3,
    wow_rotation: Vec3,
}

/// One baked submesh of a model: everything a `.bsn` entity instance needs.
struct BakedSubmesh {
    /// Whether the submesh is drawn: a glow card (WoW's camera-facing additive halo sprite)
    /// is not, a ray tracer gets the halo from the card's light (`glow_light`); a fixed quad
    /// of it reads as a flat glowing disc.
    drawn: bool,
    mesh_stem: String,
    material_fields: Vec<(&'static str, Value)>,
    /// Local centroid the geometry was centered on (translation of an un-instanced entity).
    centroid: Vec3,
    /// A glow card's halo as a point light at the centroid: (linear colour, lumens at unit
    /// scale, emitter radius).
    glow_light: Option<([f32; 3], f32, f32)>,
}

/// A source texture's derived emissive bake: whether it is a card, and the glow map's mean
/// linear colour (what the card emits on average).
type DerivedGlows = HashMap<String, (bool, [f32; 3])>;

fn main() {
    let args = Args::parse();
    let map_dir = args.wow_root.join("maps").join(&args.map);
    // Every run starts from nothing: the output dir is this importer's alone, and output kept
    // from an earlier run can disagree with a re-exported source.
    if args.out_dir.exists() {
        println!("cleaning {}", args.out_dir.display());
        fs::remove_dir_all(&args.out_dir).expect("clean the output dir");
    }
    fs::create_dir_all(args.out_dir.join("meshes")).expect("create meshes dir");
    let textures_dir = args.out_dir.join("textures");
    fs::create_dir_all(&textures_dir).expect("create textures dir");
    fs::create_dir_all(args.out_dir.join("map/tileset")).expect("create map dir");
    fs::create_dir_all(args.out_dir.join("models")).expect("create models dir");

    // Pass 1 — parse every tile's placement CSV (m2 + wmo rows), then fold in each unique WMO's
    // interior doodads. Keyed by tile coord.
    let mut tile_placements: BTreeMap<(i32, i32), Vec<Placement>> = BTreeMap::new();
    let mut all_wmos: Vec<WmoPlacement> = Vec::new();
    for ay in args.y0..=args.y1 {
        for ax in args.x0..=args.x1 {
            let (placements, wmos) = parse_tile_placements(&map_dir, &args.wow_root, ax, ay);
            println!(
                "tile {ax},{ay}: {} placements, {} wmos",
                placements.len(),
                wmos.len()
            );
            tile_placements.insert((ax, ay), placements);
            all_wmos.extend(wmos);
        }
    }

    // Interior doodads: dedupe WMOs listed by several tile CSVs, assign each doodad to the tile
    // its world position lands in (dropped when outside the requested range).
    let mut seen = HashSet::new();
    all_wmos.retain(|w| {
        seen.insert((
            w.rel_path.clone(),
            [
                w.wow_position.x.to_bits(),
                w.wow_position.y.to_bits(),
                w.wow_position.z.to_bits(),
            ],
        ))
    });
    let interior = collect_wmo_interior_doodads(&args.wow_root, &all_wmos);
    let mut interior_count = 0usize;
    for (coord, placements) in interior {
        if let Some(tile) = tile_placements.get_mut(&coord) {
            interior_count += placements.len();
            tile.extend(placements);
        }
    }
    println!(
        "{} interior doodads from {} unique WMOs",
        interior_count,
        all_wmos.len()
    );

    // Pass 2 — bake each unique model once (shared meshes/textures, instanced from the tiles).
    let unique_models: HashSet<String> = tile_placements
        .values()
        .flatten()
        .map(|p| p.model.clone())
        .collect();
    println!("baking {} unique models…", unique_models.len());
    let mut baked_models: HashMap<String, Vec<BakedSubmesh>> = HashMap::new();
    let mut cutmask_cache: HashMap<String, bool> = HashMap::new();
    let mut derived: DerivedGlows = HashMap::new();
    let mut colliders: HashMap<String, String> = HashMap::new();
    for rel in &unique_models {
        let submeshes = bake_model(
            &args,
            &args.wow_root.join(rel),
            rel,
            &textures_dir,
            &mut cutmask_cache,
            &if is_foliage(rel) {
                FOLIAGE_OMM
            } else {
                OmmOptions::from_env()
            },
            &mut derived,
        );
        baked_models.insert(rel.clone(), submeshes);
        if let Some(stem) = bake_collider(&args, &args.wow_root.join(rel), rel) {
            colliders.insert(rel.clone(), stem);
        }
    }
    println!(
        "{} of {} models collide",
        colliders.len(),
        unique_models.len()
    );

    // Each model is a `.bsn` of its own (its submeshes at their centroids, its collision
    // body), so a placement is one instance: the editor selects the tree, not one of its
    // parts, and a model is one asset however many tiles place it.
    let mut model_scenes: HashMap<String, String> = HashMap::new();
    for rel in &unique_models {
        let submeshes = &baked_models[rel];
        if submeshes.is_empty() {
            continue;
        }
        let stem = model_stem(rel);
        let name = model_name(rel);
        let mut parts: Vec<Entity> = submeshes
            .iter()
            .enumerate()
            .filter(|(_, sub)| sub.drawn)
            .map(|(i, sub)| {
                mesh_entity(
                    &args.asset_prefix,
                    &sub.mesh_stem,
                    sub.material_fields.clone(),
                    &format!("{name} {i}"),
                    sub.centroid.to_array(),
                    Quat::IDENTITY.to_array(),
                    [1.0; 3],
                    None,
                )
            })
            .collect();
        if let Some(collider) = colliders.get(rel) {
            parts.push(Entity {
                components: vec![
                    bsn::name(&format!("{name} collider")),
                    bsn::collider(&args.asset_prefix, collider),
                ],
                children: Vec::new(),
            });
        }
        let path = args.out_dir.join("models").join(format!("{stem}.bsn"));
        fs::write(&path, scene(&stem, Vec::new(), parts)).expect("write model .bsn");
        model_scenes.insert(
            rel.clone(),
            format!("{}/models/{stem}.bsn", args.asset_prefix),
        );
    }

    // Pass 3 — write each tile's `.bsn` (doodads/WMOs only) and its terrain map data.
    let mut palette = PaletteBuilder::default();
    let mut texture_effects: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut zone_tiles = Vec::new();
    for (&(ax, ay), placements) in &tile_placements {
        let mut entities = Vec::new();
        let mut instanced = 0usize;
        for p in placements {
            let (Some(submeshes), Some(model_scene)) =
                (baked_models.get(&p.model), model_scenes.get(&p.model))
            else {
                continue;
            };
            let name = model_name(&p.model);
            entities.push(Entity {
                components: vec![
                    bsn::name(&name),
                    bsn::transform(
                        p.translation.to_array(),
                        Some(p.rotation.to_array()),
                        Some([p.scale; 3]),
                    ),
                    bsn::scene_ref(model_scene),
                ],
                children: Vec::new(),
            });
            instanced += 1;
            // A glow card's light stays with the placement: its flux and radius follow the
            // placement's scale, which a light inside the model's scene could not.
            for sub in submeshes {
                let t = p.translation + p.rotation * (sub.centroid * p.scale);
                if let Some((color, lumens, radius)) = sub.glow_light {
                    entities.push(point_light(
                        &format!("{name} glow"),
                        t.to_array(),
                        color,
                        lumens * p.scale * p.scale,
                        radius * p.scale,
                    ));
                }
            }
        }

        let scene_name = format!("{}_{ax}_{ay}", args.map);
        let bsn = scene(&scene_name, Vec::new(), entities);
        let bsn_path = args.out_dir.join(format!("{scene_name}.bsn"));
        fs::write(&bsn_path, bsn).expect("write .bsn");
        println!(
            "tile {ax},{ay}: {} placements -> {instanced} entities -> {}",
            placements.len(),
            bsn_path.display()
        );

        if let Some(range) =
            emit_tile_map(&args, &map_dir, ax, ay, &mut palette, &mut texture_effects)
        {
            zone_tiles.push(((ax, ay), range));
        }
    }
    palette.write(&args);
    emit_zone(&args, &palette, &zone_tiles);
    emit_clutter(
        &args,
        &map_dir,
        &textures_dir,
        &palette,
        &texture_effects,
        &mut cutmask_cache,
    );
    println!("done.");
}

// ---- ground clutter -------------------------------------------------------------------------

/// One GroundEffectTexture row: density (doodads per 8×8 chunk cell, WoW's own unit) and the
/// (doodad id, weight) pairs.
struct GroundEffect {
    density: u32,
    doodads: Vec<(u32, f32)>,
}

fn load_ground_effects(wow_root: &Path) -> HashMap<u32, GroundEffect> {
    let mut map = HashMap::new();
    let path = wow_root.join("GroundEffectTexture.csv");
    let Ok(content) = fs::read_to_string(&path) else {
        eprintln!("clutter: cannot read {}", path.display());
        return map;
    };
    // ID;Density;Sound;DoodadID;DoodadWeight (the last two comma-separated x4).
    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < 5 {
            continue;
        }
        let Ok(id) = fields[0].trim().parse::<u32>() else {
            continue;
        };
        let density = fields[1].trim().parse().unwrap_or(0);
        let ids = fields[3]
            .split(',')
            .map(|s| s.trim().parse::<u32>().unwrap_or(0));
        let weights = fields[4]
            .split(',')
            .map(|s| s.trim().parse::<f32>().unwrap_or(1.0));
        let doodads = ids
            .zip(weights.chain(std::iter::repeat(1.0)))
            .filter(|(id, _)| *id != 0)
            .collect();
        map.insert(id, GroundEffect { density, doodads });
    }
    map
}

/// GroundEffectDoodad: doodad id -> (model file id, Animscale). Animscale is WoW's own
/// "sways in the wind" amount: 1 for grass and flowers, 0 for rocks and stumps.
fn load_ground_doodads(wow_root: &Path) -> HashMap<u32, (u32, f32)> {
    let path = wow_root.join("GroundEffectDoodad.csv");
    let Ok(content) = fs::read_to_string(&path) else {
        eprintln!("clutter: cannot read {}", path.display());
        return HashMap::new();
    };
    content
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut it = line.split(';');
            let id = it.next()?.trim().parse::<u32>().ok()?;
            let file = it.next()?.trim().parse::<u32>().ok()?;
            let _flags = it.next();
            let animscale = it
                .next()
                .and_then(|s| s.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            Some((id, (file, animscale)))
        })
        .collect()
}

/// The listfile rows for the given file ids only (2M lines; one pass).
fn load_listfile(path: &Path, wanted: &HashSet<u32>) -> HashMap<u32, String> {
    let Ok(content) = fs::read_to_string(path) else {
        eprintln!("clutter: cannot read listfile {}", path.display());
        return HashMap::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let (id, file) = line.split_once(';')?;
            let id = id.trim().parse::<u32>().ok()?;
            wanted.contains(&id).then(|| (id, file.trim().to_string()))
        })
        .collect()
}

/// Bake every plant the tiles' ground textures reference as `ScatterSpecies` of the zone's
/// landscape: `<map>_clutter.bsn`, one species per plant part, instanced under the zone by its
/// `Clutter` child. WoW's density (plants per 8x8 chunk cell, split by each plant's weight)
/// becomes the species' spacing (one spot per plant at its densest layer) and a per-palette
/// keep factor; a plant's parts share seed, spacing and factors, so they land on one spot.
/// Swaying parts (WoW Animscale > 0) carry [`WIND_POSES`] baked lean poses the scatter kernel
/// cycles.
fn emit_clutter(
    args: &Args,
    map_dir: &Path,
    textures_dir: &Path,
    palette: &PaletteBuilder,
    texture_effects: &BTreeMap<String, BTreeSet<u32>>,
    cutmask_cache: &mut HashMap<String, bool>,
) {
    let effects = load_ground_effects(&args.wow_root);
    let doodads = load_ground_doodads(&args.wow_root);
    let wanted: HashSet<u32> = doodads.values().map(|(file, _)| *file).collect();
    let listfile = load_listfile(&args.listfile, &wanted);

    // Plant name -> its baked parts, once baked (empty when it has no usable geometry).
    let mut baked: BTreeMap<String, Vec<SpeciesPart>> = BTreeMap::new();
    // Plant name -> (palette id, plants per square yard).
    let mut rates: BTreeMap<String, Vec<(u32, f32)>> = BTreeMap::new();
    let mut textures = 0;
    for (texture, effect_ids) in texture_effects {
        let id = palette.entries.iter().position(|(name, _)| name == texture);
        let Some(id) = id else {
            eprintln!("clutter: {texture} is not in the zone's palette");
            continue;
        };
        // A texture's chunks can carry several effect ids (some rows are missing from the
        // table, some have no doodads): the densest with plants wins.
        let mut best: Option<(u32, BTreeMap<String, f32>)> = None;
        for effect_id in effect_ids {
            let Some(effect) = effects.get(effect_id) else {
                continue;
            };
            if effect.density == 0 || effect.doodads.is_empty() {
                continue;
            }
            // The same doodad may fill several of the four slots: sum its weights.
            let mut plants: BTreeMap<String, f32> = BTreeMap::new();
            for &(doodad, weight) in &effect.doodads {
                let Some((file_id, animscale)) = doodads.get(&doodad) else {
                    continue;
                };
                let Some(path) = listfile.get(file_id) else {
                    eprintln!("clutter: doodad {doodad}: file id {file_id} not in the listfile");
                    continue;
                };
                let name = Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if name.is_empty() {
                    continue;
                }
                let parts = baked.entry(name.clone()).or_insert_with(|| {
                    bake_species_parts(
                        args,
                        map_dir,
                        textures_dir,
                        &name,
                        *animscale,
                        cutmask_cache,
                    )
                });
                if !parts.is_empty() {
                    *plants.entry(name).or_insert(0.0) += weight;
                }
            }
            if !plants.is_empty() && best.as_ref().is_none_or(|(d, _)| effect.density > *d) {
                best = Some((effect.density, plants));
            }
        }
        let Some((density, plants)) = best else {
            continue;
        };
        textures += 1;
        let bare = CLUTTER_BARE.iter().any(|b| texture.starts_with(b));
        let total: f32 = plants.values().sum();
        for (name, weight) in plants {
            let rate =
                CLUTTER_SCALE * density as f32 * weight / total / (CLUTTER_CELL * CLUTTER_CELL);
            if !bare {
                rates.entry(name).or_default().push((id as u32, rate));
            }
        }
    }

    let mut species = Vec::new();
    for (name, rates) in &rates {
        let densest = rates.iter().map(|r| r.1).fold(0.0, f32::max);
        if densest <= 0.0 {
            continue;
        }
        let spacing = densest.recip().sqrt().max(CLUTTER_MIN_SPACING);
        let grows_on = rates
            .iter()
            .map(|&(id, rate)| {
                Value::AnonTuple(vec![
                    Value::Int(id as i64),
                    Value::Float((rate * spacing * spacing).min(1.0)),
                ])
            })
            .collect::<Vec<_>>();
        for (i, part) in baked[name].iter().enumerate() {
            let poses = part
                .poses
                .iter()
                .map(|stem| Value::Str(format!("{}/meshes/{stem}.aurora_mesh", args.asset_prefix)))
                .collect();
            species.push(Entity {
                components: vec![
                    bsn::name(&format!("{name} #{i}")),
                    bsn::mesh(&args.asset_prefix, &part.rest),
                    bsn::material(part.material_fields.clone()),
                    Value::Struct(
                        "aurora_landscape::scatter::ScatterSpecies",
                        vec![
                            ("spacing", Value::Float(spacing)),
                            ("radius", Value::Float(160.0)),
                            ("inner_radius", Value::Float(0.0)),
                            ("fade", Value::Float(12.0)),
                            ("grows_on", Value::List(grows_on.clone())),
                            (
                                "scale",
                                Value::Struct(
                                    "glam::Vec2",
                                    vec![("x", Value::Float(0.85)), ("y", Value::Float(1.15))],
                                ),
                            ),
                            ("max_slope", Value::Float(40.0)),
                            ("sink", Value::Float(0.0)),
                            ("poses", Value::List(poses)),
                            ("wind_speed", Value::Float(0.35)),
                            ("wind_shear", Value::Float(part.wind_shear)),
                            ("seed", Value::Int(fnv(name) as i64)),
                            (
                                "density",
                                Value::Float(
                                    CLUTTER_DENSITY
                                        .iter()
                                        .find(|(plant, _)| plant == name)
                                        .map_or(1.0, |(_, d)| *d),
                                ),
                            ),
                        ],
                    ),
                ],
                children: Vec::new(),
            });
        }
    }
    let path = args.out_dir.join(format!("{}_clutter.bsn", args.map));
    let count = species.len();
    fs::write(
        &path,
        scene(&format!("{}_clutter", args.map), Vec::new(), species),
    )
    .expect("write clutter .bsn");
    println!(
        "clutter: {} plants on {textures} ground textures, {count} species -> {}",
        rates.len(),
        path.display()
    );
}

/// A WoW chunk cell (an eighth of a chunk), the area GroundEffect densities are per.
const CLUTTER_CELL: f32 = TILE / 16.0 / 8.0;
/// Plants per WoW GroundEffect plant: the client's table reads sparse at path-traced ground
/// level; spacing shrinks by its square root.
const CLUTTER_SCALE: f32 = 4.0;
/// Ground textures that grow none of WoW's table: Northshire's dirt paths and its road.
const CLUTTER_BARE: &[&str] = &["elwynn_elwynndirtbase2", "elwynn_elwynncobblestonebase"];
/// Per-plant `ScatterSpecies::density` (1 = WoW's table): Elwynn's road pebbles carpet the
/// cobblestone at the table's rate (a pebble every ~1.7 yd).
const CLUTTER_DENSITY: &[(&str, f32)] = &[("elwroc02", 0.15)];
/// Closest spacing a species gets; denser layers saturate.
const CLUTTER_MIN_SPACING: f32 = 0.3;
/// Opacity micromaps for foliage doodads (tree canopies, bushes): their leaf cards are large and
/// seen against the sky, where the default's 2-state micro-triangles (several texels wide, opaque
/// wherever they touch a leaf) fringe every leaf with solid triangles. 4-state keeps the clearly
/// opaque / clearly clear micro-triangles on the RT cores and sends only the leaf-edge band to the
/// any-hit's exact alpha test; level 8 at a texel per micro-triangle edge keeps that band thin, and
/// exact edges need no erosion.
const FOLIAGE_OMM: OmmOptions = OmmOptions {
    max_subdiv: 8,
    erode_px: 0,
    scale: 1.0,
    four_state: true,
};

/// Whether a model is foliage by its path (a doodad, not a WMO or a building among the trees).
fn is_foliage(rel: &str) -> bool {
    const KEYS: &[&str] = &[
        "tree", "bush", "plant", "grass", "flower", "vine", "leaf", "leaves", "fern", "canopy",
        "lilypad", "shrub", "hedge", "reed",
    ];
    let rel = rel.to_ascii_lowercase();
    !rel.contains("/wmo/") && !rel.contains("treehut") && KEYS.iter().any(|k| rel.contains(k))
}

/// Thin translucency of cutout foliage (`AuroraMaterial::diffuse_transmission`): the share of
/// diffuse light passed to the far side, taken from the side it lands on (0.5 = a leaf's even
/// split; 1.0 leaves the sunlit side black).
const FOLIAGE_TRANSMISSION: f32 = 0.5;
/// Baked wind poses per swaying part, at evenly spaced lean amounts; the scatter kernel picks
/// the one under a plant's continuous lean and shears the rest of the way.
const WIND_POSES: usize = 8;
/// The wind the poses lean for (zero's default `Wind::speed`), m/s.
const WIND_SPEED: f32 = 4.0;

/// One plant part as a species: its rest mesh, its wind poses, its material.
struct SpeciesPart {
    rest: String,
    poses: Vec<String>,
    /// `ScatterSpecies::wind_shear`: the shear (x per y) that stands in for one pose step.
    wind_shear: f32,
    material_fields: Vec<(&'static str, Value)>,
}

/// A plant's parts in plant space (the foliage OBJ's own origin, at the plant's base), with
/// the opacity micromap baked into every mesh. Swaying parts get [`WIND_POSES`] lean poses:
/// each vertex pushed along +X by its sway weight (height over the whole plant's, squared)
/// times the lean, 0.2..1.0 of WoW's Animscale x 0.25 m at 10 m/s in even steps.
fn bake_species_parts(
    args: &Args,
    map_dir: &Path,
    textures_dir: &Path,
    name: &str,
    animscale: f32,
    cutmask_cache: &mut HashMap<String, bool>,
) -> Vec<SpeciesPart> {
    let obj_path = map_dir.join("foliage").join(format!("{name}.obj"));
    let Some((models, materials)) = load_obj(&obj_path) else {
        eprintln!(
            "clutter: {name}: no foliage export at {}",
            obj_path.display()
        );
        return Vec::new();
    };
    copy_textures(&obj_path, &materials, &models, textures_dir);
    let obj_dir = obj_path.parent().unwrap_or_else(|| Path::new("."));
    // Every plant triangle is a whole cutout card, so the micromap is the silhouette: finer
    // than a tree's leaves (a quarter texel per micro-triangle, a texel of erosion).
    let omm = OmmOptions {
        max_subdiv: 9,
        erode_px: 1,
        scale: 0.25,
        ..OmmOptions::from_env()
    };
    let (min_y, max_y) = models
        .iter()
        .flat_map(|m| m.mesh.positions.chunks_exact(3).map(|p| p[1]))
        .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
    let span = (max_y - min_y).max(1.0e-3);
    let lean = 0.25 * animscale * WIND_SPEED / 10.0;
    let stem = sanitize(&format!("foliage/{name}"));
    let mut parts = Vec::new();
    for (i, m) in models.iter().enumerate() {
        if m.mesh.indices.is_empty() {
            continue;
        }
        let material = m.mesh.material_id.and_then(|id| materials.get(id));
        let is_cutmask =
            material.is_some_and(|mat| material_is_cutmask(obj_dir, mat, cutmask_cache));
        let bake = |file_stem: &str, push: f32| -> bool {
            let file = args
                .out_dir
                .join("meshes")
                .join(format!("{file_stem}.aurora_mesh"));
            let mut source = m.mesh.clone();
            for p in source.positions.chunks_exact_mut(3) {
                let t = ((p[1] - min_y) / span).clamp(0.0, 1.0);
                p[0] += push * t * t;
            }
            let Ok(mut cm) = AuroraMesh::clustered(&build_mesh(&source, [0.0; 3])) else {
                eprintln!("  {file_stem}: bake failed");
                return false;
            };
            if is_cutmask && let Some(mat) = material {
                let _ = attach_omm(&mut cm, obj_dir, mat, &omm);
            }
            let w = BufWriter::new(File::create(&file).expect("create .aurora_mesh"));
            write_aurora_mesh(&cm, w).expect("write .aurora_mesh");
            true
        };
        let rest = format!("{stem}_{i}_scatter");
        if !bake(&rest, 0.0) {
            continue;
        }
        let step = lean * 0.8 / (WIND_POSES - 1) as f32;
        let poses = if animscale > 0.0 {
            (0..WIND_POSES)
                .map(|k| {
                    let pose = format!("{stem}_{i}_pose{k:02}");
                    bake(&pose, lean * 0.2 + step * k as f32);
                    pose
                })
                .collect()
        } else {
            Vec::new()
        };
        // One step's push is `step * t^2` (t = height over the span); the best straight shear
        // through the base for t^2 on 0..1 is 0.75 t.
        let wind_shear = if animscale > 0.0 {
            step * 0.75 / span
        } else {
            0.0
        };
        let mut fields = material_fields(&args.asset_prefix, material, is_cutmask, &[]);
        surface::apply(&mut fields, &args.asset_prefix, textures_dir);
        // Leaf cards pass light through: lit from behind when you look sunward. What WoW sways is
        // foliage; rocks (no Animscale) stay opaque.
        if is_cutmask && animscale > 0.0 {
            fields.push(("diffuse_transmission", Value::Float(FOLIAGE_TRANSMISSION)));
        }
        parts.push(SpeciesPart {
            rest,
            poses,
            wind_shear,
            material_fields: fields,
        });
    }
    parts
}

/// FNV-1a: a plant's scatter seed, stable across imports.
fn fnv(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ b as u32).wrapping_mul(0x0100_0193)
    })
}

/// A point light entity: a glow card's halo as illumination.
fn point_light(name: &str, t: [f32; 3], [r, g, b]: [f32; 3], lumens: f32, radius: f32) -> Entity {
    Entity {
        components: vec![
            bsn::name(name),
            bsn::transform(t, Some([0.0, 0.0, 0.0, 1.0]), Some([1.0; 3])),
            Value::Struct(
                "bevy_light::point_light::PointLight",
                vec![
                    ("color", bsn::color_linear([r, g, b, 1.0])),
                    ("intensity", Value::Float(lumens)),
                    ("radius", Value::Float(radius)),
                ],
            ),
        ],
        children: Vec::new(),
    }
}

// ---- terrain map data ---------------------------------------------------------------------

/// The shared splat palette across every emitted tile: tileset textures copied into
/// `map/tileset/`, indexed by the per-chunk layer tables.
#[derive(Default)]
struct PaletteBuilder {
    /// wow-relative source path -> palette index.
    indices: HashMap<String, u32>,
    /// (map-relative file name, source path).
    entries: Vec<(String, PathBuf)>,
}

impl PaletteBuilder {
    /// The map-relative file name of a palette entry (the key ground effects are gathered by).
    fn name(&self, index: u32) -> &str {
        &self.entries[index as usize].0
    }

    /// Palette index for a layer's texture (`../../tileset/elwynn/x.png`, relative to the map
    /// dir), registering it on first sight.
    fn index(&mut self, map_dir: &Path, file: &str) -> u32 {
        let key = file.replace('\\', "/");
        if let Some(&i) = self.indices.get(&key) {
            return i;
        }
        let src = map_dir.join(&key);
        // Unique flat name: `<zone>_<stem>.png` (basenames collide across zones).
        let stem = src
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let zone = src
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = format!("{zone}_{stem}.png");
        let i = self.entries.len() as u32;
        self.indices.insert(key, i);
        self.entries.push((name, src));
        i
    }

    fn write(&self, args: &Args) {
        let dir = args.out_dir.join("map");
        for (name, src) in &self.entries {
            let dst = dir.join("tileset").join(name);
            if let Err(err) = fs::copy(src, &dst) {
                eprintln!("palette: copy {} failed: {err}", src.display());
            }
        }
        println!(
            "palette: {} textures -> {}",
            self.entries.len(),
            dir.join("tileset").display()
        );
    }
}

/// One ground-texture layer of a terrain subchunk (wow.export `tex_X_Y_<chunk>.json`).
#[derive(serde::Deserialize)]
struct LayerDef {
    /// Alpha channel in the chunk's 64×64 png (−1 = base layer, full weight).
    #[serde(rename = "channelIndex")]
    channel_index: i32,
    /// Tileset texture, relative to the map dir (`../../tileset/…`).
    file: String,
    /// GroundEffectTexture row: the ground clutter this layer grows (0 = none).
    #[serde(rename = "effectID", default)]
    effect_id: u32,
}
#[derive(serde::Deserialize)]
struct ChunkLayers {
    layers: Vec<LayerDef>,
}

/// Write one tile's terrain data: the 16-bit height grid (resampled from the ADT OBJ's
/// vertices in OUR tile frame) and its splatmap (each chunk's four layers against the shared
/// palette, weighted by its alpha png). Returns the height range the grid spans.
fn emit_tile_map(
    args: &Args,
    map_dir: &Path,
    ax: i32,
    ay: i32,
    palette: &mut PaletteBuilder,
    texture_effects: &mut BTreeMap<String, BTreeSet<u32>>,
) -> Option<(f32, f32)> {
    let dir = args.out_dir.join("map");
    let stem = format!("{}_{ax}_{ay}", args.map);
    let height_path = dir.join(format!("{stem}_height.png"));

    // Layer tables always resolve against the palette (so the palette stays complete even
    // when this tile's files already exist).
    let mut chunks: Vec<[u32; 4]> = Vec::with_capacity(256);
    for ci in 0..256u32 {
        let mut entry = [NO_LAYER; 4];
        if let Some(defs) = fs::read_to_string(map_dir.join(format!("tex_{ax}_{ay}_{ci}.json")))
            .ok()
            .and_then(|t| serde_json::from_str::<ChunkLayers>(&t).ok())
        {
            for l in defs.layers.iter() {
                // Slot by channel: base (channelIndex -1) -> 0, channels 0..2 -> 1..3.
                let slot = (l.channel_index + 1).clamp(0, 3) as usize;
                let index = palette.index(map_dir, &l.file);
                entry[slot] = index;
                if l.effect_id != 0 {
                    texture_effects
                        .entry(palette.name(index).to_string())
                        .or_default()
                        .insert(l.effect_id);
                }
            }
        }
        chunks.push(entry);
    }

    // Heights: nearest ADT vertex per grid point, through a spatial hash of the OBJ verts.
    let obj_path = map_dir.join(format!("adt_{ax}_{ay}.obj"));
    let Some((models, _)) = load_obj(&obj_path) else {
        eprintln!("tile {ax},{ay}: no terrain obj; map data skipped");
        return None;
    };
    let mut verts: Vec<[f32; 3]> = Vec::new();
    for m in &models {
        verts.extend(m.mesh.positions.chunks_exact(3).map(|c| [c[0], c[1], c[2]]));
    }
    let center_x = C - (ax as f32 + 0.5) * TILE;
    let center_z = C - (ay as f32 + 0.5) * TILE;
    let cell = TILE / (HEIGHT_RES - 1) as f32;
    let mut buckets: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, v) in verts.iter().enumerate() {
        let bx = ((v[0] - center_x) / cell).floor() as i32;
        let bz = ((v[2] - center_z) / cell).floor() as i32;
        buckets.entry((bx, bz)).or_default().push(i as u32);
    }
    let res = HEIGHT_RES as usize;
    let mut heights = vec![0.0f32; res * res];
    let (mut hmin, mut hmax) = (f32::MAX, f32::MIN);
    for gz in 0..res {
        for gx in 0..res {
            let lx = (gx as f32 / (res - 1) as f32 - 0.5) * TILE;
            let lz = (gz as f32 / (res - 1) as f32 - 0.5) * TILE;
            let x = center_x + lx;
            let z = center_z + lz;
            let bx = (lx / cell).floor() as i32;
            let bz = (lz / cell).floor() as i32;
            let mut best = f32::MAX;
            let mut h = 0.0f32;
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let Some(ids) = buckets.get(&(bx + dx, bz + dz)) else {
                        continue;
                    };
                    for &i in ids {
                        let v = verts[i as usize];
                        let d = (v[0] - x).powi(2) + (v[2] - z).powi(2);
                        if d < best {
                            best = d;
                            h = v[1];
                        }
                    }
                }
            }
            heights[gz * res + gx] = h;
            hmin = hmin.min(h);
            hmax = hmax.max(h);
        }
    }
    // Holes: WoW cuts them by leaving the chunk's triangles out (wow.export writes a holed
    // chunk with fewer faces), so a splat texel whose centre no triangle covers is a hole.
    // Rasterized in the OBJ frame, the frame of the heights above and of the splat after its
    // rotation below.
    let texel = TILE / ALPHA_ATLAS as f32;
    let mut covered = vec![false; (ALPHA_ATLAS * ALPHA_ATLAS) as usize];
    for m in &models {
        let p = &m.mesh.positions;
        let xz = |i: u32| {
            let i = i as usize * 3;
            [
                (p[i] - center_x) / texel + ALPHA_ATLAS as f32 * 0.5,
                (p[i + 2] - center_z) / texel + ALPHA_ATLAS as f32 * 0.5,
            ]
        };
        for tri in m.mesh.indices.chunks_exact(3) {
            let [a, b, c] = [xz(tri[0]), xz(tri[1]), xz(tri[2])];
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            if area.abs() < 1e-9 {
                continue;
            }
            let lo = |i: usize| a[i].min(b[i]).min(c[i]).floor().max(0.0) as u32;
            let hi = |i: usize| (a[i].max(b[i]).max(c[i]).ceil() as u32).min(ALPHA_ATLAS);
            for z in lo(1)..hi(1) {
                for x in lo(0)..hi(0) {
                    let q = [x as f32 + 0.5, z as f32 + 0.5];
                    let edge = |u: [f32; 2], v: [f32; 2]| {
                        ((v[0] - u[0]) * (q[1] - u[1]) - (v[1] - u[1]) * (q[0] - u[0])) / area
                    };
                    // Inclusive with a hair of slack: a centre on a shared edge is covered.
                    if edge(a, b) >= -1e-4 && edge(b, c) >= -1e-4 && edge(c, a) >= -1e-4 {
                        covered[(z * ALPHA_ATLAS + x) as usize] = true;
                    }
                }
            }
        }
    }

    let span = (hmax - hmin).max(1e-3);
    let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_fn(
        HEIGHT_RES,
        HEIGHT_RES,
        |x, z| {
            let h = heights[(z * HEIGHT_RES + x) as usize];
            image::Luma([(((h - hmin) / span) * 65535.0).round() as u16])
        },
    );
    img.save(&height_path).expect("write height png");

    // Alphamap atlas: each chunk's 64² png blitted into its 16×16 cell.
    let mut atlas = image::RgbaImage::new(ALPHA_ATLAS, ALPHA_ATLAS);
    for ci in 0..256u32 {
        let Ok(img) = image::open(map_dir.join(format!("tex_{ax}_{ay}_{ci}.png"))) else {
            continue;
        };
        let img = img.into_rgba8();
        let x0 = (ci % 16) * ALPHA_CELL;
        let y0 = (ci / 16) * ALPHA_CELL;
        for y in 0..ALPHA_CELL.min(img.height()) {
            for x in 0..ALPHA_CELL.min(img.width()) {
                atlas.put_pixel(x0 + x, y0 + y, *img.get_pixel(x, y));
            }
        }
    }

    // Rotate the splat data 180° into OUR tile frame. Our whole world is the wow frame
    // rotated 180° about Y (the placement transform: t → (−x, y, −z)), so BOTH atlas axes
    // mirror: measured against the abbey WMO, its cobblestone chunk sits at raw (col 5,
    // row 10) = atlas (0.34, 0.66) but the building's tile-local position is (0.67, 0.32) —
    // exactly (1−u, 1−v). rotate180 flips the cells, their reorder, AND each 64² cell's own
    // texels together; the chunk table mirrors both col and row to match. A one-axis flip
    // left z mirrored → swapped textures and broken N–S tile seams.
    let atlas = image::imageops::rotate180(&atlas);
    let chunks: Vec<[u32; 4]> = (0..256usize)
        .map(|i| chunks[(15 - i / 16) * 16 + (15 - i % 16)])
        .collect();

    // Splatmap: per texel its chunk's layers, weighted as WoW blends them (each alpha
    // channel lays its layer over the ones below).
    let mut ids = image::RgbaImage::new(ALPHA_ATLAS, ALPHA_ATLAS);
    let mut weights = image::RgbaImage::new(ALPHA_ATLAS, ALPHA_ATLAS);
    for (x, z, alpha) in atlas.enumerate_pixels() {
        let layers = chunks[((z / ALPHA_CELL) * 16 + x / ALPHA_CELL) as usize];
        let [r, g, b, _] = alpha.0.map(|c| c as f32 / 255.0);
        let w = [
            (1.0 - r) * (1.0 - g) * (1.0 - b),
            r * (1.0 - g) * (1.0 - b),
            g * (1.0 - b),
            b,
        ];
        let mut id = [0u8; 4];
        let mut weight = [0u8; 4];
        for slot in 0..4 {
            if layers[slot] != NO_LAYER {
                id[slot] = layers[slot] as u8;
                weight[slot] = (w[slot] * 255.0).round() as u8;
            }
        }
        if !covered[(z * ALPHA_ATLAS + x) as usize] {
            id = [HOLE, 0, 0, 0];
            weight = [255, 0, 0, 0];
        }
        ids.put_pixel(x, z, image::Rgba(id));
        weights.put_pixel(x, z, image::Rgba(weight));
    }
    let holes = covered.iter().filter(|c| !**c).count();
    if holes > 0 {
        println!("tile {ax},{ay}: {holes} hole texels");
    }
    ids.save(dir.join(format!("{stem}_splat_ids.png")))
        .expect("write splat ids png");
    weights
        .save(dir.join(format!("{stem}_splat_weights.png")))
        .expect("write splat weights png");
    println!("tile {ax},{ay}: map data -> {}", height_path.display());
    Some((hmin, hmin + span))
}

/// A palette entry's material name: the tileset texture's stem (`elwynn_elwynngrassbase`).
fn material_name(texture: &str) -> String {
    let stem = texture.trim_end_matches(".png");
    stem.strip_suffix("_s").unwrap_or(stem).to_string()
}

/// A tileset texture's footstep samples, guessed once from the words in its name (zero's
/// `audio/footsteps/footstep_<set>_<nnn>.ogg`); edit them per material in the scene after.
fn footstep_samples(texture: &str) -> Vec<String> {
    let name = texture.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| name.contains(w));
    let set = if has(&["cobble", "road", "rock", "stone", "brick", "black", "lava"]) {
        "concrete"
    } else if has(&["snow", "ice"]) {
        "snow"
    } else {
        "grass"
    };
    (0..5)
        .map(|i| format!("audio/footsteps/footstep_{set}_{i:03}.ogg"))
        .collect()
}

/// The zone's landscape (`<map>_zone.bsn`): the palette, then per tile one entity that is
/// both its height layer and its splat layer, then an empty sculpt and paint layer on top.
/// Texels are a 1024th of a tile, so the splatmaps land texel for texel.
fn emit_zone(args: &Args, palette: &PaletteBuilder, tiles: &[((i32, i32), (f32, f32))]) {
    let prefix = &args.asset_prefix;
    let centre = ((args.x0 + args.x1) / 2, (args.y0 + args.y1) / 2);
    let texel = TILE / ALPHA_ATLAS as f32;
    let page = 256.0 * texel;
    // Our frame mirrors wow's: one ADT east (+1 x) is -TILE in our x.
    let min = [
        ((centre.0 - args.x1) as f32 - 0.5) * TILE,
        ((centre.1 - args.y1) as f32 - 0.5) * TILE,
    ];
    let max = [
        ((centre.0 - args.x0) as f32 + 0.5) * TILE,
        ((centre.1 - args.y0) as f32 + 0.5) * TILE,
    ];
    let ivec2 = |x: f32, y: f32| {
        Value::Struct(
            "glam::IVec2",
            vec![("x", Value::Int(x as i64)), ("y", Value::Int(y as i64))],
        )
    };
    let vec2 = |x: f32, y: f32| {
        Value::Struct(
            "glam::Vec2",
            vec![("x", Value::Float(x)), ("y", Value::Float(y))],
        )
    };
    let tileset = args.out_dir.join("map/tileset");
    let materials = palette
        .entries
        .iter()
        .map(|(name, _)| {
            let kind = surface::classify_ground(name);
            let normal = surface::derive(&tileset, name, kind, false)
                .and_then(|d| d.normal)
                .map_or(Value::Path("None"), |n| {
                    Value::Tuple("Some", vec![Value::Str(format!("{prefix}/map/tileset/{n}"))])
                });
            Value::Struct(
                "aurora_landscape::shading::LandscapeMaterial",
                vec![
                    ("name", Value::Str(material_name(name))),
                    (
                        "albedo",
                        Value::Tuple(
                            "Some",
                            vec![Value::Str(format!("{prefix}/map/tileset/{name}"))],
                        ),
                    ),
                    ("normal", normal),
                    ("tile_size", Value::Float(TILE / 16.0 / REPEATS)),
                    ("roughness", Value::Float(kind.roughness)),
                    ("clearcoat", Value::Float(0.0)),
                    (
                        "footsteps",
                        Value::List(footstep_samples(name).into_iter().map(Value::Str).collect()),
                    ),
                ],
            )
        })
        .collect();
    let root = vec![
        bsn::name(&format!("{} zone", args.map)),
        Value::Struct(
            "aurora_landscape::Landscape",
            vec![
                (
                    "pages_min",
                    ivec2((min[0] / page).floor(), (min[1] / page).floor()),
                ),
                (
                    "pages_max",
                    ivec2((max[0] / page).ceil(), (max[1] / page).ceil()),
                ),
                ("texel_size", Value::Float(texel)),
            ],
        ),
        Value::Tuple(
            "aurora_landscape::shading::LandscapePalette",
            vec![Value::List(materials)],
        ),
    ];
    let mut children: Vec<Entity> = tiles
        .iter()
        .map(|&((ax, ay), (hmin, hmax))| {
            let stem = format!("{}_{ax}_{ay}", args.map);
            Entity {
                components: vec![
                    bsn::name(&format!("tile {ax},{ay}")),
                    bsn::transform(
                        [
                            (centre.0 - ax) as f32 * TILE,
                            0.0,
                            (centre.1 - ay) as f32 * TILE,
                        ],
                        None,
                        None,
                    ),
                    Value::Struct(
                        "aurora_landscape::layer::HeightmapLayer",
                        vec![
                            (
                                "image",
                                Value::Str(format!("{prefix}/map/{stem}_height.png")),
                            ),
                            ("size", vec2(TILE, TILE)),
                            ("height_min", Value::Float(hmin)),
                            ("height_max", Value::Float(hmax)),
                        ],
                    ),
                    Value::Struct(
                        "aurora_landscape::layer::SplatmapLayer",
                        vec![
                            (
                                "ids",
                                Value::Str(format!("{prefix}/map/{stem}_splat_ids.png")),
                            ),
                            (
                                "weights",
                                Value::Str(format!("{prefix}/map/{stem}_splat_weights.png")),
                            ),
                            ("size", vec2(TILE, TILE)),
                        ],
                    ),
                ],
                children: Vec::new(),
            }
        })
        .collect();
    children.push(Entity {
        components: vec![
            bsn::name("Sculpt"),
            Value::Path("aurora_landscape::layer::SculptLayer"),
        ],
        children: Vec::new(),
    });
    children.push(Entity {
        components: vec![
            bsn::name("Paint"),
            Value::Path("aurora_landscape::layer::PaintLayer"),
        ],
        children: Vec::new(),
    });
    // The ground clutter's species, regenerated every import (this zone is not).
    children.push(Entity {
        components: vec![
            bsn::name("Clutter"),
            bsn::scene_ref(&format!("{prefix}/{}_clutter.bsn", args.map)),
        ],
        children: Vec::new(),
    });
    let path = args.out_dir.join(format!("{}_zone.bsn", args.map));
    fs::write(&path, scene(&format!("{}_zone", args.map), root, children))
        .expect("write zone .bsn");
    println!("zone: {} tiles -> {}", tiles.len(), path.display());
}

// ---- model baking ---------------------------------------------------------------------------

/// A model's unique, filesystem-safe stem from its wow-root-relative path (basenames collide:
/// several zones ship a `bush01.obj`); names its meshes, collider and `.bsn`.
fn model_stem(rel: &str) -> String {
    sanitize(rel.trim_end_matches(".obj").trim_start_matches("world/"))
}

/// What a placement of the model is called: its file's own name.
fn model_name(rel: &str) -> String {
    Path::new(rel)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Bake every submesh of one model OBJ into the shared `meshes/` dir (once per run), copy its
/// textures, and return the per-submesh instance records.
fn bake_model(
    args: &Args,
    obj_path: &Path,
    rel: &str,
    textures_dir: &Path,
    cutmask_cache: &mut HashMap<String, bool>,
    omm: &OmmOptions,
    derived: &mut DerivedGlows,
) -> Vec<BakedSubmesh> {
    let Some((models, materials)) = load_obj(obj_path) else {
        return Vec::new();
    };
    copy_textures(obj_path, &materials, &models, textures_dir);
    let obj_dir = obj_path.parent().unwrap_or_else(|| Path::new("."));
    let emissives = model_emissives(args, obj_path, &models);

    let path_stem = model_stem(rel);

    let mut out: Vec<BakedSubmesh> = Vec::new();
    // A lamp whose only emitter is fullbright glass gets its light there: (submesh, weight,
    // colour, radius) of the brightest glass.
    let mut fullbright: Option<(usize, f32, [f32; 3], f32)> = None;
    for (i, m) in models.iter().enumerate() {
        let Some(centroid) = submesh_centroid(&m.mesh) else {
            continue;
        };
        let material = m.mesh.material_id.and_then(|id| materials.get(id));
        let emissive = emissives.get(i).copied().flatten();
        let is_cutmask =
            material.is_some_and(|mat| material_is_cutmask(obj_dir, mat, cutmask_cache));

        let mut glow_light = None;
        let mut is_card = false;
        let fields = match &emissive {
            Some(e) => match emissive_fields(
                args,
                obj_dir,
                textures_dir,
                material,
                is_cutmask,
                e,
                derived,
            ) {
                Some((fields, card, mean)) => {
                    let area = submesh_area(&m.mesh);
                    let light = (lamp_color(mean), (area / PI).sqrt() * 0.5);
                    if card {
                        // A card's halo is its lamp's light (none for a fake light shaft).
                        is_card = true;
                        glow_light =
                            lamp_lumens(rel).map(|lm| (light.0, lm * args.light_scale, light.1));
                    } else if !e.additive && e.nits == args.unlit_nits {
                        let luma = 0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2];
                        if fullbright.is_none_or(|(_, best, _, _)| luma * area > best) {
                            fullbright = Some((out.len(), luma * area, light.0, light.1));
                        }
                    }
                    fields
                }
                None => {
                    let mut fields = material_fields(&args.asset_prefix, material, is_cutmask, &[]);
                    surface::apply(&mut fields, &args.asset_prefix, textures_dir);
                    fields
                }
            },
            None => {
                let mut fields = material_fields(&args.asset_prefix, material, is_cutmask, &[]);
                surface::apply(&mut fields, &args.asset_prefix, textures_dir);
                // A doodad's leaf cards (tree canopies, bushes) pass light through like the
                // clutter's; other cutouts (WMO windows, tombstones, tents, lanterns) do not.
                if is_cutmask && is_foliage(rel) {
                    fields.push(("diffuse_transmission", Value::Float(FOLIAGE_TRANSMISSION)));
                }
                fields
            }
        };
        let mesh_stem = format!("{path_stem}_{i}");
        let mesh_file = args
            .out_dir
            .join("meshes")
            .join(format!("{mesh_stem}.aurora_mesh"));
        // The output dir starts empty, so an existing file was baked earlier this run. A glow
        // card is not drawn (its halo is its light), so it has no mesh.
        if !is_card && !mesh_file.exists() {
            let mesh = build_mesh(&m.mesh, centroid);
            let mut cm = match AuroraMesh::clustered(&mesh) {
                Ok(cm) => cm,
                Err(err) => {
                    eprintln!("  {mesh_stem}: bake failed: {err:?}");
                    continue;
                }
            };
            // Alpha-cutout foliage gets a baked opacity micromap (resolved by the RT cores).
            if is_cutmask && let Some(mat) = material {
                let _ = attach_omm(&mut cm, obj_dir, mat, omm);
            }
            let w = BufWriter::new(File::create(&mesh_file).expect("create .aurora_mesh"));
            write_aurora_mesh(&cm, w).expect("write .aurora_mesh");
        }

        out.push(BakedSubmesh {
            drawn: !is_card,
            material_fields: fields,
            mesh_stem,
            centroid: Vec3::new(centroid[0] as f32, centroid[1] as f32, centroid[2] as f32),
            glow_light,
        });
    }
    if let Some((i, _, color, radius)) = fullbright
        && !out.iter().any(|s| s.glow_light.is_some())
        && let Some(lumens) = lamp_lumens(rel)
        && is_lamp(rel)
    {
        out[i].glow_light = Some((color, lumens * args.light_scale, radius));
    }
    out
}

/// A glow's colour at unit luminance.
fn lamp_color(mean: [f32; 3]) -> [f32; 3] {
    let luma = (0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2]).max(1e-6);
    mean.map(|c| c / luma)
}

/// Whether a model is a light fixture (its fullbright glass gets a point light).
fn is_lamp(rel: &str) -> bool {
    const KEYS: &[&str] = &[
        "lantern",
        "lamp",
        "torch",
        "candle",
        "brazier",
        "sconce",
        "chandelier",
    ];
    let rel = rel.to_ascii_lowercase();
    KEYS.iter().any(|k| rel.contains(k))
}

/// The light (lumens, at unit scale) a model's glow gives off, by what it is; None for glows
/// that light nothing (WoW's fake volumetric shafts).
fn lamp_lumens(rel: &str) -> Option<f32> {
    let rel = rel.to_ascii_lowercase();
    if rel.contains("volumetric") || rel.contains("lightshaft") {
        return None;
    }
    Some(if rel.contains("firefl") {
        2.0
    } else if rel.contains("candle") {
        40.0
    } else if rel.contains("torch") || rel.contains("brazier") || rel.contains("fire") {
        600.0
    } else {
        800.0
    })
}

// ---- collision -----------------------------------------------------------------------------

/// A model's collision triangles in model space (the frame its placements transform).
#[derive(Default)]
struct CollisionMesh {
    positions: Vec<[f32; 3]>,
    indices: Vec<[u32; 3]>,
}

impl CollisionMesh {
    fn extend(&mut self, mesh: &tobj::Mesh, keep: impl Fn(usize) -> bool) {
        let base = self.positions.len() as u32;
        self.positions
            .extend(mesh.positions.chunks_exact(3).map(|p| [p[0], p[1], p[2]]));
        for (triangle, i) in mesh.indices.chunks_exact(3).enumerate() {
            if keep(triangle) {
                self.indices.push([base + i[0], base + i[1], base + i[2]]);
            }
        }
    }

    /// `.collider`: `ACOL`, version, vertex count, triangle count (u32 LE), then the
    /// positions (f32 LE x 3) and the triangles (u32 LE x 3). Read by zero's `physics`.
    fn write(&self, path: &Path) {
        let mut bytes =
            Vec::with_capacity(16 + self.positions.len() * 12 + self.indices.len() * 12);
        bytes.extend_from_slice(b"ACOL");
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&(self.positions.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.indices.len() as u32).to_le_bytes());
        for v in self.positions.iter().flatten() {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        for i in self.indices.iter().flatten() {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        fs::write(path, bytes).expect("write .collider");
    }
}

/// Bakes a model's collision to `meshes/<stem>.collider` and returns the stem, or `None`
/// when WoW gives the model none (most bushes, all glow cards). An M2 collides with the
/// collision mesh wow.export writes beside it (`<model>.phys.obj`: a tree is its trunk, not
/// its leaf cards); a WMO with its `.phys.obj` (BSP faces), else all its render triangles.
fn bake_collider(args: &Args, obj_path: &Path, rel: &str) -> Option<String> {
    let stem = model_stem(rel);
    let file = args.out_dir.join("meshes").join(format!("{stem}.collider"));
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(obj_path.with_extension("json")).ok()?).ok()?;
    let mut collision = CollisionMesh::default();
    match json["fileType"].as_str()? {
        "m2" => {
            let phys = obj_path.with_extension("phys.obj");
            if !phys.exists() {
                return None;
            }
            for m in load_obj(&phys)?.0 {
                collision.extend(&m.mesh, |_| true);
            }
        }
        // wow.export (local fork) writes a WMO's BSP faces -- what the client collides with,
        // collision-only ramps and caps included -- as `<wmo>.phys.obj` too.
        "wmo" if obj_path.with_extension("phys.obj").exists() => {
            for m in load_obj(&obj_path.with_extension("phys.obj"))?.0 {
                collision.extend(&m.mesh, |_| true);
            }
        }
        // An older dump: every render triangle, as core did. Detail geometry (steps, floor
        // trim) has to stay -- the collision-only ramps and caps that replace it in the client
        // are not in the OBJ.
        "wmo" => {
            for m in load_obj(obj_path)?.0 {
                collision.extend(&m.mesh, |_| true);
            }
        }
        _ => return None,
    }
    if collision.indices.is_empty() {
        return None;
    }
    collision.write(&file);
    Some(stem)
}

// ---- placements ------------------------------------------------------------------------------

/// Parse one tile's `_ModelPlacementInformation.csv` into OUR tile-local frame. Ported from
/// `core/tools/import_wow` (proven math), then mapped through the 180°-about-Y frame change.
/// Rows whose center lands outside the tile are skipped (wow.export lists overlapping models in
/// every tile they touch). Returns the placements plus the `wmo` rows for the interior pass.
fn parse_tile_placements(
    map_dir: &Path,
    wow_root: &Path,
    ax: i32,
    ay: i32,
) -> (Vec<Placement>, Vec<WmoPlacement>) {
    let csv_path = map_dir.join(format!("adt_{ax}_{ay}_ModelPlacementInformation.csv"));
    let Ok(content) = fs::read_to_string(&csv_path) else {
        return (Vec::new(), Vec::new()); // tiles without placements ship no CSV
    };

    let mut placements = Vec::new();
    let mut wmos = Vec::new();
    let half = TILE / 2.0;

    for line in content.lines().skip(1) {
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() < 11 {
            continue;
        }
        let model_type = fields[10];
        let Some(model_rel) = resolve_model(wow_root, &csv_path, fields[0]) else {
            continue; // model OBJ missing from the dump
        };

        let wow_x: f32 = fields[1].parse().unwrap_or(0.0);
        let wow_y: f32 = fields[2].parse().unwrap_or(0.0); // height
        let wow_z: f32 = fields[3].parse().unwrap_or(0.0);
        let rot_x: f32 = fields[4].parse().unwrap_or(0.0);
        let rot_y: f32 = fields[5].parse().unwrap_or(0.0);
        let rot_z: f32 = fields[6].parse().unwrap_or(0.0);
        let scale: f32 = fields[8].parse().unwrap_or(1.0);

        // Tile-local in the CSV's frame (+wow_x/+wow_z), core's bounds test.
        let tx = (wow_x - ax as f32 * TILE) - half;
        let tz = (wow_z - ay as f32 * TILE) - half;
        if tx < -half || tx >= half || tz < -half || tz >= half {
            continue;
        }

        let rotation = Quat::from_rotation_y(PI) * csv_rotation(rot_x, rot_y, rot_z);

        placements.push(Placement {
            model: model_rel,
            translation: Vec3::new(-tx, wow_y, -tz),
            rotation,
            scale,
        });

        if model_type == "wmo" {
            wmos.push(WmoPlacement {
                rel_path: fields[0].to_string(),
                wow_position: Vec3::new(wow_x, wow_y, wow_z),
                wow_rotation: Vec3::new(rot_x, rot_y, rot_z),
            });
        }
    }
    (placements, wmos)
}

/// A placement CSV row's rotation (degrees) in core's Y-up frame, before our 180°-about-Y frame
/// change: what wow.export's own Blender addon applies (Euler XYZ of `RotationZ`, `RotationX`,
/// `90 + RotationY` over its Y-up -> Z-up turn), carried into Y-up. Yaw is `RotationY`; the
/// two tilts are a roll of `-RotationX` about Z, then a pitch of `RotationZ` about X. A sloped
/// placement (a fence down a hill) leans the wrong way with either sign or order off.
fn csv_rotation(rot_x: f32, rot_y: f32, rot_z: f32) -> Quat {
    Quat::from_rotation_y((rot_y - 90.0).to_radians())
        * Quat::from_rotation_z((-rot_x).to_radians())
        * Quat::from_rotation_x(rot_z.to_radians())
}

/// Interior doodads: each WMO ships its own `_ModelPlacementInformation.csv` with positions local
/// to the WMO origin (Z-up) and quaternion rotations. Ported from core, then frame-changed.
/// Returned keyed by the ADT tile the doodad's world position lands in.
fn collect_wmo_interior_doodads(
    wow_root: &Path,
    wmos: &[WmoPlacement],
) -> BTreeMap<(i32, i32), Vec<Placement>> {
    let mut by_tile: BTreeMap<(i32, i32), Vec<Placement>> = BTreeMap::new();
    let half = TILE / 2.0;

    for wmo in wmos {
        let clean = wmo
            .rel_path
            .trim_start_matches("../")
            .trim_start_matches("../");
        let wmo_obj = wow_root.join(clean);
        let Some(wmo_dir) = wmo_obj.parent() else {
            continue;
        };
        let Some(stem) = wmo_obj
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
        else {
            continue;
        };
        let csv_path = wmo_dir.join(format!("{stem}_ModelPlacementInformation.csv"));
        let Ok(content) = fs::read_to_string(&csv_path) else {
            continue; // WMO without interior doodads
        };

        // The WMO's own rotation in core's Y-up frame (same formula as terrain placements).
        let wmo_rot = csv_rotation(wmo.wow_rotation.x, wmo.wow_rotation.y, wmo.wow_rotation.z);

        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split(';').collect();
            if fields.len() < 9 {
                continue;
            }
            let Some(model_rel) = resolve_model(wow_root, &csv_path, fields[0]) else {
                continue;
            };

            let local_x: f32 = fields[1].parse().unwrap_or(0.0);
            let local_y: f32 = fields[2].parse().unwrap_or(0.0);
            let local_z: f32 = fields[3].parse().unwrap_or(0.0);
            // Quaternion in W;X;Y;Z column order.
            let rot_w: f32 = fields[4].parse().unwrap_or(1.0);
            let rot_x: f32 = fields[5].parse().unwrap_or(0.0);
            let rot_y: f32 = fields[6].parse().unwrap_or(0.0);
            let rot_z: f32 = fields[7].parse().unwrap_or(0.0);
            let scale: f32 = fields[8].parse().unwrap_or(1.0);

            // Z-up local position → Y-up, rotate by the WMO, add the WMO's world position.
            let local_yup = Vec3::new(local_x, local_z, -local_y);
            let world_pos = wmo_rot * local_yup + wmo.wow_position;
            // Z-up local quaternion → Y-up, combined with the WMO rotation.
            let world_rot = wmo_rot * Quat::from_xyzw(rot_x, rot_z, -rot_y, rot_w);

            let ax = (world_pos.x / TILE).floor() as i32;
            let ay = (world_pos.z / TILE).floor() as i32;
            let tx = (world_pos.x - ax as f32 * TILE) - half;
            let tz = (world_pos.z - ay as f32 * TILE) - half;

            by_tile.entry((ax, ay)).or_default().push(Placement {
                model: model_rel,
                translation: Vec3::new(-tx, world_pos.y, -tz),
                rotation: Quat::from_rotation_y(PI) * world_rot,
                scale,
            });
        }
    }
    by_tile
}

/// Resolve a CSV `ModelFile` cell (relative to the CSV) to a wow-root-relative path, or `None`
/// when the OBJ isn't in the dump.
fn resolve_model(wow_root: &Path, csv_path: &Path, cell: &str) -> Option<String> {
    let src = csv_path.parent()?.join(cell);
    let canonical = src.canonicalize().ok()?;
    let root = wow_root.canonicalize().ok()?;
    Some(
        canonical
            .strip_prefix(&root)
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// `tobj` load with the same options the rest of aurora_bsn uses.
fn load_obj(path: &Path) -> Option<(Vec<tobj::Model>, Vec<tobj::Material>)> {
    match tobj::load_obj(
        path,
        &tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ignore_points: true,
            ignore_lines: true,
        },
    ) {
        Ok((models, materials)) => Some((models, materials.unwrap_or_default())),
        Err(err) => {
            eprintln!("  {}: load failed: {err}", path.display());
            None
        }
    }
}
