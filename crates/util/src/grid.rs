//! The editor floor: a grid plane, built from one repeating texture.
//!
//! Every tool that previews something in 3D wants the same thing under it — a surface with scale
//! on it, so a prop has somewhere to stand, a contact shadow to read against, and a sense of how
//! big it is. This is that floor, shared rather than copied per tool.
//!
//! One texture, not a shader: the tile carries its own heavier boundary line, so a major line
//! lands every [`GRID_MAJOR`] metres without a second material, a second draw, or any per-pixel
//! work at all. Aurora's samplers are REPEAT, so tiling is purely a UV question.

use bevy::{
    asset::RenderAssetUsages,
    image::Image,
    math::primitives::Plane3d,
    mesh::{Mesh, MeshBuilder, Meshable, VertexAttributeValues},
};
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

/// Default floor extent in metres.
pub const FLOOR_SIZE: f32 = 40.0;
/// World size of one grid cell, in metres.
pub const GRID_CELL: f32 = 1.0;
/// Cells per texture tile, so a heavier line lands every this many metres.
pub const GRID_MAJOR: u32 = 8;

/// A plane whose UVs run `0..tiles` instead of `0..1`, so [`grid_texture`] repeats across it.
///
/// `PlaneMeshBuilder` has no UV scale, so the attribute is scaled after the fact.
pub fn grid_plane(size: f32, tiles: f32) -> Mesh {
    let mut mesh = Plane3d::default().mesh().size(size, size).build();
    let repeats = tiles / GRID_MAJOR as f32;
    if let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute_mut(Mesh::ATTRIBUTE_UV_0) {
        for uv in uvs.iter_mut() {
            uv[0] *= repeats;
            uv[1] *= repeats;
        }
    }
    mesh
}

/// A floor sized to `size` metres with one-metre cells. The common case.
pub fn floor_mesh(size: f32) -> Mesh {
    grid_plane(size, size / GRID_CELL)
}

/// One tile of the grid: [`GRID_MAJOR`] cells square, with a heavier line on the tile boundary.
///
/// The values are ALBEDO, so they stay well clear of 0 and 1 — a pure black floor gives the path
/// tracer nothing to bounce, and a pure white one blows out under the sun exposure these tools
/// run at.
pub fn grid_texture() -> Image {
    const CELL: u32 = 32;
    const SIZE: u32 = CELL * GRID_MAJOR;
    const BASE: [f32; 3] = [0.055, 0.058, 0.065];
    const MINOR: [f32; 3] = [0.12, 0.13, 0.15];
    const MAJOR: [f32; 3] = [0.26, 0.28, 0.32];

    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            // A line sits on the low edge of a cell; the major one is two pixels wide so it
            // still reads once the floor is a long way from the camera.
            let major = x < 2 || y < 2;
            let minor = x % CELL == 0 || y % CELL == 0;
            let color = if major {
                MAJOR
            } else if minor {
                MINOR
            } else {
                BASE
            };
            data.extend(color.iter().map(|c| (c * 255.0) as u8));
            data.push(255);
        }
    }
    Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::all(),
    )
}
