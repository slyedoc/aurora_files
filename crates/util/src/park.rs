//! One rule instead of per-panel hover observers: the free camera parks while
//! the pointer is over ANY UI node, and flies otherwise. This also keeps the
//! wheel from silently retuning fly speed while scrolling a panel.

use bevy::{
    camera_controller::free_camera::FreeCameraState, picking::hover::HoverMap, prelude::*,
    ui::ComputedNode,
};

pub struct HoverParkPlugin;

impl Plugin for HoverParkPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, park_over_ui);
    }
}

/// Disable every [`FreeCameraState`] while any pointer hovers a UI node.
fn park_over_ui(
    hover: Res<HoverMap>,
    nodes: Query<(), With<ComputedNode>>,
    mut cameras: Query<&mut FreeCameraState>,
) {
    let over_ui = hover
        .values()
        .any(|hits| hits.keys().any(|entity| nodes.contains(*entity)));
    for mut state in &mut cameras {
        // Manual neq guard: don't dirty the state every frame.
        if state.enabled == over_ui {
            state.enabled = !over_ui;
        }
    }
}
