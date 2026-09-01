//! `bsn` — general `.bsn` scene viewer on aurora: load any baked scene under hardware ray
//! tracing and fly a free camera through it.
//!
//!   cargo run --release -p bsn -- bistro/bistro.bsn
//!   bsn assets/lunarbase/KB3D_LNB_Lamp_A.bsn        (installed, from any repo)
//!
//! The asset root is `$BEVY_ASSET_ROOT` if set, else the current directory when it has an
//! `assets/` folder (so the installed binary works from any repo), else this workspace. Scene
//! paths resolve against that root, which is also what the `.bsn`'s own mesh/texture strings
//! assume. `--timeout` auto-exits (always on under CLAUDECODE). F2 toggles aurora's dev panel,
//! F1 the world inspector, Space toggles accumulation.

use bevy::{
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    prelude::*,
    scene::ScenePatchInstance,
};
use bevy_aurora::{
    auto_exposure::AuroraExposure,
    dev_shaders::DevShaderPlugin,
    dev_ui::DevUIPlugin,
    dlss::{DlssPlugin, RrPreset},
    ray_default_plugins::RayDefaultPlugins,
    sky::Sky,
    util::{ScreenshotExt, TimeoutAppExt},
};
use clap::Parser;
use util::park::HoverParkPlugin;

#[derive(Parser, Resource, Clone)]
#[command(name = "bsn", about = "Aurora .bsn scene viewer")]
struct Args {
    /// One or more `.bsn` scenes — asset-relative (`bistro/bistro.bsn`) or filesystem paths
    /// (shell globs like `assets/lunarbase/*` work: non-.bsn entries are skipped). Multiple
    /// scenes lay out in a grid.
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

    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,

    /// Equirectangular HDR sky (asset-relative path, e.g. `sky.hdr`); default is the
    /// procedural clear sky. Texels are scaled by `--sky-scale` to nits.
    #[arg(long)]
    sky: Option<String>,

    /// Nits per HDR texel unit for `--sky`.
    #[arg(long, default_value_t = 8000.0)]
    sky_scale: f32,

    /// Ray Reconstruction model preset: `default` (NVIDIA's pick, currently D), `d`, or `e`
    /// (latest transformer).
    #[arg(long, default_value = "default", value_parser = parse_preset)]
    dlss_preset: RrPreset,
}

fn parse_preset(s: &str) -> Result<RrPreset, String> {
    match s {
        "default" => Ok(RrPreset::Default),
        "d" => Ok(RrPreset::D),
        "e" => Ok(RrPreset::E),
        _ => Err("expected default|d|e".into()),
    }
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
    // Root rule: explicit $BEVY_ASSET_ROOT > the current repo (a cwd with an assets/ dir — the
    // installed-binary case) > this workspace (dev runs from crate dirs, where cwd has no assets/).
    if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
        let root = if std::path::Path::new("assets").is_dir() {
            std::env::current_dir().expect("cwd")
        } else {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        };
        // SAFETY: single-threaded — before App construction spawns anything.
        unsafe {
            std::env::set_var("BEVY_ASSET_ROOT", &root);
        }
    }
    let args = Args::parse();

    let mut app = App::new();
    app.add_plugins((
        RayDefaultPlugins
            .set(bevy::log::LogPlugin {
                filter: util::LOG_FILTER.into(),
                ..default()
            })
            .set(DlssPlugin {
                preset: args.dlss_preset,
            }),
        DevShaderPlugin,
        DevUIPlugin,
        FreeCameraPlugin::default(),
        HoverParkPlugin,
    ));    
    app.add_screenshot(KeyCode::F12);
    app.add_timeout_exit(args.timeout, 60.0);
    app.insert_resource(args.clone());
    app.add_systems(Startup, setup);
    app.run();
}

/// A scene arg as the asset server wants it: filesystem paths (absolute, cwd-relative, or
/// `assets/`-prefixed — what shell globs produce) reduce to asset-root-relative; non-`.bsn`
/// entries (globbed dirs) are dropped.
fn normalize(raw: &str) -> Option<String> {
    if !raw.ends_with(".bsn") {
        warn!("bsn: skipping non-.bsn arg {raw}");
        return None;
    }
    let root = std::path::PathBuf::from(std::env::var_os("BEVY_ASSET_ROOT").expect("set in main"))
        .join("assets");
    if let (Ok(abs), Ok(root)) = (std::fs::canonicalize(raw), std::fs::canonicalize(&root))
        && let Ok(rel) = abs.strip_prefix(&root)
    {
        return Some(rel.to_string_lossy().into_owned());
    }
    Some(raw.strip_prefix("assets/").unwrap_or(raw).to_string())
}

fn setup(
    mut commands: Commands,
    args: Res<Args>,
    asset_server: Res<AssetServer>,
    mut windows: Query<&mut Window>,
) {
    if let Some(path) = &args.sky {
        commands.insert_resource(Sky::Hdr {
            image: asset_server.load(path.clone()),
            scale: args.sky_scale,
        });
    }

    if let Ok(mut window) = windows.single_mut() {
        window.title = match args.scenes.as_slice() {
            [one] => format!("bsn — {one}"),
            many => format!("bsn — {} scenes", many.len()),
        };
    }

    // Each scene gets a root with a Transform: the patch parents its entities there. Multiple
    // scenes tile a square grid — the KB3D group `.bsn`s are origin-centered props, so a kit
    // browses as rows of pedestals.
    let scenes: Vec<String> = args.scenes.iter().filter_map(|s| normalize(s)).collect();
    let cols = (scenes.len() as f32).sqrt().ceil().max(1.0) as usize;
    for (i, scene) in scenes.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        commands.spawn((
            Name::new(scene.clone()),
            Transform::from_xyz(col as f32 * args.spacing, 0.0, row as f32 * args.spacing),
            Visibility::Visible,
            ScenePatchInstance(asset_server.load(scene)),
        ));
    }

    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60.0f32.to_radians(),
            ..default()
        }),
        FreeCamera::default(),
        // A locked look (tune it live on the camera in the F1 inspector; RR's input is
        // metered underneath either way).
        AuroraExposure::SUNLIGHT,
        Transform::from_translation(args.pos).looking_at(args.target, Vec3::Y),
    ));
}

