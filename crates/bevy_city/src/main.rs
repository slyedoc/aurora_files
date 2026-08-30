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
    ray_render_plugin::RenderConfig,
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
    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,
}

#[derive(Resource)]
struct Timeout(f32);

/// Moving cars: set every frame by [`simulate_cars`].
#[derive(Component)]
pub struct Car {
    pub offset: Vec3,
    pub distance_traveled: f32,
    pub dir: f32,
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
    mut render_config: ResMut<RenderConfig>,
) {
    render_config.skydome = None;
    render_config.sky_color = Vec4::new(0.75, 0.85, 1.0, 0.0) * 1.5;
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

/// Same motion as upstream: each car rides its road's segment back and forth on its lane offset.
fn simulate_cars(
    args: Res<Args>,
    roads: Query<(&Road, &Children), Without<Car>>,
    mut cars: Query<(&mut Car, &mut Transform), Without<Road>>,
    time: Res<Time>,
) {
    if args.no_cars {
        return;
    }
    let speed = 1.5;
    for (road, children) in &roads {
        for child in children.iter() {
            let Ok((mut car, mut car_transform)) = cars.get_mut(child) else {
                continue;
            };
            car.distance_traveled += speed * time.delta_secs();
            let road_len = (road.end - road.start).length();
            if car.distance_traveled > road_len {
                car.distance_traveled = 0.0;
            }
            let direction = (road.end - road.start).normalize() * car.dir;
            let progress = car.distance_traveled / road_len;
            car_transform.translation = (road.start + car.offset) + direction * road_len * progress;
        }
    }
}

fn exit_on_timeout(time: Res<Time>, timeout: Res<Timeout>, mut exit: MessageWriter<AppExit>) {
    if time.elapsed_secs() >= timeout.0 {
        exit.write(AppExit::Success);
    }
}
