//! Person bake (docs/todo.md §2): the CAST, in code, → BAKED skinned prefabs
//! — pre-clustered meshes + a `.bsn` any aurora app can spawn without the
//! make_human runtime. People are baked (decisions.md 2026-08-12): no material
//! persistence + a fixed cast means dress-up matters rarely, and cluster
//! meshes load in milliseconds instead of re-generating for seconds.
//!
//! ```text
//! cargo run -p bake_person                # bake everyone in CAST
//! cargo run -p bake_person -- clerk       # iterate on one person
//! ```
//!
//! The cast lives in `CAST` below — plain Rust, no config files. Each entry
//! writes `assets/people/<name>/meshes/*.cluster_mesh` plus
//! `assets/people/<name>.bsn`:
//!
//! * the skeleton as a named bone tree in BIND POSE (f64 Transforms — an f32
//!   literal silently zeroes, same trap the g1 importer documents);
//! * one entity per part carrying `RaytracingMesh3d` + `AuroraMaterial3d`
//!   (textures by asset path) + `SkinJointsByName` — aurora's
//!   `resolve_skin_joints` rebuilds the real `SkinnedMesh` at spawn — and the
//!   `BakedPerson` marker that drives zero's spawn hydration.
//!
//! The bake runs the FULL app (GPU and all): heavier than a headless world,
//! but byte-identical to what `examples/human.rs` renders, with zero
//! missing-plugin drift. This tool moves to aurora_files together with the
//! make_human crate once zero drops it.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;

use bevy::{math::DVec3, mesh::skinning::SkinnedMesh, prelude::*};
use bevy_aurora::{
    AuroraPlugins,
    geometry::asset::{ClusterMesh, write_cluster_mesh_sync},
    instance::RaytracingMesh3d,
    material::{AuroraMaterial3d, StandardAuroraMaterial},
    util::timeout::TimeoutAppExt,
};
use clap::Parser;
use make_human::prelude::*;

#[derive(Parser)]
#[command(about = "Bake the cast (or just the named ones) -> <out>/people/<name>.bsn")]
struct Args {
    /// Bake only these cast members (default: everyone in `CAST`).
    names: Vec<String>,
    /// Asset root the bake writes into (bsn texture paths are relative to
    /// it). Defaults to zero's assets, sitting next to this workspace.
    #[arg(long)]
    out: Option<PathBuf>,
}

/// One cast member, in code — the single source of truth for who exists and
/// what they wear. Iterate here, re-run the tool, done.
struct Person {
    name: &'static str,
    skin_mesh: SkinMesh,
    skin_material: SkinMaterial,
    eyes: Eyes,
    hair: Option<Hair>,
    eyebrows: Eyebrows,
    eyelashes: Eyelashes,
    teeth: Teeth,
    /// The macro body shape. FEMALE MESHES NEED ONE — generation's async task
    /// dies silently without it (the bake then never completes).
    morph: Option<MacroMorph>,
    outfit: &'static [Clothing],
}

/// THE CAST (docs/design/story/cast/). Every run bakes all of them unless
/// names are passed.
const CAST: &[Person] = &[
    // The isekai'd protagonist: arrived with nothing, wearing guild charity.
    Person {
        name: "player",
        skin_mesh: SkinMesh::MaleGeneric,
        skin_material: SkinMaterial::YoungAfricanMale,
        eyes: Eyes::HighPolyBrown,
        hair: Some(Hair::Afro01),
        eyebrows: Eyebrows::Eyebrow001,
        eyelashes: Eyelashes::Eyelashes01,
        teeth: Teeth::TeethBase,
        morph: None,
        outfit: &[Clothing::DonitzMonkRobe],
    },
    // Older warrior gone administrative; tunic and sash, sturdy boots.
    Person {
        name: "guildmaster",
        skin_mesh: SkinMesh::MaleGeneric,
        skin_material: SkinMaterial::MiddleageAfricanMale,
        eyes: Eyes::HighPolyBrown,
        hair: Some(Hair::CortuShortMessyHair),
        eyebrows: Eyebrows::Eyebrow009,
        eyelashes: Eyelashes::Eyelashes01,
        teeth: Teeth::TeethBase,
        morph: None,
        outfit: &[
            Clothing::DrednicolsonAsymmetricTunicAndSash,
            Clothing::CulturalibreMaleBoots,
        ],
    },
    // The meticulous desk clerk: tidy bun, practical dress.
    Person {
        name: "clerk",
        skin_mesh: SkinMesh::FemaleGeneric,
        skin_material: SkinMaterial::YoungCaucasianFemale2,
        eyes: Eyes::HighPolyGreen,
        // NOT ElvsBraidBun: its mhmat deterministically wedges bevy's
        // recursive-dependency tracking (deps stay Loading forever unless an
        // outside load touches the same paths) — parked in docs/todo.md.
        hair: Some(Hair::Braid01),

        eyebrows: Eyebrows::Eyebrow010,
        eyelashes: Eyelashes::Eyelashes01,
        teeth: Teeth::TeethBase,
        morph: Some(MacroMorph::CaucasianFemaleYoung),
        outfit: &[
            Clothing::PunkduckMedievalDress,
            Clothing::PunkduckMedievalBoots,
        ],

    },
];

/// The subject entities in flight this run: `entity handled by Subject(index
/// into CAST)`; the run exits once every one has baked.
#[derive(Resource)]
struct BakeState {
    /// The make_human library root (aurora_files/raw) — texture SOURCES.
    library_root: PathBuf,
    /// The consuming game's asset root — everything the bake writes.
    out_root: PathBuf,
    remaining: usize,
}

/// Which cast member a generating entity is.
#[derive(Component, Clone, Copy)]
struct Subject(usize);

fn main() {
    let args = Args::parse();
    let selected: Vec<usize> = if args.names.is_empty() {
        (0..CAST.len()).collect()
    } else {
        args.names
            .iter()
            .map(|n| {
                CAST.iter()
                    .position(|p| p.name == n)
                    .unwrap_or_else(|| panic!("'{n}' is not in CAST"))
            })
            .collect()
    };
    // The tool runs from anywhere: the make_human library (LOAD side) is the
    // workspace's raw/ dir; the bake output (WRITE side) defaults to zero's
    // assets next door. Textures are COPIED into people/<name>/textures/ so
    // the output needs no make_human library at runtime.
    let library_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../raw")
        .canonicalize()
        .expect("canonicalize raw/ library root");
    let out_root = args
        .out
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../zero/assets"))
        .canonicalize()
        .expect("canonicalize out root");

    App::new()
        .add_timeout_exit(None, 300.0)
        .add_plugins((
            // The asset root is the LIBRARY's, so "make_human/…" paths resolve.
            AuroraPlugins.build().set(bevy::asset::AssetPlugin {
                file_path: library_root.to_string_lossy().into_owned(),
                ..Default::default()
            }),
            MakeHumanPlugin,
        ))
        .insert_resource(BakeState {
            library_root,
            out_root,
            remaining: selected.len(),
        })
        .insert_resource(Selected(selected))
        .add_systems(Startup, spawn_subjects)
        .add_observer(bake_on_complete)
        .run();
}

#[derive(Resource)]
struct Selected(Vec<usize>);

/// The subjects, exactly as a game would spawn them — plus a camera so the
/// frame loop runs (the bake exits before anything meaningful renders).
fn spawn_subjects(mut commands: Commands, selected: Res<Selected>) {
    commands.spawn((
        Camera::default(),
        bevy_aurora::render::AuroraCamera::default(),
        Transform::from_xyz(0.0, 1.6, -3.0).looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));

    for (i, &index) in selected.0.iter().enumerate() {
        let cfg = &CAST[index];
        let mut subject = commands.spawn((
            Name::new(cfg.name),
            Subject(index),
            Human,
            Rig::GameEngine,
            cfg.skin_mesh,
            cfg.skin_material,
            cfg.eyes,
            cfg.eyebrows,
            cfg.eyelashes,
            cfg.teeth,
            Outfit(cfg.outfit.to_vec()),
            Transform::from_translation(DVec3::new(i as f64 * 2.0, 0.0, 0.0)),
        ));
        if let Some(hair) = cfg.hair {
            subject.insert(hair);
        }
        if let Some(morph) = cfg.morph {
            subject.insert(Morphs(vec![Morph::new(MorphTarget::Macro(morph), 1.0)]));
        }
        info!("bake: generating '{}'", cfg.name);
    }
}

/// Fixed-point float literal — the `.bsn` lexer rejects scientific notation.
fn f(x: f64) -> String {
    format!("{x:.7}")
}

fn transform_bsn(tf: &Transform) -> String {
    format!(
        "bevy_transform::components::transform::Transform {{ translation: glam::DVec3 {{ x: {}, y: {}, z: {} }}, rotation: glam::DQuat {{ x: {}, y: {}, z: {}, w: {} }}, scale: glam::DVec3 {{ x: {}, y: {}, z: {} }} }}",
        f(tf.translation.x), f(tf.translation.y), f(tf.translation.z),
        f(tf.rotation.x), f(tf.rotation.y), f(tf.rotation.z), f(tf.rotation.w),
        f(tf.scale.x), f(tf.scale.y), f(tf.scale.z),
    )
}

/// Copies referenced textures into `people/<name>/textures/` and records the
/// self-contained path — the baked person must need NO make_human library at
/// runtime. Same-named files from different sources get a part prefix.
struct TextureBake {
    library_root: PathBuf,
    textures_dir: PathBuf,
    person: String,
    copied: std::collections::HashMap<String, PathBuf>,
}

impl TextureBake {
    fn bake(&mut self, part: &str, source_rel: &std::path::Path) -> Option<String> {
        let mut file = source_rel.file_name()?.to_string_lossy().into_owned();
        let source = self.library_root.join(source_rel);
        if self.copied.get(&file).is_some_and(|prev| prev != &source) {
            file = format!("{}_{file}", part.to_lowercase());
        }
        if let Some(prev) = self.copied.get(&file) {
            if prev != &source {
                panic!("texture name collision baking '{}': {file}", self.person);
            }
            // Already copied (parts share skins/materials).
        } else {
            fs::create_dir_all(&self.textures_dir).expect("create textures dir");
            fs::copy(&source, self.textures_dir.join(&file))
                .unwrap_or_else(|e| panic!("copy texture {source:?}: {e}"));
            self.copied.insert(file.clone(), source);
        }
        Some(format!("people/{}/textures/{file}", self.person))
    }
}

/// One part's material, serialized field-by-field: the scalar surface plus
/// every texture that has an asset PATH (a runtime-generated image without one
/// cannot be baked and warns). Texture files are copied out via [`TextureBake`].
fn material_bsn(
    mat: &StandardAuroraMaterial,
    server: &AssetServer,
    part: &str,
    tex: &mut TextureBake,
) -> String {
    let mut fields = String::new();
    let srgba = mat.base_color.to_srgba();
    let _ = write!(
        fields,
        "base_color: bevy_color::color::Color::Srgba(bevy_color::srgba::Srgba {{ red: {}, green: {}, blue: {}, alpha: {} }}), metallic: {}, perceptual_roughness: {}, ",
        f(srgba.red as f64), f(srgba.green as f64), f(srgba.blue as f64), f(srgba.alpha as f64),
        f(mat.metallic as f64), f(mat.perceptual_roughness as f64),
    );
    let mut texture = |field: &str, handle: &Option<Handle<Image>>| {
        let Some(handle) = handle else { return };
        match server
            .get_path(handle.id())
            .and_then(|path| tex.bake(part, path.path()))
        {
            Some(out_path) => {
                let _ = write!(fields, "{field}: \"{out_path}\", ");
            }
            None => warn!("bake: {part}.{field} has no asset path — dropped from the bake"),
        }
    };
    texture("base_color_texture", &mat.base_color_texture);
    texture("normal_map_texture", &mat.normal_map_texture);
    texture("metallic_roughness_texture", &mat.metallic_roughness_texture);
    texture("emissive_texture", &mat.emissive_texture);
    format!(
        "bevy_aurora::material::AuroraMaterial3d(bevy_aurora::material::StandardAuroraMaterial {{ {fields}}})"
    )
}

/// The bone tree, bind pose, recursively — names + f64 Transforms only; the
/// runtime resolver derives everything else from exactly these.
#[allow(clippy::too_many_arguments)]
fn bone_bsn(
    entity: Entity,
    names: &Query<&Name>,
    transforms: &Query<&Transform>,
    children: &Query<&Children>,
    depth: usize,
    out: &mut String,
) {
    let pad = "    ".repeat(depth);
    let name = names.get(entity).map(|n| n.as_str().to_owned()).unwrap_or_default();
    let tf = transforms.get(entity).copied().unwrap_or_default();
    let _ = write!(out, "{pad}bevy_ecs::name::Name(\"{name}\")\n{pad}{}\n", transform_bsn(&tf));
    let kids: Vec<Entity> = children.get(entity).map(|c| c.iter().collect()).unwrap_or_default();
    if kids.is_empty() {
        let _ = writeln!(out, "{pad},\n");
        return;
    }
    let _ = writeln!(out, "{pad}bevy_ecs::hierarchy::Children [");
    for kid in kids {
        bone_bsn(kid, names, transforms, children, depth + 1, out);
    }
    let _ = writeln!(out, "{pad}]\n{pad},\n");
}

/// `HumanComplete` → walk the generated entity tree and write the bake.
#[allow(clippy::too_many_arguments)]
fn bake_on_complete(
    complete: On<HumanComplete>,
    mut state: ResMut<BakeState>,
    subjects: Query<&Subject>,
    parents: Query<&ChildOf>,
    cluster_meshes: Res<Assets<ClusterMesh>>,
    materials: Res<Assets<StandardAuroraMaterial>>,
    server: Res<AssetServer>,
    names: Query<&Name>,
    transforms: Query<&Transform>,
    children: Query<&Children>,
    armatures: Query<Entity, With<make_human::components::Armature>>,
    parts: Query<(Entity, &RaytracingMesh3d, &AuroraMaterial3d, &SkinnedMesh, Option<&MHTag>)>,
    mut exit: MessageWriter<AppExit>,
) {
    let root = complete.entity;
    let Ok(subject) = subjects.get(root) else { return };
    let name = CAST[subject.0].name;
    let dir = state.out_root.join("people").join(name);
    let meshes_dir = dir.join("meshes");
    fs::create_dir_all(&meshes_dir).expect("create output dirs");
    let mut tex = TextureBake {
        library_root: state.library_root.clone(),
        textures_dir: dir.join("textures"),
        person: name.to_string(),
        copied: Default::default(),
    };

    // ---- the parts (the root carries the skin; children carry the rest) ----
    let mut part_nodes = String::new();
    let mut baked = 0usize;
    let mut joint_names: Option<Vec<String>> = None;
    // Outfit items all share the `Clothes` tag — suffix repeats or the
    // second item's mesh OVERWRITES the first's.
    let mut name_counts: std::collections::HashMap<String, usize> = Default::default();
    // Feet-at-origin contract: the skin mesh's lowest point lifts the whole
    // armature (bind poses are armature-LOCAL, so the armature transform is
    // what places the character — see aurora's `resolve_skin_joints`).
    let mut floor_lift = 0.0f32;
    for (entity, mesh3d, material, skin, tag) in &parts {
        // Several subjects generate concurrently: only THIS one's parts (the
        // root itself, or its direct children — `update_human`'s layout).
        let mine = entity == root
            || parents.get(entity).is_ok_and(|p| p.parent() == root);
        if !mine {
            continue;
        }
        let part_name = match tag {
            Some(MHTag::Collider) => continue,
            Some(tag) => format!("{tag}"),
            None if entity == root => "Skin".to_string(),
            None => continue,
        };
        let Some(cluster) = cluster_meshes.get(&mesh3d.0) else {
            warn!("bake: {part_name} cluster mesh not ready — skipped");
            continue;
        };
        if part_name == "Skin" {
            floor_lift = -(cluster.aabb.center[1] - cluster.aabb.half_extent[1]);
        }
        let n = name_counts.entry(part_name.clone()).or_insert(0);
        let file = if *n == 0 {
            format!("{}.cluster_mesh", part_name.to_lowercase())
        } else {
            format!("{}_{n}.cluster_mesh", part_name.to_lowercase())
        };
        *n += 1;
        let out_path = meshes_dir.join(&file);
        let writer = std::io::BufWriter::new(fs::File::create(&out_path).expect("create mesh file"));
        write_cluster_mesh_sync(cluster, writer).expect("write cluster mesh");
        baked += 1;

        // Joint names once (every part shares the palette).
        if joint_names.is_none() {
            joint_names = Some(
                skin.joints
                    .iter()
                    .map(|&j| names.get(j).map(|n| n.as_str().to_owned()).unwrap_or_default())
                    .collect(),
            );
        }
        let joints = joint_names.as_ref().unwrap();
        let joints_list = joints
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect::<Vec<_>>()
            .join(", ");

        let mat = materials.get(&material.0).cloned().unwrap_or_default();
        let _ = write!(
            part_nodes,
            "    bevy_ecs::name::Name(\"{part_name}\")\n    \
             bevy_aurora::bindings::types::RaytracingMesh3d(\"people/{name}/meshes/{file}\")\n    \
             {}\n    \
             bevy_aurora::bindings::types::SkinJointsByName([{joints_list}])\n    ,\n\n",
            material_bsn(&mat, &server, &part_name, &mut tex),
        );
    }

    // ---- the skeleton (bind pose) ----------------------------------------
    // The armature node is emitted by hand so the floor lift lands on ITS
    // transform; the bone tree below it stays exactly the generator's bind
    // pose.
    let mut armature_nodes = String::new();
    for armature in &armatures {
        // Only this bake's armature (direct child of the subject root).
        let Ok(kids) = children.get(root) else { continue };
        if !kids.iter().any(|k| k == armature) {
            continue;
        }
        let lift = Transform::from_translation(DVec3::new(0.0, f64::from(floor_lift), 0.0));
        let _ = write!(
            armature_nodes,
            "    bevy_ecs::name::Name(\"Armature\")\n    {}\n    bevy_ecs::hierarchy::Children [\n",
            transform_bsn(&lift),
        );
        let bones: Vec<Entity> = children.get(armature).map(|c| c.iter().collect()).unwrap_or_default();
        for bone in bones {
            bone_bsn(bone, &names, &transforms, &children, 2, &mut armature_nodes);
        }
        let _ = writeln!(armature_nodes, "    ]\n    ,\n");
    }

    // NO root-level `Name`: a Name on the scene root fails bsn resolution
    // ("Type is not reflectable / registered") — the spawning app names the
    // instance entity itself, exactly as the g1/mech prefabs expect. The
    // `BakedPerson` marker drives zero's spawn hydration (AnimationTargetId /
    // AnimatedBy / AnimationPlayer — the entity-reference components a text
    // bsn cannot carry), making the prefab identical to a generated human.
    let bsn = format!(
        "// Generated by bake_person. Do not edit by hand.\n\
         #{name}\n\
         zero::game::retarget::BakedPerson\n\
         bevy_ecs::hierarchy::Children [\n{armature_nodes}{part_nodes}]\n",
    );
    let bsn_path = dir.parent().unwrap().join(format!("{name}.bsn"));
    fs::write(&bsn_path, bsn).expect("write person bsn");
    info!("bake: '{name}' — {baked} part mesh(es); wrote {}", bsn_path.display());
    state.remaining -= 1;
    if state.remaining == 0 {
        exit.write(AppExit::Success);
    }
}
