//! Headless validation that `SanMiguel.bsn` parses, spawns, and resolves its handles — without
//! the GPU app. Registers just the types/assets the scene references, loads the `.bsn` through the
//! dynamic loader, spawns it, and reports how many `RaytracingMesh3d` entities materialized and
//! whether an inline `SolariMaterial` resolved.
//!
//!   cargo run --release --bin load_test     # run from solari_files (asset root = ./assets)

use bevy::asset::AssetApp;
use bevy::prelude::*;
use bevy::scene::{ScenePatch, ScenePatchInstance, ScenePlugin};
use bevy::solari::geometry::{ClusterMesh, ClusterMeshLoader};
use bevy::solari::material::SolariMaterial;
use bevy::solari::prelude::{RaytracingMesh3d, SolariMaterial3d};

fn main() {
    // Optional args: <asset_root> <scene_path_relative_to_root>
    // Defaults to the in-tree assets/san_miguel/SanMiguel.bsn.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Default points at the workspace-root `assets/` (this crate now lives under `importers/`, so
    // bevy's CARGO_MANIFEST_DIR-relative asset root needs the `../../` hop). Override with arg 1.
    let asset_root = args.first().cloned().unwrap_or_else(|| "../../assets".to_string());
    let scene_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "san_miguel/SanMiguel.bsn".to_string());

    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin {
            file_path: asset_root,
            ..default()
        },
        TransformPlugin,
        ScenePlugin,
    ));

    // Register exactly what `SanMiguel.bsn` names (normally done by SolariPlugin, which needs a GPU).
    app.register_type::<RaytracingMesh3d>()
        .register_type::<SolariMaterial3d>()
        .register_type::<SolariMaterial>()
        // `SolariMaterial.alpha_mode`'s enum, named in the `.bsn` as
        // `bevy_material::alpha::AlphaMode::Mask(..)` — must be registered to resolve (the GPU app
        // gets this from `SolariMaterialPlugin`; this headless harness registers types by hand).
        .register_type::<AlphaMode>()
        .init_asset::<SolariMaterial>()
        .register_asset_reflect::<SolariMaterial>()
        .init_asset::<ClusterMesh>()
        .init_asset_loader::<ClusterMeshLoader>()
        .register_asset_reflect::<ClusterMesh>()
        .init_asset::<Image>()
        .register_asset_reflect::<Image>();

    let handle: Handle<ScenePatch> = app
        .world()
        .resource::<AssetServer>()
        .load(scene_path);
    app.world_mut().spawn(ScenePatchInstance(handle.clone()));

    // Pump the app until the scene loads + spawns (or we give up).
    let mut meshes = 0usize;
    for _ in 0..5000 {
        app.update();
        meshes = app
            .world_mut()
            .query_filtered::<(), With<RaytracingMesh3d>>()
            .iter(app.world())
            .count();
        if meshes > 0 {
            break;
        }
    }

    // Pump more so the referenced `.cluster_mesh` assets actually load (not just resolve handles).
    for _ in 0..3000 {
        app.update();
    }
    let cluster_meshes_loaded = app.world().resource::<Assets<ClusterMesh>>().len();
    // OMM round-trip check: how many loaded `.cluster_mesh` carry a baked opacity
    // micro-map that survived v3 (de)serialization, and total array bytes.
    let (omm_meshes, omm_bytes) = app
        .world()
        .resource::<Assets<ClusterMesh>>()
        .iter()
        .filter(|(_, cm)| cm.has_opacity_micromap())
        .fold((0usize, 0usize), |(n, b), (_, cm)| {
            (n + 1, b + cm.omm_array_data().len())
        });
    println!("ClusterMesh with OMM (v3 round-trip): {omm_meshes} ({omm_bytes} array bytes)");
    for (i, (_, cm)) in app
        .world()
        .resource::<Assets<ClusterMesh>>()
        .iter()
        .take(5)
        .enumerate()
    {
        let aabb = cm.aabb();
        println!(
            "  cluster {i}: aabb center={:?} half_extent={:?}",
            &aabb.center[..3],
            &aabb.half_extent[..3],
        );
    }
    let sample_world: Vec<Vec3> = app
        .world_mut()
        .query::<(&GlobalTransform, &RaytracingMesh3d)>()
        .iter(app.world())
        .take(3)
        .map(|(gt, _)| gt.translation())
        .collect();
    println!("ClusterMesh assets loaded:          {cluster_meshes_loaded} / {meshes}");
    println!("sample entity world translations:   {sample_world:?}");

    let materials = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .count();
    // Reduce to a `bool`/count immediately so neither holds a borrow of `app` across the next
    // `world_mut()` query (the borrow checker rejects an overlapping `&app` + `&mut app`).
    let resolved_material = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .filter_map(|m| app.world().resource::<Assets<SolariMaterial>>().get(&m.0))
        .any(|mat| mat.base_color_texture.is_some());

    let masked = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .filter_map(|m| app.world().resource::<Assets<SolariMaterial>>().get(&m.0))
        .filter(|mat| matches!(mat.alpha_mode, AlphaMode::Mask(_)))
        .count();

    println!("RaytracingMesh3d entities spawned: {meshes}");
    println!("SolariMaterial3d components:        {materials}");
    println!("a material with a base_color_texture resolved: {resolved_material}");
    println!("materials with AlphaMode::Mask (cutouts):      {masked}");
    if meshes == 0 {
        eprintln!("FAILED: nothing spawned (check the log above for a parse/resolve error)");
        std::process::exit(1);
    }
    println!("OK: SanMiguel.bsn parsed, spawned, and resolved handles");
}
