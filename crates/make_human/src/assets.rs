use crate::{components::*, loaders::*};
use bevy::prelude::*;
use bevy_aurora::material::StandardAuroraMaterial;
use serde::{Deserialize, Serialize};
#[allow(unused_imports)]
use strum::{Display, EnumCount, EnumIter, EnumProperty, IntoEnumIterator};

// See build.rs for more details
// Asset enums generated at compile time
include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Trait for assets with thumbnail
pub trait MHThumb: Copy + 'static {
    fn thumb(&self) -> &'static str;
}

/// Trait for MakeHuman part assets with mhclo/mhmat/obj/thumb
pub trait MHPart: MHThumb {
    fn mhclo(&self) -> &'static str;
    fn mhmat(&self) -> &'static str;
    fn obj(&self) -> &'static str;
}

/// Converts Enums to Handles
#[derive(Component)]
#[component(storage = "SparseSet")]
pub struct HumanAssets {
    pub skin_obj_base: Handle<ObjBaseMesh>,
    pub skin_proxy: Handle<ProxyAsset>,
    pub skin_material: Handle<StandardAuroraMaterial>,

    pub parts: Vec<MHItem>,

    pub rig_bones: Handle<RigBones>,
    pub rig_weights: Handle<SkinningWeights>,

    /// All morph targets (body morphs + macro morphs)
    pub morphs: Vec<(Handle<MorphTargetData>, f32)>,
    /// Offset to push clothing outward (prevents skin poke-through)
    pub clothing_offset: f32,

    #[cfg(feature = "arkit")]
    /// ARKit blend shape targets (52 shapes)
    pub arkit_targets: Vec<Handle<MorphTargetData>>,
}

impl HumanAssets {
    /// Get all handles for progress tracking
    pub fn all_handles(&self) -> Vec<UntypedHandle> {
        let mut handles = vec![
            self.skin_obj_base.clone().untyped(),
            self.skin_proxy.clone().untyped(),
            self.skin_material.clone().untyped(),
            self.rig_bones.clone().untyped(),
            self.rig_weights.clone().untyped(),
        ];

        for part in &self.parts {
            handles.extend(part.handles());
        }

        for (handle, _value) in &self.morphs {
            handles.push(handle.clone().untyped());
        }

        #[cfg(feature = "arkit")]
        for handle in &self.arkit_targets {
            handles.push(handle.clone().untyped());
        }

        handles
    }
}

pub struct MHItem {
    pub tag: MHTag,
    pub clo: Handle<MhcloAsset>,
    pub mat: Handle<StandardAuroraMaterial>,
    pub obj_base: Handle<ObjBaseMesh>, // Mesh + original verts for mhid_lookup
}

impl MHItem {
    /// Load assets (clo, mat, obj with verts)
    pub fn load<T: MHPart>(tag: MHTag, part: &T, asset_server: &AssetServer) -> Self {
        Self {
            tag,
            clo: asset_server.load(part.mhclo().to_string()),
            mat: asset_server.load(part.mhmat().to_string()),
            obj_base: asset_server.load(part.obj().to_string()),
        }
    }

    /// Get all handles for this item
    pub fn handles(&self) -> Vec<UntypedHandle> {
        vec![
            self.clo.clone().untyped(),
            self.mat.clone().untyped(),
            self.obj_base.clone().untyped(),
        ]
    }
}

pub struct MHItemLoaded {
    pub tag: MHTag,
    pub mat: Handle<StandardAuroraMaterial>, // dont do anything currently with material, but we need pass it along
    pub clo: MhcloAsset,
    pub base: ObjBaseMesh,
}

pub struct MHItemResult {
    pub tag: MHTag,
    pub mat: Handle<StandardAuroraMaterial>, // dont do anything currently with material, but we need pass it along
    pub mesh: Mesh,
}

pub struct MHItemFinal {
    pub tag: MHTag,
    pub mat: Handle<StandardAuroraMaterial>, // dont do anything currently with material, but we need pass it along
    pub mesh: Handle<Mesh>,
}

/// A morph target with a value
/// - Binary morphs (body parts): -1.0 to 1.0 (neg=decr, pos=incr)
/// - Single/Macro morphs: 0.0 to 1.0
#[derive(Debug, Clone, Reflect)]
pub struct Morph {
    pub target: MorphTarget,
    pub value: f32,
}

impl Morph {
    pub fn new(target: MorphTarget, value: f32) -> Self {
        let (min, max) = target.value_range();
        Self {
            target,
            value: value.clamp(min, max),
        }
    }

    /// Create a macro morph (convenience for MorphTarget::Macro)
    pub fn macro_morph(target: MacroMorph, value: f32) -> Self {
        Self::new(MorphTarget::Macro(target), value)
    }
}

impl From<(MorphTarget, f32)> for Morph {
    fn from((target, value): (MorphTarget, f32)) -> Self {
        Self::new(target, value)
    }
}
