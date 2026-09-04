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
//!   assets/wow/<map>_X_Y.bsn                        doodad/WMO entities, TILE-LOCAL coords
//!   assets/wow/map/<map>_X_Y_height.png             129² 16-bit height grid (min/max in json)
//!   assets/wow/map/<map>_X_Y_alpha.png              1024² RGBA alphamap atlas (16×16 × 64²)
//!   assets/wow/map/<map>_X_Y_layers.json            per-chunk palette indices + height range
//!   assets/wow/map/palette.json + map/tileset/*.png the shared splat palette
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
//! Deferred: liquids, ground-clutter grass effects, creatures, M2 blend-mode JSONs.
//!
//!   cargo run --release -p wow_import                        # Northshire (31-33 × 47-49)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::f32::consts::PI;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use aurora_bsn::bsn::{material_fields, scene, write_entity_trs};
use aurora_bsn::discovery::{copy_textures, material_is_cutmask, sanitize};
use aurora_bsn::mesh::{attach_omm, build_mesh, submesh_centroid};
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
}

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
    for rel in &unique_models {
        let submeshes = bake_model(&args, &args.wow_root.join(rel), rel, &textures_dir, &mut cutmask_cache);
        baked_models.insert(rel.clone(), submeshes);
    }

    // Pass 3 — write each tile's `.bsn` (doodads/WMOs only) and its terrain map data.
    let mut palette = PaletteBuilder::default();
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
                );
                instanced += 1;
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

        emit_tile_map(&args, &map_dir, ax, ay, &mut palette);
    }
    palette.write(&args);
    println!("done.");
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
}
#[derive(serde::Deserialize)]
struct ChunkLayers {
    layers: Vec<LayerDef>,
}

/// Write one tile's editable terrain data: the 16-bit height grid (resampled from the ADT
/// OBJ's vertices in OUR tile frame), the alphamap atlas (chunk pngs blitted into their
/// cells), and the per-chunk layer table against the shared palette. Existing files are left
/// alone unless `--replace` — they may carry in-game edits.
fn emit_tile_map(args: &Args, map_dir: &Path, ax: i32, ay: i32, palette: &mut PaletteBuilder) {
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
                entry[slot] = palette.index(map_dir, &l.file);
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
) -> Vec<BakedSubmesh> {
    let Some((models, materials)) = load_obj(obj_path) else {
        return Vec::new();
    };
    copy_textures(obj_path, &materials, &models, textures_dir);
    let obj_dir = obj_path.parent().unwrap_or_else(|| Path::new("."));

    // Unique, filesystem-safe stem from the wow-root-relative path (basenames collide:
    // several zones ship a `bush01.obj`).
    let path_stem = sanitize(rel.trim_end_matches(".obj").trim_start_matches("world/"));

    let mut out = Vec::new();
    for (i, m) in models.iter().enumerate() {
        let Some(centroid) = submesh_centroid(&m.mesh) else {
            continue;
        };
        let material = m.mesh.material_id.and_then(|id| materials.get(id));
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
                let _ = attach_omm(&mut cm, obj_dir, mat);
            }
            let w = BufWriter::new(File::create(&mesh_file).expect("create .cluster_mesh"));
            write_cluster_mesh_sync(&cm, w).expect("write .cluster_mesh");
        }

        out.push(BakedSubmesh {
            material_fields: material_fields(&args.asset_prefix, material, is_cutmask, &[]),
            mesh_stem,
            centroid: Vec3::new(centroid[0] as f32, centroid[1] as f32, centroid[2] as f32),
        });
    }
    out
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
