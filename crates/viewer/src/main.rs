//! General `.bsn` scene viewer on aurora — load any importer-baked scene under
//! full ray tracing and fly a free camera through it.
//!
//!   cargo run --release -p aurora_view -- bistro/bistro.bsn
//!   cargo run --release -p aurora_view -- san_miguel/SanMiguel.bsn --pos -8,2,4
//!
//! Scene paths are relative to this workspace's `assets/` root (the bake
//! output), which is also what the `.bsn`'s own mesh/texture strings assume.
//! `--timeout` auto-exits (always on under CLAUDECODE); F12 screenshots.
//!
//! This is the aurora rewrite of the old 1000-line fork viewer: the debug
//! panels rode the wgpu inspector stack and are gone; what remains is the part
//! that earns its keep — load, light, fly, screenshot.

use bevy::{
    camera_controller::free_camera::FreeCamera,
    prelude::*,
    scene::ScenePatchInstance,
};
use bevy_aurora::{
    AuroraPlugins,
    dlss::AuroraDlss,
    lights::AuroraDirectionLight,
    render::{AuroraCamera, AuroraSky},
    util::{screenshot::ScreenshotExt, timeout::TimeoutAppExt},
};
use clap::Parser;
use util::{
    park::HoverParkPlugin,
    sun::{Sun, SunPlugin, SunSettings},
};

#[derive(Parser, Resource, Clone)]
#[command(name = "aurora_view", about = "Aurora .bsn scene viewer")]
struct Args {
    /// Scene path under `assets/` (e.g. `bistro/bistro.bsn`).
    scene: String,

    /// Camera start position, `x,y,z`.
    #[arg(long, default_value = "-10.0,2.0,-2.0", value_parser = parse_vec3)]
    pos: Vec3,

    /// Camera look-at target, `x,y,z`.
    #[arg(long, default_value = "0.0,2.0,0.0", value_parser = parse_vec3)]
    target: Vec3,

    /// Sun azimuth/elevation in degrees.
    #[arg(long, default_value_t = -23.0)]
    azimuth: f32,
    #[arg(long, default_value_t = 63.0)]
    elevation: f32,

    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,
}

fn parse_vec3(s: &str) -> Result<Vec3, String> {
    let parts: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse().map_err(|e| format!("{e}")))
        .collect::<Result<_, _>>()?;
    match parts[..] {
        [x, y, z] => Ok(Vec3::new(x, y, z)),
        _ => Err("expected x,y,z".into()),
    }
}

fn main() {
    // Scene paths are workspace-assets-relative; bevy's asset root otherwise
    // resolves to this crate's own (nonexistent) assets/. Overridable as ever.
    if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
        // SAFETY: single-threaded — before App construction spawns anything.
        unsafe {
            std::env::set_var(
                "BEVY_ASSET_ROOT",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
            );
        }
    }
    let args = Args::parse();
    App::new()
        .add_plugins(AuroraPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("aurora_view — {}", args.scene),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((SunPlugin, HoverParkPlugin))
        .insert_resource(SunSettings {
            enabled: true,
            azimuth: args.azimuth,
            elevation: args.elevation,
        })
        .insert_resource(args.clone())
        .add_timeout_exit(args.timeout, 60.0)
        .add_screenshot(KeyCode::F12)
        .add_systems(Startup, setup)
        .run();
}

fn setup(mut commands: Commands, args: Res<Args>, asset_server: Res<AssetServer>) {
    // Root with a Transform: the scene patch parents its entities here, and
    // aurora's transform graph composes children against this basis.
    commands.spawn((
        Name::new(args.scene.clone()),
        Transform::default(),
        Visibility::Visible,
        ScenePatchInstance(asset_server.load(&args.scene)),
    ));

    commands.spawn((
        Name::new("sun"),
        Sun,
        AuroraDirectionLight {
            illuminance: 20_000.0,
            ..default()
        },
        Transform::default(),
    ));

    commands.spawn((
        Name::new("camera"),
        AuroraCamera::default(),
        AuroraDlss::Dlaa,
        AuroraSky::PROCEDURAL,
        Camera::default(),
        FreeCamera::default(),
        Transform::from_translation(args.pos.as_dvec3())
            .looking_at(args.target.as_dvec3(), Vec3::Y),
    ));
}
