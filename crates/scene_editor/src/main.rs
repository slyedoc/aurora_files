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
    camera_controller::free_camera::{FreeCamera, FreeCameraPlugin, FreeCameraState},
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
    app.init_resource::<Shelf>();
    app.add_systems(Startup, setup);
    app.add_systems(
        Update,
        (seed_filter, rebuild_palette, rebuild_shelf, show_selection).chain(),
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

/// A row's prop: the manifest entry whole, not a copy of some of its fields.
///
/// The bounds are the reason. Scale, shelf placement and camera framing all come off the baked
/// AABB, and an origin that is not centred in its own box (the blacksmith station runs -2.5 to
/// +3.8 across) has to be corrected for — which needs `min` and `max`, not just their difference.
#[derive(Component, Clone, Default)]
struct PropRow {
    prop: kit::KitProp,
}

/// Substring match against prop names, lower-cased by the time it gets here.
#[derive(Resource, Default)]
struct Filter(String);

/// The prop currently selected in the palette.
#[derive(Resource, Default)]
struct Selection(Option<kit::KitProp>);

/// The props currently on the shelf, in layout order — the filtered set.
#[derive(Resource, Default)]
struct Shelf(Vec<kit::KitProp>);

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
    info!("palette: selected {} ({})", row.prop.name, row.prop.bsn);
    selection.0 = Some(row.prop.clone());
}

/// Metres per shelf cell.
const CELL: f32 = 1.6;
/// Fraction of the window the palette pane covers on the left. The shelf is framed into what is
/// LEFT of it rather than on the window centre, or half the grid hides behind the list.
const PANE: f32 = 0.30;

/// Columns for `n` props: roughly square, so the grid frames well whether the search left three
/// props or three hundred. A fixed width makes 297 props a 38-row corridor the camera has to
/// retreat down until nothing is legible.
fn columns_for(n: usize) -> usize {
    (n as f32).sqrt().ceil().max(1.0) as usize
}

/// A prop standing on the shelf, and where it sits.
#[derive(Component)]
struct ShelfItem {
    prop: kit::KitProp,
    /// Cell centre on the floor, so a pick can test against the prop's own box.
    centre: Vec3,
    scale: f32,
}

/// Lay the filtered props out as LIVE instances on a grid.
///
/// No thumbnails, no render-to-texture, no icon bake: the shelf shows the real `.bsn`, because on
/// this engine that is the cheap option rather than the expensive one — aurora carries ~1M
/// shared-BLAS instances, so a few hundred props are nothing, and every prop that shares geometry
/// with another shares its BLAS too.
///
/// The manifest is what makes the layout possible in one pass. Scale and offset come from the
/// baked AABB, so each prop is placed correctly on the frame it is requested; measuring the
/// spawned scenes instead would mean laying out only after 297 assets had finished streaming,
/// and re-laying out as each one arrived.
fn rebuild_shelf(
    mut commands: Commands,
    assets: Res<AssetServer>,
    shelf: Res<Shelf>,
    previous: Query<Entity, With<ShelfItem>>,
    camera: Single<(&mut Transform, &mut FreeCameraState), With<Camera3d>>,
) {
    if !shelf.is_changed() {
        return;
    }
    for entity in &previous {
        commands.entity(entity).despawn();
    }
    let columns = columns_for(shelf.0.len());
    let rows = shelf.0.len().div_ceil(columns).max(1);
    // Centre the grid on the origin so the camera framing below is symmetric.
    let origin = Vec3::new(
        -(columns.min(shelf.0.len().max(1)) as f32 - 1.0) * CELL * 0.5,
        0.0,
        -(rows as f32 - 1.0) * CELL * 0.5,
    );
    for (i, prop) in shelf.0.iter().enumerate() {
        let centre = origin
            + Vec3::new(
                (i % columns) as f32 * CELL,
                0.0,
                (i / columns) as f32 * CELL,
            );
        let scale = prop.fit(CELL * 0.8);
        commands.spawn((
            Name::new(prop.name.clone()),
            ShelfItem {
                prop: prop.clone(),
                centre,
                scale,
            },
            ScenePatchInstance(assets.load(&prop.bsn)),
            Transform::from_translation(centre + prop.shelf_offset(scale))
                .with_scale(Vec3::splat(scale)),
            Visibility::Visible,
        ));
    }
    // Frame the whole grid. The extents are known up front, so the shot is right immediately
    // rather than after 297 assets have streamed in.
    let (width, depth) = (columns as f32 * CELL, rows as f32 * CELL);
    let span = width.max(depth);
    // Fit the span into the part of the window the pane leaves, and slide the whole view sideways
    // by half the pane so the grid sits in the clear.
    let fit = span / (1.0 - PANE);
    let shift = -fit * PANE * 0.5;
    let eye = Vec3::new(shift, fit * 0.62, depth * 0.5 + fit * 0.72);
    aim(camera, eye, Vec3::new(shift, CELL * 0.25, 0.0));
    info!("shelf: {} props over {rows} row(s)", shelf.0.len());
}

/// Point the free camera at something, and tell the CONTROLLER where it now looks.
///
/// `FreeCameraState` caches yaw and pitch, seeded once from the transform and authoritative from
/// then on: the controller rebuilds `rotation` from that cache the moment the mouse moves. So
/// writing `Transform` alone appears to work and then snaps back to the startup orientation on
/// the first mouse input — the camera "resets" exactly when you touch it. Anything that aims this
/// camera has to move both.
fn aim(
    mut camera: Single<(&mut Transform, &mut FreeCameraState), With<Camera3d>>,
    eye: Vec3,
    focus: Vec3,
) {
    let (transform, state) = &mut *camera;
    **transform = Transform::from_translation(eye).looking_at(focus, Vec3::Y);
    // The controller reads YXZ and writes ZYX; for a roll-free look-at the yaw/pitch pair is the
    // same either way, which is what lets the cache be refreshed from the result.
    let (yaw, pitch, _roll) = transform.rotation.to_euler(EulerRot::YXZ);
    state.yaw = yaw;
    state.pitch = pitch;
}

/// Lift the selected prop clear of the shelf.
///
/// A nudge upward rather than a second instance at the origin: the prop is already on the shelf,
/// and spawning a copy somewhere else asks you to find it again. Rising out of the grid reads at
/// a glance and keeps the thing you picked in the place you picked it from.
fn show_selection(selection: Res<Selection>, mut items: Query<(&ShelfItem, &mut Transform)>) {
    if !selection.is_changed() {
        return;
    }
    let chosen = selection.0.as_ref().map(|p| p.bsn.as_str());
    for (item, mut transform) in &mut items {
        let lifted = Some(item.prop.bsn.as_str()) == chosen;
        let base = item.centre + item.prop.shelf_offset(item.scale);
        let want = base
            + if lifted {
                Vec3::Y * CELL * 0.35
            } else {
                Vec3::ZERO
            };
        if transform.translation != want {
            transform.translation = want;
        }
    }
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
    mut shelf: ResMut<Shelf>,
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
                prop.clone(),
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
        selection.0 = Some(prop.clone());
    }

    // The shelf shows exactly what the list shows, so searching narrows both.
    shelf.0 = pending;

    let shown = rows.len();
    // `FeathersListViewProps::rows` is a `Box<dyn SceneList>`; `Vec<S: Scene>` is a `SceneList`,
    // which is what lets the rows be built at runtime rather than written out in the macro.
    let rows: Box<dyn SceneList> = Box::new(rows);
    let list = commands
        .spawn_scene(bsn! {
            @FeathersListView { @rows: {rows}, }
            // The list view needs a DEFINITE height or nothing inside it ever scrolls: its inner
            // `ScrollArea` can only clip once its own parent has a size that does not come from
            // the rows. `flex_grow` does not give one here, so the list is pinned to its
            // container instead — absolute with every inset at zero takes it out of flow, so 297
            // rows cannot push it taller, and its height is exactly the body's.
            //
            // From there the widget does the rest: the scroll area's automatic minimum height is
            // zero because its overflow is not visible, so it shrinks to the bounded parent and
            // clips, and the scrollbar the scene already carries has something to drive.
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                right: Val::Px(0.0),
                bottom: Val::Px(0.0),
            }
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
fn row(label: String, trailing: String, prop: kit::KitProp) -> impl Scene {
    bsn! {
        @FeathersListRow
        PropRow { prop: {prop}, }
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
