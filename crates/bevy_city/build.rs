// RPATH for libshaderc_shared.so.1: bevy_aurora links shaderc from the Vulkan SDK, and the SDK's
// setup-env.sh does not put $VULKAN_SDK/lib on LD_LIBRARY_PATH. `cargo:rustc-link-arg` does NOT
// propagate from a dependency's build script, so this binary embeds the rpath itself (the engine's
// own examples get it from the engine's build.rs).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    if let Ok(sdk) = std::env::var("VULKAN_SDK") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{sdk}/lib");
    }
}
