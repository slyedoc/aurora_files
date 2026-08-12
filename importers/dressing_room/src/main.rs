//! Dressing room (zero docs/todo.md §2): the make_human RUNTIME survives here
//! as the authoring tool — spawn a human over the raw/ library, dress it with
//! the crate's dropdown UI, then transcribe the combo into `bake_person`'s
//! `CAST`. Games only ever ship the baked output.
//!
//! ```text
//! cargo run -p dressing_room
//! ```

use std::path::PathBuf;

use bevy::{
    camera_controller::free_camera::FreeCamera,
    feathers::{dark_theme::create_dark_theme, theme::UiTheme},
    math::DVec3,
    prelude::*,
};
use bevy_aurora::{
    AuroraPlugins,
    geometry::asset::ClusterMesh,
    instance::RaytracingMesh3d,
    lights::AuroraDirectionLight,
    material::{AuroraMaterial3d, StandardAuroraMaterial},
    render::AuroraCamera,
    util::{screenshot::ScreenshotExt, timeout::TimeoutAppExt},
};
use make_human::prelude::*;

fn main() {
    let library_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../raw")
        .canonicalize()
        .expect("canonicalize raw/ library root");
    App::new()
        .add_timeout_exit(None, 600.0)
        .add_plugins((
            AuroraPlugins.build().set(bevy::asset::AssetPlugin {
                file_path: library_root.to_string_lossy().into_owned(),
                ..Default::default()
            }),
            MakeHumanPlugin,
        ))
        .insert_resource(UiTheme(create_dark_theme()))
        .add_screenshot(KeyCode::F12)
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<ClusterMesh>>,
    mut materials: ResMut<Assets<StandardAuroraMaterial>>,
) {
    use bevy::math::primitives::Plane3d;
    use bevy::mesh::{Mesh, Meshable};

    commands.spawn((
        Camera::default(),
        AuroraCamera::default(),
        FreeCamera::default(),
        Transform::from_translation(DVec3::new(0.0, 1.6, -3.0))
            .looking_at(DVec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    commands.spawn((
        AuroraDirectionLight { illuminance: 20_000.0, ..default() },
        Transform::from_translation(DVec3::new(4.0, 8.0, -4.0)).looking_at(DVec3::ZERO, Vec3::Y),
    ));
    let ground: Mesh = Plane3d::default().mesh().size(20.0, 20.0).build();
    commands.spawn((
        Name::new("Ground"),
        RaytracingMesh3d(meshes.add(ClusterMesh::try_from(&ground).expect("bake ground"))),
        AuroraMaterial3d(materials.add(StandardAuroraMaterial {
            base_color: Color::srgb(0.3, 0.3, 0.35),
            perceptual_roughness: 0.9,
            ..default()
        })),
        Transform::default(),
    ));

    // The subject: start from the bake's default player look; every part is
    // swappable live through the crate's dropdown panels.
    commands.spawn((
        Name::new("Subject"),
        Human,
        Rig::GameEngine,
        SkinMesh::MaleGeneric,
        SkinMaterial::YoungAfricanMale,
        Eyes::HighPolyBrown,
        Hair::Afro01,
        Eyebrows::Eyebrow001,
        Eyelashes::Eyelashes01,
        Teeth::TeethBase,
        Tongue::Tongue01,
        Outfit(vec![Clothing::DonitzMonkRobe]),
        Transform::default(),
    ));
}
