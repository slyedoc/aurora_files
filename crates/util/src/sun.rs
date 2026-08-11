//! Sun controls: [`SunSettings`] is a reflected resource mapped by
//! [`apply_sun`] onto every [`Sun`]-marked `AuroraDirectionLight`.
//!
//! The old fork-era feathers inspector card is gone with the wgpu stack;
//! settings are edited in code / CLI for now and the resource stays reflected
//! so a future aurora inspector can pick it straight back up.

use bevy::{light::light_consts::lux, math::DQuat, prelude::*};
use bevy_aurora::{
    lights::AuroraDirectionLight,
    render::{AuroraCamera, CameraReset},
};

/// The sun entity the settings steer.
#[derive(Component)]
pub struct Sun;

/// Sun steering, degrees.
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
            .add_systems(Update, (apply_sun, reset_on_change));
    }
}

/// Map the settings onto every [`Sun`] light: rotation from the angles,
/// illuminance from the enabled switch. Also covers freshly spawned suns
/// (`is_changed` is true on insert, and `Added<Sun>` re-arms it).
fn apply_sun(
    settings: Res<SunSettings>,
    mut sun: Query<(&mut Transform, &mut AuroraDirectionLight), With<Sun>>,
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
    mut cameras: Query<&mut CameraReset, With<AuroraCamera>>,
) {
    if !settings.is_changed() || settings.is_added() {
        return;
    }
    for mut reset in &mut cameras {
        reset.history = true;
    }
}
