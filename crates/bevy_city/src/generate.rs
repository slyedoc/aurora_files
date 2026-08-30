//! The block generator, as upstream: a grid of crossroads, each block's density from simplex
//! noise (forest / suburban / commercial / skyscrapers), roads with lanes of cars.
//!
//! Every prop is one entity: parent `Transform` + child `Mesh3d` for buildings, trees and cars
//! (the same two-level shape upstream uses, so the hierarchy is exercised), a single mesh
//! entity for roads, fences, paths and ground tiles.

use bevy::prelude::*;
use bevy_aurora::material::AuroraMaterial3d;
use noise::{NoiseFn, OpenSimplex};
use rand::{RngExt, SeedableRng, rngs::SmallRng};

use crate::{
    Car, Road,
    assets::{Buildings, CityAssets},
};

#[derive(Component)]
pub struct CityRoot;

#[derive(Default)]
pub struct CityStats {
    pub buildings: u32,
    pub trees: u32,
    pub cars: u32,
    pub fences: u32,
    pub paths: u32,
    pub roads: u32,
    pub ground_tile: u32,
}

/// Each city block is 5.5 x 4.0 units, everything relative to its crossroad.
pub fn spawn_city(
    commands: &mut Commands,
    assets: &CityAssets,
    seed: u64,
    size: u32,
    car_density: f32,
    stats: &mut CityStats,
) {
    let mut rng = SmallRng::seed_from_u64(seed);
    let noise = OpenSimplex::new(rng.random());
    let noise_scale = 0.025;

    commands
        .spawn((
            CityRoot,
            Name::new("city"),
            Transform::default(),
            Visibility::default(),
        ))
        .with_children(|commands| {
            let half_size = size as i32 / 2;
            for x in -half_size..half_size {
                for z in -half_size..half_size {
                    let offset = Vec3::new(x as f32 * 5.5, 0.0, z as f32 * 4.0);
                    spawn_roads_and_cars(commands, assets, &mut rng, offset, car_density, stats);

                    let density = noise.get([
                        offset.x as f64 * noise_scale,
                        offset.z as f64 * noise_scale,
                        0.0,
                    ]) * 0.5
                        + 0.5;
                    let forest = 0.45;
                    let low_density = 0.6;
                    let medium_density = 0.7;

                    let ground_tile_scale = Vec3::new(4.5, 1.0, 3.0);
                    commands.spawn((
                        Mesh3d(assets.ground_tile.clone()),
                        AuroraMaterial3d(if density < low_density {
                            assets.grass_material.clone()
                        } else {
                            assets.road_material.clone()
                        }),
                        Transform::from_translation(
                            Vec3::new(0.5, -0.5005, 0.5) + ground_tile_scale / 2.0 + offset,
                        )
                        .with_scale(ground_tile_scale),
                    ));
                    stats.ground_tile += 1;

                    if density < forest {
                        spawn_forest(commands, assets, &mut rng, offset, stats);
                    } else if density < low_density {
                        spawn_low_density(commands, assets, &mut rng, offset, stats);
                    } else if density < medium_density {
                        spawn_medium_density(commands, assets, &mut rng, offset, stats);
                    } else {
                        spawn_high_density(commands, assets, &mut rng, offset, stats);
                    }
                }
            }
        });
}

fn spawn_prop(
    commands: &mut ChildSpawnerCommands,
    mesh: &Handle<Mesh>,
    material: &Handle<bevy_aurora::material::AuroraMaterial>,
    transform: Transform,
) {
    commands.spawn((
        Mesh3d(mesh.clone()),
        AuroraMaterial3d(material.clone()),
        transform,
    ));
}

/// Parent transform + child mesh, as upstream spawns buildings, trees and cars.
fn spawn_two_level(
    commands: &mut ChildSpawnerCommands,
    parts: (Mesh3d, AuroraMaterial3d),
    transform: Transform,
) {
    commands
        .spawn((transform, Visibility::default()))
        .with_child((parts.0, parts.1, Transform::default()));
}

fn spawn_building<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    buildings: &Buildings,
    rng: &mut R,
    transform: Transform,
) {
    spawn_two_level(commands, buildings.random(rng), transform);
}

fn spawn_tree(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    mesh: &Handle<Mesh>,
    transform: Transform,
) {
    spawn_two_level(
        commands,
        (
            Mesh3d(mesh.clone()),
            AuroraMaterial3d(assets.suburban_material.clone()),
        ),
        transform,
    );
}

fn spawn_car<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    transform: Transform,
    car: Car,
) {
    let (mesh, material) = assets.random_car(rng);
    commands
        .spawn((transform, Visibility::default(), car))
        .with_child((mesh, material, Transform::default()));
}

fn spawn_roads_and_cars<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    offset: Vec3,
    max_car_density: f32,
    stats: &mut CityStats,
) {
    spawn_prop(
        commands,
        &assets.crossroad,
        &assets.road_material,
        Transform::from_translation(offset),
    );
    stats.roads += 1;

    // Roads are one stretched segment each; the magic numbers are upstream's hand-tuned layout.
    let car_count = 9;
    commands
        .spawn((
            Transform::from_translation(offset),
            Visibility::default(),
            Road {
                start: Vec3::new(0.75, 0.0, 0.0),
                end: Vec3::new(0.75 + (0.5 * car_count as f32), 0.0, 0.0),
            },
        ))
        .with_children(|commands| {
            spawn_prop(
                commands,
                &assets.road_straight,
                &assets.road_material,
                Transform::from_translation(Vec3::new(2.75, 0.0, 0.0))
                    .with_scale(Vec3::new(4.5, 1.0, 1.0)),
            );
            stats.roads += 1;
            for i in 0..car_count {
                let car_pos = Vec3::new(0.0, 0.0, 0.75 + i as f32 * 0.5);
                if rng.random::<f32>() < max_car_density {
                    spawn_car(
                        commands,
                        assets,
                        rng,
                        Transform::from_translation(car_pos + Vec3::new(0.0, 0.0, -0.15))
                            .with_scale(Vec3::splat(0.15))
                            .with_rotation(Quat::from_axis_angle(
                                Vec3::Y,
                                3.0 * std::f32::consts::FRAC_PI_2,
                            )),
                        Car {
                            distance_traveled: i as f32 * 0.5,
                            dir: -1.0,
                            offset: Vec3::new(4.25, 0.0, -0.15),
                        },
                    );
                    stats.cars += 1;
                }
                if rng.random::<f32>() < max_car_density {
                    spawn_car(
                        commands,
                        assets,
                        rng,
                        Transform::from_translation(car_pos + Vec3::new(0.0, 0.0, 0.15))
                            .with_scale(Vec3::splat(0.15))
                            .with_rotation(Quat::from_axis_angle(
                                Vec3::Y,
                                std::f32::consts::FRAC_PI_2,
                            )),
                        Car {
                            distance_traveled: i as f32 * 0.5,
                            dir: 1.0,
                            offset: Vec3::new(-0.25, 0.0, 0.15),
                        },
                    );
                    stats.cars += 1;
                }
            }
        });

    let car_count = 6;
    commands
        .spawn((
            Transform::from_translation(offset),
            Visibility::default(),
            Road {
                start: Vec3::new(0.0, 0.0, 0.75),
                end: Vec3::new(0.0, 0.0, 0.75 + (0.5 * car_count as f32)),
            },
        ))
        .with_children(|commands| {
            spawn_prop(
                commands,
                &assets.road_straight,
                &assets.road_material,
                Transform::from_translation(Vec3::new(0.0, 0.0, 2.0))
                    .with_scale(Vec3::new(3.0, 1.0, 1.0))
                    .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2)),
            );
            stats.roads += 1;
            for i in 0..car_count {
                let car_pos = Vec3::new(0.0, 0.0, 0.75 + i as f32 * 0.5);
                if rng.random::<f32>() < max_car_density {
                    spawn_car(
                        commands,
                        assets,
                        rng,
                        Transform::from_translation(car_pos + Vec3::new(0.15, 0.0, 0.0))
                            .with_scale(Vec3::splat(0.15)),
                        Car {
                            distance_traveled: i as f32 * 0.5,
                            dir: 1.0,
                            offset: Vec3::new(-0.15, 0.0, -0.25),
                        },
                    );
                    stats.cars += 1;
                }
                if rng.random::<f32>() < max_car_density {
                    spawn_car(
                        commands,
                        assets,
                        rng,
                        Transform::from_translation(car_pos + Vec3::new(-0.15, 0.0, 0.0))
                            .with_scale(Vec3::splat(0.15))
                            .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::PI)),
                        Car {
                            distance_traveled: i as f32 * 0.5,
                            dir: -1.0,
                            offset: Vec3::new(0.15, 0.0, 2.75),
                        },
                    );
                    stats.cars += 1;
                }
            }
        });
}

fn spawn_low_density<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    offset: Vec3,
    stats: &mut CityStats,
) {
    for x in 1..=2 {
        let x_factor = 1.8;
        spawn_building(
            commands,
            &assets.low_density,
            rng,
            Transform::from_translation(Vec3::new(x as f32 * x_factor, 0.0, 1.25) + offset),
        );
        spawn_building(
            commands,
            &assets.low_density,
            rng,
            Transform::from_translation(Vec3::new(x as f32 * x_factor, 0.0, 2.75) + offset)
                .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::PI)),
        );
        stats.buildings += 2;
    }
    for i in 0..=6 {
        spawn_prop(
            commands,
            &assets.fence,
            &assets.suburban_material,
            Transform::from_translation(Vec3::new(2.75, 0.0, 0.75 + i as f32 * 0.4) + offset)
                .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2)),
        );
        stats.fences += 1;
    }
    for z in 0..=8 {
        spawn_tree(
            commands,
            assets,
            &assets.tree_small,
            Transform::from_translation(Vec3::new(0.75, 0.0, 0.75 + z as f32 * 0.3) + offset),
        );
        spawn_tree(
            commands,
            assets,
            &assets.tree_small,
            Transform::from_translation(Vec3::new(4.75, 0.0, 0.75 + z as f32 * 0.3) + offset),
        );
        stats.trees += 2;
    }
}

fn spawn_medium_density<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    offset: Vec3,
    stats: &mut CityStats,
) {
    let x_factor = 0.9;
    for x in 1..=5 {
        spawn_building(
            commands,
            &assets.medium_density,
            rng,
            Transform::from_translation(Vec3::new(x as f32 * x_factor, 0.0, 1.0) + offset),
        );
        stats.buildings += 1;
        for tree_x in 0..=1 {
            let tree_x = tree_x as f32 * 0.5;
            if x == 5 && tree_x == 0.5 {
                break;
            }
            for z in [1.75, 2.25] {
                spawn_tree(
                    commands,
                    assets,
                    &assets.tree_large,
                    Transform::from_translation(
                        Vec3::new(tree_x + x as f32 * x_factor, 0.0, z) + offset,
                    ),
                );
            }
            stats.trees += 2;
        }
        spawn_building(
            commands,
            &assets.medium_density,
            rng,
            Transform::from_translation(Vec3::new(x as f32 * x_factor, 0.0, 3.0) + offset)
                .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::PI)),
        );
        stats.buildings += 1;
    }
    for x in 0..=10 {
        spawn_prop(
            commands,
            &assets.path_stones_long,
            &assets.suburban_material,
            Transform::from_translation(Vec3::new(0.75 + (x as f32 * 0.4), 0.02, 2.0) + offset)
                .with_scale(Vec3::new(1.0, 2.0, 1.0))
                .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2)),
        );
        stats.paths += 1;
        for z in [1.85, 2.15] {
            spawn_prop(
                commands,
                &assets.fence,
                &assets.suburban_material,
                Transform::from_translation(Vec3::new(0.75 + (x as f32 * 0.4), 0.02, z) + offset),
            );
        }
        stats.fences += 2;
    }
}

fn spawn_high_density<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    offset: Vec3,
    stats: &mut CityStats,
) {
    for x in 0..3 {
        let x = x as f32;
        spawn_building(
            commands,
            &assets.high_density,
            rng,
            Transform::from_translation(Vec3::new(1.25 + x * 1.5, 0.0, 1.25) + offset),
        );
        spawn_building(
            commands,
            &assets.high_density,
            rng,
            Transform::from_translation(Vec3::new(1.25 + x * 1.5, 0.0, 2.75) + offset)
                .with_rotation(Quat::from_axis_angle(Vec3::Y, std::f32::consts::PI)),
        );
        stats.buildings += 2;
    }
}

fn spawn_forest<R: RngExt>(
    commands: &mut ChildSpawnerCommands,
    assets: &CityAssets,
    rng: &mut R,
    offset: Vec3,
    stats: &mut CityStats,
) {
    for x in 0..=12 {
        for z in 0..=8 {
            let transform = Transform::from_translation(
                Vec3::new(x as f32, 0.0, z as f32) * Vec3::new(0.325, 0.0, 0.3)
                    + Vec3::new(0.75, 0.0, 0.85)
                    + offset,
            );
            match rng.random_range(0..3) {
                1 => {
                    spawn_tree(commands, assets, &assets.tree_small, transform);
                    stats.trees += 1;
                }
                2 => {
                    spawn_tree(commands, assets, &assets.tree_large, transform);
                    stats.trees += 1;
                }
                _ => {}
            }
        }
    }
}
