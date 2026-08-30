//! The city's props: merged `.cluster_mesh` handles (see `importers/kenney`) and the materials
//! the upstream example builds in code — kit colormaps and their variations, flat grass and
//! tree colours.

use bevy::prelude::*;
use bevy_aurora::material::{AuroraMaterial, AuroraMaterial3d};
use rand::RngExt;

#[derive(Resource)]
pub struct CityAssets {
    pub cars: Vec<Handle<Mesh>>,
    pub car_material: Handle<AuroraMaterial>,
    pub crossroad: Handle<Mesh>,
    pub road_straight: Handle<Mesh>,
    pub road_material: Handle<AuroraMaterial>,
    pub high_density: Buildings,
    pub medium_density: Buildings,
    pub low_density: Buildings,
    pub ground_tile: Handle<Mesh>,
    pub grass_material: Handle<AuroraMaterial>,
    pub tree_small: Handle<Mesh>,
    pub tree_large: Handle<Mesh>,
    pub path_stones_long: Handle<Mesh>,
    pub fence: Handle<Mesh>,
    pub suburban_material: Handle<AuroraMaterial>,
}

pub struct Buildings {
    meshes: Vec<Handle<Mesh>>,
    materials: Vec<Handle<AuroraMaterial>>,
}

impl Buildings {
    pub fn random<R: RngExt>(&self, rng: &mut R) -> (Mesh3d, AuroraMaterial3d) {
        let mesh = self.meshes[rng.random_range(0..self.meshes.len())].clone();
        let material = self.materials[rng.random_range(0..self.materials.len())].clone();
        (Mesh3d(mesh), AuroraMaterial3d(material))
    }
}

impl CityAssets {
    pub fn random_car<R: RngExt>(&self, rng: &mut R) -> (Mesh3d, AuroraMaterial3d) {
        let mesh = self.cars[rng.random_range(0..self.cars.len())].clone();
        (Mesh3d(mesh), AuroraMaterial3d(self.car_material.clone()))
    }
}

pub fn load_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<AuroraMaterial>>,
) {
    let mesh = |kit: &str, name: &str| -> Handle<Mesh> {
        asset_server.load(format!("kenney/{kit}/meshes/{name}.cluster_mesh"))
    };
    let mut textured = |kit: &str, texture: &str| -> Handle<AuroraMaterial> {
        materials.add(AuroraMaterial {
            base_color_texture: Some(
                asset_server.load(format!("kenney/{kit}/textures/{texture}.png")),
            ),
            perceptual_roughness: 0.9,
            ..default()
        })
    };

    let car_material = textured("car-kit", "colormap");
    let cars = [
        "hatchback-sports",
        "suv",
        "suv-luxury",
        "sedan",
        "sedan-sports",
        "truck",
        "truck-flat",
        "van",
        "delivery",
        "delivery-flat",
        "taxi",
        "garbage-truck",
        "ambulance",
        "police",
        "firetruck",
    ]
    .iter()
    .map(|name| mesh("car-kit", name))
    .collect();

    let road_material = textured("city-kit-roads", "colormap");
    let crossroad = mesh("city-kit-roads", "road-crossroad-path");
    let road_straight = mesh("city-kit-roads", "road-straight");
    let ground_tile = mesh("city-kit-roads", "tile-low");

    let commercial: Vec<Handle<AuroraMaterial>> = ["colormap", "variation-a", "variation-b"]
        .iter()
        .map(|v| textured("city-kit-commercial", v))
        .collect();
    let high_density = Buildings {
        meshes: ["a", "b", "c", "d", "e"]
            .iter()
            .map(|t| mesh("city-kit-commercial", &format!("building-skyscraper-{t}")))
            .chain(
                ["m", "l"]
                    .iter()
                    .map(|t| mesh("city-kit-commercial", &format!("building-{t}"))),
            )
            .collect(),
        materials: commercial.clone(),
    };
    let medium_density = Buildings {
        meshes: ["a", "b", "c", "d", "f", "g", "h"]
            .iter()
            .map(|t| mesh("city-kit-commercial", &format!("building-{t}")))
            .collect(),
        materials: commercial,
    };

    let suburban: Vec<Handle<AuroraMaterial>> =
        ["colormap", "variation-a", "variation-b", "variation-c"]
            .iter()
            .map(|v| textured("city-kit-suburban", v))
            .collect();
    let low_density = Buildings {
        meshes: ["b", "c", "d", "e", "f", "g", "h", "i", "k", "l", "o", "u"]
            .iter()
            .map(|t| mesh("city-kit-suburban", &format!("building-type-{t}")))
            .collect(),
        materials: suburban.clone(),
    };
    let suburban_material = suburban[0].clone();

    let grass_material = materials.add(AuroraMaterial {
        base_color: Color::srgb_u8(97, 203, 139),
        perceptual_roughness: 1.0,
        ..default()
    });

    commands.insert_resource(CityAssets {
        cars,
        car_material,
        crossroad,
        road_straight,
        road_material,
        high_density,
        medium_density,
        low_density,
        ground_tile,
        grass_material,
        tree_small: mesh("city-kit-suburban", "tree-small"),
        tree_large: mesh("city-kit-suburban", "tree-large"),
        path_stones_long: mesh("city-kit-suburban", "path-stones-long"),
        fence: mesh("city-kit-suburban", "fence"),
        suburban_material,
    });
}
