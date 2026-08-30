//! `bevy_city` — bevy's procedural Kenney city, ray traced on aurora.
//!
//! A port of `examples/large_scenes/bevy_city` (the fork): the same block layout, density noise,
//! seeds and car simulation, with every prop a single merged `.cluster_mesh` instance carrying an
//! `AuroraMaterial` (kit colormap / variation / flat colour). No LODs, no visibility ranges, no
//! loading screen: the acceleration structure is the culling structure, and meshes stream in.
//!
//!   scripts/fetch_kenney.sh && cargo run --release -p kenney_import
//!   cargo run --release -p bevy_city -- --size 30 --car-density 0.3
//!
//! `--size 100` is the 2M-entity configuration. F2 toggles aurora's dev panel, F1 the world
//! inspector, Space toggles accumulation; `--timeout` auto-exits (always on under CLAUDECODE).

use bevy::{
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    prelude::*,
};
use bevy_aurora::{
    dev_shaders::DevShaderPlugin, dev_ui::DevUIPlugin, ray_default_plugins::RayDefaultPlugins,
};
use clap::Parser;

use crate::{
    assets::{CityAssets, load_assets},
    generate::{CityStats, spawn_city},
};

mod assets;
mod generate;

#[derive(Parser, Resource, Clone)]
#[command(name = "bevy_city", about = "Kenney city, ray traced on aurora")]
struct Args {
    /// Generator seed.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// City size in blocks per side (each block is 5.5 x 4.0 units).
    #[arg(long, default_value_t = 30)]
    size: u32,
    /// Probability of a car on each road slot.
    #[arg(long, default_value_t = 0.3)]
    car_density: f32,
    /// Freeze the cars.
    #[arg(long)]
    no_cars: bool,
    /// DLSS Ray Reconstruction: off, dlaa, quality, balanced, performance, ultra-performance.
    #[arg(long, default_value = "off")]
    dlss: String,
    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,
}

#[derive(Resource)]
struct Timeout(f32);

/// A moving car: its lane baked as a segment at spawn (upstream keeps `Road` + lane offset
/// and looks the road up per car; here the per-frame update is a pure per-element function so
/// the whole table can be swept contiguously).
#[derive(Component)]
pub struct Car {
    /// Lane start in the road's frame (`road.start + lane offset`).
    pub origin: Vec3,
    /// Lane end minus start, already signed by direction of travel.
    pub travel: Vec3,
    pub road_len: f32,
    pub distance_traveled: f32,
}

impl Car {
    pub fn new(road: &Road, offset: Vec3, dir: f32, distance_traveled: f32) -> Self {
        let span = road.end - road.start;
        Self {
            origin: road.start + offset,
            travel: span * dir,
            road_len: span.length(),
            distance_traveled,
        }
    }
}

#[derive(Component)]
pub struct Road {
    pub start: Vec3,
    pub end: Vec3,
}

fn main() {
    // Asset root: explicit $BEVY_ASSET_ROOT > the cwd when it has an assets/ dir > this workspace.
    if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
        let root = if std::path::Path::new("assets").is_dir() {
            std::env::current_dir().expect("cwd")
        } else {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        };
        // SAFETY: single-threaded -- before App construction spawns anything.
        unsafe {
            std::env::set_var("BEVY_ASSET_ROOT", &root);
        }
    }
    let args = Args::parse();
    let timeout = args
        .timeout
        .or_else(|| std::env::var_os("CLAUDECODE").map(|_| 60.0));

    let mut app = App::new();
    app.add_plugins(RayDefaultPlugins.set(bevy::log::LogPlugin {
        filter: util::LOG_FILTER.into(),
        ..default()
    }));
    app.add_plugins((DevShaderPlugin, DevUIPlugin, FreeCameraPlugin::default()));
    app.insert_resource(args.clone());
    app.add_systems(Startup, (setup, load_assets, spawn).chain());
    app.add_systems(Update, simulate_cars);
    if let Some(seconds) = timeout {
        app.insert_resource(Timeout(seconds));
        app.add_systems(Update, exit_on_timeout);
    }
    app.run();
}

fn setup(
    mut commands: Commands,
    args: Res<Args>,
    mut windows: Query<&mut Window>,
    mut dev_ui: ResMut<bevy_aurora::dev_ui::DevUIState>,
) {
    // Physical-ish daylight: the importers write emitters in nits (Bistro's lamps are 20,000),
    // the procedural sky sits at ~8,000 and the camera at -13 EV (1/8192) to match.
    dev_ui.exposure_ev = -13.0;
    let dlss = bevy_aurora::dlss::AuroraDlss::parse(&args.dlss).unwrap_or_else(|| {
        warn!("unknown --dlss mode {:?}; staying off", args.dlss);
        default()
    });
    if let Ok(mut window) = windows.single_mut() {
        window.title = format!("bevy_city — {}x{} blocks", args.size, args.size);
    }
    let extent = args.size as f32 * 2.5;
    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60.0f32.to_radians(),
            ..default()
        }),
        FreeCamera::default(),
        dlss,
        Transform::from_xyz(-extent, 6.0, -extent).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y),
    ));
}

fn spawn(mut commands: Commands, assets: Res<CityAssets>, args: Res<Args>) {
    let mut stats = CityStats::default();
    spawn_city(
        &mut commands,
        &assets,
        args.seed,
        args.size,
        args.car_density,
        &mut stats,
    );
    info!(
        "bevy_city {}x{}: {} buildings, {} trees, {} cars, {} fences, {} paths, {} roads, {} ground tiles",
        args.size,
        args.size,
        stats.buildings,
        stats.trees,
        stats.cars,
        stats.fences,
        stats.paths,
        stats.roads,
        stats.ground_tile
    );
}

/// Same motion as upstream (each car loops along its lane), swept as contiguous table slices
/// in parallel: no per-car `Query::get`, no per-element change ticks. `Car` writes bypass
/// change detection (nothing reads them); `Transform` is stamped changed once per chunk, which
/// is what the GPU transform table's extraction listens for.
fn simulate_cars(args: Res<Args>, mut cars: Query<(&mut Car, &mut Transform)>, time: Res<Time>) {
    if args.no_cars {
        return;
    }
    let step = 1.5 * time.delta_secs();
    cars.contiguous_par_iter_mut()
        .expect("Car + Transform are table components")
        .for_each(|(mut cars, mut transforms)| {
            {
                let cars = cars.bypass_change_detection();
                let transforms = transforms.bypass_change_detection();
                for (car, transform) in cars.iter_mut().zip(transforms.iter_mut()) {
                    car.distance_traveled += step;
                    if car.distance_traveled > car.road_len {
                        car.distance_traveled = 0.0;
                    }
                    let progress = car.distance_traveled / car.road_len;
                    transform.translation = car.origin + car.travel * progress;
                }
            }
            transforms.mark_all_as_changed();
        });
}

fn exit_on_timeout(time: Res<Time>, timeout: Res<Timeout>, mut exit: MessageWriter<AppExit>) {
    if time.elapsed_secs() >= timeout.0 {
        exit.write(AppExit::Success);
    }
}
