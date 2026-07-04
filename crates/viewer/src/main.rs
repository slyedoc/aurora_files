//! General `bevy_solari` `.bsn` scene viewer.
//!
//! Loads any importer-baked `.bsn` (entities arrive carrying `RaytracingMesh3d` + inline
//! `SolariMaterial` already — no runtime mesh conversion) under full ray tracing and flies a free
//! camera through it. Run from the repo root (the asset root defaults to the current directory) and
//! pass a tab-completed path or a glob that expands to one or more `.bsn` files:
//!
//!   cargo run --release -p solari_view -- assets/lunarbase/KB3D_LNB_BldgLG*
//!
//! Every match becomes a row in the scene picker (bottom-left); click one (or press `[`/`]`) to
//! switch the live scene.
//!
//! Keys: `1`/`2`/`3` jump to saved viewpoints, `[`/`]` switch scenes, `I` prints the current camera
//! transform, `Space` toggles the fly-through, `B` runs the benchmark. Generalized from bevy's
//! `san_miguel` example.

use std::{f32::consts::PI as PI_f32, f64::consts::PI};

use bevy::{
    camera::{CameraMainTextureUsages, Hdr, visibility::NoCpuCulling},
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin, FreeCameraState},
    dev_tools::{
        fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig},
        render_debug::RenderDebugOverlayPlugin,
    },
    diagnostic::FrameTimeDiagnosticsPlugin,
    feathers::{FeathersPlugins, dark_theme::create_dark_theme, theme::UiTheme},
    image::{ImageAddressMode, ImageSamplerDescriptor},
    input::mouse::{MouseScrollUnit, MouseWheel},
    light::{cluster::ClusterConfig, light_consts::lux},
    log::LogPlugin,
    math::{DQuat, DVec3},
    picking::{Pickable, hover::HoverMap},
    ui::{ComputedNode, OverflowAxis, ScrollPosition},
    pbr::PbrPlugin,
    prelude::*,
    render::render_resource::TextureUsages,
    solari::prelude::*,
    window::WindowResolution,
    winit::WinitSettings,
};
use clap::Parser;

mod settings;

/// Log filter: silence noisy startup INFO lines (keep warn/error).
pub const LOG_FILTER: &str = concat!(
    "bevy_camera_controller=off",
    ",bevy_winit=warn",
    ",bevy_render=warn",
    ",wgpu_hal=warn",
    ",bevy_diagnostic::system_information_diagnostics_plugin=warn",
);

#[derive(Parser, Resource, Clone)]
#[command(about = "bevy_solari .bsn scene viewer")]
pub struct Args {
    /// Path(s) or glob(s) (relative to the working directory) selecting the `.bsn` scene(s) to load,
    /// e.g. `assets/san_miguel/SanMiguel.bsn` or `assets/lunarbase/KB3D_LNB_BldgLG*`. Quote a glob to
    /// let the viewer expand it; an unquoted glob the shell already expanded works too. Each match
    /// becomes a row in the scene picker. A leading `assets/` is accepted (and stripped to the
    /// asset-relative path the asset server wants).
    #[arg(required = true)]
    pub scenes: Vec<String>,

    /// Spin the camera (and saved viewpoints) about the camera position.
    #[arg(long)]
    pub spin: bool,

    /// Don't show the frame-time text overlay.
    #[arg(long)]
    pub hide_frame_time: bool,
}

pub fn main() {
    let args = Args::parse();
    if let Ok(cwd) = std::env::current_dir() {
        // SAFETY: set before any threads are spawned (AssetPlugin reads it during plugin build).
        unsafe { std::env::set_var("BEVY_ASSET_ROOT", cwd) };
    }

    // Expand the scene argument (path or glob) against the filesystem into the asset-relative `.bsn`
    // paths the picker switches between.
    let scenes = Scenes::resolve(&args.scenes);
    if scenes.paths.is_empty() {
        eprintln!("no `.bsn` scenes matched {:?}", args.scenes);
        std::process::exit(1);
    }

    let mut app = App::new();

    #[cfg(feature = "dlss")]
    app.insert_resource(bevy::anti_alias::dlss::DlssProjectId(
        bevy::asset::uuid::uuid!("b1f7d9e3-2a4c-4d6b-8f1e-3c5a7b9d0f2e"),
    ));

    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Solari Viewer".into(),
                    resolution: WindowResolution::default(),
                    ..default()
                }),
                ..default()
            })
            .set(ImagePlugin {
                default_sampler: ImageSamplerDescriptor {
                    address_mode_u: ImageAddressMode::Repeat,
                    address_mode_v: ImageAddressMode::Repeat,
                    address_mode_w: ImageAddressMode::Repeat,
                    ..ImageSamplerDescriptor::linear()
                },
                ..default()
            })
            .set(LogPlugin {
                filter: LOG_FILTER.into(),
                ..default()
            })
            .disable::<TransformPlugin>()
            .disable::<PbrPlugin>()
            .disable::<RenderDebugOverlayPlugin>(),
    )
    .add_plugins((
        FrameTimeDiagnosticsPlugin {
            max_history_length: 1000,
            ..default()
        },
        FreeCameraPlugin,
        FeathersPlugins,
        FpsOverlayPlugin {
            config: FpsOverlayConfig {
                frame_time_graph_config: FrameTimeGraphConfig {
                    enabled: true,
                    target_fps: 240.0,
                    min_fps: 60.0,
                },
                ..default()
            },
        },
        SolariPlugin,
        settings::LightSettingsPlugin,
    ))
    .insert_resource(GlobalAmbientLight::NONE)
    .insert_resource(args.clone())
    .insert_resource(scenes)
    .insert_resource(ClearColor(Color::srgb(1.75, 1.9, 1.99)))
    .insert_resource(WinitSettings::continuous())
    .insert_resource(UiTheme(create_dark_theme()))
    .init_resource::<GroundAssets>()
    .init_resource::<RefCubeAssets>()
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            handle_input,
            switch_scene,
            highlight_scene_buttons,
            ensure_visibility,
            update_loading_screen,
        )
            .chain(),
    )
    .add_systems(Update, (send_scroll_events, apply_ground, apply_ref_cube))
    .add_observer(on_scroll)
    .run();
}

#[derive(Component)]
pub struct Spin;

/// The togglable ground-plane instance (see [`apply_ground`]).
#[derive(Component)]
struct GroundPlane;

/// Baked-once ground mesh + material; [`apply_ground`] spawns instances from these.
#[derive(Resource)]
struct GroundAssets {
    mesh: Handle<ClusterMesh>,
    material: Handle<StandardSolariMaterial>,
}

impl FromWorld for GroundAssets {
    fn from_world(world: &mut World) -> Self {
        // A large plane at y=0; each borrow of the asset store is a scoped temporary.
        let plane = Plane3d::default()
            .mesh()
            .size(4000.0, 4000.0)
            .build()
            .with_generated_tangents()
            .expect("ground tangents");
        let mesh = world
            .resource_mut::<Assets<ClusterMesh>>()
            .add(ClusterMesh::try_from(&plane).expect("ground bake"));
        let material = world.resource_mut::<Assets<StandardSolariMaterial>>().add(
            StandardSolariMaterial {
                base_color: Color::srgb(0.30, 0.29, 0.27),
                perceptual_roughness: 1.0,
                ..default()
            },
        );
        Self { mesh, material }
    }
}

/// Spawn or despawn the ground instance when the "Ground" checkbox flips.
fn apply_ground(
    mut commands: Commands,
    settings: Res<settings::LightSettings>,
    assets: Res<GroundAssets>,
    ground: Query<Entity, With<GroundPlane>>,
    mut last: Local<Option<bool>>,
) {
    if *last == Some(settings.ground_enabled) {
        return;
    }
    *last = Some(settings.ground_enabled);
    if settings.ground_enabled {
        commands.spawn((
            Name::new("GroundPlane"),
            GroundPlane,
            RaytracingMesh3d(assets.mesh.clone()),
            SolariMaterial3d(assets.material.clone()),
            Transform::default(),
            Visibility::Visible,
            NoCpuCulling,
        ));
    } else {
        for entity in &ground {
            commands.entity(entity).despawn();
        }
    }
}

/// The togglable 1 m reference cube (see [`apply_ref_cube`]).
#[derive(Component)]
struct RefCube;

/// Baked-once reference cube mesh + material.
#[derive(Resource)]
struct RefCubeAssets {
    mesh: Handle<ClusterMesh>,
    material: Handle<StandardSolariMaterial>,
}

impl FromWorld for RefCubeAssets {
    fn from_world(world: &mut World) -> Self {
        let cube = Cuboid::from_length(1.0)
            .mesh()
            .build()
            .with_generated_tangents()
            .expect("ref cube tangents");
        let mesh = world
            .resource_mut::<Assets<ClusterMesh>>()
            .add(ClusterMesh::try_from(&cube).expect("ref cube bake"));
        let material = world.resource_mut::<Assets<StandardSolariMaterial>>().add(
            StandardSolariMaterial {
                base_color: Color::srgb(0.9, 0.2, 0.2),
                perceptual_roughness: 0.6,
                ..default()
            },
        );
        Self { mesh, material }
    }
}

/// Spawn or despawn the reference cube when the "Ref cube" checkbox flips.
/// Base sits on the ground plane (y = 0), offset so it stands beside the scene.
fn apply_ref_cube(
    mut commands: Commands,
    settings: Res<settings::LightSettings>,
    assets: Res<RefCubeAssets>,
    cube: Query<Entity, With<RefCube>>,
    mut last: Local<Option<bool>>,
) {
    if *last == Some(settings.ref_cube_enabled) {
        return;
    }
    *last = Some(settings.ref_cube_enabled);
    if settings.ref_cube_enabled {
        commands.spawn((
            Name::new("RefCube1m"),
            RefCube,
            RaytracingMesh3d(assets.mesh.clone()),
            SolariMaterial3d(assets.material.clone()),
            Transform::from_xyz(3.0, 0.5, 0.0),
            Visibility::Visible,
            NoCpuCulling,
        ));
    } else {
        for entity in &cube {
            commands.entity(entity).despawn();
        }
    }
}

#[derive(Component)]
struct FrameTimeText;

/// Fullscreen loading overlay, shown while the per-submesh `.cluster_mesh` assets stream in.
#[derive(Component)]
struct LoadingScreen;
/// The text line inside [`LoadingScreen`] that reports load progress.
#[derive(Component)]
struct LoadingText;

/// A cluster mesh is "ready" if it loaded (asset-server-managed) or was added directly — directly
/// added assets aren't managed, so `is_loaded_with_dependencies` is false for them.
fn cluster_ready(asset_server: &AssetServer, handle: &Handle<ClusterMesh>) -> bool {
    !asset_server.is_managed(handle) || asset_server.is_loaded_with_dependencies(handle)
}

/// Report `.bsn` load progress on the [`LoadingScreen`], removing it once every spawned
/// [`RaytracingMesh3d`] instance has its [`ClusterMesh`] asset loaded.
///
/// The `.bsn` spawns all instances atomically (each already carrying `RaytracingMesh3d`), so
/// "spawned" is instant — the slow part is streaming in the per-submesh `.cluster_mesh` assets,
/// which is what this tracks.
fn update_loading_screen(
    mut commands: Commands,
    screen: Query<Entity, With<LoadingScreen>>,
    mut text: Query<&mut Text, With<LoadingText>>,
    instances: Query<&RaytracingMesh3d>,
    asset_server: Res<AssetServer>,
) {
    let Ok(screen) = screen.single() else {
        return;
    };
    let total = instances.iter().count();
    // Track the asset-server load state, NOT `Assets<ClusterMesh>`: solari consumes each
    // `ClusterMesh` when it uploads to the GPU, so it vanishes from `Assets` immediately.
    // `is_loaded_with_dependencies` persists past the upload — and is exactly what solari gates
    // instance binding on.
    let loaded = instances
        .iter()
        .filter(|mesh| cluster_ready(&asset_server, &mesh.0))
        .count();

    if total > 0 && loaded == total {
        commands.entity(screen).despawn();
        return;
    }
    if let Ok(mut text) = text.single_mut() {
        text.0 = if total == 0 {
            "Loading scene...".into()
        } else {
            format!("Loading clusters: {loaded}/{total}")
        };
    }
}

/// `.bsn` entities arrive with no visibility components (`RaytracingMesh3d` doesn't require them);
/// give the RT meshes the visibility chain so the instance extract / hierarchy is well-formed.
fn ensure_visibility(
    mut commands: Commands,
    query: Query<Entity, (With<RaytracingMesh3d>, Without<Visibility>)>,
) {
    for entity in &query {
        commands.entity(entity).insert(Visibility::Visible);
    }
}

/// The set of `.bsn` scenes the `scene` argument expanded to, and which one is live. The picker and
/// `[`/`]` keys move `current`; `switch_scene` reacts to the change.
#[derive(Resource)]
pub struct Scenes {
    /// Asset-relative `.bsn` paths (no leading `assets/`), sorted.
    paths: Vec<String>,
    /// Index into `paths` of the scene currently spawned.
    current: usize,
}

impl Scenes {
    /// Expand the path/glob argument(s) (relative to the working directory) into the asset-relative
    /// `.bsn` paths they match, unioned and sorted. A bare argument with no wildcard and no `.bsn`
    /// extension is treated as a prefix (a trailing `*` is added) so partial names like `…/BldgLG`
    /// still match. Non-`.bsn` matches (e.g. cluster-mesh dirs a shell glob also expands to) are
    /// dropped.
    fn resolve(args: &[String]) -> Self {
        let mut paths: Vec<String> = args
            .iter()
            .flat_map(|arg| {
                let pattern = if !arg.contains(['*', '?', '[']) && !arg.ends_with(".bsn") {
                    format!("{arg}*")
                } else {
                    arg.clone()
                };
                glob::glob(&pattern).into_iter().flatten().flatten()
            })
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "bsn"))
            .map(|p| {
                let s = p.to_string_lossy().replace('\\', "/");
                s.strip_prefix("assets/").unwrap_or(&s).to_string()
            })
            .collect();
        paths.sort();
        paths.dedup();
        Self { paths, current: 0 }
    }

    /// The asset-relative path of the live scene.
    fn current_path(&self) -> &str {
        &self.paths[self.current]
    }
}

/// Marks everything belonging to the live scene (its `ScenePatchInstance` root and the loading
/// overlay) so `switch_scene` can despawn the lot when the selection changes.
#[derive(Component)]
struct ActiveScene;

/// A scene-picker row; the payload is the index into [`Scenes::paths`] it selects.
#[derive(Component)]
struct SceneButton(usize);

/// Spawn the live scene: the importer-baked `.bsn` instance plus the fullscreen loading overlay that
/// `update_loading_screen` removes once every cluster has streamed in. Both carry [`ActiveScene`] so
/// a later switch tears them down together.
fn spawn_active_scene(commands: &mut Commands, asset_server: &AssetServer, path: &str) {
    // The `.bsn`: entities arrive carrying `RaytracingMesh3d` + inline `SolariMaterial`, so there's
    // no runtime conversion. `Visibility` on the root gives the child instances' `InheritedVisibility`
    // a parent. The patch applies onto this entity, so despawning it (recursive) drops the scene.
    commands.spawn((
        ActiveScene,
        ScenePatchInstance(asset_server.load(path.to_string())),
        Visibility::Visible,
    ));

    commands
        .spawn((
            ActiveScene,
            LoadingScreen,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(100),
        ))
        .with_children(|parent| {
            parent.spawn((
                LoadingText,
                Text::new("Loading scene..."),
                TextColor(Color::WHITE),
            ));
        });
}

const SCENE_BUTTON_BG: Color = Color::srgba(0.12, 0.12, 0.14, 0.85);
const SCENE_BUTTON_BG_ACTIVE: Color = Color::srgba(0.20, 0.42, 0.80, 0.95);

/// Middle-left panel with one clickable row per matched scene; clicking a row sets it live. The
/// panel is height-capped and scrolls (mouse wheel via [`on_scroll`]) so a long list stays compact.
/// A full-height `Pickable::IGNORE` wrapper vertically centers it without eating camera drags;
/// hovering the panel itself suspends the free camera. Skipped when only one scene matched.
fn spawn_scene_picker(commands: &mut Commands, scenes: &Scenes) {
    if scenes.paths.len() < 2 {
        return;
    }
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(12),
                top: px(0),
                bottom: px(0),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Start,
                ..default()
            },
            Pickable::IGNORE,
            GlobalZIndex(50),
        ))
        .with_children(|wrap| {
            wrap.spawn((
                Node {
                    max_height: percent(70),
                    max_width: px(220),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Stretch,
                    row_gap: px(2),
                    padding: UiRect::all(px(6)),
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
                ScrollPosition::default(),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            ))
            .observe(
                |_: On<Pointer<Over>>, mut state: Single<&mut FreeCameraState>| {
                    state.enabled = false;
                },
            )
            .observe(
                |_: On<Pointer<Out>>, mut state: Single<&mut FreeCameraState>| {
                    state.enabled = true;
                },
            )
            .with_children(|panel| {
                for (i, path) in scenes.paths.iter().enumerate() {
                    let label = std::path::Path::new(path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| path.clone());
                    panel
                        .spawn((
                            SceneButton(i),
                            Node {
                                padding: UiRect::axes(px(8), px(4)),
                                flex_shrink: 0.0,
                                ..default()
                            },
                            BackgroundColor(if i == scenes.current {
                                SCENE_BUTTON_BG_ACTIVE
                            } else {
                                SCENE_BUTTON_BG
                            }),
                        ))
                        .observe(move |_: On<Pointer<Click>>, mut scenes: ResMut<Scenes>| {
                            scenes.current = i;
                        })
                        .with_child((
                            Text::new(label),
                            TextFont {
                                font_size: 13.0.into(),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                }
            });
        });
}

/// A wheel-scroll event injected into the hovered UI subtree; propagates to the nearest scrollable
/// ancestor (the picker panel). Mirrors bevy's `ui/scroll` example.
#[derive(EntityEvent, Debug)]
#[entity_event(propagate, auto_propagate)]
struct Scroll {
    entity: Entity,
    delta: Vec2,
}

/// One wheel "line" in logical px.
const SCROLL_LINE: f32 = 18.0;

/// Turn mouse-wheel input into [`Scroll`] events on whatever UI is under the pointer.
fn send_scroll_events(
    mut wheel: MessageReader<MouseWheel>,
    hover_map: Res<HoverMap>,
    mut commands: Commands,
) {
    for ev in wheel.read() {
        let mut delta = -Vec2::new(ev.x, ev.y);
        if ev.unit == MouseScrollUnit::Line {
            delta *= SCROLL_LINE;
        }
        for pointer_map in hover_map.values() {
            for entity in pointer_map.keys().copied() {
                commands.trigger(Scroll { entity, delta });
            }
        }
    }
}

/// Apply a [`Scroll`] to a scrollable node's [`ScrollPosition`], clamping at the ends.
fn on_scroll(mut scroll: On<Scroll>, mut nodes: Query<(&mut ScrollPosition, &Node, &ComputedNode)>) {
    let Ok((mut pos, node, computed)) = nodes.get_mut(scroll.entity) else {
        return;
    };
    let max_offset = (computed.content_size() - computed.size()) * computed.inverse_scale_factor();
    let delta = &mut scroll.delta;
    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0.0 {
        let at_end = if delta.y > 0.0 { pos.y >= max_offset.y } else { pos.y <= 0.0 };
        if !at_end {
            pos.y += delta.y;
            delta.y = 0.0;
        }
    }
    if *delta == Vec2::ZERO {
        scroll.propagate(false);
    }
}

/// Despawn the previous scene and spawn the selected one whenever [`Scenes::current`] changes (and
/// on the first frame, to spawn the initial scene). Clears the ray tracer's temporal history so the
/// new scene doesn't ghost from the old.
fn switch_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    scenes: Res<Scenes>,
    active: Query<Entity, With<ActiveScene>>,
    mut cameras: Query<&mut CameraReset, With<SolariCamera>>,
    mut last: Local<Option<usize>>,
) {
    if *last == Some(scenes.current) {
        return;
    }
    let first = last.is_none();
    *last = Some(scenes.current);

    for entity in &active {
        commands.entity(entity).despawn();
    }
    spawn_active_scene(&mut commands, &asset_server, scenes.current_path());

    if !first {
        for mut reset in &mut cameras {
            reset.history = true;
        }
    }
}

/// Tint the picker rows so the live scene's row reads as selected.
fn highlight_scene_buttons(
    scenes: Res<Scenes>,
    mut rows: Query<(&SceneButton, &mut BackgroundColor)>,
) {
    if !scenes.is_changed() {
        return;
    }
    for (button, mut bg) in &mut rows {
        bg.0 = if button.0 == scenes.current {
            SCENE_BUTTON_BG_ACTIVE
        } else {
            SCENE_BUTTON_BG
        };
    }
}

pub fn setup(mut commands: Commands, args: Res<Args>, scenes: Res<Scenes>) {
    // The scene itself (and its loading overlay) is spawned by `switch_scene`, which also handles
    // swapping it out when the picker selection changes. Here we just build the persistent rig:
    // the scene picker, sun, camera and frame-time overlay. The ground is a `GroundAssets` resource
    // (`FromWorld`), spawned/despawned on the "Ground" checkbox by `apply_ground`.
    spawn_scene_picker(&mut commands, &scenes);

    // Sun: a `SolariDirectionLight` (own light type, direction resolved from the GPU transform
    // table). Shadows/cascades are raster concepts — the ray tracer traces shadow rays directly.
    // The light-settings panel (top right) steers it via the `Sun` marker.
    commands.spawn((
        Transform::from_rotation(DQuat::from_euler(
            EulerRot::XYZ,
            PI * -0.35f64,
            PI * -0.13,
            0.0,
        )),
        SolariDirectionLight {
            color: Color::srgb(1.0, 0.87, 0.78),
            illuminance: lux::FULL_DAYLIGHT,
            ..default()
        },
        settings::Sun,
    ));

    // Camera: driven by the ray tracer. `STORAGE_BINDING` lets the RT compute pass write the view's
    // main texture; `ClusterConfig::None` skips raster light clustering; `SolariAtmosphere`'s
    // metres-scale defaults supply the sky (background + IBL on ray miss).
    commands.spawn((
        Msaa::Off,
        Camera3d::default(),
        Hdr,
        Transform::from_xyz(12.0, 2.0, 12.0).looking_at(DVec3::new(0.0, 2.5, 0.0), Vec3::Y),
        Projection::Perspective(PerspectiveProjection {
            fov: PI_f32 / 3.0,
            near: 0.1,
            far: 1000.0,
            aspect_ratio: 1.0,
            ..default()
        }),
        FreeCamera::default(),
        Spin,
        SolariCamera::default(),
        SolariAtmosphere::default(),
        ClusterConfig::None,
        CameraMainTextureUsages::default().with(TextureUsages::STORAGE_BINDING),
    ));

    if !args.hide_frame_time {
        commands
            .spawn((
                Node {
                    left: px(1.5),
                    top: px(1.5),
                    ..default()
                },
                GlobalZIndex(-1),
            ))
            .with_children(|parent| {
                parent.spawn((Text::new(""), TextColor(Color::BLACK), FrameTimeText));
            });
        commands.spawn(Node::default()).with_children(|parent| {
            parent.spawn((Text::new(""), TextColor(Color::WHITE), FrameTimeText));
        });
    }
}

fn handle_input(input: Res<ButtonInput<KeyCode>>, mut scenes: ResMut<Scenes>) {
    // `[`/`]` cycle the live scene (wrapping), matching the picker rows.
    let n = scenes.paths.len();
    if n > 1 {
        if input.just_pressed(KeyCode::BracketRight) {
            scenes.current = (scenes.current + 1) % n;
        }
        if input.just_pressed(KeyCode::BracketLeft) {
            scenes.current = (scenes.current + n - 1) % n;
        }
    }
}
