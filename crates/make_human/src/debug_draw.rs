use bevy::{animation::AnimationTargetId, color::palettes::css, math::ToRender, prelude::*};

/// Gizmo config for skeleton bone visualization
#[derive(Reflect, GizmoConfigGroup)]
pub struct SkeletonGizmos {
    /// Radius of joint spheres
    pub joint_radius: f32,
    /// Color for root bones
    pub root_color: Color,
    /// Color for child bones
    pub bone_color: Color,
}

impl Default for SkeletonGizmos {
    fn default() -> Self {
        Self {
            joint_radius: 0.005,
            root_color: Color::srgb(1.0, 0.0, 0.0), // Red
            bone_color: Color::srgb(0.0, 1.0, 0.0), // Green
        }
    }
}

/// Gizmo config for joint axes (RGB = XYZ) + stroke-text name labels
#[derive(Reflect, GizmoConfigGroup)]
pub struct JointAxesGizmos {
    /// Length of axis lines
    pub axis_length: f32,
    /// X axis color
    pub x_color: Color,
    /// Y axis color
    pub y_color: Color,
    /// Z axis color
    pub z_color: Color,
    /// Label text size (world metres)
    pub label_scale: f32,
    /// Label Y offset above joint
    pub label_offset: f32,
}

impl Default for JointAxesGizmos {
    fn default() -> Self {
        Self {
            axis_length: 0.04,
            x_color: css::RED.into(),
            y_color: css::GREEN.into(),
            z_color: css::BLUE.into(),
            label_scale: 0.02,
            label_offset: 0.03,
        }
    }
}

pub struct MakeHumanDebugPlugin;

impl Plugin for MakeHumanDebugPlugin {
    fn build(&self, app: &mut App) {
        // Skeleton bones gizmo
        app.insert_gizmo_config(
            SkeletonGizmos::default(),
            GizmoConfig {
                enabled: false,   // Off by default
                depth_bias: -1.0, // Render through geometry
                ..Default::default()
            },
        );

        // Joint axes gizmo
        app.insert_gizmo_config(
            JointAxesGizmos::default(),
            GizmoConfig {
                enabled: false, // Off by default
                depth_bias: -1.0,
                ..Default::default()
            },
        );

        app.add_systems(
            Update,
            (
                draw_skeleton_gizmos.run_if(|store: Res<GizmoConfigStore>| {
                    store.config::<SkeletonGizmos>().0.enabled
                }),
                draw_joint_axes_gizmos.run_if(|store: Res<GizmoConfigStore>| {
                    store.config::<JointAxesGizmos>().0.enabled
                }),
            ),
        );
    }
}

/// Draw skeleton bones as gizmos for debugging
/// Reads from actual bone entity GlobalTransforms (animated)
pub fn draw_skeleton_gizmos(
    bones: Query<(&GlobalTransform, Option<&ChildOf>), With<AnimationTargetId>>,
    parent_transforms: Query<&GlobalTransform>,
    mut gizmos: Gizmos<SkeletonGizmos>,
    store: Res<GizmoConfigStore>,
) {
    let (_, config) = store.config::<SkeletonGizmos>();

    for (global_transform, parent) in bones.iter() {
        let head_world = global_transform.translation().to_render();

        // Color based on whether bone has parent
        let color = if parent.is_none() {
            config.root_color
        } else {
            config.bone_color
        };

        // Draw line to parent if exists
        if let Some(child_of) = parent {
            if let Ok(parent_transform) = parent_transforms.get(child_of.parent()) {
                let parent_pos = parent_transform.translation().to_render();
                gizmos.line(head_world, parent_pos, color);
            }
        }

        // Draw a small sphere at the joint position
        gizmos.sphere(
            Isometry3d::from_translation(head_world),
            config.joint_radius,
            color,
        );
    }
}

/// Draw local XYZ axes at each joint (RGB = XYZ convention) + camera-facing name labels
/// (stroke-text gizmos — no label entities to manage)
pub fn draw_joint_axes_gizmos(
    joints: Query<(&GlobalTransform, Option<&Name>), With<AnimationTargetId>>,
    camera: Query<&Transform, With<bevy_aurora::render::AuroraCamera>>,
    mut gizmos: Gizmos<JointAxesGizmos>,
    store: Res<GizmoConfigStore>,
) {
    let (_, config) = store.config::<JointAxesGizmos>();
    let face_camera = camera
        .iter()
        .next()
        .map(|t| t.rotation.to_render())
        .unwrap_or(Quat::IDENTITY);

    for (transform, name) in &joints {
        let pos = transform.translation().to_render();
        let rot = transform.to_scale_rotation_translation().1.to_render();

        // RGB = XYZ convention (configurable)
        gizmos.line(pos, pos + rot * Vec3::X * config.axis_length, config.x_color);
        gizmos.line(pos, pos + rot * Vec3::Y * config.axis_length, config.y_color);
        gizmos.line(pos, pos + rot * Vec3::Z * config.axis_length, config.z_color);

        if let Some(name) = name {
            gizmos.text(
                Isometry3d::new(pos + Vec3::Y * config.label_offset, face_camera),
                name.as_str(),
                config.label_scale,
                Vec2::ZERO,
                css::WHITE,
            );
        }
    }
}
