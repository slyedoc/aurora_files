//! Safe wrapper over the OMM CPU-baker shim (`csrc/omm_shim.*`).
//!
//! Bakes one opacity micro-map from an alpha texture + a mesh's UVs/indices.
//! The returned [`OmmBake`] holds VK-ready arrays (owned Rust copies) that map
//! directly to a `VkMicromapEXT` build + a per-triangle OMM index buffer; they
//! ship in the `.cluster_mesh` v3 slices and aurora attaches them to the mesh's
//! BLAS (`bevy_aurora::omm`).

use std::os::raw::c_int;

#[repr(C)]
struct OmmShimDesc {
    offset: u32,
    subdivision_level: u16,
    format: u16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct OmmUsage {
    pub count: u32,
    pub subdivision_level: u16,
    pub format: u16,
}

#[repr(C)]
struct OmmShimInput {
    alpha: *const f32,
    width: u32,
    height: u32,
    uvs: *const f32,
    uv_count: u32,
    indices: *const u32,
    index_count: u32,
    alpha_cutoff: f32,
    format: u32,
    max_subdivision_level: u32,
    address_mode_wrap: u32,
}

#[repr(C)]
struct OmmShimResult {
    array_data: *mut u8,
    array_data_size: u32,
    desc_array: *mut OmmShimDesc,
    desc_count: u32,
    desc_histogram: *mut OmmUsage,
    desc_histogram_count: u32,
    index_buffer: *mut u8,
    index_count: u32,
    index_format: u32,
    index_histogram: *mut OmmUsage,
    index_histogram_count: u32,
    stat_opaque: u64,
    stat_transparent: u64,
    stat_unknown_opaque: u64,
    stat_unknown_transparent: u64,
    known_area_metric: f32,
}

unsafe extern "C" {
    fn omm_shim_bake(input: *const OmmShimInput, out: *mut OmmShimResult) -> c_int;
    fn omm_shim_free(out: *mut OmmShimResult);
}

/// 4-state OMM = 2 bits/micro-triangle (opaque/transparent/unknown-o/unknown-t).
pub const OMM_FORMAT_OC1_4_STATE: u32 = 2;
/// 2-state OMM = 1 bit/micro-triangle (opaque/transparent only).
pub const OMM_FORMAT_OC1_2_STATE: u32 = 1;

/// One baked OMM, as VK-ready arrays (owned).
pub struct OmmBake {
    /// `VkMicromapEXT` opacity-array build data.
    pub array_data: Vec<u8>,
    /// Per-OMM micromap-triangle descriptors `(offset, subdiv, format)`.
    pub descs: Vec<(u32, u16, u16)>,
    /// Usage histogram for the micromap build (`VkMicromapUsageEXT[]`).
    pub desc_histogram: Vec<OmmUsage>,
    /// Per-triangle OMM index, widened to i32 (negative = special index:
    /// -1 fully-transparent, -2 fully-opaque, -3 unknown-T, -4 unknown-O).
    pub indices: Vec<i32>,
    /// Native index width the baker chose (1/2/4 bytes), for the VK attach.
    pub index_format_bytes: u32,
    /// Usage histogram for the BLAS/CLAS attach.
    pub index_histogram: Vec<OmmUsage>,
    /// Micro-triangle state counts (bake quality). Pure opaque/transparent skip
    /// the any-hit; "unknown" still invokes it — high unknown = poor OMM accel.
    pub stat_opaque: u64,
    pub stat_transparent: u64,
    pub stat_unknown_opaque: u64,
    pub stat_unknown_transparent: u64,
    /// Fraction of UV area resolved to a known state ([0,1]; higher is better).
    pub known_area_metric: f32,
}

/// Bake an OMM. `alpha` is `width*height` row-major FP32 alpha; `uvs` is two
/// f32s per vertex (parallel to the mesh's vertices); `indices` are the mesh's
/// triangle indices in the final post-cluster order. Returns `Err(ommResult)`
/// on baker failure.
pub fn bake(
    alpha: &[f32],
    width: u32,
    height: u32,
    uvs: &[f32],
    indices: &[u32],
    alpha_cutoff: f32,
    format: u32,
    max_subdivision_level: u32,
    address_mode_wrap: bool,
) -> Result<OmmBake, i32> {
    assert_eq!(
        alpha.len(),
        (width * height) as usize,
        "alpha size mismatch"
    );
    assert_eq!(uvs.len() % 2, 0, "uvs must be vec2-packed");
    assert_eq!(indices.len() % 3, 0, "indices must be triangles");

    let input = OmmShimInput {
        alpha: alpha.as_ptr(),
        width,
        height,
        uvs: uvs.as_ptr(),
        uv_count: (uvs.len() / 2) as u32,
        indices: indices.as_ptr(),
        index_count: indices.len() as u32,
        alpha_cutoff,
        format,
        max_subdivision_level,
        address_mode_wrap: address_mode_wrap as u32,
    };

    let mut result = OmmShimResult {
        array_data: std::ptr::null_mut(),
        array_data_size: 0,
        desc_array: std::ptr::null_mut(),
        desc_count: 0,
        desc_histogram: std::ptr::null_mut(),
        desc_histogram_count: 0,
        index_buffer: std::ptr::null_mut(),
        index_count: 0,
        index_format: 0,
        index_histogram: std::ptr::null_mut(),
        index_histogram_count: 0,
        stat_opaque: 0,
        stat_transparent: 0,
        stat_unknown_opaque: 0,
        stat_unknown_transparent: 0,
        known_area_metric: -1.0,
    };

    // SAFETY: pointers above outlive the call; the shim only reads them and
    // writes owned buffers into `result`, which we copy out then free.
    let code = unsafe { omm_shim_bake(&input, &mut result) };
    if code != 0 {
        unsafe { omm_shim_free(&mut result) };
        return Err(code);
    }

    let index_format_bytes = match result.index_format {
        0 => 2, // UINT_16
        2 => 1, // UINT_8
        _ => 4, // UINT_32
    };

    let bake = unsafe {
        let array_data = slice_to_vec(result.array_data, result.array_data_size as usize);
        let descs = std::slice::from_raw_parts(result.desc_array, result.desc_count as usize)
            .iter()
            .map(|d| (d.offset, d.subdivision_level, d.format))
            .collect();
        let desc_histogram =
            slice_to_vec(result.desc_histogram, result.desc_histogram_count as usize);
        let indices = read_omm_indices(
            result.index_buffer,
            result.index_count as usize,
            index_format_bytes,
        );
        let index_histogram = slice_to_vec(
            result.index_histogram,
            result.index_histogram_count as usize,
        );

        OmmBake {
            array_data,
            descs,
            desc_histogram,
            indices,
            index_format_bytes,
            index_histogram,
            stat_opaque: result.stat_opaque,
            stat_transparent: result.stat_transparent,
            stat_unknown_opaque: result.stat_unknown_opaque,
            stat_unknown_transparent: result.stat_unknown_transparent,
            known_area_metric: result.known_area_metric,
        }
    };

    unsafe { omm_shim_free(&mut result) };
    Ok(bake)
}

unsafe fn slice_to_vec<T: Clone>(ptr: *const T, len: usize) -> Vec<T> {
    if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
    }
}

// Widen the baker's native-width OMM index buffer to i32. Special indices are
// the top-4 unsigned values for the width (two's-complement of -1..-4); only
// those map to negatives — a plain signed read would corrupt valid positive
// descArray indices above the signed midpoint.
unsafe fn read_omm_indices(ptr: *const u8, count: usize, width: u32) -> Vec<i32> {
    if ptr.is_null() || count == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr, count * width as usize) };
    let modulus: u64 = 1u64 << (width * 8);
    (0..count)
        .map(|i| {
            let u = match width {
                1 => bytes[i] as u64,
                2 => u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]]) as u64,
                _ => u32::from_le_bytes([
                    bytes[4 * i],
                    bytes[4 * i + 1],
                    bytes[4 * i + 2],
                    bytes[4 * i + 3],
                ]) as u64,
            };
            if u >= modulus - 4 {
                (u as i64 - modulus as i64) as i32
            } else {
                u as i32
            }
        })
        .collect()
}
