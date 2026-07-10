//! Headless validation that the Zero-Day HIERARCHY `.bsn` parses, spawns the nested tree, and
//! resolves its handles — without a GPU. Extends the San Miguel harness with the types the animated
//! scene adds: `Name` (+ the `String -> HashedStr` conversion), nested `Children`, and `LinearRgba`
//! (emissive). Reports entity/mesh counts, tree depth, and how many materials carry emissive — a
//! stock `TransformPlugin` here propagates through the empty parents on the CPU, so the reported
//! world translations also sanity-check the local-transform hierarchy math.
//!
//!   cargo run --release -p zeroday_import --bin load_test        # asset root = ./assets

use bevy::asset::AssetApp;
use bevy::color::LinearRgba;
use bevy::ecs::name::HashedStr;
use bevy::prelude::*;
use bevy::scene::{ScenePatch, ScenePatchInstance, ScenePlugin};
use bevy::solari::geometry::{ClusterMesh, ClusterMeshLoader};
use bevy::solari::material::StandardSolariMaterial;
use bevy::solari::prelude::{RaytracingMesh3d, SolariMaterial3d};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let asset_root = args.first().cloned().unwrap_or_else(|| "assets".to_string());
    let scene_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "zeroday/MeasureOne.bsn".to_string());

    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin { file_path: asset_root, ..default() },
        TransformPlugin,
        ScenePlugin,
    ));

    // Types the `.bsn` names (the GPU app gets these from SolariPlugin/SolariTransformPlugin).
    app.register_type::<RaytracingMesh3d>()
        .register_type::<SolariMaterial3d>()
        .register_type::<StandardSolariMaterial>()
        .register_type::<AlphaMode>()
        .register_type::<LinearRgba>()
        // Every node carries a `Name` — register it + the `String -> HashedStr` conversion the
        // loader uses to build `Name(HashedStr)` from a string literal.
        .register_type::<Name>()
        .register_type::<HashedStr>()
        .register_type_conversion::<String, HashedStr, _>(|s| Ok(s.into()))
        .init_asset::<StandardSolariMaterial>()
        .register_asset_reflect::<StandardSolariMaterial>()
        .init_asset::<ClusterMesh>()
        .init_asset_loader::<ClusterMeshLoader>()
        .register_asset_reflect::<ClusterMesh>()
        .init_asset::<Image>()
        .register_asset_reflect::<Image>();

    let handle: Handle<ScenePatch> = app.world().resource::<AssetServer>().load(scene_path);
    app.world_mut().spawn(ScenePatchInstance(handle.clone()));

    // Pump until the scene spawns (entities with a Name materialize).
    let mut named = 0usize;
    for _ in 0..5000 {
        app.update();
        named = app.world_mut().query_filtered::<(), With<Name>>().iter(app.world()).count();
        if named > 0 {
            break;
        }
    }
    for _ in 0..2000 {
        app.update();
    }

    let meshes = app
        .world_mut()
        .query_filtered::<(), With<RaytracingMesh3d>>()
        .iter(app.world())
        .count();
    let with_children = app
        .world_mut()
        .query_filtered::<(), With<Children>>()
        .iter(app.world())
        .count();

    // Max tree depth (walk ChildOf up from each leaf) — confirms the nested Children survived spawn.
    let mut max_depth = 0usize;
    {
        let mut q = app.world_mut().query::<(Entity, Option<&ChildOf>)>();
        let world = app.world();
        let parents: std::collections::HashMap<Entity, Option<Entity>> =
            q.iter(world).map(|(e, c)| (e, c.map(|c| c.parent()))).collect();
        for &e in parents.keys() {
            let (mut d, mut cur) = (0usize, e);
            while let Some(Some(p)) = parents.get(&cur) {
                d += 1;
                cur = *p;
                if d > 10_000 {
                    break;
                }
            }
            max_depth = max_depth.max(d);
        }
    }

    let cluster_meshes_loaded = app.world().resource::<Assets<ClusterMesh>>().len();
    let materials = app.world_mut().query::<&SolariMaterial3d>().iter(app.world()).count();
    let resolved_base = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .filter_map(|m| app.world().resource::<Assets<StandardSolariMaterial>>().get(&m.0))
        .any(|mat| mat.base_color_texture.is_some());
    let emissive = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .filter_map(|m| app.world().resource::<Assets<StandardSolariMaterial>>().get(&m.0))
        .filter(|mat| mat.emissive != LinearRgba::BLACK || mat.emissive_texture.is_some())
        .count();
    let sample_world: Vec<_> = app
        .world_mut()
        .query::<(&GlobalTransform, &RaytracingMesh3d)>()
        .iter(app.world())
        .take(3)
        .map(|(gt, _)| gt.translation())
        .collect();

    println!("named entities spawned:             {named}");
    println!("RaytracingMesh3d entities:          {meshes}");
    println!("entities with Children (parents):   {with_children}");
    println!("max hierarchy depth:                {max_depth}");
    println!("ClusterMesh assets loaded:          {cluster_meshes_loaded} / {meshes}");
    println!("SolariMaterial3d components:        {materials}");
    println!("a base_color_texture resolved:      {resolved_base}");
    println!("materials with emissive:            {emissive}");
    println!("sample entity world translations:   {sample_world:?}");
}
