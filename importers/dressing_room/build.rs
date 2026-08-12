// RPATH for libGFSDK_Aftermath_Lib.x64.so. The link itself now comes from
// bevy_aurora (its `aftermath` feature is default-on and its build.rs emits
// the `-L`/`-l`, which ride the rlib here) — but `cargo:rustc-link-arg` does
// NOT propagate from a dependency's build script, so zero's binaries embed
// their own rpath. Emitted only when the lib exists; without the SDK, aurora
// compiles its no-op stubs and nothing here is needed.
use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=AFTERMATH_SDK");
    let sdk = env::var("AFTERMATH_SDK")
        .unwrap_or_else(|_| format!("{}/nvidia/aftermath", env::var("HOME").unwrap_or_default()));
    let lib = PathBuf::from(sdk).join("lib/x64");
    if lib.join("libGFSDK_Aftermath_Lib.x64.so").exists() {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib.display());
    }
}
