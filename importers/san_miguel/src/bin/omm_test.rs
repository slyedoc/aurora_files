//! End-to-end smoke test for the OMM CPU-baker FFI (no GPU, no bevy).
//!
//! Reproduces the SDK's "MinimalSample": a procedural alpha ring over a small
//! triangle fan, bakes a 4-state OMM, and prints the result shape so we can
//! confirm the shim links and the baker runs.
//!
//!   cargo run --release --bin omm_test

use aurora_bsn::omm;

fn main() {
    // Procedural alpha ring (rMin..rMax around center), matching the SDK sample.
    let (w, h) = (256u32, 256u32);
    let (r_min, r_max) = (0.2f32, 0.3f32);
    let mut alpha = vec![0.0f32; (w * h) as usize];
    for j in 0..h {
        for i in 0..w {
            let uv = (i as f32 / w as f32 - 0.5, j as f32 / w as f32 - 0.5);
            let r = (uv.0 * uv.0 + uv.1 * uv.1).sqrt();
            alpha[(j * w + i) as usize] = if r > r_min && r < r_max { 1.0 } else { 0.0 };
        }
    }

    // Triangle "diamond" fan covering the ring (UVs == positions, 5 verts / 4 tris).
    #[rustfmt::skip]
    let uvs: Vec<f32> = vec![
        0.05, 0.50,
        0.50, 0.05,
        0.50, 0.50,
        0.95, 0.50,
        0.50, 0.95,
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        0, 1, 2,
        1, 3, 2,
        3, 4, 2,
        2, 4, 0,
    ];

    match omm::bake(
        &alpha, w, h, &uvs, &indices,
        0.5,                          // alpha cutoff
        omm::OMM_FORMAT_OC1_4_STATE,  // 4-state
        8,                            // max subdivision level
        false,                        // clamp addressing
    ) {
        Ok(b) => {
            let specials = b.indices.iter().filter(|&&i| i < 0).count();
            println!("OMM bake OK:");
            println!("  array_data       : {} bytes", b.array_data.len());
            println!("  descs            : {}", b.descs.len());
            println!("  desc_histogram   : {:?}", b.desc_histogram);
            println!(
                "  per-tri indices  : {} ({} special) -> {:?}",
                b.indices.len(),
                specials,
                b.indices
            );
            println!("  index width      : {} bytes", b.index_format_bytes);
            assert_eq!(b.indices.len(), 4, "one OMM index per triangle");
        }
        Err(code) => {
            eprintln!("OMM bake FAILED, ommResult = {code}");
            std::process::exit(1);
        }
    }
}
