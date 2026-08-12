pub mod assets;
pub mod blend_shapes;
pub mod components;
#[cfg(feature = "debug_draw")]
pub mod debug_draw;
pub mod loaders;
pub mod retarget;
pub mod skeleton;
pub mod util;
pub mod ui;

pub use crate::assets::MHThumb;
use crate::{assets::*, components::*, loaders::*, skeleton::*, util::*};

pub mod prelude {
    #[cfg(feature = "debug_draw")]
    pub use crate::debug_draw::*;

    #[allow(unused_imports)]
    pub use crate::{
        HumanComplete, MHState, MHThumb, MakeHumanPlugin, assets::*, components::*, loaders::*,
        skeleton::*, util::*,
    };
}

use avian3d::prelude::*;
use bevy::math::ToPrecision;
use bevy::animation::AnimatedBy;
#[cfg(feature = "arkit")]
#[cfg(feature = "arkit")]
use bevy::mesh::morph::{MeshMorphWeights, MorphAttributes};
use bevy::{
    animation::AnimationTargetId,
    mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use bevy_asset_loader::prelude::*;
use bevy_aurora::material::{AuroraMaterial3d, StandardAuroraMaterial};
#[cfg(feature = "aurora")]
use bevy_aurora::{geometry::asset::ClusterMesh, instance::RaytracingMesh3d};
#[cfg(feature = "arkit")]
use crate::blend_shapes::ARKit;
#[cfg(feature = "arkit")]
use strum::IntoEnumIterator;

#[derive(Default, States, Debug, Clone, Eq, PartialEq, Hash, Reflect)]
pub enum MHState {
    #[default]
    LoadingAssets,
    LoadingBasemesh,
    Ready,
}

/// Trigger to notify Human generation is complete
#[derive(EntityEvent)]
pub struct HumanComplete {
    pub entity: Entity,
}

#[derive(Default)]
pub struct MakeHumanPlugin;

impl Plugin for MakeHumanPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            #[cfg(feature = "debug_draw")]
            debug_draw::MakeHumanDebugPlugin,
        ))
        .init_state::<MHState>()
        .add_loading_state(
            LoadingState::new(MHState::LoadingAssets)
                .load_collection::<BaseMeshAssets>()
                .continue_to_state(MHState::LoadingBasemesh),
        )
        // TODO: save this work out instead of rebuilding every time
        .add_systems(OnEnter(MHState::LoadingBasemesh), build_basemesh)
        .add_systems(
            Update,
            poll_basemesh_task.run_if(in_state(MHState::LoadingBasemesh)),
        )
        // Steps:
        // 1. On load or change, load needed assets,
        // 2. Once loaded, generate new assets(mesh) in async task
        // 3. Apply generated assets
        .add_systems(
            Update,
            (
                dirty_check,
                human_changed,
                loading_human_assets,
                update_human,
            )
                .run_if(in_state(MHState::Ready)),
        );

        // asset loaders
        app
            // base mesh .obj loader with original verts
            .init_asset::<ObjBaseMesh>()
            .init_asset_loader::<ObjBaseMeshLoader>()
            // vertex groups loader
            .init_asset::<VertexGroups>()
            .init_asset_loader::<VertexGroupsLoader>()
            // morph target loader
            .init_asset::<MorphTargetData>()
            .init_asset_loader::<MorphTargetLoader>()
            // // faceshapes (.mxa) loader for FACS expressions
            // .init_asset::<FaceshapesData>()
            // .init_asset_loader::<FaceshapesLoader>()
            // mhclo loader
            .init_asset::<MhcloAsset>()
            .init_asset_loader::<MhcloLoader>()
            // proxy mesh loader
            .init_asset::<ProxyAsset>()
            .init_asset_loader::<ProxyLoader>()
            // bones
            .init_asset::<RigBones>()
            .init_asset_loader::<RigLoader>()
            // skinning weights
            .init_asset::<SkinningWeights>()
            .init_asset_loader::<SkinningWeightsLoader>()
            // mhmat to material loader
            .init_asset_loader::<MhmatLoader>() // -> StandardAuroraMaterial
            // thumb image loader (PNG thumbnails)
            .init_asset_loader::<ThumbLoader>() // -> Image
            // bvh pose loader
            .init_asset::<Pose>()
            .init_asset_loader::<BvhPoseLoader>()
            // egui registration
            .register_type::<Outfit>()
            .register_type::<MHTag>()
            .register_type::<ClothingOffset>()
            .register_type::<FloorOffset>()
            .register_type::<Morph>();
    }
}

#[derive(AssetCollection, Resource)]
pub struct BaseMeshAssets {
    #[asset(path = "make_human/3dobjs/base.obj")]
    pub obj: Handle<ObjBaseMesh>,
    #[asset(path = "make_human/mesh_metadata/basemesh.vertex_groups.json")]
    pub vertex_groups: Handle<VertexGroups>,
    // TODO: will most likely need this later
    // #[asset(path = "make_human/mesh_metadata/hm08_config.json")]
    // pub config: Handle<BasemeshConfig>,
}

#[derive(Resource, Default)]
pub struct BaseMesh {
    /// The vertices in the base mesh
    pub vertices: Vec<Vec3>,
    /// Maps Bevy mesh vertex idx -> MH obj vertex idx (handles UV seam duplicates)
    pub mhid_lookup: Vec<u16>,
    /// Vertex groups for bone CUBE/MEAN strategies
    pub vertex_groups: VertexGroups,
}

#[derive(Resource)]
pub struct PrepareBasemeshTask(Task<PrepareBasemeshOutput>);

pub struct PrepareBasemeshOutput {
    pub mhid_lookup: Vec<u16>,
}

fn build_basemesh(
    mut commands: Commands,
    base_mesh_assets: Res<BaseMeshAssets>,
    obj_assets: ResMut<Assets<ObjBaseMesh>>,
) {
    // grab copy for async task
    let obj_base_mesh = obj_assets.get(&base_mesh_assets.obj).unwrap().clone();

    let task = AsyncComputeTaskPool::get().spawn(async move {
        // Get mesh and vertex map and build mhid_lookup, takes 220ms
        let vtx_data = get_vertex_positions(&obj_base_mesh.mesh);
        let vertex_map = generate_vertex_map(&obj_base_mesh.vertices, &vtx_data);
        let mhid_lookup = generate_mhid_lookup(&vertex_map);

        PrepareBasemeshOutput { mhid_lookup }
    });
    commands.insert_resource(PrepareBasemeshTask(task));
}

fn poll_basemesh_task(
    mut commands: Commands,
    base_mesh_assets: Res<BaseMeshAssets>,
    obj_assets: ResMut<Assets<ObjBaseMesh>>,
    vg_assets: Res<Assets<VertexGroups>>,
    mut prepare_task: ResMut<PrepareBasemeshTask>,
) {
    if let Some(PrepareBasemeshOutput { mhid_lookup }) =
        future::block_on(future::poll_once(&mut prepare_task.0))
    {
        let obj_base_mesh = obj_assets
            .get(&base_mesh_assets.obj)
            .expect("Basemesh ojb loaded")
            .clone();

        let vg = vg_assets
            .get(&base_mesh_assets.vertex_groups)
            .expect("vg loaded")
            .clone();

        commands.insert_resource(BaseMesh {
            vertices: obj_base_mesh.vertices.clone(),
            mhid_lookup,
            vertex_groups: vg.clone(),
        });
        commands.remove_resource::<BaseMeshAssets>();
        commands.set_state(MHState::Ready);
    }
}

// TODO: yea changed check is slow, replace with observers on change maybe?
/// mark Human as dirty when relevant components change
fn dirty_check(
    mut commands: Commands,
    query: Query<
        Entity,
        Or<(
            Changed<Rig>,
            Changed<SkinMesh>,
            Changed<SkinMaterial>,
            Changed<Eyes>,
            Changed<Eyebrows>,
            Changed<Eyelashes>,
            Changed<Teeth>,
            Changed<Tongue>,
            Changed<Hair>,
            Changed<Outfit>,
            Changed<ClothingOffset>,
            Changed<Morphs>,
            Changed<FloorOffset>,
        )>,
    >,
    mut removed_hair: RemovedComponents<Hair>,
) {
    for e in query.iter() {
        commands.entity(e).insert(HumanDirty);
    }
    for e in removed_hair.read() {
        if let Ok(mut ec) = commands.get_entity(e) {
            ec.insert(HumanDirty);
        }
    }
}

/// Task component for async character processing
#[derive(Component)]
pub struct HumanProcessingTask(Task<HumanProcessingOutput>);

/// All data needed for human processing (extracted from assets)
struct HumanProcessingInput {
    base_vertices: Vec<Vec3>,
    base_vertex_groups: VertexGroups,

    // All morphs (body morphs + macro morphs)
    morphs: Vec<(MorphTargetData, f32)>,
    // Rig
    rig_bones: RigBones,
    skinning_weights: SkinningWeights,

    // Skin proxy
    skin_proxy: (ProxyAsset, ObjBaseMesh),
    skin_material: Handle<StandardAuroraMaterial>,

    // Parts
    parts: Vec<MHItemLoaded>,

    clothing_offset: f32,

    #[cfg(feature = "arkit")]
    /// ARKit blend shape targets (52 shapes)
    arkit_targets: Vec<MorphTargetData>,
}

/// Result of human processing
struct HumanProcessingOutput {
    skeleton: Skeleton,
    parts: Vec<MHItemResult>,
    /// Height (max_y - min_y of morphed vertices)
    height: f32,
    /// Min Y of morphed vertices (for ground offset)
    min_y: f32,
    #[cfg(feature = "arkit")]
    /// ARKit morph deltas transferred to proxy mesh (52 x vertex_count)
    arkit_morphs: Vec<Vec<Vec3>>,
}

fn human_changed(
    mut commands: Commands,
    query: Query<HumanQuery, With<HumanDirty>>,
    asset_server: Res<AssetServer>,
) {
    for h in query.iter() {
        let mut parts = vec![];
        if let Some(hair_item) = h.hair {
            parts.push(MHItem::load(MHTag::Hair, hair_item, &asset_server));
        }
        parts.push(MHItem::load(MHTag::Eyes, h.eyes, &asset_server));
        parts.push(MHItem::load(MHTag::Eyebrows, h.eyebrows, &asset_server));
        parts.push(MHItem::load(MHTag::Eyelashes, h.eyelashes, &asset_server));
        parts.push(MHItem::load(MHTag::Teeth, h.teeth, &asset_server));
        parts.push(MHItem::load(MHTag::Tongue, h.tongue, &asset_server));

        for clothing_item in h.clothing.iter() {
            parts.push(MHItem::load(MHTag::Clothes, clothing_item, &asset_server));
        }

        // Load all morph targets (unified - body morphs + macro morphs)
        // For interpolated macro morphs, load min/avg/max with interpolated weights
        let mut morphs: Vec<(Handle<MorphTargetData>, f32)> = Vec::new();

        for Morph { target, value } in h.morphs.iter() {
            if target.is_interpolated() {
                // Interpolated macro morph: value 0..1 maps to min->avg->max
                if let Some((min_path, avg_path, max_path)) = target.macro_paths() {
                    let v = value.clamp(0.0, 1.0);
                    if v < 0.5 {
                        // Blend min -> avg
                        let t = v / 0.5;
                        if let Some(min) = min_path {
                            morphs.push((asset_server.load(min.to_string()), 1.0 - t));
                        }
                        if let Some(avg) = avg_path {
                            morphs.push((asset_server.load(avg.to_string()), t));
                        }
                    } else {
                        // Blend avg -> max
                        let t = (v - 0.5) / 0.5;
                        if let Some(avg) = avg_path {
                            morphs.push((asset_server.load(avg.to_string()), 1.0 - t));
                        }
                        if let Some(max) = max_path {
                            morphs.push((asset_server.load(max.to_string()), t));
                        }
                    }
                }
            } else if let Some(path) = target.target_path(*value) {
                // Simple morph - single path
                morphs.push((asset_server.load(path.to_string()), value.abs()));
            }
        }

        #[cfg(feature = "arkit")]
        let arkit_targets: Vec<Handle<MorphTargetData>> = ARKit::iter()
            .map(|shape| {
                // Asset files use kebab-case (e.g. "eye-blink-left.target")
                let name = shape
                    .as_ref()
                    .chars()
                    .fold(String::new(), |mut s, c| {
                        if c.is_uppercase() && !s.is_empty() {
                            s.push('-');
                        }
                        s.push(c.to_ascii_lowercase());
                        s
                    });
                asset_server.load(format!("make_human/targets/arkit/{name}.target"))
            })
            .collect();

        commands
            .entity(h.entity)
            .remove::<HumanDirty>()
            // TODO: test that drop does cancel existing task
            .remove::<HumanProcessingTask>() // stop current builds,
            .insert(HumanAssets {
                skin_obj_base: asset_server.load(h.skin_mesh.obj().to_string()),
                skin_proxy: asset_server.load(h.skin_mesh.proxy().to_string()),
                skin_material: asset_server.load(h.skin_material.mhmat().to_string()),
                rig_bones: asset_server.load(h.rig.rig_json_path().to_string()),
                rig_weights: asset_server.load(h.rig.weights().to_string()),
                clothing_offset: h.clothing_offset.0,
                parts,
                morphs,
                #[cfg(feature = "arkit")]
                arkit_targets,
            });
    }
}

fn loading_human_assets(
    mut commands: Commands,
    mut query: Query<(Entity, &HumanAssets)>,
    asset_server: Res<AssetServer>,
    base_mesh: Res<BaseMesh>,
    mhclo_assets: Res<Assets<MhcloAsset>>,
    proxy_assets: Res<Assets<ProxyAsset>>,
    obj_base_assets: Res<Assets<ObjBaseMesh>>,
    rig_bones_assets: Res<Assets<RigBones>>,
    skinning_weights_assets: Res<Assets<SkinningWeights>>,
    morph_target_assets: Res<Assets<MorphTargetData>>,
) {
    for (e, assets) in query.iter_mut() {
        let handles = assets.all_handles();
        let total = handles.len();
        let loaded = handles
            .iter()
            .filter(|h| asset_server.is_loaded_with_dependencies(h.id()))
            .count();
        debug!("make_human: {e:?} waiting {loaded}/{total}");
        for h in &handles {
            if !asset_server.is_loaded_with_dependencies(h.id()) {
                debug!(
                    "make_human:   pending {:?} state {:?}",
                    asset_server.get_path(h.id()),
                    asset_server.get_load_states(h.id()),
                );
            }
        }
        // A failed handle would otherwise stall generation FOREVER, silently.
        for h in &handles {
            if let Some(bevy::asset::LoadState::Failed(err)) = asset_server.get_load_state(h.id()) {
                error!(
                    "make_human: asset FAILED (generation is stalled): {:?}: {err}",
                    asset_server.get_path(h.id()),
                );
            }
        }

        if loaded >= total {
            let parts = assets
                .parts
                .iter()
                .map(|a| MHItemLoaded {
                    tag: a.tag,
                    base: obj_base_assets.get(&a.obj_base).unwrap().clone(),
                    mat: a.mat.clone(),
                    clo: mhclo_assets.get(&a.clo).unwrap().clone(),
                })
                .collect::<Vec<_>>();

            // Build skin proxy data
            let skin_proxy = (
                proxy_assets.get(&assets.skin_proxy).unwrap().clone(),
                obj_base_assets.get(&assets.skin_obj_base).unwrap().clone(),
            );

            #[cfg(feature = "arkit")]
            let arkit_targets: Vec<MorphTargetData> = assets
                .arkit_targets
                .iter()
                .filter_map(|h| morph_target_assets.get(h).cloned())
                .collect();

            // Extract for task
            let input = HumanProcessingInput {
                base_vertices: base_mesh.vertices.clone(),
                base_vertex_groups: base_mesh.vertex_groups.clone(),
                morphs: assets
                    .morphs
                    .iter()
                    .filter_map(|(h, w)| morph_target_assets.get(h).map(|m| (m.clone(), *w)))
                    .collect(),
                rig_bones: rig_bones_assets.get(&assets.rig_bones).unwrap().clone(),
                skinning_weights: skinning_weights_assets
                    .get(&assets.rig_weights)
                    .unwrap()
                    .clone(),
                skin_material: assets.skin_material.clone(),
                skin_proxy,
                clothing_offset: assets.clothing_offset,
                parts,
                #[cfg(feature = "arkit")]
                arkit_targets,
            };

            // Spawn async task
            let task = AsyncComputeTaskPool::get().spawn(async move { process_human(input) });
            commands
                .entity(e)
                .remove::<HumanAssets>()
                .insert(HumanProcessingTask(task));
        }
    }
}

fn process_human(input: HumanProcessingInput) -> HumanProcessingOutput {
    let mut morphed_vertices = input.base_vertices.clone();

    // Apply all morphs (unified - body morphs + macro morphs)
    for (morph_data, weight) in &input.morphs {
        if *weight < 0.001 {
            continue;
        }
        for (&mh_idx, &offset) in &morph_data.offsets {
            let idx = mh_idx as usize;
            if idx < morphed_vertices.len() {
                morphed_vertices[idx] += offset * *weight;
            }
        }
    }

    // Build skeleton
    let skeleton = 
        input
            .rig_bones
            .build_skeleton(&morphed_vertices, &input.base_vertex_groups);

    let mut parts = input
        .parts
        .into_iter()
        .map(|s| MHItemResult {
            tag: s.tag,
            mesh: {
                let mesh = apply_mhclo_fitting(
                    &s.base.mesh,
                    &s.clo,
                    &s.base.mhid_lookup,
                    &morphed_vertices,
                    match s.tag {
                        MHTag::Clothes => input.clothing_offset,
                        _ => 0.0,
                    },
                );
                apply_skinning_weights_via_mhclo(
                    mesh,
                    &s.clo,
                    &s.base.mhid_lookup,
                    &skeleton,
                    &input.skinning_weights,
                )
            },
            mat: s.mat,
        })
        .collect::<Vec<_>>();

    // Skin mesh via proxy
    let (proxy_asset, proxy_obj) = &input.skin_proxy;
    let mut skin_mesh = apply_proxy_fitting(
        &proxy_obj.mesh,
        proxy_asset,
        &morphed_vertices,
        &proxy_obj.vertices,
    );
    skin_mesh = apply_skinning_weights_to_proxy(
        skin_mesh,
        proxy_asset,
        &proxy_obj.mhid_lookup,
        &skeleton,
        &input.skinning_weights,
    );
    parts.push(MHItemResult {
        tag: MHTag::Skin,
        mesh: skin_mesh,
        mat: input.skin_material.clone(),
    });

    // Calculate human height from morphed vertices
    let (min_y, max_y) = morphed_vertices
        .iter()
        .fold((f32::MAX, f32::MIN), |(min, max), v| {
            (min.min(v.y), max.max(v.y))
        });
    let height = max_y - min_y;

    #[cfg(feature = "arkit")]
    let arkit_morphs: Vec<Vec<Vec3>> = input
        .arkit_targets
        .iter()
        .map(|morph_data| {
            proxy_obj
                .mhid_lookup
                .iter()
                .map(|&obj_idx| {
                    let binding = &proxy_asset.bindings[obj_idx as usize];
                    let d0 = morph_data
                        .offsets
                        .get(&binding.triangle[0])
                        .copied()
                        .unwrap_or(Vec3::ZERO);
                    let d1 = morph_data
                        .offsets
                        .get(&binding.triangle[1])
                        .copied()
                        .unwrap_or(Vec3::ZERO);
                    let d2 = morph_data
                        .offsets
                        .get(&binding.triangle[2])
                        .copied()
                        .unwrap_or(Vec3::ZERO);
                    // Barycentric interpolation of offsets
                    d0 * binding.weights[0] + d1 * binding.weights[1] + d2 * binding.weights[2]
                })
                .collect()
        })
        .collect();

    HumanProcessingOutput {
        skeleton,
        parts,
        height,
        min_y,
        #[cfg(feature = "arkit")]
        arkit_morphs,
    }
}

/// Update human and trigger HumanGenerate
#[allow(unused_mut, unused_variables)]
fn update_human(
    mut commands: Commands,
    mut query: Query<(
        Entity,
        Option<&Children>,
        &mut HumanProcessingTask,
        &FloorOffset,
    )>,
    mut inverse_bindpose_assets: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardAuroraMaterial>>,
    #[cfg(feature = "aurora")] mut cluster_meshes: ResMut<Assets<ClusterMesh>>,
) {
    for (entity, children_maybe, mut task, floor_offset) in query.iter_mut() {
        if let Some(HumanProcessingOutput {
            skeleton,
            parts,
            height,
            min_y,
            #[cfg(feature = "arkit")]
            arkit_morphs,
        }) = future::block_on(future::poll_once(&mut task.0))
        {
            commands.entity(entity).remove::<HumanProcessingTask>();

            // remove all children
            if let Some(children) = children_maybe {
                for e in children.iter() {
                    commands.entity(e).despawn();
                }
            }

            // Spawn Armature entity as root - matches GLTF skeleton structure for animation compatibility
            let armature_name = Name::new("Armature");
            let armature_target_id = AnimationTargetId::from_names([armature_name.clone()].iter());
            let armature_entity = commands
                .spawn((
                    armature_name.clone(),
                    Transform::IDENTITY,
                    GlobalTransform::default(),
                    armature_target_id,
                    AnimatedBy(entity),
                    AnimationPlayer::default(),
                    Armature,
                    Visibility::default(),
                ))
                .id();

            commands.entity(entity).add_children(&[armature_entity]);

            let mut bone_entities = Vec::with_capacity(skeleton.bones.len());

            // Spawn all bones
            for (bone_idx, bone) in skeleton.bones.iter().enumerate() {
                // Build hierarchical name path for AnimationTarget
                // Path includes Armature root for GLTF animation compatibility
                let mut path = vec![Name::new(bone.name.clone())];
                let mut current_idx = bone_idx;

                while let Some(parent_idx) = skeleton.hierarchy[current_idx] {
                    path.push(Name::new(skeleton.bones[parent_idx].name.clone()));
                    current_idx = parent_idx;
                }
                // Add Armature as root of path
                path.push(armature_name.clone());

                let bone_entity = commands
                    .spawn((
                        Name::new(bone.name.clone()),
                        skeleton.bind_pose[bone_idx],
                        GlobalTransform::default(),
                        AnimationTargetId::from_names(path.iter().rev()),
                        AnimatedBy(armature_entity),
                        Visibility::default(),
                    ))
                    .id();
                bone_entities.push(bone_entity);
            }

            // Wire up parent-child hierarchy
            for (bone_idx, &parent_idx_opt) in skeleton.hierarchy.iter().enumerate() {
                let bone = bone_entities[bone_idx];
                if let Some(parent_idx) = parent_idx_opt {
                    commands
                        .entity(bone_entities[parent_idx])
                        .add_children(&[bone]);
                } else {
                    // Root bones attach to Armature entity
                    commands.entity(armature_entity).add_children(&[bone]);
                }
            }

            // Create SkinnedMesh component - shared by body and all parts
            let inverse_bindposes = inverse_bindpose_assets.add(skeleton.inverse_bind_matrices);
            let skinned_mesh = SkinnedMesh {
                inverse_bindposes,
                joints: bone_entities.clone(),
            };

            // Capsule collider sized to character
            let radius = 0.25;
            let length = (height - radius * 2.0).max(0.1);
            let offset_y = min_y - floor_offset.0 + radius + length / 2.0;
            commands
                .entity(entity)
                .insert(RigidBody::Dynamic)
                .insert(LockedAxes::ROTATION_LOCKED)
                .with_child((
                    Name::new("Collider"),
                    Transform::from_translation((-Vec3::Y * offset_y).to_precision()),
                    Collider::capsule(radius, length),
                    MHTag::Collider,
                ));

            // parts
            for a in parts.into_iter() {
                #[allow(unused_mut)]
                let mut mesh = a.mesh;
                let tag = a.tag;
                // Skin rides the root entity; every other part is a child of it.
                let target = match tag {
                    MHTag::Skin => {
                        #[cfg(feature = "arkit")]
                        {
                            let morph_targets: Vec<MorphAttributes> = arkit_morphs
                                .iter()
                                .flat_map(|mesh_offsets| {
                                    mesh_offsets.iter().map(|&offset| {
                                        MorphAttributes::new(offset, Vec3::ZERO, Vec3::ZERO)
                                    })
                                })
                                .collect();

                            mesh.set_morph_targets(morph_targets);
                            commands.entity(entity).insert(MeshMorphWeights::Value {
                                weights: vec![0.0; arkit_morphs.len()],
                            });
                        }

                        commands
                            .entity(entity)
                            .insert((AuroraMaterial3d(a.mat), skinned_mesh.clone()));
                        entity
                    }
                    _ => {
                        // Add clearcoat for glossy wet eye look
                        #[cfg(feature = "glossy_eyes")]
                        if tag == MHTag::Eyes {
                            if let Some(mut mat) = materials.get_mut(&a.mat) {
                                mat.clearcoat = 1.0;
                                mat.clearcoat_perceptual_roughness = 0.1;
                            }
                        }
                        commands
                            .spawn((
                                ChildOf(entity),
                                Name::new(format!("{tag}")),
                                AuroraMaterial3d(a.mat),
                                skinned_mesh.clone(),
                                tag,
                            ))
                            .id()
                    }
                };

                // Aurora takes a baked ClusterMesh; the raster path takes Mesh3d.
                #[cfg(feature = "aurora")]
                match ClusterMesh::try_from(&mesh) {
                    Ok(cluster) => {
                        commands
                            .entity(target)
                            .insert(RaytracingMesh3d(cluster_meshes.add(cluster)));
                    }
                    Err(err) => {
                        warn!("make_human: cluster bake failed for {tag} ({err}); using Mesh3d");
                        commands.entity(target).insert(Mesh3d(meshes.add(mesh)));
                    }
                }
                #[cfg(not(feature = "aurora"))]
                commands.entity(target).insert(Mesh3d(meshes.add(mesh)));
            }

            // Notify character complete
            commands.trigger(HumanComplete { entity });
        }
    }
}
