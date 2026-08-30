pub mod park;

pub mod prelude {
    pub use crate::park::*;
    pub use crate::LOG_FILTER;
}

/// Log filter: silence noisy startup INFO lines (keep warn/error).
pub const LOG_FILTER: &str = concat!(
    "bevy_camera_controller=off",
    ",bevy_winit=warn",
    // winit-on-Wayland teardown race — benign upstream noise.
    ",calloop=error",
);
