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
    input::mouse::MouseScrollUnit,
    prelude::*,
    reflect::{PartialReflect, ReflectRef},
    ui::{
        AlignItems, BackgroundColor, ComputedNode, FlexDirection, JustifyContent, Overflow,
        PositionType, UiRect, Val,
    },
    ui_widgets::{observe, slider_self_update, SliderValue, ValueChange},
};
use bevy_animation_graph::{
    core::{
        animation_clip::GraphClip,
        animation_graph::{
            serial::AnimationGraphSerializer, AnimationGraph, NodeId, SourcePin, TargetPin,
        },
        animation_graph_player::AnimationGraphPlayer,
        animation_node::AnimationNode,
        context::spec_context::{NodeInput, NodeOutput, NodeSpec, SpecResources},
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

    /// Select this node by name once `--open` loads, as a click would. Scriptable, and how
    /// the node-parameter panel is smoke-tested.
    #[arg(long, short = 's')]
    select: Option<String>,

    /// Write the computed layout back into `--open`'s `editor_metadata` and exit. Batch
    /// re-layout for a hand-written graph, and how the save path is smoke-tested — a
    /// screenshot run cannot press Ctrl+S.
    #[arg(long)]
    save_layout: bool,

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

/// Where the selected node's parameters are listed.
#[derive(Component)]
struct NodeParamsHost;

/// The canvas node the inspector is showing, and whether that list needs rebuilding.
#[derive(Resource, Default)]
struct Selected {
    node: Option<NodeId>,
    dirty: bool,
}

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
        RayDefaultPlugins
            .set(bevy::log::LogPlugin {
                filter: util::LOG_FILTER.into(),
                ..default()
            })
            // Three panes and a node canvas need room; the default window leaves the centre
            // narrower than one node box.
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "animgraph editor".into(),
                    resolution: (1760u32, 990u32).into(),
                    ..default()
                }),
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
    app.init_resource::<CanvasView>();
    app.init_resource::<Selected>();
    // Start with every top-level directory open, so the tree is not a wall of `+`.
    app.insert_resource(BrowserDirty(true));
    app.add_systems(Startup, (scan_library, setup_ui, attach_panes).chain());
    app.add_systems(
        Update,
        (
            rebuild_browser,
            bind_when_loaded,
            arm_preview,
            draw_canvas,
            highlight_selected,
            show_node_params,
            save_graph,
            save_layout_and_exit,
        ),
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
                            // Node boxes are absolutely positioned and pan freely, so without
                            // this a panned graph spills over the browser and the inspector.
                            overflow: Overflow::clip(),
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
                    // The selected node's parameters. Separate from the asset inspector below
                    // because `AnimationGraph::nodes` is `#[reflect(ignore)]`, so no reflection
                    // path reaches a node from the asset root — see `show_node_params`.
                    Spawn((
                        Name::new("node params"),
                        NodeParamsHost,
                        Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(2.0),
                            padding: UiRect::horizontal(Val::Px(8.0)),
                            ..default()
                        },
                    )),
                )),
            )),
        )),
    ));
}

/// Parent the pre-spawned inspector body into its pane, and give the canvas its pan/zoom
/// observers — the canvas is spawned inside a `Children::spawn` tuple, which has nowhere to
/// hang an `.observe()`.
fn attach_panes(
    mut commands: Commands,
    pane: Res<InspectorPane>,
    host: Single<Entity, With<InspectorHost>>,
    canvas: Single<Entity, With<Canvas>>,
) {
    commands.entity(*host).add_child(pane.0);
    commands
        .entity(*canvas)
        // MIDDLE- or right-drag the background to pan. Not the left button: that is the node
        // drag, and a left-drag on the background would then fight it — and a stray press from
        // the window manager on focus arrives as a left drag the moment the window opens.
        .observe(|drag: On<PointerDrag>, mut view: ResMut<CanvasView>| {
            if !matches!(drag.button, PointerButton::Middle | PointerButton::Secondary) {
                return;
            }
            view.pan += drag.delta;
            view.dirty = true;
        })
        // Scroll to zoom, about the centre of the canvas — the pan that is already there is
        // what puts a region under the cursor, so anchoring on the pointer buys little.
        .observe(
            |scroll: On<PointerScroll>,
             mut view: ResMut<CanvasView>,
             nodes: Query<&ComputedNode>| {
                let ticks = match scroll.unit {
                    MouseScrollUnit::Line => scroll.y,
                    // A trackpad reports pixels; 40 of them is about one wheel notch.
                    MouseScrollUnit::Pixel => scroll.y / 40.0,
                };
                let old = view.zoom;
                let new = (old * 1.12f32.powf(ticks)).clamp(0.25, 2.5);
                if new == old {
                    return;
                }
                // `ComputedNode::size` is PHYSICAL pixels; `Val::Px` is logical.
                let centre = nodes
                    .get(scroll.entity)
                    .map(|n| n.size * n.inverse_scale_factor() * 0.5)
                    .unwrap_or(Vec2::splat(400.0));
                // Keep whatever sits at `centre` fixed: px = world * zoom + pan.
                view.pan = centre - (centre - view.pan) * (new / old);
                view.zoom = new;
                view.dirty = true;
            },
        );
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
    graphs: Res<Assets<AnimationGraph>>,
    args: Res<Args>,
    mut view: ResMut<CanvasView>,
    mut selected: ResMut<Selected>,
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
        let handle = opening.handle.clone().typed::<AnimationGraph>();
        if let Some(graph) = graphs.get(&handle) {
            seed_layout(&mut view, graph);
            view.graph = Some(handle.clone());
            view.path = Some(opening.path.clone());
            view.dirty = true;
            if let Some(want) = &args.select {
                selected.node = graph
                    .nodes
                    .iter()
                    .find(|(_, node)| &node.name == want)
                    .map(|(id, _)| *id);
                selected.dirty = true;
                if selected.node.is_none() {
                    warn!("--select {want}: no node by that name");
                }
            }
        }
        commands.insert_resource(ArmGraph(handle));
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

/// A node box on the canvas. Dragging one moves it; the id is what the drag writes back to.
#[derive(Component)]
struct CanvasNode(NodeId);

/// Node box geometry, in CANVAS coordinates — screen pixels are `canvas * zoom + pan`. Boxes
/// are laid out arithmetically rather than measured, so a link endpoint is known the moment
/// the box is spawned instead of a frame later.
const NODE_W: f32 = 210.0;
const HEADER_H: f32 = 22.0;
const PIN_H: f32 = 15.0;
const BOX_PAD: f32 = 5.0;
const LINK_THICKNESS: f32 = 2.0;

/// Canvas view state. The node layout lives HERE rather than in the asset so a drag does not
/// write `Assets<AnimationGraph>` every frame — which would fire `AssetEvent::Modified` at the
/// running player sixty times a second. It reaches the asset only on save.
#[derive(Resource)]
struct CanvasView {
    graph: Option<Handle<AnimationGraph>>,
    /// Asset-relative path of the open graph, which is where a save writes.
    path: Option<String>,
    positions: std::collections::HashMap<NodeId, Vec2>,
    input_pos: Vec2,
    output_pos: Vec2,
    pan: Vec2,
    zoom: f32,
    dirty: bool,
}

impl Default for CanvasView {
    fn default() -> Self {
        Self {
            graph: None,
            path: None,
            positions: std::collections::HashMap::new(),
            input_pos: Vec2::ZERO,
            output_pos: Vec2::ZERO,
            pan: Vec2::splat(24.0),
            zoom: 1.0,
            dirty: false,
        }
    }
}

/// Take the graph's authored layout, or compute one.
///
/// The loader fills in a position for EVERY node whether the file carried one or not, so an
/// unlaid-out graph arrives as a pile at the origin rather than as `None` — hence the
/// all-zero test rather than a per-node one. The fallback is a longest-path layering: a node's
/// column is one past its deepest predecessor, which for a locomotion graph reads left to
/// right in evaluation order.
fn seed_layout(view: &mut CanvasView, graph: &AnimationGraph) {
    view.positions.clear();
    view.pan = Vec2::splat(24.0);
    view.zoom = 1.0;

    let authored = graph
        .editor_metadata
        .node_positions
        .values()
        .any(|p| *p != Vec2::ZERO);
    // A stable order so the fallback does not reshuffle between runs.
    let mut ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    ids.sort_by_key(|id| format!("{id:?}"));

    if authored {
        for id in &ids {
            let pos = graph
                .editor_metadata
                .node_positions
                .get(id)
                .copied()
                .unwrap_or(Vec2::ZERO);
            view.positions.insert(*id, pos);
        }
        view.input_pos = graph.editor_metadata.input_position;
        view.output_pos = graph.editor_metadata.output_position;
        frame_layout(view);
        return;
    }

    let mut preds: std::collections::HashMap<NodeId, Vec<NodeId>> =
        ids.iter().map(|id| (*id, Vec::new())).collect();
    for (target, source) in graph.edges_inverted.iter() {
        let (Some(to), Some(from)) = (target_node(target), source_node(source)) else {
            continue;
        };
        if let Some(list) = preds.get_mut(&to) {
            list.push(from);
        }
    }

    // Iterative relaxation rather than a DFS: bounded by the node count, and a cycle just
    // stops improving instead of recursing forever.
    let mut depth: std::collections::HashMap<NodeId, usize> =
        ids.iter().map(|id| (*id, 0usize)).collect();
    for _ in 0..ids.len() {
        let mut changed = false;
        for id in &ids {
            let want = preds[id]
                .iter()
                .filter_map(|p| depth.get(p).copied())
                .max()
                .map_or(0, |d| d + 1);
            if want > depth[id] {
                depth.insert(*id, want);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut column_y: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
    for id in &ids {
        let column = depth[id];
        let y = column_y.entry(column).or_insert(0.0);
        view.positions
            .insert(*id, Vec2::new(column as f32 * (NODE_W + 80.0), *y));
        *y += 130.0;
    }
    let last = depth.values().copied().max().unwrap_or(0);
    view.input_pos = Vec2::new(-(NODE_W + 80.0), 0.0);
    view.output_pos = Vec2::new((last + 1) as f32 * (NODE_W + 80.0), 0.0);
    frame_layout(view);
}

/// Pan so the top-left of the laid-out graph sits just inside the canvas. The input rail is at
/// a negative x by construction, so without this a fresh graph opens with its left edge off
/// screen.
fn frame_layout(view: &mut CanvasView) {
    let min = view
        .positions
        .values()
        .chain([&view.input_pos, &view.output_pos])
        .fold(Vec2::splat(f32::MAX), |acc, p| acc.min(*p));
    if min.x < f32::MAX {
        view.pan = Vec2::splat(24.0) - min * view.zoom;
    }
}

/// Upstream's node names are emoji-prefixed (`∑ Blend`, `⌚ Speed`). This shell inherits
/// whatever font feathers ships, which has none of them, so a prefix renders as tofu.
fn ascii(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_string()
}

/// The node a target pin belongs to, or `None` for a graph-level output.
fn target_node(target: &TargetPin) -> Option<NodeId> {
    match target {
        TargetPin::NodeData(id, _) | TargetPin::NodeTime(id, _) => Some(*id),
        TargetPin::OutputData(_) | TargetPin::OutputTime => None,
    }
}

/// The node a source pin belongs to, or `None` for a graph-level input.
fn source_node(source: &SourcePin) -> Option<NodeId> {
    match source {
        SourcePin::NodeData(id, _) | SourcePin::NodeTime(id) => Some(*id),
        SourcePin::InputData(_) | SourcePin::InputTime(_) => None,
    }
}

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
}

/// What a canvas box stands for, and therefore where a drag on it writes back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BoxKind {
    Node(NodeId),
    /// The graph's own inputs — sources, so this rail has only output pins.
    Inputs,
    /// The graph's own outputs — targets, so this rail has only input pins.
    Outputs,
}

/// One box on the canvas: a node, or one of the two graph-level rails.
struct Boxed {
    pos: Vec2,
    title: String,
    kind: BoxKind,
    /// Left-hand pins, top to bottom, each with the edge target it terminates.
    inputs: Vec<(String, TargetPin)>,
    /// Right-hand pins, top to bottom, each with the edge source it originates.
    outputs: Vec<(String, SourcePin)>,
}

impl Boxed {
    fn height(&self) -> f32 {
        let rows = self.inputs.len().max(self.outputs.len()).max(1) as f32;
        HEADER_H + rows * PIN_H + BOX_PAD * 2.0
    }

    /// Centre of pin row `i`, measured down from the box's top edge.
    fn pin_y(&self, i: usize) -> f32 {
        HEADER_H + BOX_PAD + (i as f32 + 0.5) * PIN_H
    }
}

/// A node's pin list, or an empty one if the spec could not be computed (a node pointing at an
/// unloaded sub-graph, say) — a box with no pins still draws, which beats dropping the node.
fn node_spec(
    node: &AnimationNode,
    graphs: &Assets<AnimationGraph>,
    fsms: &Assets<StateMachine>,
) -> NodeSpec {
    node.new_spec(SpecResources {
        graph_assets: graphs,
        fsm_assets: fsms,
    })
    .unwrap_or_default()
}

/// R3: draw the graph. A box per node with its real pin rows, plus the two graph-level rails,
/// and a link per edge MANHATTAN-routed as three plain rectangles — ordinary bevy_ui, with no
/// line primitive and no new render pass.
fn draw_canvas(
    mut commands: Commands,
    mut view: ResMut<CanvasView>,
    graphs: Res<Assets<AnimationGraph>>,
    fsms: Res<Assets<StateMachine>>,
    canvas: Single<(Entity, Option<&Children>), With<Canvas>>,
) {
    if !view.dirty {
        return;
    }
    let Some(handle) = view.graph.clone() else {
        view.dirty = false;
        return;
    };
    let Some(graph) = graphs.get(&handle) else {
        return;
    };
    view.dirty = false;

    let (canvas_entity, existing) = *canvas;
    if let Some(existing) = existing {
        for child in existing.iter() {
            commands.entity(child).despawn();
        }
    }

    let mut boxes: Vec<Boxed> = Vec::new();

    // The graph's own inputs are SOURCES on the canvas, and its outputs are TARGETS: the rails
    // are ordinary boxes with one side empty.
    boxes.push(Boxed {
        pos: view.input_pos,
        title: "inputs".into(),
        kind: BoxKind::Inputs,
        inputs: Vec::new(),
        outputs: graph
            .io_spec
            .sorted_inputs()
            .into_iter()
            .map(|input| match input {
                NodeInput::Time(pin) => {
                    (format!("t {}", pin_name(&pin)), SourcePin::InputTime(pin))
                }
                NodeInput::Data(pin, _) => (pin_name(&pin), SourcePin::InputData(pin)),
            })
            .collect(),
    });
    boxes.push(Boxed {
        pos: view.output_pos,
        title: "outputs".into(),
        kind: BoxKind::Outputs,
        inputs: graph
            .io_spec
            .sorted_outputs()
            .into_iter()
            .map(|output| match output {
                NodeOutput::Time => ("t".to_string(), TargetPin::OutputTime),
                NodeOutput::Data(pin, _) => (pin.clone(), TargetPin::OutputData(pin)),
            })
            .collect(),
        outputs: Vec::new(),
    });

    let mut ids: Vec<NodeId> = graph.nodes.keys().copied().collect();
    ids.sort_by_key(|id| format!("{id:?}"));
    for id in &ids {
        let Some(node) = graph.nodes.get(id) else {
            continue;
        };
        let spec = node_spec(node, &graphs, &fsms);
        boxes.push(Boxed {
            pos: view.positions.get(id).copied().unwrap_or(Vec2::ZERO),
            title: if node.name.is_empty() {
                ascii(&node.inner.display_name())
            } else {
                format!("{}  ({})", node.name, ascii(&node.inner.display_name()))
            },
            kind: BoxKind::Node(*id),
            inputs: spec
                .sorted_inputs()
                .into_iter()
                .map(|input| match input {
                    NodeInput::Time(pin) => {
                        (format!("t {pin}"), TargetPin::NodeTime(*id, pin))
                    }
                    NodeInput::Data(pin, _) => (pin.clone(), TargetPin::NodeData(*id, pin)),
                })
                .collect(),
            outputs: spec
                .sorted_outputs()
                .into_iter()
                .map(|output| match output {
                    NodeOutput::Time => ("t".to_string(), SourcePin::NodeTime(*id)),
                    NodeOutput::Data(pin, _) => (pin.clone(), SourcePin::NodeData(*id, pin)),
                })
                .collect(),
        });
    }

    // Endpoint tables in canvas coordinates, so a link is two lookups rather than a search.
    let zoom = view.zoom;
    let pan = view.pan;
    let to_px = |p: Vec2| p * zoom + pan;
    let mut source_at: std::collections::HashMap<&SourcePin, Vec2> =
        std::collections::HashMap::new();
    let mut target_at: std::collections::HashMap<&TargetPin, Vec2> =
        std::collections::HashMap::new();
    for boxed in &boxes {
        for (i, (_, pin)) in boxed.inputs.iter().enumerate() {
            target_at.insert(pin, boxed.pos + Vec2::new(0.0, boxed.pin_y(i)));
        }
        for (i, (_, pin)) in boxed.outputs.iter().enumerate() {
            source_at.insert(pin, boxed.pos + Vec2::new(NODE_W, boxed.pin_y(i)));
        }
    }

    let mut children = Vec::new();

    // Links first, so the boxes draw over them.
    let mut drawn = 0;
    for (target, source) in graph.edges_inverted.iter() {
        let (Some(from), Some(to)) = (source_at.get(source), target_at.get(target)) else {
            continue;
        };
        // Time edges carry the clock, data edges carry poses and values: colour them apart, so
        // the timing spine of a graph is visible at a glance.
        let is_time = matches!(target, TargetPin::NodeTime(..) | TargetPin::OutputTime);
        let color = if is_time {
            Color::srgba(0.85, 0.62, 0.30, 0.85)
        } else {
            Color::srgba(0.35, 0.55, 0.85, 0.85)
        };
        children.extend(manhattan(&mut commands, to_px(*from), to_px(*to), zoom, color));
        drawn += 1;
    }

    for boxed in &boxes {
        children.push(spawn_box(&mut commands, boxed, to_px(boxed.pos), zoom));
    }

    commands.entity(canvas_entity).add_children(&children);
    info!(
        "canvas: {} nodes, {}/{} links drawn, zoom {:.2}",
        graph.nodes.len(),
        drawn,
        graph.edges_inverted.len(),
        zoom
    );
}

/// One box: a title strip, then the input pins down the left and the output pins down the
/// right. Dragging a node box moves it and stops there, so the canvas does not also pan.
fn spawn_box(commands: &mut Commands, boxed: &Boxed, px: Vec2, zoom: f32) -> Entity {
    let font = |size: f32| TextFont {
        font_size: FontSize::Px(size * zoom),
        ..default()
    };
    let pin_row = |commands: &mut Commands, label: &str, right: bool| {
        commands
            .spawn((
                Node {
                    height: Val::Px(PIN_H * zoom),
                    align_items: AlignItems::Center,
                    justify_content: if right {
                        JustifyContent::FlexEnd
                    } else {
                        JustifyContent::FlexStart
                    },
                    padding: UiRect::horizontal(Val::Px(6.0 * zoom)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                Children::spawn(Spawn((
                    Text::new(label.to_string()),
                    ThemedText,
                    font(10.0),
                    TextLayout {
                        linebreak: bevy::text::LineBreak::NoWrap,
                        ..default()
                    },
                ))),
            ))
            .id()
    };

    let column = |commands: &mut Commands, pins: &[String], right: bool| {
        let rows: Vec<Entity> = pins
            .iter()
            .map(|label| pin_row(commands, label, right))
            .collect();
        commands
            .spawn(Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                flex_direction: FlexDirection::Column,
                ..default()
            })
            .add_children(&rows)
            .id()
    };

    let in_labels: Vec<String> = boxed.inputs.iter().map(|(l, _)| l.clone()).collect();
    let out_labels: Vec<String> = boxed.outputs.iter().map(|(l, _)| l.clone()).collect();
    let left = column(commands, &in_labels, false);
    let right = column(commands, &out_labels, true);

    let header = commands
        .spawn((
            Node {
                height: Val::Px(HEADER_H * zoom),
                align_items: AlignItems::Center,
                padding: UiRect::horizontal(Val::Px(7.0 * zoom)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(match boxed.kind {
                BoxKind::Node(_) => Color::srgba(0.22, 0.25, 0.32, 1.0),
                // The rails are the graph's own boundary; warm them so they read as different.
                _ => Color::srgba(0.30, 0.25, 0.17, 1.0),
            }),
            Children::spawn(Spawn((
                Text::new(boxed.title.clone()),
                ThemedText,
                font(12.0),
                TextLayout {
                    linebreak: bevy::text::LineBreak::NoWrap,
                    ..default()
                },
            ))),
        ))
        .id();
    let body = commands
        .spawn(Node {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Row,
            padding: UiRect::vertical(Val::Px(BOX_PAD * zoom)),
            ..default()
        })
        .add_children(&[left, right])
        .id();

    let entity = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(px.x),
                top: Val::Px(px.y),
                width: Val::Px(NODE_W * zoom),
                height: Val::Px(boxed.height() * zoom),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(BOX_IDLE),
        ))
        .add_children(&[header, body])
        .id();

    // The rails drag too — they are boxes like any other, they just write a different field.
    let kind = boxed.kind;
    commands.entity(entity).observe(
        move |mut drag: On<PointerDrag>, mut view: ResMut<CanvasView>| {
            if drag.button != PointerButton::Primary {
                return;
            }
            // Without this the drag also reaches the canvas and the whole view pans with it.
            drag.propagate(false);
            let delta = drag.delta / view.zoom;
            match kind {
                BoxKind::Node(id) => {
                    if let Some(pos) = view.positions.get_mut(&id) {
                        *pos += delta;
                    }
                }
                BoxKind::Inputs => view.input_pos += delta,
                BoxKind::Outputs => view.output_pos += delta,
            }
            view.dirty = true;
        },
    );
    if let BoxKind::Node(id) = kind {
        commands.entity(entity).insert(CanvasNode(id)).observe(
            move |mut click: On<PointerClick>, mut selected: ResMut<Selected>| {
                click.propagate(false);
                selected.node = Some(id);
                selected.dirty = true;
            },
        );
    }
    entity
}

/// The canvas box colours, picked apart enough that a selected node reads at a glance.
const BOX_IDLE: Color = Color::srgba(0.13, 0.14, 0.17, 0.97);
const BOX_SELECTED: Color = Color::srgba(0.20, 0.26, 0.34, 0.99);

/// Tint the selected node's box. Guarded on the current value rather than run on a change
/// filter, because `draw_canvas` respawns every box and the new ones start idle.
fn highlight_selected(selected: Res<Selected>, mut boxes: Query<(&CanvasNode, &mut BackgroundColor)>) {
    for (node, mut background) in &mut boxes {
        let want = if selected.node == Some(node.0) {
            BOX_SELECTED
        } else {
            BOX_IDLE
        };
        if background.0 != want {
            background.0 = want;
        }
    }
}

/// List the selected node's parameters.
///
/// This does NOT go through `bevy_feathers_inspector`, and cannot: `AnimationGraph::nodes` is
/// `#[reflect(ignore)]`, and `DynNodeLike` — the `Box<dyn NodeLike>` wrapper each node's body
/// sits in — carries a hand-written `Reflect` impl that reports ZERO fields, so there is no
/// reflection path from the asset root down to a node's body. What there IS: `NodeLike: Reflect`,
/// so `inner_ref()` is a `&dyn PartialReflect` over the CONCRETE node struct. Walking that
/// directly gives a read-only list now; making it editable means teaching the inspector a root
/// that resolves through a closure rather than a `ParsedPath`.
fn show_node_params(
    mut commands: Commands,
    mut selected: ResMut<Selected>,
    view: Res<CanvasView>,
    graphs: Res<Assets<AnimationGraph>>,
    host: Single<(Entity, Option<&Children>), With<NodeParamsHost>>,
) {
    if !selected.dirty {
        return;
    }
    selected.dirty = false;
    let (host_entity, existing) = *host;
    if let Some(existing) = existing {
        for child in existing.iter() {
            commands.entity(child).despawn();
        }
    }
    let (Some(id), Some(handle)) = (selected.node, view.graph.clone()) else {
        return;
    };
    let Some(node) = graphs.get(&handle).and_then(|g| g.nodes.get(&id)) else {
        return;
    };

    let mut rows = vec![commands
        .spawn((
            Text::new(format!(
                "{}  [{}]",
                node.name,
                ascii(&node.inner.display_name())
            )),
            ThemedText,
        ))
        .id()];
    for (name, value) in reflect_fields(node.inner_ref()) {
        rows.push(
            commands
                .spawn((
                    Text::new(format!("  {name}: {value}")),
                    ThemedText,
                    TextFont {
                        font_size: FontSize::Px(11.0),
                        ..default()
                    },
                ))
                .id(),
        );
    }
    commands.entity(host_entity).add_children(&rows);
}

/// A node body's fields as name/value pairs, one line each.
fn reflect_fields(value: &dyn PartialReflect) -> Vec<(String, String)> {
    match value.reflect_ref() {
        ReflectRef::Struct(s) => (0..s.field_len())
            .map(|i| {
                (
                    s.name_at(i).unwrap_or("?").to_string(),
                    s.field_at(i).map(one_line).unwrap_or_default(),
                )
            })
            .collect(),
        ReflectRef::TupleStruct(t) => (0..t.field_len())
            .map(|i| (i.to_string(), t.field(i).map(one_line).unwrap_or_default()))
            .collect(),
        _ => vec![("value".to_string(), one_line(value))],
    }
}

/// A reflected value as one short line: `Debug` where a type has it, its type otherwise.
fn one_line(value: &dyn PartialReflect) -> String {
    // A handle's `Debug` is its `AssetIndex`, which says nothing. The path is the whole point
    // of the field on a clip / graph / fsm node, so pull that out instead.
    if let Some(reflect) = value.try_as_reflect() {
        if let Some(path) = handle_path::<GraphClip>(reflect)
            .or_else(|| handle_path::<AnimationGraph>(reflect))
            .or_else(|| handle_path::<Skeleton>(reflect))
            .or_else(|| handle_path::<StateMachine>(reflect))
        {
            return path;
        }
    }
    let text = value
        .try_as_reflect()
        .map(|r| format!("{r:?}"))
        .unwrap_or_else(|| value.reflect_type_path().to_string());
    let text = text.replace('\n', " ");
    if text.chars().count() > 72 {
        format!("{}...", text.chars().take(69).collect::<String>())
    } else {
        text
    }
}

/// Three rectangles: out from the source, across, then in to the target. No rotation, so this
/// is plain bevy_ui — which is why the canvas needed no rendering work.
fn manhattan(
    commands: &mut Commands,
    from: Vec2,
    to: Vec2,
    zoom: f32,
    color: Color,
) -> Vec<Entity> {
    let thickness = (LINK_THICKNESS * zoom).max(1.0);
    // Step out of the source before turning, so a link leaving a pin is readable even when the
    // target sits to its left (a feedback edge) and the midpoint lands inside the box.
    let stub = 14.0 * zoom;
    let mid_x = if to.x > from.x + stub * 2.0 {
        (from.x + to.x) * 0.5
    } else {
        from.x + stub
    };
    let mut rect = |x: f32, y: f32, w: f32, h: f32| {
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(x),
                    top: Val::Px(y),
                    width: Val::Px(w.max(thickness)),
                    height: Val::Px(h.max(thickness)),
                    ..default()
                },
                BackgroundColor(color),
                // Links must not swallow a background drag meant to pan the canvas.
                Pickable::IGNORE,
            ))
            .id()
    };
    vec![
        rect(
            from.x.min(mid_x),
            from.y,
            (mid_x - from.x).abs(),
            thickness,
        ),
        rect(mid_x, from.y.min(to.y), thickness, (to.y - from.y).abs()),
        rect(mid_x.min(to.x), to.y, (to.x - mid_x).abs(), thickness),
    ]
}

/// Ctrl+S: write the open graph back to its `.ron`, carrying the canvas layout into
/// `editor_metadata`. A `.bak` is left the first time, because a hand-authored graph's comments
/// do not survive a round trip through the serializer.
fn save_graph(
    keys: Res<ButtonInput<KeyCode>>,
    view: Res<CanvasView>,
    graphs: Res<Assets<AnimationGraph>>,
    registry: Res<AppTypeRegistry>,
) {
    if !keys.just_pressed(KeyCode::KeyS)
        || !(keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        return;
    }
    write_graph(&view, &graphs, &registry);
}

/// `--save-layout`: lay the open graph out, write it, and exit. The wait is for the asset to
/// finish loading, which is what puts a handle in `CanvasView`.
fn save_layout_and_exit(
    args: Res<Args>,
    view: Res<CanvasView>,
    graphs: Res<Assets<AnimationGraph>>,
    registry: Res<AppTypeRegistry>,
    mut exit: MessageWriter<AppExit>,
) {
    if !args.save_layout || view.graph.is_none() {
        return;
    }
    write_graph(&view, &graphs, &registry);
    exit.write(AppExit::Success);
}

/// Serialize the open graph with the canvas layout folded into its `editor_metadata`.
fn write_graph(
    view: &CanvasView,
    graphs: &Assets<AnimationGraph>,
    registry: &AppTypeRegistry,
) {
    let (Some(handle), Some(path)) = (&view.graph, &view.path) else {
        return;
    };
    let Some(graph) = graphs.get(handle) else {
        return;
    };

    let mut graph = graph.clone();
    for (id, pos) in &view.positions {
        graph.editor_metadata.node_positions.insert(*id, *pos);
    }
    graph.editor_metadata.input_position = view.input_pos;
    graph.editor_metadata.output_position = view.output_pos;

    let registry = registry.read();
    let mut serial = AnimationGraphSerializer::new(&graph, &registry);
    // `graph.nodes` is a HashMap, so without this the node order churns on every save.
    serial.nodes.sort_by_key(|node| format!("{:?}", node.id));
    let text = match ron::ser::to_string_pretty(&serial, ron::ser::PrettyConfig::default()) {
        Ok(text) => text,
        Err(err) => {
            error!("save {path}: {err}");
            return;
        }
    };

    let file = PathBuf::from(std::env::var_os("BEVY_ASSET_ROOT").expect("set in main"))
        .join("assets")
        .join(path);
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let text = format!("{}{text}", leading_comment(&existing));
    let backup = file.with_extension("ron.bak");
    if file.exists() && !backup.exists() {
        let _ = std::fs::copy(&file, &backup);
    }
    match std::fs::write(&file, text) {
        Ok(()) => info!("saved {}", file.display()),
        Err(err) => error!("save {}: {err}", file.display()),
    }
}

/// A round trip through the serializer drops every comment in the file. The header block is
/// where an authored graph keeps its design record — the blend formulas, in zero's case — so
/// carry that much across; comments further in are lost, and the `.bak` is the recourse.
fn leading_comment(existing: &str) -> String {
    let header: Vec<&str> = existing
        .lines()
        .take_while(|line| line.trim_start().starts_with("//") || line.trim().is_empty())
        .collect();
    if header.iter().all(|line| line.trim().is_empty()) {
        return String::new();
    }
    format!("{}\n", header.join("\n"))
}

/// `Some(path)` if `value` is a `Handle<A>` — a weak or pathless one reads as `<unsaved>`.
fn handle_path<A: Asset>(value: &dyn Reflect) -> Option<String> {
    let handle = value.downcast_ref::<Handle<A>>()?;
    Some(
        handle
            .path()
            .map(|path| path.to_string())
            .unwrap_or_else(|| "<unsaved>".to_string()),
    )
}
