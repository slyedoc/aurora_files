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
    // Same story for Nsight Aftermath (bevy_aurora's `dev` feature): the engine links
    // libGFSDK_Aftermath_Lib.x64.so from $AFTERMATH_SDK/lib/x64, this binary needs the rpath.
    println!("cargo:rerun-if-env-changed=AFTERMATH_SDK");
    let aftermath = std::env::var("AFTERMATH_SDK").unwrap_or_else(|_| {
        format!(
            "{}/nvidia/aftermath",
            std::env::var("HOME").unwrap_or_default()
        )
    });
    println!("cargo:rustc-link-arg=-Wl,-rpath,{aftermath}/lib/x64");
}
