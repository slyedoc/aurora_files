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
    /// One or more `.bsn` scenes — asset-relative (`bistro/bistro.bsn`) or
    /// filesystem paths (shell globs like `assets/lunarbase/*` work: non-.bsn
    /// entries are skipped). Multiple scenes lay out in a grid.
    #[arg(required = true)]
    scenes: Vec<String>,

    /// Grid spacing between scenes (m).
    #[arg(long, default_value_t = 20.0)]
    spacing: f32,

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
                title: match args.scenes.as_slice() {
                    [one] => format!("aurora_view — {one}"),
                    many => format!("aurora_view — {} scenes", many.len()),
                },
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

/// A scene arg as the asset server wants it: filesystem paths (absolute,
/// cwd-relative, or `assets/`-prefixed — what shell globs produce) reduce to
/// asset-root-relative; non-`.bsn` entries (globbed dirs) are dropped.
fn normalize(raw: &str) -> Option<String> {
    if !raw.ends_with(".bsn") {
        warn!("aurora_view: skipping non-.bsn arg {raw}");
        return None;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    if let (Ok(abs), Ok(root)) = (std::fs::canonicalize(raw), std::fs::canonicalize(&root))
        && let Ok(rel) = abs.strip_prefix(&root)
    {
        return Some(rel.to_string_lossy().into_owned());
    }
    Some(raw.strip_prefix("assets/").unwrap_or(raw).to_string())
}

fn setup(mut commands: Commands, args: Res<Args>, asset_server: Res<AssetServer>) {
    // Each scene gets a root with a Transform: the patch parents its entities
    // there, and aurora's transform graph composes children against that
    // basis. Multiple scenes tile a square grid — the KB3D group `.bsn`s are
    // origin-centered props, so a kit browses as rows of pedestals.
    let scenes: Vec<String> = args.scenes.iter().filter_map(|s| normalize(s)).collect();
    let cols = (scenes.len() as f32).sqrt().ceil().max(1.0) as usize;
    for (i, scene) in scenes.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        commands.spawn((
            Name::new(scene.clone()),
            Transform::from_xyz(
                col as f64 * args.spacing as f64,
                0.0,
                row as f64 * args.spacing as f64,
            ),
            Visibility::Visible,
            ScenePatchInstance(asset_server.load(scene)),
        ));
    }

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
