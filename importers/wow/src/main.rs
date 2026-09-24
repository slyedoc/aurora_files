//! WoW zone importer: wow.export OBJ dumps → one `.bsn` per ADT tile (doodads + WMOs) plus
//! EDITABLE terrain map data for aurora's GPU terrain (`bevy_aurora::terrain`).
//!
//! Source layout (wow.export v0.2.6, see `~/code/p/bevy_wow/assets/wow`):
//!   maps/<map>/adt_X_Y.obj                          terrain (256 subchunk groups)
//!   maps/<map>/adt_X_Y_ModelPlacementInformation.csv  doodad (m2) + building (wmo) placements
//!   maps/<map>/tex_X_Y_<chunk>.{json,png}           per-subchunk splat layers + 64² alphas
//!   world/…/<model>.obj (+ .mtl + textures)         referenced models, shared across tiles
//!   world/wmo/…/<wmo>_ModelPlacementInformation.csv  interior doodads, local to the WMO
//!
//! Output:
//!   assets/wow/meshes/*.cluster_mesh + textures/*   baked models (shared)
//!   assets/wow/meshes/*.collider                     one collision mesh per model that has one
//!   assets/wow/<map>_X_Y.bsn                        doodad/WMO entities, TILE-LOCAL coords
//!   assets/wow/map/<map>_X_Y_height.png             129² 16-bit height grid (min/max in json)
//!   assets/wow/map/<map>_X_Y_alpha.png              1024² RGBA alphamap atlas (16×16 × 64²)
//!   assets/wow/map/<map>_X_Y_layers.json            per-chunk palette indices + height range
//!   assets/wow/map/palette.json + map/tileset/*.png the shared splat palette
//!   assets/wow/plants/<plant>.bsn                    one prefab per ground-clutter plant
//!                                                    (mesh + material + WindSway; edit freely)
//!   assets/wow/clutter.ron                           ground texture -> plants + density
//!                                                    (seeded from the GroundEffect tables;
//!                                                    edit freely)
//!
//! Terrain is NOT baked to a mesh: zero builds it at runtime with
//! `bevy_aurora::terrain::terrain_mesh` and edits it on the GPU. Existing map files are left
//! alone unless `--replace` (they may carry in-game edits).
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
//! at `--sidn-nits` (WoW only shows it at night).
//!
//! What emits is baked into derived textures under `textures/`. A glow CARD (any emissive
//! material whose texture has real alpha -- the shape lives there, the RGB is bright
//! everywhere) is WoW's additive billboard halo, which a flat emitter cannot be: a path
//! tracer shows it as a disc. So a card becomes two things: its CORE (`<tex>_card.png`,
//! black RGB with the alpha, cut at 0.5, emitting `<tex>_glow.png` = RGB x alpha) -- the
//! visible flame -- and a `PointLight` at the card's centre carrying the whole card's flux
//! (pi x radiance x area), which is the halo's illumination without the halo. A FULLBRIGHT
//! material (opaque alpha) keeps its base colour and emits `<tex>_glow.png` =
//! RGB^`--unlit-contrast`, so a lantern's bright glass glows and its dark frame does not.
//! The tracer's light table averages the emissive texture, so an emitter lights the scene
//! by what it actually emits.
//!
//! Collision: every placement of a model with collision gets a second entity carrying
//! `bevy_aurora::collision::CollisionMesh`, which shares one built collider between all of the
//! model's instances (zero turns it into an avian static body). An M2's comes from wow.export's `<model>.phys.obj` -- WoW's own
//! collision mesh, so a tree is its trunk and bushes are walked through; a WMO's from its
//! render triangles, keeping what the json's per-triangle flags mark collidable (collision-
//! only triangles are not in the OBJ and are lost). Ground clutter never collides.
//!
//! Ground clutter: each splat layer's `effectID` (wow.export tex json) -> GroundEffectTexture
//! (density, up to four doodads with weights) -> GroundEffectDoodad (model file id) -> the
//! community listfile (path) -> `maps/<map>/foliage/<basename>.obj`. The plants are baked
//! like any model (cutmask -> Mask + opacity micromap), each gets a `.bsn` prefab, and
//! `clutter.ron` maps every ground texture that has an effect to its plants. Zero assembles
//! the chunks at runtime from those (game/wow_clutter.rs). Plant `.bsn`s and `clutter.ron`
//! are never overwritten without `--replace`.
//!
//! Deferred: liquids, creatures.
//!
//!   cargo run --release -p wow_import                        # Northshire (31-33 × 47-49)

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::f32::consts::PI;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use aurora_bsn::bsn::{material_fields, scene, write_entity_trs};
use aurora_bsn::discovery::{copy_textures, material_is_cutmask, sanitize};
use aurora_bsn::mesh::{OmmOptions, attach_omm, build_mesh, submesh_centroid};
use aurora_cluster_mesh::{ClusterMeshData, write_cluster_mesh_sync};
use bevy::math::{EulerRot, Quat, Vec3};
use clap::Parser;

/// One WoW ADT tile, in yards (= world units): `1600/3`.
const TILE: f32 = 1600.0 / 3.0;
/// wow.export's map-space origin constant: `32 · TILE` (maps span ADT 0..64 centered at 32).
const C: f32 = 32.0 * TILE;
/// Height grid edge: 16 subchunks × 8 cells + 1.
const HEIGHT_RES: u32 = 129;
/// Alphamap atlas edge (16 chunk cells of 64px).
const ALPHA_ATLAS: u32 = 1024;
const ALPHA_CELL: u32 = 64;
/// No texture in a chunk-layer slot (bevy_aurora::terrain::NO_LAYER).
const NO_LAYER: u32 = u32::MAX;

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
    /// Re-bake meshes and REWRITE map files even when they exist (discards in-game edits!).
    #[arg(long)]
    replace: bool,
    /// The community listfile (`<file id>;<path>` per line): resolves GroundEffectDoodad
    /// model ids to plant model names.
    #[arg(long, default_value = "/home/slyedoc/code/p/core/assets/terrain/dbc/listfile.csv")]
    listfile: PathBuf,
    /// Radiance (nits) of additive-blended materials: lamp glows, flames, fireflies.
    #[arg(long, default_value_t = 20000.0)]
    emissive_nits: f32,
    /// Radiance (nits) of unlit (fullbright) materials: lantern glass, lit logs.
    #[arg(long, default_value_t = 8000.0)]
    unlit_nits: f32,
    /// Radiance (nits) of WMO self-illuminated colours (lit windows).
    #[arg(long, default_value_t = 5000.0)]
    sidn_nits: f32,
    /// Exponent on a fullbright material's linear colour for its glow map: higher keeps the
    /// emission to the texture's brightest parts (lantern glass, not its frame).
    #[arg(long, default_value_t = 3.0)]
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

fn m2_emissives(args: &Args, json: &serde_json::Value, models: &[tobj::Model]) -> Vec<Option<Emissive>> {
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
                nits: if additive { args.emissive_nits } else { args.unlit_nits },
                additive,
            })
        })
        .collect()
}

fn wmo_emissives(args: &Args, json: &serde_json::Value, models: &[tobj::Model]) -> Vec<Option<Emissive>> {
    let (Some(materials), Some(groups)) = (json["materials"].as_array(), json["groups"].as_array())
    else {
        return vec![None; models.len()];
    };
    // OBJ group `<GroupName><batch>` -> WMO material id, from each group's render batches.
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for group in groups {
        let Some(name) = group["groupName"].as_str() else { continue };
        for (batch, info) in group["renderBatches"].as_array().into_iter().flatten().enumerate() {
            if let Some(id) = info["materialID"].as_u64() {
                by_name.entry(format!("{name}{batch}")).or_insert(id as usize);
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
) -> Option<(String, bool, [f32; 3])> {
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
        let img = image::open(obj_dir.join(tex.replace('\\', "/"))).ok()?.into_rgba8();
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
            glow.put_pixel(x, y, image::Rgba([enc(rgb[0]), enc(rgb[1]), enc(rgb[2]), 255]));
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

    let mut fields = String::new();
    if card {
        // The card's core only: a black cutout that emits; the halo is the point light's.
        let _ = write!(
            fields,
            " base_color_texture: \"{prefix}/textures/{card_name}\", \
             alpha_mode: bevy_aurora::material::AlphaMode::Mask(0.5),"
        );
    } else {
        fields.push_str(&material_fields(prefix, material, is_cutmask, &[]));
    }
    let [r, g, b] = emissive.tint.map(|t| t * emissive.nits);
    let _ = write!(
        fields,
        " emissive: bevy_color::linear_rgba::LinearRgba {{ red: {}, green: {}, blue: {}, alpha: 1.0 }}, \
         emissive_texture: \"{prefix}/textures/{glow_name}\",",
        fmt_f(r),
        fmt_f(g),
        fmt_f(b)
    );
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
    mesh_stem: String,
    material_fields: String,
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
    fs::create_dir_all(args.out_dir.join("meshes")).expect("create meshes dir");
    let textures_dir = args.out_dir.join("textures");
    fs::create_dir_all(&textures_dir).expect("create textures dir");
    fs::create_dir_all(args.out_dir.join("map/tileset")).expect("create map dir");

    // Pass 1 — parse every tile's placement CSV (m2 + wmo rows), then fold in each unique WMO's
    // interior doodads. Keyed by tile coord.
    let mut tile_placements: BTreeMap<(i32, i32), Vec<Placement>> = BTreeMap::new();
    let mut all_wmos: Vec<WmoPlacement> = Vec::new();
    for ay in args.y0..=args.y1 {
        for ax in args.x0..=args.x1 {
            let (placements, wmos) = parse_tile_placements(&map_dir, &args.wow_root, ax, ay);
            println!("tile {ax},{ay}: {} placements, {} wmos", placements.len(), wmos.len());
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
    println!("{} interior doodads from {} unique WMOs", interior_count, all_wmos.len());

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
            &OmmOptions::from_env(),
            &mut derived,
        );
        baked_models.insert(rel.clone(), submeshes);
        if let Some(stem) = bake_collider(&args, &args.wow_root.join(rel), rel) {
            colliders.insert(rel.clone(), stem);
        }
    }
    println!("{} of {} models collide", colliders.len(), unique_models.len());

    // Pass 3 — write each tile's `.bsn` (doodads/WMOs only) and its terrain map data.
    let mut palette = PaletteBuilder::default();
    let mut texture_effects: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (&(ax, ay), placements) in &tile_placements {
        let mut entities = String::new();
        let mut instanced = 0usize;
        for p in placements {
            let Some(submeshes) = baked_models.get(&p.model) else {
                continue;
            };
            let name = Path::new(&p.model)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Some(stem) = colliders.get(&p.model) {
                write_collider_entity(&mut entities, &args.asset_prefix, &name, stem, p);
            }
            for sub in submeshes {
                // Instance transform composed with the submesh's local centroid offset.
                let t = p.translation + p.rotation * (sub.centroid * p.scale);
                write_entity_trs(
                    &mut entities,
                    &args.asset_prefix,
                    &sub.mesh_stem,
                    &sub.material_fields,
                    &name,
                    t.to_array(),
                    p.rotation.to_array(),
                    [p.scale, p.scale, p.scale],
                    None,
                    None,
                );
                instanced += 1;
                if let Some((color, lumens, radius)) = sub.glow_light {
                    write_point_light(
                        &mut entities,
                        &format!("{name} glow"),
                        t.to_array(),
                        color,
                        lumens * p.scale * p.scale,
                        radius * p.scale,
                    );
                }
            }
        }

        let scene_name = format!("{}_{ax}_{ay}", args.map);
        let bsn = scene(&scene_name, &entities);
        let bsn_path = args.out_dir.join(format!("{scene_name}.bsn"));
        fs::write(&bsn_path, bsn).expect("write .bsn");
        println!(
            "tile {ax},{ay}: {} placements -> {instanced} entities -> {}",
            placements.len(),
            bsn_path.display()
        );

        emit_tile_map(&args, &map_dir, ax, ay, &mut palette, &mut texture_effects);
    }
    palette.write(&args);
    emit_clutter(&args, &map_dir, &textures_dir, &texture_effects, &mut cutmask_cache, &mut derived);
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
        let ids = fields[3].split(',').map(|s| s.trim().parse::<u32>().unwrap_or(0));
        let weights = fields[4].split(',').map(|s| s.trim().parse::<f32>().unwrap_or(1.0));
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
            let animscale = it.next().and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(1.0);
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

/// Bake every plant the tiles' ground textures reference, write a `.bsn` prefab per plant and
/// the `clutter.ron` texture -> plants table.
fn emit_clutter(
    args: &Args,
    map_dir: &Path,
    textures_dir: &Path,
    texture_effects: &BTreeMap<String, BTreeSet<u32>>,
    cutmask_cache: &mut HashMap<String, bool>,
    derived: &mut DerivedGlows,
) {
    let effects = load_ground_effects(&args.wow_root);
    let doodads = load_ground_doodads(&args.wow_root);
    let wanted: HashSet<u32> = doodads.values().map(|(file, _)| *file).collect();
    let listfile = load_listfile(&args.listfile, &wanted);
    let plants_dir = args.out_dir.join("plants");
    fs::create_dir_all(&plants_dir).expect("create plants dir");

    // Plant name -> (bsn asset path) once baked.
    let mut baked: BTreeMap<String, String> = BTreeMap::new();
    let mut layers: BTreeMap<String, (u32, Vec<(String, f32)>)> = BTreeMap::new();
    // A texture's chunks can carry several effect ids (some rows are missing from the table,
    // some have no doodads): every candidate is tried, the densest with plants wins.
    for (texture, effect_ids) in texture_effects {
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
            let Some((file_id, animscale)) = doodads.get(&doodad) else { continue };
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
            if !baked.contains_key(&name) {
                let obj = map_dir.join("foliage").join(format!("{name}.obj"));
                if !obj.exists() {
                    eprintln!("clutter: {name}: no foliage export at {}", obj.display());
                    continue;
                }
                // Every plant triangle is a whole cutout card, so the micromap is the
                // silhouette: finer than a tree's leaves (a quarter texel per micro-triangle,
                // a texel of erosion) at a still-small cost, some KB per plant.
                let plant_omm = OmmOptions {
                    max_subdiv: 9,
                    erode_px: 1,
                    scale: 0.25,
                    ..OmmOptions::from_env()
                };
                let submeshes = bake_model(
                    args,
                    &obj,
                    &format!("foliage/{name}.obj"),
                    textures_dir,
                    cutmask_cache,
                    &plant_omm,
                    derived,
                );
                if submeshes.is_empty() {
                    continue;
                }
                let bsn_path = plants_dir.join(format!("{name}.bsn"));
                if args.replace || !bsn_path.exists() {
                    let mut entities = String::new();
                    for sub in &submeshes {
                        write_plant_entity(&mut entities, &args.asset_prefix, sub, &name, *animscale);
                    }
                    let bsn = format!(
                        "// Ground-clutter plant, seeded by the wow importer. Edit freely (the\n\
                         // material, the WindSway amplitude); never overwritten without --replace.\n\
                         #{name}\nbevy_ecs::hierarchy::Children [\n{entities}]\n"
                    );
                    fs::write(&bsn_path, bsn).expect("write plant .bsn");
                }
                baked.insert(name.clone(), format!("{}/plants/{name}.bsn", args.asset_prefix));
            }
            *plants.entry(name).or_insert(0.0) += weight;
        }
        if plants.is_empty() {
            continue;
        }
        let entry = layers.entry(texture.clone()).or_insert((0, Vec::new()));
        if effect.density > entry.0 {
            entry.0 = effect.density;
            entry.1 = plants
                .into_iter()
                .map(|(name, weight)| (baked[&name].clone(), weight))
                .collect();
        }
      }
    }

    let ron_path = args.out_dir.join("clutter.ron");
    if args.replace || !ron_path.exists() {
        let mut out = String::new();
        out.push_str(
            "// Ground clutter: splat palette texture -> the plants it grows and how many\n\
             // (WoW's density: plants per 8x8 chunk cell, ~4.2 yd square). Seeded from the\n\
             // GroundEffect tables by the wow importer; edit freely, it is never overwritten.\n\
             (\n    layers: {\n",
        );
        for (texture, (density, plants)) in &layers {
            let _ = writeln!(out, "        \"{texture}\": (");
            let _ = writeln!(out, "            density: {density}.0,");
            out.push_str("            plants: [\n");
            for (path, weight) in plants {
                let _ = writeln!(out, "                (\"{path}\", {weight:.1}),");
            }
            out.push_str("            ],\n        ),\n");
        }
        out.push_str("    },\n)\n");
        fs::write(&ron_path, out).expect("write clutter.ron");
    }
    println!(
        "clutter: {} plants baked, {} ground textures -> {}",
        baked.len(),
        layers.len(),
        ron_path.display()
    );
}

/// A plant prefab part: the submesh at its centroid, the material, and -- for anything WoW
/// animates (Animscale > 0) -- the wind deformer the clutter chunks inherit. Rocks get none.
/// Edit the amplitude per plant here.
fn write_plant_entity(
    out: &mut String,
    asset_prefix: &str,
    sub: &BakedSubmesh,
    name: &str,
    animscale: f32,
) {
    let c = sub.centroid;
    let wind = if animscale > 0.0 {
        format!(
            "    bevy_aurora::skinning::WindSway {{ amplitude: {} }}\n",
            fmt_f(0.25 * animscale)
        )
    } else {
        String::new()
    };
    let _ = write!(
        out,
        "    bevy_ecs::name::Name(\"{name}\")\n    \
         bevy_transform::components::transform::Transform {{ \
         translation: glam::Vec3 {{ x: {}, y: {}, z: {} }}, \
         rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }}, \
         scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }} }}\n    \
         bevy_mesh::components::Mesh3d(\"{asset_prefix}/meshes/{}.cluster_mesh\")\n{wind}    \
         bevy_aurora::material::AuroraMaterial3d(bevy_aurora::material::AuroraMaterial {{{}}}),\n\n",
        fmt_f(c.x),
        fmt_f(c.y),
        fmt_f(c.z),
        sub.mesh_stem,
        sub.material_fields,
    );
}

/// A point light entity: a glow card's halo as illumination.
fn write_point_light(out: &mut String, name: &str, t: [f32; 3], color: [f32; 3], lumens: f32, radius: f32) {
    let _ = write!(
        out,
        "    bevy_ecs::name::Name(\"{}\")\n    \
         bevy_transform::components::transform::Transform {{ \
         translation: glam::Vec3 {{ x: {}, y: {}, z: {} }}, \
         rotation: glam::Quat {{ x: 0.0, y: 0.0, z: 0.0, w: 1.0 }}, \
         scale: glam::Vec3 {{ x: 1.0, y: 1.0, z: 1.0 }} }}\n    \
         bevy_light::point_light::PointLight {{ \
         color: bevy_color::color::Color::LinearRgba(bevy_color::linear_rgba::LinearRgba {{ red: {}, green: {}, blue: {}, alpha: 1.0 }}), \
         intensity: {}, radius: {} }},\n\n",
        name.replace('"', "'"),
        fmt_f(t[0]),
        fmt_f(t[1]),
        fmt_f(t[2]),
        fmt_f(color[0]),
        fmt_f(color[1]),
        fmt_f(color[2]),
        fmt_f(lumens),
        fmt_f(radius),
    );
}

/// `.bsn` float literal: fixed point with a decimal point (the lexer rejects exponents).
fn fmt_f(v: f32) -> String {
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
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
    /// The map-relative file name of a palette entry (the key `clutter.ron` uses).
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
        let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
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
            if !dst.exists() || args.replace {
                if let Err(err) = fs::copy(src, &dst) {
                    eprintln!("palette: copy {} failed: {err}", src.display());
                }
            }
        }
        let textures: Vec<String> = self.entries.iter().map(|(n, _)| n.clone()).collect();
        let json = serde_json::json!({
            // Texture repeats per subchunk cell edge (WoW ground textures tile ~every 4.2 yd).
            "repeats": 8.0,
            "textures": textures,
        });
        fs::write(dir.join("palette.json"), serde_json::to_string_pretty(&json).unwrap())
            .expect("write palette.json");
        println!("palette: {} textures -> {}", textures.len(), dir.join("palette.json").display());
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

/// Write one tile's editable terrain data: the 16-bit height grid (resampled from the ADT
/// OBJ's vertices in OUR tile frame), the alphamap atlas (chunk pngs blitted into their
/// cells), and the per-chunk layer table against the shared palette. Existing files are left
/// alone unless `--replace` — they may carry in-game edits.
fn emit_tile_map(
    args: &Args,
    map_dir: &Path,
    ax: i32,
    ay: i32,
    palette: &mut PaletteBuilder,
    texture_effects: &mut BTreeMap<String, BTreeSet<u32>>,
) {
    let dir = args.out_dir.join("map");
    let stem = format!("{}_{ax}_{ay}", args.map);
    let height_path = dir.join(format!("{stem}_height.png"));
    let alpha_path = dir.join(format!("{stem}_alpha.png"));
    let layers_path = dir.join(format!("{stem}_layers.json"));

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

    let exists = height_path.exists() && alpha_path.exists() && layers_path.exists();
    if exists && !args.replace {
        return;
    }

    // Heights: nearest ADT vertex per grid point, through a spatial hash of the OBJ verts.
    let obj_path = map_dir.join(format!("adt_{ax}_{ay}.obj"));
    let Some((models, _)) = load_obj(&obj_path) else {
        eprintln!("tile {ax},{ay}: no terrain obj; map data skipped");
        return;
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
                    let Some(ids) = buckets.get(&(bx + dx, bz + dz)) else { continue };
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
    atlas.save(&alpha_path).expect("write alpha png");
    let chunks: Vec<[u32; 4]> = (0..256usize)
        .map(|i| chunks[(15 - i / 16) * 16 + (15 - i % 16)])
        .collect();

    let json = serde_json::json!({
        "height_min": hmin,
        "height_max": hmax,
        "resolution": HEIGHT_RES,
        "chunks": chunks,
    });
    fs::write(&layers_path, serde_json::to_string(&json).unwrap()).expect("write layers json");
    println!("tile {ax},{ay}: map data -> {}", height_path.display());
}

// ---- model baking ---------------------------------------------------------------------------

/// Bake every submesh of one model OBJ into the shared `meshes/` dir (skipping ones already on
/// disk unless `--replace`), copy its textures, and return the per-submesh instance records.
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

    // Unique, filesystem-safe stem from the wow-root-relative path (basenames collide:
    // several zones ship a `bush01.obj`).
    let path_stem = sanitize(rel.trim_end_matches(".obj").trim_start_matches("world/"));

    let mut out = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let Some(centroid) = submesh_centroid(&m.mesh) else {
            continue;
        };
        let material = m.mesh.material_id.and_then(|id| materials.get(id));
        let emissive = emissives.get(i).copied().flatten();
        let is_cutmask =
            material.is_some_and(|mat| material_is_cutmask(obj_dir, mat, cutmask_cache));

        let mesh_stem = format!("{path_stem}_{i}");
        let mesh_file = args
            .out_dir
            .join("meshes")
            .join(format!("{mesh_stem}.cluster_mesh"));
        if args.replace || !mesh_file.exists() {
            let mesh = build_mesh(&m.mesh, centroid);
            let mut cm = match ClusterMeshData::from_mesh_flat(&mesh) {
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
            let w = BufWriter::new(File::create(&mesh_file).expect("create .cluster_mesh"));
            write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
        }

        let mut glow_light = None;
        let fields = match &emissive {
            Some(e) => match emissive_fields(args, obj_dir, textures_dir, material, is_cutmask, e, derived) {
                Some((fields, card, mean)) => {
                    if card {
                        // The whole card's flux, as a Lambertian emitter of its mean radiance.
                        let area = submesh_area(&m.mesh);
                        let radiance = mean.map(|c| c * e.nits);
                        let lumens = PI * (0.2126 * radiance[0] + 0.7152 * radiance[1] + 0.0722 * radiance[2]) * area;
                        let luma = (0.2126 * mean[0] + 0.7152 * mean[1] + 0.0722 * mean[2]).max(1e-6);
                        let color = mean.map(|c| c / luma);
                        glow_light = Some((color, lumens, (area / PI).sqrt() * 0.5));
                    }
                    fields
                }
                None => material_fields(&args.asset_prefix, material, is_cutmask, &[]),
            },
            None => material_fields(&args.asset_prefix, material, is_cutmask, &[]),
        };
        out.push(BakedSubmesh {
            material_fields: fields,
            mesh_stem,
            centroid: Vec3::new(centroid[0] as f32, centroid[1] as f32, centroid[2] as f32),
            glow_light,
        });
    }
    out
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
        let mut bytes = Vec::with_capacity(16 + self.positions.len() * 12 + self.indices.len() * 12);
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
/// its leaf cards); a WMO with the render triangles its json marks collidable.
fn bake_collider(args: &Args, obj_path: &Path, rel: &str) -> Option<String> {
    let stem = sanitize(rel.trim_end_matches(".obj").trim_start_matches("world/"));
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
        "wmo" => {
            let (models, _) = load_obj(obj_path)?;
            // OBJ group `<GroupName><batch>` -> that batch's triangles' MOPY flags.
            let mut flags: HashMap<String, Vec<u64>> = HashMap::new();
            for group in json["groups"].as_array()? {
                let Some(name) = group["groupName"].as_str() else { continue };
                let info = group["materialInfo"].as_array();
                for (batch, b) in group["renderBatches"].as_array().into_iter().flatten().enumerate() {
                    let first = b["firstFace"].as_u64().unwrap_or(0) as usize / 3;
                    let count = b["numFaces"].as_u64().unwrap_or(0) as usize / 3;
                    let batch_flags = (first..first + count)
                        .map(|t| info.and_then(|i| i.get(t)).and_then(|m| m["flags"].as_u64()).unwrap_or(0x20))
                        .collect();
                    flags.entry(format!("{name}{batch}")).or_insert(batch_flags);
                }
            }
            for m in &models {
                // MOPY: 0x08 collision, 0x20 render, 0x04 detail (render-only trim).
                let batch = flags.get(&m.name).filter(|f| f.len() == m.mesh.indices.len() / 3);
                collision.extend(&m.mesh, |t| {
                    batch.is_none_or(|f| f[t] & 0x08 != 0 || (f[t] & 0x20 != 0 && f[t] & 0x04 == 0))
                });
            }
        }
        _ => return None,
    }
    if collision.indices.is_empty() {
        return None;
    }
    if args.replace || !file.exists() {
        collision.write(&file);
    }
    Some(stem)
}

/// A collision body at a placement: the model's shared `.collider`, under the placement's own
/// transform (collision lives in model space, not on the centred render submeshes).
///
/// The component is aurora's, which carries GEOMETRY only — zero's `BakedColliderPlugin` turns
/// it into an avian `Collider` + `RigidBody::Static`. Naming avian here instead made the tile
/// unopenable by anything without avian in its type registry, the plain `bsn` viewer included
/// (`unknown type: avian3d::dynamics::rigid_body::RigidBody`, and the whole scene is refused).
fn write_collider_entity(out: &mut String, prefix: &str, name: &str, stem: &str, p: &Placement) {
    let (t, r) = (p.translation, p.rotation);
    let _ = write!(
        out,
        "    bevy_ecs::name::Name(\"{} collider\")\n    \
         bevy_transform::components::transform::Transform {{ \
         translation: glam::Vec3 {{ x: {}, y: {}, z: {} }}, \
         rotation: glam::Quat {{ x: {}, y: {}, z: {}, w: {} }}, \
         scale: glam::Vec3 {{ x: {s}, y: {s}, z: {s} }} }}\n    \
         bevy_aurora::collision::CollisionMesh(\"{prefix}/meshes/{stem}.collider\"),\n\n",
        name.replace('"', "'"),
        fmt_f(t.x),
        fmt_f(t.y),
        fmt_f(t.z),
        fmt_f(r.x),
        fmt_f(r.y),
        fmt_f(r.z),
        fmt_f(r.w),
        s = fmt_f(p.scale),
    );
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

        // Core's rotation (WoW Z-up Euler degrees → Y-up), then our 180°-about-Y frame change.
        let rotation = Quat::from_rotation_y(PI)
            * Quat::from_rotation_y((-90.0f32).to_radians())
            * Quat::from_euler(
                EulerRot::YXZ,
                rot_y.to_radians(),
                rot_z.to_radians(),
                rot_x.to_radians(),
            );

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
        let clean = wmo.rel_path.trim_start_matches("../").trim_start_matches("../");
        let wmo_obj = wow_root.join(clean);
        let Some(wmo_dir) = wmo_obj.parent() else { continue };
        let Some(stem) = wmo_obj.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        let csv_path = wmo_dir.join(format!("{stem}_ModelPlacementInformation.csv"));
        let Ok(content) = fs::read_to_string(&csv_path) else {
            continue; // WMO without interior doodads
        };

        // The WMO's own rotation in core's Y-up frame (same formula as terrain placements).
        let wmo_rot = Quat::from_rotation_y((-90.0f32).to_radians())
            * Quat::from_euler(
                EulerRot::YXZ,
                wmo.wow_rotation.y.to_radians(),
                wmo.wow_rotation.z.to_radians(),
                wmo.wow_rotation.x.to_radians(),
            );

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
    Some(canonical.strip_prefix(&root).ok()?.to_string_lossy().into_owned())
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
