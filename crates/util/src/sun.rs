//! Inspector-driven sun controls: [`SunSettings`] is a reflected resource
//! (bounded sliders come from the range attributes), [`apply_sun`] maps it
//! onto every [`Sun`]-marked `SolariDirectionLight`, and the panel is a
//! `bevy_feathers_inspector` resource card — no hand-rolled sliders.

use bevy::{
    feathers::{self, theme::ThemeBackgroundColor},
    light::light_consts::lux,
    math::DQuat,
    prelude::*,
    solari::prelude::*,
};
use core::any::TypeId;

/// The sun entity the settings steer.
#[derive(Component)]
pub struct Sun;

/// Sun steering, degrees. Edited through the inspector card; [`apply_sun`]
/// stamps it onto the [`Sun`] light every change (including spawn).
#[derive(Resource, Reflect)]
#[reflect(Resource, Default)]
pub struct SunSettings {
    /// Whether the sun emits at all.
    pub enabled: bool,
    /// Heading in degrees (rotation about world Y).
    #[reflect(@-180.0..=180.0f32)]
    pub azimuth: f32,
    /// Height above the horizon in degrees; negative is twilight.
    #[reflect(@-15.0..=89.0f32)]
    pub elevation: f32,
}

impl Default for SunSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            azimuth: -23.0,
            elevation: 63.0,
        }
    }
}

pub struct SunPlugin;

impl Plugin for SunPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<SunSettings>()
            .init_resource::<SunSettings>()
            .add_systems(Update, (apply_sun, reset_on_change, spawn_sun_panel));
    }
}

/// Map the settings onto every [`Sun`] light: rotation from the angles,
/// illuminance from the enabled switch. Also covers freshly spawned suns
/// (`is_changed` is true on insert, and `Added<Sun>` re-arms it).
fn apply_sun(
    settings: Res<SunSettings>,
    mut sun: Query<(&mut Transform, &mut SolariDirectionLight), With<Sun>>,
    added: Query<(), Added<Sun>>,
) {
    if !settings.is_changed() && added.is_empty() {
        return;
    }
    for (mut transform, mut light) in &mut sun {
        transform.rotation = DQuat::from_euler(
            EulerRot::YXZ,
            settings.azimuth.to_radians() as f64,
            -settings.elevation.to_radians() as f64,
            0.0,
        );
        let illuminance = if settings.enabled { lux::FULL_DAYLIGHT } else { 0.0 };
        if light.illuminance != illuminance {
            light.illuminance = illuminance;
        }
    }
}

/// A lighting change invalidates accumulated/temporal history — request a
/// camera reset for the frame.
fn reset_on_change(
    settings: Res<SunSettings>,
    mut cameras: Query<&mut CameraReset, With<SolariCamera>>,
) {
    if !settings.is_changed() || settings.is_added() {
        return;
    }
    for mut reset in &mut cameras {
        reset.history = true;
    }
}

/// Marker for the spawned sun panel (spawn once, only while a sun exists).
#[derive(Component)]
struct SunPanel;

/// Top-right inspector card for [`SunSettings`], spawned once a [`Sun`]
/// exists. Honors `SolariDebugUi(false)` — graded screenshot runs stay clean.
/// (Free-camera parking over UI is global — see [`crate::park`].)
fn spawn_sun_panel(
    suns: Query<(), With<Sun>>,
    panels: Query<(), With<SunPanel>>,
    debug_ui: Option<Res<SolariDebugUi>>,
    mut commands: Commands,
) {
    if suns.is_empty() || !panels.is_empty() || debug_ui.is_some_and(|ui| !ui.0) {
        return;
    }
    let card = commands
        .spawn((
            SunPanel,
            Node {
                position_type: PositionType::Absolute,
                top: px(10),
                right: px(10),
                width: px(300),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(8)),
                ..Default::default()
            },
            ThemeBackgroundColor(feathers::tokens::WINDOW_BG),
        ))
        .id();
    let panel = commands
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(2),
            ..Default::default()
        })
        .id();
    commands.entity(card).add_child(panel);
    commands.queue(bevy::feathers_inspector::BuildResourceInspector {
        type_id: TypeId::of::<SunSettings>(),
        panel,
    });
}
