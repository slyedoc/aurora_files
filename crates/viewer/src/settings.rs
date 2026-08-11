//! Viewer-local settings panel (below the sun card, top right): ground /
//! reference-cube toggles. Sun controls live in `util::sun`; exposure lives
//! in the solari camera card (an `Exposure` inspector section).

use bevy::{
    feathers::{
        self,
        controls::FeathersCheckbox,
        theme::{ThemeBackgroundColor, ThemedText},
    }, prelude::*, ui::Checked, ui_widgets::{
        ValueChange, checkbox_self_update,
    },
};

#[derive(Resource)]
pub struct LightSettings {
    /// Whether the ground plane is on the camera's render layer (visible). Toggled by moving the
    /// ground between layers, not `Visibility` — solari keeps hidden geometry in the AS otherwise.
    pub ground_enabled: bool,
    /// Whether the 1 m reference cube is spawned (scale sanity check).
    pub ref_cube_enabled: bool,
}

impl Default for LightSettings {
    fn default() -> Self {
        Self {
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
            .add_systems(Update, reset_on_change);
    }
}

/// A scene-content change (ground layer swap) invalidates accumulated/temporal
/// history — request a camera reset for the frame.
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

fn settings_ui() -> impl Scene {
    bsn! {
        Node {
            position_type: PositionType::Absolute,
            top: px(180),
            right: px(10),
            padding: px(8),
        }
        ThemeBackgroundColor(feathers::tokens::WINDOW_BG)
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
                Text("Scene"),
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
            ]
        )]
    }
}
