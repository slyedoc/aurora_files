//! `animgraph_editor` — bevy_animation_graph's editor on feathers, rendering on aurora.
//!
//! ```text
//! cargo run --release -p animgraph_editor -- -a /mnt/code/p/zero/assets
//! ```
//!
//! R1 of the ladder in `animgraph_editor.md`: the shell and the asset browser. The left pane
//! lists every graph / clip / skeleton / state machine under the asset root; picking one loads
//! it and points `bevy_feathers_inspector` at the loaded ASSET, which is what R0 taught it to
//! address. The centre pane is where the preview (R2) and the node canvas (R3) go.

use bevy::{
    animation::{AnimatedBy, AnimationTargetId},
    feathers::{
        controls::{slider_bundle, FeathersSliderProps},
        dark_theme::create_dark_theme,
        theme::{ThemedText, UiTheme},
    },
    feathers_inspector::BuildAssetInspector,
    prelude::*,
    ui::{
        AlignItems, BackgroundColor, FlexDirection, JustifyContent, Overflow, PositionType,
        UiRect, Val,
    },
    ui_widgets::{observe, slider_self_update, SliderValue, ValueChange},
};
use bevy_animation_graph::{
    core::{
        animation_clip::GraphClip,
        animation_graph::{AnimationGraph, NodeId},
        animation_graph_player::AnimationGraphPlayer,
        edge_data::DataValue,
        skeleton::Skeleton,
        state_machine::high_level::StateMachine,
    },
    AnimationGraphPlugin,
};
use bevy_aurora::{
    auto_exposure::AuroraExposure,
    dev_shaders::DevShaderPlugin,
    dev_ui::DevUIPlugin,
    ray_default_plugins::RayDefaultPlugins,
    util::{ScreenshotExt, TimeoutAppExt},
};
use clap::Parser;

use std::any::TypeId;
use std::path::{Path, PathBuf};

#[derive(Parser, Resource, Clone)]
#[command(name = "animgraph_editor", about = "Animation graph editor (feathers/aurora)")]
struct Args {
    /// Asset root to browse. Defaults to `$BEVY_ASSET_ROOT`, else a cwd that has `assets/`.
    #[arg(long, short = 'a')]
    assets: Option<PathBuf>,

    /// Open this asset on startup, as an asset-relative path
    /// (`anim/human/locomotion.animgraph.ron`). Scriptable, and how the editor is smoke-tested.
    #[arg(long, short = 'o')]
    open: Option<String>,

    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,
}

/// One browsable file under the asset root.
#[derive(Clone)]
struct Entry {
    /// Asset-server path, e.g. `anim/human/locomotion.animgraph.ron`.
    path: String,
    kind: Kind,
}

/// Which loader owns an extension, and therefore which asset type the inspector binds to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Graph,
    Clip,
    Skeleton,
    StateMachine,
}

impl Kind {
    /// Longest-suffix match, because every one of these ends in `.ron`.
    fn of(name: &str) -> Option<Self> {
        if name.ends_with(".animgraph.ron") {
            Some(Self::Graph)
        } else if name.ends_with(".anim.ron") {
            Some(Self::Clip)
        } else if name.ends_with(".skn.ron") {
            Some(Self::Skeleton)
        } else if name.ends_with(".fsm.ron") {
            Some(Self::StateMachine)
        } else {
            None
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Graph => "graph",
            Self::Clip => "clip",
            Self::Skeleton => "skeleton",
            Self::StateMachine => "fsm",
        }
    }

    fn type_id(self) -> TypeId {
        match self {
            Self::Graph => TypeId::of::<AnimationGraph>(),
            Self::Clip => TypeId::of::<GraphClip>(),
            Self::Skeleton => TypeId::of::<Skeleton>(),
            Self::StateMachine => TypeId::of::<StateMachine>(),
        }
    }
}

#[derive(Resource, Default)]
struct Library {
    entries: Vec<Entry>,
    /// The same entries as a directory tree, which is what the browser draws.
    root: Dir,
}

/// One directory in the browser tree. `BTreeMap` so children come out sorted without a pass.
#[derive(Default)]
struct Dir {
    dirs: std::collections::BTreeMap<String, Dir>,
    files: Vec<Entry>,
}

impl Dir {
    fn insert(&mut self, entry: Entry) {
        let mut cursor = self;
        let segments: Vec<&str> = entry.path.split('/').collect();
        for segment in &segments[..segments.len().saturating_sub(1)] {
            cursor = cursor.dirs.entry((*segment).to_string()).or_default();
        }
        cursor.files.push(entry);
    }
}

/// Which directories are expanded, keyed by their path from the root ("anim/human").
#[derive(Resource, Default)]
struct Expanded(std::collections::HashSet<String>);

/// Set when the tree needs respawning (a directory was toggled).
#[derive(Resource, Default)]
struct BrowserDirty(bool);

/// The pane the inspector builds into.
#[derive(Resource)]
struct InspectorPane(Entity);

/// The title strip above the inspector.
#[derive(Component)]
struct SelectionLabel;

/// The centre pane's input-slider column.
#[derive(Component)]
struct InputsHost;

/// The browser pane, so the pre-spawned rows can be parented to it.
#[derive(Component)]
struct Browser;

/// The inspector pane's parent, so the pre-spawned body can be parented to it.
#[derive(Component)]
struct InspectorHost;

/// The rig the preview plays on, and the graph currently armed on it.
#[derive(Resource)]
struct Preview {
    /// Prefab root, so it can be despawned when the rig changes.
    root: Entity,
    armature: Option<Entity>,
    graph: Option<Handle<AnimationGraph>>,
    skeleton: Handle<Skeleton>,
}

/// One generated slider's binding: which graph input it drives.
#[derive(Component)]
struct GraphInput(String);

/// The pane the generated input sliders live in.
#[derive(Resource)]
struct InputsPane(Entity);

/// A handle kept alive while its asset is open, plus what to bind once it finishes loading.
#[derive(Resource)]
struct Opening {
    handle: UntypedHandle,
    kind: Kind,
    path: String,
    bound: bool,
}

fn main() {
    // Same root rule as the `bsn` viewer: explicit flag > $BEVY_ASSET_ROOT > a cwd with an
    // assets/ dir > this workspace.
    let args = Args::parse();
    if let Some(dir) = &args.assets {
        // SAFETY: single-threaded — before App construction spawns anything.
        unsafe { std::env::set_var("BEVY_ASSET_ROOT", dir.parent().unwrap_or(dir)) };
    } else if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
        let root = if Path::new("assets").is_dir() {
            std::env::current_dir().expect("cwd")
        } else {
            PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        };
        unsafe { std::env::set_var("BEVY_ASSET_ROOT", &root) };
    }

    let mut app = App::new();
    app.insert_resource(UiTheme(create_dark_theme()));
    app.add_plugins((
        RayDefaultPlugins.set(bevy::log::LogPlugin {
            filter: util::LOG_FILTER.into(),
            ..default()
        }),
        DevShaderPlugin,
        // aurora's UI pass owns the draw params every bevy_ui node extracts through; without
        // it every UI system fails validation with "Resource does not exist". It also brings
        // FeathersPlugins and FeathersInspectorPlugins (guarded), so this app adds neither —
        // and it keeps a theme the app inserted first, which is why that comes before.
        DevUIPlugin,
        // AnimationGraphPlugin runs a deferred-gizmo system (debug bones) that needs
        // GizmoConfigStore; the `bevy_gizmos` feature alone does not install it.
        bevy::gizmos::GizmoPlugin,
        AnimationGraphPlugin::default(),
    ));
    app.add_screenshot(KeyCode::F12);
    app.add_timeout_exit(args.timeout, 60.0);
    app.insert_resource(args);
    app.init_resource::<Library>();
    app.init_resource::<Expanded>();
    // Start with every top-level directory open, so the tree is not a wall of `+`.
    app.insert_resource(BrowserDirty(true));
    app.add_systems(Startup, (scan_library, setup_ui, attach_panes).chain());
    app.add_systems(
        Update,
        (rebuild_browser, bind_when_loaded, arm_preview, draw_canvas),
    );
    app.run();
}

/// Walk the asset root for everything the editor can open. Asset paths are root-relative with
/// `/` separators, which is what the asset server wants on every platform.
fn scan_library(mut library: ResMut<Library>, mut expanded: ResMut<Expanded>) {
    let root = PathBuf::from(std::env::var_os("BEVY_ASSET_ROOT").expect("set in main"))
        .join("assets");
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(kind) = Kind::of(name) else { continue };
            let Ok(rel) = path.strip_prefix(&root) else {
                continue;
            };
            library.entries.push(Entry {
                path: rel.to_string_lossy().replace('\\', "/"),
                kind,
            });
        }
    }
    library.entries.sort_by(|a, b| a.path.cmp(&b.path));
    let entries = library.entries.clone();
    for entry in entries {
        library.root.insert(entry);
    }
    for name in library.root.dirs.keys() {
        expanded.0.insert(name.clone());
    }
    info!(
        "library: {} assets under {}",
        library.entries.len(),
        root.display()
    );
}

fn setup_ui(mut commands: Commands, assets: Res<AssetServer>, args: Res<Args>) {
    // aurora's render_frame wants exactly one of ITS cameras, so even a UI-only shell needs a
    // 3d one. It also becomes the preview camera at R2.
    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        AuroraExposure::SUNLIGHT,
        // Pulled back and offset: the browser and inspector eat the left and right thirds, so
        // the rig is framed into what is left rather than centred on the window.
        Transform::from_xyz(-0.35, 1.15, -4.6).looking_at(Vec3::new(-0.35, 0.95, 0.0), Vec3::Y),
    ));

    // The preview rig: the mannequin, because its clips are an identity retarget and so show
    // a graph as authored. `Locomotion`-style arming is done here rather than pulled from
    // zero, which this workspace does not depend on.
    let root = commands
        .spawn((
            Name::new("preview rig"),
            ScenePatchInstance(assets.load("ual/Mannequin.bsn")),
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
            Visibility::Visible,
        ))
        .id();
    commands.insert_resource(Preview {
        root,
        armature: None,
        graph: None,
        skeleton: assets.load("ual/Mannequin.skn.ron"),
    });

    if let Some(path) = &args.open {
        if let Some(kind) = Kind::of(path) {
            let handle: UntypedHandle = match kind {
                Kind::Graph => assets.load::<AnimationGraph>(path).untyped(),
                Kind::Clip => assets.load::<GraphClip>(path).untyped(),
                Kind::Skeleton => assets.load::<Skeleton>(path).untyped(),
                Kind::StateMachine => assets.load::<StateMachine>(path).untyped(),
            };
            commands.insert_resource(Opening {
                handle,
                kind,
                path: path.clone(),
                bound: false,
            });
        } else {
            warn!("--open {path}: not a type this editor knows");
        }
    }

    // Left: the browser. Centre: preview + the generated input sliders. Right: the inspector.
    let inspector = commands
        .spawn((
            Name::new("inspector body"),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(8.0)),
                overflow: Overflow::scroll_y(),
                flex_grow: 1.0,
                ..default()
            },
        ))
        .id();
    commands.insert_resource(InspectorPane(inspector));

    commands.spawn((
        Name::new("root"),
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            ..default()
        },
        Children::spawn((
            // browser — rows are added below, they already exist
            Spawn((
                Name::new("browser"),
                Browser,
                // The rig renders BEHIND the ui, so a pane without a background reads as text
                // floating on the mannequin. Near-black rather than a theme token: this is a
                // backdrop, not a surface the theme has an opinion about.
                BackgroundColor(Color::srgba(0.06, 0.06, 0.07, 0.94)),
                Node {
                    width: Val::Px(420.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    // Top padding clears aurora's dev panel, which is an overlay pinned to the
                    // top-left corner and would otherwise sit on the first rows.
                    padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(160.0), Val::Px(8.0)),
                    overflow: Overflow::scroll_y(),
                    ..default()
                },
            )),
            // centre — R2 preview and R3 canvas land here
            Spawn((
                Name::new("centre"),
                Node {
                    flex_grow: 1.0,
                    height: Val::Percent(100.0),
                    align_items: AlignItems::FlexEnd,
                    justify_content: JustifyContent::FlexEnd,
                    ..default()
                },
                Children::spawn((
                    Spawn((
                        Name::new("canvas"),
                        Canvas,
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(0.0),
                            top: Val::Px(0.0),
                            right: Val::Px(0.0),
                            bottom: Val::Px(0.0),
                            ..default()
                        },
                    )),
                    Spawn((
                    Name::new("inputs"),
                    InputsHost,
                    BackgroundColor(Color::srgba(0.06, 0.06, 0.07, 0.86)),
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(4.0),
                        padding: UiRect::all(Val::Px(8.0)),
                        width: Val::Px(320.0),
                        ..default()
                    },
                )),
                )),
            )),
            // inspector
            Spawn((
                Name::new("inspector"),
                InspectorHost,
                BackgroundColor(Color::srgba(0.06, 0.06, 0.07, 0.94)),
                Node {
                    width: Val::Px(380.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
                Children::spawn((
                    Spawn((
                        Name::new("selection"),
                        SelectionLabel,
                        Text::new("nothing selected"),
                        ThemedText,
                        Node {
                            padding: UiRect::all(Val::Px(8.0)),
                            ..default()
                        },
                    )),
                )),
            )),
        )),
    ));
}

/// Parent the pre-spawned inspector body into its pane.
fn attach_panes(
    mut commands: Commands,
    pane: Res<InspectorPane>,
    host: Single<Entity, With<InspectorHost>>,
) {
    commands.entity(*host).add_child(pane.0);
}

/// Respawn the browser tree whenever a directory is toggled (and once at startup).
fn rebuild_browser(
    mut commands: Commands,
    mut dirty: ResMut<BrowserDirty>,
    library: Res<Library>,
    expanded: Res<Expanded>,
    browser: Single<(Entity, Option<&Children>), With<Browser>>,
) {
    if !dirty.0 {
        return;
    }
    dirty.0 = false;
    let (browser, children) = *browser;
    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }
    let mut rows = Vec::new();
    spawn_dir(&mut commands, &library.root, "", 0, &expanded.0, &mut rows);
    commands.entity(browser).add_children(&rows);
}

/// One row per directory, then one per file, depth-first. Only an expanded directory
/// recurses, so a collapsed subtree costs nothing.
fn spawn_dir(
    commands: &mut Commands,
    dir: &Dir,
    path: &str,
    depth: usize,
    expanded: &std::collections::HashSet<String>,
    rows: &mut Vec<Entity>,
) {
    let indent = Val::Px(6.0 + depth as f32 * 14.0);
    for (name, child) in &dir.dirs {
        let child_path = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        let open = expanded.contains(&child_path);
        // ASCII markers: this shell inherits whatever font feathers ships, and a missing
        // glyph reads as tofu rather than as a disclosure arrow.
        let label = format!("{} {}/", if open { "-" } else { "+" }, name);
        let toggle_path = child_path.clone();
        let row = commands
            .spawn((
                Node {
                    padding: UiRect::new(indent, Val::Px(6.0), Val::Px(3.0), Val::Px(3.0)),
                    ..default()
                },
                Children::spawn(Spawn((
                    Text::new(label),
                    ThemedText,
                    TextLayout {
                        linebreak: bevy::text::LineBreak::NoWrap,
                        ..default()
                    },
                ))),
            ))
            .observe(
                move |_: On<PointerClick>,
                      mut expanded: ResMut<Expanded>,
                      mut dirty: ResMut<BrowserDirty>| {
                    if !expanded.0.remove(&toggle_path) {
                        expanded.0.insert(toggle_path.clone());
                    }
                    dirty.0 = true;
                },
            )
            .id();
        rows.push(row);
        if open {
            spawn_dir(commands, child, &child_path, depth + 1, expanded, rows);
        }
    }
    for entry in &dir.files {
        let entry = entry.clone();
        let leaf = entry.path.rsplit('/').next().unwrap_or(&entry.path).to_string();
        let label = format!("  {}   [{}]", leaf, entry.kind.label());
        let row = commands
            .spawn((
                Node {
                    padding: UiRect::new(indent, Val::Px(6.0), Val::Px(3.0), Val::Px(3.0)),
                    ..default()
                },
                Children::spawn(Spawn((
                    Text::new(label),
                    ThemedText,
                    TextLayout {
                        linebreak: bevy::text::LineBreak::NoWrap,
                        ..default()
                    },
                ))),
            ))
            .observe(
                move |_: On<PointerClick>, assets: Res<AssetServer>, mut commands: Commands| {
                    // Typed load per kind so the right loader runs; the binding itself is
                    // untyped, keyed on the asset id.
                    let handle: UntypedHandle = match entry.kind {
                        Kind::Graph => assets.load::<AnimationGraph>(&entry.path).untyped(),
                        Kind::Clip => assets.load::<GraphClip>(&entry.path).untyped(),
                        Kind::Skeleton => assets.load::<Skeleton>(&entry.path).untyped(),
                        Kind::StateMachine => assets.load::<StateMachine>(&entry.path).untyped(),
                    };
                    commands.insert_resource(Opening {
                        handle,
                        kind: entry.kind,
                        path: entry.path.clone(),
                        bound: false,
                    });
                },
            )
            .id();
        rows.push(row);
    }
}

/// An asset is only inspectable once it has finished loading, so binding waits for it.
fn bind_when_loaded(
    mut commands: Commands,
    opening: Option<ResMut<Opening>>,
    assets: Res<AssetServer>,
    pane: Res<InspectorPane>,
    mut label: Single<&mut Text, With<SelectionLabel>>,
) {
    let Some(mut opening) = opening else { return };
    if opening.bound {
        return;
    }
    if !assets.is_loaded(&opening.handle) {
        return;
    }
    opening.bound = true;
    label.0 = opening.path.clone();
    commands.queue(BuildAssetInspector {
        asset_id: opening.handle.id(),
        type_id: opening.kind.type_id(),
        panel: pane.0,
    });
    if opening.kind == Kind::Graph {
        commands.insert_resource(ArmGraph(opening.handle.clone().typed::<AnimationGraph>()));
    }
    info!("opened {}", opening.path);
}

/// `GraphInputPin`'s `Debug` is `Passthrough("speed")`; the pin NAME is what belongs on a
/// slider. No public accessor for it, so unwrap the one shape it prints.
fn pin_name(pin: &impl std::fmt::Debug) -> String {
    let text = format!("{pin:?}");
    text.split_once('"')
        .and_then(|(_, rest)| rest.rsplit_once('"'))
        .map(|(name, _)| name.to_string())
        .unwrap_or(text)
}

/// The canvas pane, which the node boxes and links are spawned into.
#[derive(Component)]
struct Canvas;

/// A node box on the canvas; clicking one points the inspector at that node.
#[derive(Component)]
struct CanvasNode(String);

/// Node box geometry, in canvas pixels. Boxes are a fixed size so link endpoints can be
/// computed without waiting for layout to measure anything.
const NODE_W: f32 = 200.0;
const NODE_H: f32 = 40.0;
const LINK_THICKNESS: f32 = 2.0;

/// A graph waiting to be put on the preview rig.
#[derive(Resource)]
struct ArmGraph(Handle<AnimationGraph>);

/// Find the rig's armature (once it streams in), swap in an `AnimationGraphPlayer` for the
/// requested graph, and rebuild the input sliders from the graph's own `io_spec`.
fn arm_preview(
    mut commands: Commands,
    arm: Option<Res<ArmGraph>>,
    mut preview: ResMut<Preview>,
    graphs: Res<Assets<AnimationGraph>>,
    names: Query<&Name>,
    children: Query<&Children>,
    host: Single<(Entity, Option<&Children>), With<InputsHost>>,
) {
    let Some(arm) = arm else { return };
    let Some(graph) = graphs.get(&arm.0) else {
        return;
    };
    // Hydrate the name-path components a text `.bsn` cannot carry, exactly as zero's
    // locomotion module does, then bind the player.
    let Some(armature) = children
        .get(preview.root)
        .ok()
        .and_then(|kids| {
            kids.iter()
                .find(|&k| names.get(k).is_ok_and(|n| n.as_str() == "Armature"))
        })
    else {
        return;
    };
    let Ok(bones) = children.get(armature) else {
        return;
    };
    let root_name = Name::new("Armature");
    commands.entity(armature).insert((
        AnimationTargetId::from_names([root_name.clone()].iter()),
        AnimatedBy(preview.root),
    ));
    let mut stack: Vec<(Entity, Vec<Name>)> = bones
        .iter()
        .map(|bone| (bone, vec![root_name.clone()]))
        .collect();
    while let Some((bone, path)) = stack.pop() {
        let Ok(name) = names.get(bone) else { continue };
        let mut path = path;
        path.push(name.clone());
        commands.entity(bone).insert((
            AnimationTargetId::from_names(path.iter()),
            AnimatedBy(armature),
        ));
        if let Ok(kids) = children.get(bone) {
            stack.extend(kids.iter().map(|kid| (kid, path.clone())));
        }
    }
    commands
        .entity(armature)
        .remove::<AnimationPlayer>()
        .insert(AnimationGraphPlayer::new(preview.skeleton.clone()).with_graph(arm.0.clone()));
    preview.armature = Some(armature);
    preview.graph = Some(arm.0.clone());

    // One row per F32 input, seeded from the graph's own default. `default_data` rather than
    // `io_spec.input_data` because the spec's map has no public reader — and the defaults are
    // what a slider wants to start at anyway.
    let (host_entity, existing) = *host;
    if let Some(existing) = existing {
        for child in existing.iter() {
            commands.entity(child).despawn();
        }
    }
    let mut rows = Vec::new();
    let mut inputs: Vec<(String, f32)> = graph
        .default_data
        .iter()
        .filter_map(|(pin, value)| match value {
            DataValue::F32(v) => Some((pin_name(pin), *v)),
            _ => None,
        })
        .collect();
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, value) in inputs {
        // Range is a guess until a graph declares one: 0 ..= max(4, 2x the default) covers a
        // speed in m/s and a 0..1 factor alike without clipping either.
        let max = (value * 2.0).max(4.0);
        let pin = name.clone();
        let readout = commands
            .spawn((
                Text::new(format!("{name}  {value:.2}")),
                ThemedText,
                GraphInput(pin.clone()),
            ))
            .id();
        let observer_pin = pin.clone();
        #[expect(
            deprecated,
            reason = "the BSN slider() builds a scene; these rows are spawned imperatively"
        )]
        let slider = commands
            .spawn((
                // `SliderValue` rides the bundle already, so it goes in as an INSERT below —
                // passing it as an override duplicates the component and panics the spawn.
                slider_bundle(FeathersSliderProps { min: 0.0, max }, GraphInput(pin.clone())),
                // Feathers sliders are INERT without this: the thumb only moves because
                // `slider_self_update` writes the new SliderValue back onto the entity.
                observe(slider_self_update),
            ))
            .insert(SliderValue(value))
            .observe(
                move |change: On<ValueChange<f32>>,
                      mut players: Query<&mut AnimationGraphPlayer>,
                      mut texts: Query<(&mut Text, &GraphInput)>| {
                    let v = change.value;
                    for mut player in &mut players {
                        player.set_input_data(observer_pin.clone(), DataValue::F32(v));
                    }
                    for (mut text, input) in &mut texts {
                        if input.0 == observer_pin {
                            text.0 = format!("{observer_pin}  {v:.2}");
                        }
                    }
                },
            )
            .id();
        let row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            })
            .add_children(&[readout, slider])
            .id();
        rows.push(row);
    }
    commands.entity(host_entity).add_children(&rows);
    commands.remove_resource::<ArmGraph>();
    info!("preview armed, {} f32 inputs", rows.len());
    commands.insert_resource(DrawCanvas(arm.0.clone()));
}

/// A graph waiting to be drawn on the canvas.
#[derive(Resource)]
struct DrawCanvas(Handle<AnimationGraph>);

/// R3: draw the graph read-only. Node boxes sit at `editor_metadata.node_positions`; links are
/// MANHATTAN-routed, three plain rectangles per edge, so the whole canvas is ordinary bevy_ui
/// with no line primitive and no new render pass.
fn draw_canvas(
    mut commands: Commands,
    draw: Option<Res<DrawCanvas>>,
    graphs: Res<Assets<AnimationGraph>>,
    canvas: Single<(Entity, Option<&Children>), With<Canvas>>,
) {
    let Some(draw) = draw else { return };
    let Some(graph) = graphs.get(&draw.0) else {
        return;
    };
    let (canvas_entity, existing) = *canvas;
    if let Some(existing) = existing {
        for child in existing.iter() {
            commands.entity(child).despawn();
        }
    }

    // Where each node sits. A graph saved by upstream's editor carries positions; a
    // hand-written one (zero's locomotion graph) has none, so fall back to a grid — the
    // layout is wrong but every node and link is still visible and inspectable.
    let mut placed: std::collections::HashMap<NodeId, Vec2> = std::collections::HashMap::new();
    let mut ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    // A stable order so the grid fallback does not reshuffle between runs.
    ids.sort_by_key(|id| format!("{id:?}"));
    for (i, id) in ids.iter().enumerate() {
        let pos = graph
            .editor_metadata
            .node_positions
            .get(id)
            .copied()
            // The loader gives every node a position whether the file had one or not, so an
            // absent layout arrives as a pile at the origin rather than as `None`.
            .filter(|p| *p != Vec2::ZERO)
            .unwrap_or_else(|| {
                Vec2::new(
                    40.0 + (i / 6) as f32 * (NODE_W + 40.0),
                    40.0 + (i % 6) as f32 * (NODE_H + 28.0),
                )
            });
        placed.insert(*id, pos);
    }

    let mut children = Vec::new();

    // Links first, so node boxes draw over them.
    for (target, source) in graph.edges_inverted.iter() {
        let (Some(from), Some(to)) = (source_pos(source, &placed), target_pos(target, &placed))
        else {
            continue;
        };
        children.extend(manhattan(&mut commands, from, to));
    }

    for id in &ids {
        let Some(node) = graph.nodes.get(id) else {
            continue;
        };
        let pos = placed[id];
        let title = if node.name.is_empty() {
            node.inner.display_name()
        } else {
            format!("{}  ({})", node.name, node.inner.display_name())
        };
        let node_id = format!("{id:?}");
        let boxed = commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(pos.x),
                    top: Val::Px(pos.y),
                    width: Val::Px(NODE_W),
                    height: Val::Px(NODE_H),
                    align_items: AlignItems::Center,
                    padding: UiRect::horizontal(Val::Px(8.0)),
                    // A long node name must not spill across its neighbours.
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.16, 0.17, 0.20, 0.98)),
                CanvasNode(node_id.clone()),
                Children::spawn(Spawn((
                    Text::new(title),
                    ThemedText,
                    TextLayout {
                        linebreak: bevy::text::LineBreak::NoWrap,
                        ..default()
                    },
                ))),
            ))
            .id();
        children.push(boxed);
    }

    commands.entity(canvas_entity).add_children(&children);
    commands.remove_resource::<DrawCanvas>();
    info!(
        "canvas: {} nodes, {} links",
        graph.nodes.len(),
        graph.edges_inverted.len()
    );
}

/// Right edge of the source node (or the graph input column, for a graph-level source).
fn source_pos(
    source: &bevy_animation_graph::core::animation_graph::SourcePin,
    placed: &std::collections::HashMap<NodeId, Vec2>,
) -> Option<Vec2> {
    use bevy_animation_graph::core::animation_graph::SourcePin as S;
    match source {
        S::NodeData(id, _) | S::NodeTime(id) => {
            placed.get(id).map(|p| *p + Vec2::new(NODE_W, NODE_H * 0.5))
        }
        // Graph inputs have no box yet; park them on a left-hand rail.
        S::InputData(_) | S::InputTime(_) => Some(Vec2::new(8.0, 8.0)),
    }
}

/// Left edge of the target node (or the output rail).
fn target_pos(
    target: &bevy_animation_graph::core::animation_graph::TargetPin,
    placed: &std::collections::HashMap<NodeId, Vec2>,
) -> Option<Vec2> {
    use bevy_animation_graph::core::animation_graph::TargetPin as T;
    match target {
        T::NodeData(id, _) | T::NodeTime(id, _) => {
            placed.get(id).map(|p| *p + Vec2::new(0.0, NODE_H * 0.5))
        }
        T::OutputData(_) | T::OutputTime => None,
    }
}

/// Three rectangles: out from the source, across, then in to the target. No rotation, so this
/// is plain bevy_ui — which is why the canvas needed no rendering work.
fn manhattan(commands: &mut Commands, from: Vec2, to: Vec2) -> Vec<Entity> {
    let mid_x = (from.x + to.x) * 0.5;
    let color = Color::srgba(0.35, 0.55, 0.85, 0.9);
    let mut rect = |x: f32, y: f32, w: f32, h: f32| {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(x),
                    top: Val::Px(y),
                    width: Val::Px(w.max(LINK_THICKNESS)),
                    height: Val::Px(h.max(LINK_THICKNESS)),
                    ..default()
                },
                BackgroundColor(color),
            ))
            .id()
    };
    vec![
        rect(from.x.min(mid_x), from.y, (mid_x - from.x).abs(), LINK_THICKNESS),
        rect(mid_x, from.y.min(to.y), LINK_THICKNESS, (to.y - from.y).abs()),
        rect(mid_x.min(to.x), to.y, (to.x - mid_x).abs(), LINK_THICKNESS),
    ]
}
