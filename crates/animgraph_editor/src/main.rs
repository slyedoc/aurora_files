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
    feathers::{
        dark_theme::create_dark_theme,
        theme::{ThemedText, UiTheme},
    },
    feathers_inspector::BuildAssetInspector,
    prelude::*,
    ui::{AlignItems, FlexDirection, JustifyContent, Overflow, UiRect, Val},
};
use bevy_animation_graph::{
    core::{
        animation_clip::GraphClip, animation_graph::AnimationGraph, skeleton::Skeleton,
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
}

/// The pane the inspector builds into.
#[derive(Resource)]
struct InspectorPane(Entity);

/// The title strip above the inspector.
#[derive(Component)]
struct SelectionLabel;

/// The browser pane, so the pre-spawned rows can be parented to it.
#[derive(Component)]
struct Browser;

/// The inspector pane's parent, so the pre-spawned body can be parented to it.
#[derive(Component)]
struct InspectorHost;

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
    app.add_systems(Startup, (scan_library, setup_ui, attach_panes).chain());
    app.add_systems(Update, bind_when_loaded);
    app.run();
}

/// Walk the asset root for everything the editor can open. Asset paths are root-relative with
/// `/` separators, which is what the asset server wants on every platform.
fn scan_library(mut library: ResMut<Library>) {
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
    library
        .entries
        .sort_by(|a, b| a.path.cmp(&b.path));
    info!(
        "library: {} assets under {}",
        library.entries.len(),
        root.display()
    );
}

fn setup_ui(mut commands: Commands, library: Res<Library>) {
    // aurora's render_frame wants exactly one of ITS cameras, so even a UI-only shell needs a
    // 3d one. It also becomes the preview camera at R2.
    commands.spawn((
        Name::new("Camera"),
        Camera3d::default(),
        AuroraExposure::SUNLIGHT,
        Transform::from_xyz(0.0, 1.2, -3.2).looking_at(Vec3::new(0.0, 0.95, 0.0), Vec3::Y),
    ));

    // Left: the browser. Centre: preview + canvas, once R2/R3 land. Right: the inspector.
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

    // Rows are plain picked nodes rather than feathers buttons: the button is a BSN scene
    // component now, and a list of hundreds of them is not what that API is shaped for.
    let mut rows: Vec<Entity> = Vec::new();
    for entry in &library.entries {
        let entry = entry.clone();
        let label = format!("[{}]  {}", entry.kind.label(), entry.path);
        let row = commands
            .spawn((
                Node {
                    padding: UiRect::axes(Val::Px(6.0), Val::Px(3.0)),
                    ..default()
                },
                Children::spawn(Spawn((Text::new(label), ThemedText))),
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

    commands.insert_resource(BrowserRows(rows));
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
                Node {
                    width: Val::Px(320.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    padding: UiRect::all(Val::Px(8.0)),
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
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                Children::spawn(Spawn((
                    Text::new("preview + canvas land here (R2/R3)"),
                    ThemedText,
                ))),
            )),
            // inspector
            Spawn((
                Name::new("inspector"),
                InspectorHost,
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

/// Parent the pre-spawned browser rows and inspector body into their panes.
fn attach_panes(
    mut commands: Commands,
    rows: Res<BrowserRows>,
    pane: Res<InspectorPane>,
    browser: Single<Entity, With<Browser>>,
    host: Single<Entity, With<InspectorHost>>,
) {
    commands.entity(*browser).add_children(&rows.0);
    commands.entity(*host).add_child(pane.0);
    commands.remove_resource::<BrowserRows>();
}

/// The rows built in `setup_ui`, handed to `attach_panes` once the panes exist.
#[derive(Resource)]
struct BrowserRows(Vec<Entity>);

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
    info!("opened {}", opening.path);
}
