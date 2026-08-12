use crate::assets::*;
use bevy::{ecs::query::QueryData, prelude::*};

#[derive(QueryData)]
pub struct HumanQuery {
    pub entity: Entity,
    pub name: Option<&'static Name>,
    pub rig: &'static Rig,
    pub skin_mesh: &'static SkinMesh,
    pub skin_material: &'static SkinMaterial,
    pub eyes: &'static Eyes,
    pub eyebrows: &'static Eyebrows,
    pub eyelashes: &'static Eyelashes,
    pub teeth: &'static Teeth,
    pub tongue: &'static Tongue,
    pub hair: Option<&'static Hair>,
    pub morphs: &'static Morphs,
    pub clothing: &'static Outfit,
    pub floor_offset: &'static FloorOffset,
    pub clothing_offset: &'static ClothingOffset,
}

/// Human
#[derive(Component, Default)]
#[require(
    Rig,
    SkinMesh,
    SkinMaterial,
    // Hair, optional
    Eyes,
    Eyebrows,
    Eyelashes,
    Teeth,
    Tongue,
    Outfit,
    ClothingOffset,
    FloorOffset,
    Morphs,
    HumanDirty, // will trigger a generate when spawned        
    Transform,
    Visibility,
)]
pub struct Human;

// marker comonent to track if human needs to be rebuilt
#[derive(Component, Clone, Reflect, Default)]
pub struct HumanDirty;

// === PARTS ===
// Eyes, Eyebrows, Eyelashes, Teeth, Tongue, Hair are generated in build.rs with Component derive
// They implement MHPart trait for generic handling

// Clothing is the only multi-item part, needs wrapper
#[derive(Component, Clone, Default, Debug, Reflect, Deref, DerefMut)]
pub struct Outfit(pub Vec<Clothing>);

/// Uses normals to offset clothing away from skin, hack to reduce z-fighting
// TODO: replace with Delete Vertex Groups
#[derive(Component, Clone, Copy, Default, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component)]
pub struct ClothingOffset(pub f32);

impl From<f32> for ClothingOffset {
    fn from(v: f32) -> Self {
        Self(v)
    }
}

/// Vertical offset to adjust for floor contact (shoes, bare feet, etc)
#[derive(Component, Clone, Copy, Default, Debug, Reflect, Deref, DerefMut)]
#[reflect(Component)]
pub struct FloorOffset(pub f32);

impl From<f32> for FloorOffset {
    fn from(v: f32) -> Self {
        Self(v)
    }
}

#[derive(Component, Clone, Debug, Default, Deref, DerefMut, Reflect)]
pub struct Morphs(pub Vec<Morph>);

// Marker components body parts
#[derive(Component, Copy, Clone, strum::Display, PartialEq, Eq, Debug, Reflect)]
pub enum MHTag {
    Armature,
    Skin,
    Hair,
    Eyes,
    Teeth,
    Tongue,
    Eyebrows,
    Eyelashes,
    Clothes,
    Collider,
}

#[derive(Component, Default)]
pub struct Armature;

#[derive(Component)]
#[require(MHTag::Skin)]
pub struct SkinPart;

#[derive(Component)]
#[require(MHTag::Hair)]
pub struct HairPart;

#[derive(Component)]
#[require(MHTag::Eyes)]
pub struct EyesPart;

#[derive(Component)]
#[require(MHTag::Teeth)]
pub struct TeethPart;

#[derive(Component)]
#[require(MHTag::Tongue)]
pub struct TonguePart;

#[derive(Component)]
#[require(MHTag::Eyebrows)]
pub struct EyebrowsPart;

#[derive(Component)]
#[require(MHTag::Eyelashes)]
pub struct EyelashesPart;

#[derive(Component)]
#[require(MHTag::Clothes)]
pub struct ClothesPart;

// TODO: Below Here, only used in raven_ai for now

#[derive(Component)]
pub struct LeftEyeMesh;

#[derive(Component)]
pub struct RightEyeMesh;

#[derive(Component)]
pub struct JawMesh;

#[derive(Component)]
pub struct UpperJawMesh;
