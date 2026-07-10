//! Light-control panel (top right): sun azimuth/elevation sliders and a camera
//! exposure (EV100) slider.
//!
//! Emissive magnitude is baked physically at import (see `solari_bsn::gltf`), so
//! overall brightness is a camera/view control, not a per-material scale. The
//! exposure slider drives the `Exposure` component on the `SolariCamera`.

use bevy::{
    camera::Exposure, camera_controller::free_camera::FreeCameraState, feathers::{
        self,
        controls::{FeathersCheckbox, FeathersSlider},
        theme::{ThemeBackgroundColor, ThemeTextColor, ThemedText},
    }, light::light_consts::lux, math::DQuat, prelude::*, solari::prelude::*, ui::Checked, ui_widgets::{
        SliderPrecision, SliderStep, ValueChange, checkbox_self_update, slider_self_update,
    },
};

/// The sun entity the sliders steer.
#[derive(Component)]
pub struct Sun;

#[derive(Resource)]
pub struct LightSettings {
    /// Whether the sun emits at all.
    pub sun_enabled: bool,
    /// Sun heading in degrees (rotation about world Y).
    pub sun_azimuth: f32,
    /// Sun height above the horizon in degrees; negative is twilight.
    pub sun_elevation: f32,
    /// Whether the ground plane is on the camera's render layer (visible). Toggled by moving the
    /// ground between layers, not `Visibility` — solari keeps hidden geometry in the AS otherwise.
    pub ground_enabled: bool,
    /// Whether the 1 m reference cube is spawned (scale sanity check).
    pub ref_cube_enabled: bool,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self {
            sun_enabled: true,
            sun_azimuth: -23.0,
            sun_elevation: 63.0,
            ground_enabled: true,
            ref_cube_enabled: std::env::var_os("VIEWER_REF_CUBE").is_some(),
        }
    }
}

pub struct LightSettingsPlugin;

impl Plugin for LightSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LightSettings>()
            .add_systems(Startup, (|| settings_ui()).spawn())
            .add_systems(Update, (apply_sun, reset_on_change));
    }
}

/// Any lighting change invalidates accumulated/temporal history — request a
/// camera reset for the frame.
fn reset_on_change(
    settings: Res<LightSettings>,
    mut cameras: Query<&mut CameraReset, With<SolariCamera>>,
) {
    if !settings.is_changed() || settings.is_added() {
        return;
    }
    for mut reset in &mut cameras {
        reset.history = true;
    }
}

fn apply_sun(
    settings: Res<LightSettings>,
    mut sun: Query<(&mut Transform, &mut SolariDirectionLight), With<Sun>>,
) {
    if !settings.is_changed() {
        return;
    }
    for (mut transform, mut light) in &mut sun {
        transform.rotation = DQuat::from_euler(
            EulerRot::YXZ,
            settings.sun_azimuth.to_radians() as f64,
            -settings.sun_elevation.to_radians() as f64,
            0.0,
        );
        let illuminance = if settings.sun_enabled {
            lux::FULL_DAYLIGHT
        } else {
            0.0
        };
        if light.illuminance != illuminance {
            light.illuminance = illuminance;
        }
    }
}

fn settings_ui() -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            top: px(10),
            right: px(10),
            padding: px(8),
        }
        ThemeBackgroundColor(feathers::tokens::WINDOW_BG)
        on(|_: On<Pointer<Over>>, mut free_camera_state: Single<&mut FreeCameraState>| {
            free_camera_state.enabled = false;
        })
        on(|_: On<Pointer<Out>>, mut free_camera_state: Single<&mut FreeCameraState>| {
            free_camera_state.enabled = true;
        })
        Children [(
            Node {
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Stretch,
                justify_content: JustifyContent::Start,
                row_gap: px(8),
                min_width: px(180),
            }
            Children [
                Text("Lights"),
                (
                    @FeathersCheckbox {
                        @caption: bsn! { Text("Sun") ThemedText }
                    }
                    Checked
                    on(checkbox_self_update)
                    on(|change: On<ValueChange<bool>>, mut settings: ResMut<LightSettings>| {
                        settings.sun_enabled = change.value;
                    })
                ),
                (
                    @FeathersCheckbox {
                        @caption: bsn! { Text("Ground") ThemedText }
                    }
                    Checked
                    on(checkbox_self_update)
                    on(|change: On<ValueChange<bool>>, mut settings: ResMut<LightSettings>| {
                        settings.ground_enabled = change.value;
                    })
                ),
                (
                    @FeathersCheckbox {
                        @caption: bsn! { Text("Ref cube (1m)") ThemedText }
                    }
                    on(checkbox_self_update)
                    on(|change: On<ValueChange<bool>>, mut settings: ResMut<LightSettings>| {
                        settings.ref_cube_enabled = change.value;
                    })
                ),
                (
                    Text("Sun azimuth")
                    TextFont { font_size: FontSize::Px(14.0) }
                    ThemeTextColor(feathers::tokens::CHECKBOX_TEXT)
                ),
                (
                    @FeathersSlider {
                        @min: -180.0,
                        @max: 180.0,
                        @value: -23.0,
                    }
                    SliderStep(1.0)
                    SliderPrecision(0)
                    on(slider_self_update)
                    on(|change: On<ValueChange<f32>>, mut settings: ResMut<LightSettings>| {
                        settings.sun_azimuth = change.value;
                    })
                ),
                (
                    Text("Sun elevation")
                    TextFont { font_size: FontSize::Px(14.0) }
                    ThemeTextColor(feathers::tokens::CHECKBOX_TEXT)
                ),
                (
                    @FeathersSlider {
                        @min: -15.0,
                        @max: 89.0,
                        @value: 63.0,
                    }
                    SliderStep(1.0)
                    SliderPrecision(0)
                    on(slider_self_update)
                    on(|change: On<ValueChange<f32>>, mut settings: ResMut<LightSettings>| {
                        settings.sun_elevation = change.value;
                    })
                ),
                (
                    Text("Exposure (EV100)")
                    TextFont { font_size: FontSize::Px(14.0) }
                    ThemeTextColor(feathers::tokens::CHECKBOX_TEXT)
                ),
                (
                    @FeathersSlider {
                        @min: 0.0,
                        @max: 20.0,
                        @value: 9.7,
                    }
                    SliderStep(0.5)
                    SliderPrecision(1)
                    on(slider_self_update)
                    on(|change: On<ValueChange<f32>>, mut cams: Query<&mut Exposure, With<SolariCamera>>| {
                        for mut e in &mut cams { e.ev100 = change.value; }
                    })
                ),
            ]
        )]
    }
}
