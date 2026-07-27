//! Headless validation that the TEXTURE-FREE hoverboard `.bsn` parses and resolves — no GPU.
//!
//! The thing actually under test is the scalar factor path: `base_color` is a `Color` ENUM, so the
//! `.bsn` carries a tuple-variant literal (`Color::LinearRgba(LinearRgba { .. })`) where every
//! other field is a plain struct. A wrong type path there bakes fine and fails only at LOAD, so
//! this asserts the factors survive the round trip with the values the exporter wrote.
//!
//!   cargo run --release -p hoverboard_import --bin load_test        # asset root = ./assets

use bevy::asset::AssetApp;
use bevy::color::{Color, LinearRgba};
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
        .unwrap_or_else(|| "hoverboard/hoverboard.bsn".to_string());

    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::log::LogPlugin::default(),
        AssetPlugin { file_path: asset_root, ..default() },
        TransformPlugin,
        ScenePlugin,
    ));

    app.register_type::<RaytracingMesh3d>()
        .register_type::<SolariMaterial3d>()
        .register_type::<StandardSolariMaterial>()
        .register_type::<AlphaMode>()
        .register_type::<LinearRgba>()
        .register_type::<Color>()
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
    let cluster_meshes_loaded = app.world().resource::<Assets<ClusterMesh>>().len();

    let mats: Vec<StandardSolariMaterial> = app
        .world_mut()
        .query::<&SolariMaterial3d>()
        .iter(app.world())
        .filter_map(|m| app.world().resource::<Assets<StandardSolariMaterial>>().get(&m.0))
        .cloned()
        .collect();

    // A material left at every default is the failure signature: it means the factors never made
    // it across and the prop would render as white plastic.
    let non_default_base = mats
        .iter()
        .filter(|m| m.base_color != Color::WHITE)
        .count();
    let non_default_rough = mats.iter().filter(|m| m.perceptual_roughness != 0.5).count();
    let metallic = mats.iter().filter(|m| m.metallic > 0.0).count();
    let emissive = mats.iter().filter(|m| m.emissive != LinearRgba::BLACK).count();
    let any_texture = mats.iter().any(|m| {
        m.base_color_texture.is_some()
            || m.metallic_roughness_texture.is_some()
            || m.normal_map_texture.is_some()
            || m.emissive_texture.is_some()
    });
    let peak_emissive = mats
        .iter()
        .map(|m| m.emissive.red.max(m.emissive.green).max(m.emissive.blue))
        .fold(0.0f32, f32::max);

    println!("named entities spawned:             {named}");
    println!("RaytracingMesh3d entities:          {meshes}");
    println!("ClusterMesh assets loaded:          {cluster_meshes_loaded} / {meshes}");
    println!("materials resolved:                 {}", mats.len());
    println!("  base_color != WHITE:              {non_default_base}");
    println!("  perceptual_roughness != 0.5:      {non_default_rough}");
    println!("  metallic > 0:                     {metallic}");
    println!("  emissive != BLACK:                {emissive}  (peak radiance {peak_emissive})");
    println!("ANY texture handle set:             {any_texture}   <- must be false");

    let ok = named > 0 && meshes > 0 && cluster_meshes_loaded == meshes && non_default_base > 0
        && emissive > 0 && !any_texture;
    println!("\n{}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
}
