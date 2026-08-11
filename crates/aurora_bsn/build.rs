// Compiles the OMM CPU-baker shim (csrc/omm_shim.cpp) and links the prebuilt
// NVIDIA OMM SDK static libs. Override the SDK location with OMM_SDK; it must
// contain libraries/omm-lib/include/omm.h and a CPU build under build/ (see
// crates/bevy_solari memory note for the exact cmake invocation).
use std::path::Path;

fn main() {
    let sdk = std::env::var("OMM_SDK").unwrap_or_else(|_| "/mnt/code/f/OMM".to_string());
    let include = format!("{sdk}/libraries/omm-lib/include");
    let build = format!("{sdk}/build");

    let omm_lib = format!("{build}/libraries/omm-lib");
    let lz4_lib = format!("{build}/external/lz4/build/cmake");
    let xxhash_lib = format!("{build}/external/xxHash/build/cmake");

    assert!(
        Path::new(&format!("{omm_lib}/libomm-lib.a")).exists(),
        "OMM SDK not built at {omm_lib}/libomm-lib.a — build it first (see memory note), or set OMM_SDK"
    );

    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .file("csrc/omm_shim.cpp")
        .include(&include)
        .compile("omm_shim");

    println!("cargo:rustc-link-search=native={omm_lib}");
    println!("cargo:rustc-link-search=native={lz4_lib}");
    println!("cargo:rustc-link-search=native={xxhash_lib}");
    println!("cargo:rustc-link-lib=static=omm-lib");
    println!("cargo:rustc-link-lib=static=lz4");
    println!("cargo:rustc-link-lib=static=xxhash");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=gomp"); // OpenMP (EnableInternalThreads)

    println!("cargo:rerun-if-changed=csrc/omm_shim.cpp");
    println!("cargo:rerun-if-changed=csrc/omm_shim.h");
    println!("cargo:rerun-if-env-changed=OMM_SDK");
}
