//! `scene_editor` — build a world out of baked `.bsn` props.
//!
//! **R0, the palette listing.** Find every kit manifest under the asset root, load it, and show
//! what it holds: one selectable row per placeable prop, with the size it was baked at, over a
//! search box.
//!
//! ```text
//! cargo run --release -p scene_editor            # every kit under $BEVY_ASSET_ROOT
//! scene_editor --kit fantasy_props               # just one
//! ```
//!
//! The listing exists because a bevy `AssetServer` cannot enumerate — see `kit.rs`. Everything
//! later in the ladder (a shelf of live instances, click-to-place, saving the result back out as
//! a `.bsn`) reads the same manifest, so this rung is the foundation rather than a browser bolted
//! on the side. Ladder in `scene_editor.md`.
//!
//! The UI is feathers widgets, not hand-built ones. `FeathersListView` brings the scroll area,
//! the scrollbar, mutually-exclusive selection and keyboard navigation; `FeathersTextInput`
//! brings focus handling — which is what keeps the search from eating the free camera's WASD.
//!
//! F2 aurora's dev panel, F1 the world inspector, F12 screenshot.

mod kit;

use bevy::{
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin},
    feathers::{
        controls::{
            FeathersListRow, FeathersListView, FeathersTextInput, FeathersTextInputContainer,
        },
        font_styles::InheritableFont,
        theme::ThemedText,
    },
    prelude::*,
    scene::{Scene, SceneList, ScenePatchInstance},
    text::{EditableText, TextEdit, TextEditChange},
    ui_widgets::ValueChange,
};
use bevy_aurora::{
    auto_exposure::AuroraExposure,
    dev_shaders::DevShaderPlugin,
    dev_ui::DevUIPlugin,
    material::{AuroraMaterial, AuroraMaterial3d},
    ray_default_plugins::RayDefaultPlugins,
    sky::Sky,
    util::{ScreenshotExt, TimeoutAppExt},
};
use clap::Parser;

use kit::{Kit, KitLoader, Kits};

#[derive(Parser, Resource, Clone)]
#[command(name = "scene_editor", about = "Build a world out of baked .bsn props")]
struct Args {
    /// Show only the kit with this manifest stem (`fantasy_props`). Default is every kit found.
    #[arg(long)]
    kit: Option<String>,

    /// Start with this text in the search box.
    #[arg(long, default_value = "")]
    filter: String,

    /// Select this prop at startup (short or full name), so a preview can be checked without
    /// clicking. The same hook the animgraph editor's `--select` gives.
    #[arg(long)]
    select: Option<String>,

    /// Seconds before auto-exit.
    #[arg(long, short)]
    timeout: Option<f32>,
}

fn main() {
    // Root rule, matching the `bsn` viewer: explicit $BEVY_ASSET_ROOT > the current repo (a cwd
    // with an assets/ dir, which is the installed-binary case) > this workspace.
    if std::env::var_os("BEVY_ASSET_ROOT").is_none() {
        let root = if std::path::Path::new("assets").is_dir() {
            std::env::current_dir().expect("cwd")
        } else {
            std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        };
        // SAFETY: single-threaded, before App construction spawns anything.
        unsafe {
            std::env::set_var("BEVY_ASSET_ROOT", &root);
        }
    }
    let args = Args::parse();

    let mut app = App::new();
    // Aurora's `DevUIPlugin` already adds `FeathersPlugins` (and the inspector's), which is where
    // the list view, scrollbar and text input systems come from.
    app.add_plugins((
        RayDefaultPlugins.set(bevy::log::LogPlugin {
            filter: util::LOG_FILTER.into(),
            ..default()
        }),
        DevShaderPlugin,
        DevUIPlugin,
        FreeCameraPlugin::default(),
    ));
    app.init_asset::<Kit>().register_asset_loader(KitLoader);
    app.add_screenshot(KeyCode::F12);
    app.add_timeout_exit(args.timeout, 60.0);
    app.init_resource::<Filter>();
    app.insert_resource(args);
    app.init_resource::<PaletteDirty>();
    app.init_resource::<Selection>();
    app.add_systems(Startup, setup);
    app.add_systems(
        Update,
        (seed_filter, rebuild_palette, show_selection).chain(),
    );
    app.run();
}

/// The node the list view is rebuilt into.
#[derive(Component, Clone, Default)]
struct PaletteBody;

/// The header line above the search box.
#[derive(Component, Clone, Default)]
struct PaletteHeader;

/// The search box, so its text can be read back on change.
#[derive(Component, Clone, Default)]
struct PaletteSearch;

/// A row's prop, so a selection says which one — and how big it is, which is what lets the
/// preview frame it without loading it first.
#[derive(Component, Clone, Default)]
struct PropRow {
    name: String,
    bsn: String,
    size: Vec3,
}

/// Substring match against prop names, lower-cased by the time it gets here.
#[derive(Resource, Default)]
struct Filter(String);

/// The prop currently selected in the palette.
#[derive(Resource, Default)]
struct Selection(Option<PropRow>);

/// The prop instance standing in the world, so the next selection can replace it.
#[derive(Component)]
struct Preview;

/// Set when the rows need respawning — the search changed, or a manifest finished loading.
#[derive(Resource, Default)]
struct PaletteDirty(bool);

fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<AuroraMaterial>>,
) {
    let root = std::path::PathBuf::from(std::env::var_os("BEVY_ASSET_ROOT").expect("set in main"))
        .join("assets");
    let kits = kit::discover(&assets, &root);
    info!(
        "palette: {} kit manifest(s) under {}",
        kits.0.len(),
        root.display()
    );
    for entry in &kits.0 {
        info!("  {}", entry.name);
    }
    commands.insert_resource(kits);

    commands.spawn((
        Name::new("camera"),
        Camera3d::default(),
        FreeCamera::default(),
        AuroraExposure::SUNLIGHT,
        Sky::default(),
        Transform::from_xyz(0.0, 1.6, 6.0).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
    ));
    // A grid floor, the same one the animgraph editor stands its rig on. A plain plane gives a
    // prop somewhere to stand but says nothing about how big it is; one-metre cells turn the
    // floor into a ruler, which is most of what a prop preview is for.
    commands.spawn((
        Name::new("floor"),
        Mesh3d(meshes.add(util::grid::floor_mesh(util::grid::FLOOR_SIZE))),
        AuroraMaterial3d(materials.add(AuroraMaterial {
            base_color_texture: Some(images.add(util::grid::grid_texture())),
            perceptual_roughness: 0.85,
            ..default()
        })),
        // Just below the origin props stand on, so the floor never z-fights their base.
        Transform::from_xyz(0.0, -0.002, 0.0),
    ));
    commands.spawn((
        Name::new("sun"),
        DirectionalLight {
            illuminance: 12_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(6.0, 12.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.spawn_scene(bsn! {
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
        }
        Children [(
            Name::new("palette")
            Node {
                // Proportional with a floor and a ceiling: a tiling window manager hands
                // this app whatever column it likes.
                width: Val::Percent(26.0),
                min_width: Val::Px(320.0),
                max_width: Val::Px(460.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(6.0),
                // Top padding clears aurora's dev panel, an overlay pinned to the top-left
                // that would otherwise sit on the first rows.
                padding: {UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(160.0), Val::Px(8.0))},
            }
            // Props render BEHIND the ui from R1 on, so a pane with no background would
            // read as text floating over them.
            BackgroundColor({Color::srgba(0.06, 0.06, 0.07, 0.94)})
            Children [
                (
                    PaletteHeader
                    Text::new("scanning...")
                    ThemedText
                    TextLayout { linebreak: {bevy::text::LineBreak::NoWrap} }
                ),
                (
                    @FeathersTextInputContainer
                    // The container's own scene sets `flex_grow: 1`, which is right in the ROW
                    // it was designed for (a label, a spacer, then the field taking the rest)
                    // and wrong here: in a column, growing means growing TALL, so the search
                    // box swallows whatever the list leaves behind and balloons when the
                    // results are few. Its height already comes from the widget.
                    Node { flex_grow: 0.0, flex_shrink: 0.0 }
                    Children [(
                        @FeathersTextInput { @max_characters: 40usize, }
                        PaletteSearch
                        on(search_changed)
                    )]
                ),
                (
                    Name::new("body")
                    PaletteBody
                    Node {
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        // A flex item will not shrink below its own content unless told it may,
                        // and 297 rows of content is taller than any window. Without this the
                        // body grows to fit them and the list runs off the bottom of the page
                        // instead of scrolling inside the pane.
                        min_height: Val::Px(0.0),
                    }
                ),
            ]
        )]
    });
}

/// Type `--filter` into the search box once, rather than setting the resource behind its back.
///
/// The widget owns the text: it fires a change on startup with its own (empty) buffer, which
/// would clobber any value written straight into the resource — and the box would still look
/// empty while the list was filtered, which is worse than not supporting the flag. Inserting the
/// text makes the widget and the resource agree by construction.
fn seed_filter(
    mut done: Local<bool>,
    args: Res<Args>,
    search: Query<Entity, With<PaletteSearch>>,
    mut editors: Query<&mut EditableText>,
) {
    if *done || args.filter.is_empty() {
        *done = true;
        return;
    }
    let Some(entity) = search.iter().next() else {
        return;
    };
    let Ok(mut editor) = editors.get_mut(entity) else {
        return;
    };
    editor.queue_edit(TextEdit::Insert(args.filter.as_str().into()));
    *done = true;
}

/// The search box changed: take its text and mark the list stale.
///
/// `EditableText::value()` is the widget's own buffer, so there is no parallel copy of the
/// string to keep in step — the resource exists only so the rebuild can read it without
/// querying the widget.
fn search_changed(
    _: On<TextEditChange>,
    search: Single<&EditableText, With<PaletteSearch>>,
    mut filter: ResMut<Filter>,
    mut dirty: ResMut<PaletteDirty>,
) {
    let text = search.value().to_string();
    if text != filter.0 {
        filter.0 = text;
        dirty.0 = true;
    }
}

/// A row was picked. `ListBox` does the mutual exclusion and keyboard navigation; this only has
/// to say which prop the chosen entity stands for.
fn row_selected(
    change: On<ValueChange<Entity>>,
    rows: Query<&PropRow>,
    mut selection: ResMut<Selection>,
) {
    let Ok(row) = rows.get(change.value) else {
        return;
    };
    info!("palette: selected {} ({})", row.name, row.bsn);
    selection.0 = Some(row.clone());
}

/// Show the selected prop: one instance at the origin, with the camera pulled back to frame it.
///
/// Framing comes from the manifest rather than from the loaded scene, which is the whole reason
/// the extent is baked in. A `.bsn` streams in over several frames, so measuring the spawned
/// entity would mean waiting, guessing when it had settled, and moving the camera after the prop
/// was already on screen. Knowing the size up front means the shot is right on the first frame —
/// and it is the same fact R1 needs to lay 297 of these out in uniform cells.
fn show_selection(
    mut commands: Commands,
    assets: Res<AssetServer>,
    selection: Res<Selection>,
    previous: Query<Entity, With<Preview>>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
) {
    if !selection.is_changed() {
        return;
    }
    for entity in &previous {
        commands.entity(entity).despawn();
    }
    let Some(prop) = &selection.0 else {
        return;
    };
    commands.spawn((
        Name::new(prop.name.clone()),
        Preview,
        ScenePatchInstance(assets.load(&prop.bsn)),
        // Props bake with their origin on the floor, so nothing needs lifting.
        Transform::IDENTITY,
        Visibility::Visible,
    ));

    // Far enough back that the longest side fits, and never closer than arm's length — a teaspoon
    // would otherwise put the camera inside its own near plane.
    let longest = prop.size.max_element().max(0.1);
    let eye = Vec3::new(0.6, 0.55, 1.0).normalize() * (longest * 1.9).max(0.8);
    let focus = Vec3::Y * prop.size.y * 0.45;
    **camera = Transform::from_translation(eye + focus).looking_at(focus, Vec3::Y);
}

/// Respawn the list whenever the search changes or a manifest finishes loading.
fn rebuild_palette(
    mut commands: Commands,
    mut dirty: ResMut<PaletteDirty>,
    mut loaded: Local<usize>,
    mut selected_once: Local<bool>,
    args: Res<Args>,
    kits: Res<Kits>,
    manifests: Res<Assets<Kit>>,
    filter: Res<Filter>,
    mut selection: ResMut<Selection>,
    body: Single<(Entity, Option<&Children>), With<PaletteBody>>,
    mut header: Single<&mut Text, With<PaletteHeader>>,
) {
    // A manifest arriving is the other reason to rebuild, and there is no change signal for
    // "this handle resolved" — counting the ones that have is cheap and says exactly that.
    let ready = kits
        .0
        .iter()
        .filter(|entry| manifests.get(&entry.handle).is_some())
        .count();
    if ready != *loaded {
        *loaded = ready;
        dirty.0 = true;
    }
    if !dirty.0 {
        return;
    }
    dirty.0 = false;

    let (body, children) = *body;
    if let Some(children) = children {
        for child in children.iter() {
            commands.entity(child).despawn();
        }
    }

    let needle = filter.0.to_lowercase();
    let multiple = kits.0.len() > 1;
    let mut rows = Vec::new();
    let mut pending: Vec<kit::KitProp> = Vec::new();
    let mut total = 0usize;
    for entry in &kits.0 {
        if args.kit.as_deref().is_some_and(|want| want != entry.name) {
            continue;
        }
        let Some(manifest) = manifests.get(&entry.handle) else {
            continue;
        };
        total += manifest.props.len();
        for prop in &manifest.props {
            let short = prop.short_name();
            if !needle.is_empty() && !short.to_lowercase().contains(&needle) {
                continue;
            }
            // Qualify by kit only when there is more than one, rather than spending a header row
            // on the common case of a single kit.
            let label = if multiple {
                format!("{}/{short}", entry.name)
            } else {
                short.to_string()
            };
            pending.push(prop.clone());
            rows.push(row(
                label,
                // One figure, not three: at a glance you want "furniture or architecture", and
                // the longest side answers that.
                format!("{:.1}m", prop.size().max_element()),
                PropRow {
                    name: prop.name.clone(),
                    bsn: prop.bsn.clone(),
                    size: prop.size(),
                },
            ));
        }
    }

    // `--select` applies once, as soon as the manifest it names has actually loaded.
    //
    // Gated on its own flag, NOT on the selection being empty: the list box picks a row of its
    // own accord as the rows spawn, in the same frame, so "nothing is selected yet" is false by
    // the time this runs and the flag would never fire. A later click still overrides it, which
    // is the behaviour wanted — this only has to beat the list's own opening guess.
    if let Some(want) = &args.select
        && !*selected_once
        && let Some(prop) = pending
            .iter()
            .find(|p| p.name == *want || p.short_name() == want)
    {
        *selected_once = true;
        info!("palette: --select {} ({})", prop.name, prop.bsn);
        selection.0 = Some(PropRow {
            name: prop.name.clone(),
            bsn: prop.bsn.clone(),
            size: prop.size(),
        });
    }

    let shown = rows.len();
    // `FeathersListViewProps::rows` is a `Box<dyn SceneList>`; `Vec<S: Scene>` is a `SceneList`,
    // which is what lets the rows be built at runtime rather than written out in the macro.
    let rows: Box<dyn SceneList> = Box::new(rows);
    let list = commands
        .spawn_scene(bsn! {
            @FeathersListView { @rows: {rows}, }
            // Same story one level down: the list view must be allowed to shrink below its own
            // rows, or its inner `ScrollArea` never has anything to scroll against.
            Node { flex_grow: 1.0, min_height: Val::Px(0.0) }
            on(row_selected)
        })
        .id();
    commands.entity(body).add_child(list);

    header.0 = if kits.0.is_empty() {
        "no .kit.ron manifests found -- bake one with `prop_import --per-group`".to_string()
    } else if filter.0.is_empty() {
        format!("{shown} props in {} kit(s)", kits.0.len())
    } else {
        format!("{shown}/{total} props")
    };
}

/// One palette row: a name that gives way, and a size that does not.
///
/// Two columns rather than one string, because the pane is narrow by design and the name is the
/// part that can afford to be cut. A single formatted line clips from the right, which loses the
/// size — the one field you cannot reconstruct by squinting at the name.
fn row(label: String, trailing: String, prop: PropRow) -> impl Scene {
    bsn! {
        @FeathersListRow
        PropRow { name: {prop.name}, bsn: {prop.bsn}, size: {prop.size}, }
        Children [
            (
                Node {
                    // Takes the slack and yields it: `min_width: 0` is what lets a flex item
                    // shrink below its content, which is what makes the clip land here.
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    min_width: Val::Px(0.0),
                    overflow: {Overflow::clip_x()},
                }
                Children [(
                    Text::new(label)
                    ThemedText
                    TextLayout { linebreak: {bevy::text::LineBreak::NoWrap} }
                )]
            ),
            (
                Text::new(trailing)
                TextColor({Color::srgb(0.50, 0.52, 0.56)})
                ThemedText
                InheritableFont { font_size: {bevy::text::FontSize::Px(12.0)} }
                TextLayout { linebreak: {bevy::text::LineBreak::NoWrap} }
            ),
        ]
    }
}
