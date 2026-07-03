# Validation config for the solari RT path.
#
# NV RT validation (NV_ALLOW_RAYTRACING_VALIDATION) is the targeted, low-overhead
# detector for AS build/traversal faults — the suspected cause of the move-around
# device-lost. GPU-AV + sync-val are intentionally OFF: stacking them on the
# massive initial tessellation + OMM build overloads load before you reach
# interactive. To hunt descriptor/BDA issues instead, re-enable these:
#   VK_KHRONOS_VALIDATION_GPUAV_ENABLE=true
#   VK_KHRONOS_VALIDATION_GPUAV_RAY_TRACING_BUFFERS_CONSISTENCY=true
#   VK_KHRONOS_VALIDATION_VALIDATE_SYNC=true
#
# MESSAGE_ID_FILTER suppresses confirmed-benign VUIDs at the LAYER (the only place
# that works — many of these, e.g. vkCreateShaderModule spirv-val errors, are
# printed directly by the layer to stderr and bypass bevy's tracing fmt filter).
# Keep this list in sync with `tess_log_filter::BENIGN` in src/main.rs.
WGPU_VALIDATION=1 \
NV_ALLOW_RAYTRACING_VALIDATION=1 \
VK_LAYER_KHRONOS_VALIDATION_MESSAGE_ID_FILTER=VUID-StandaloneSpirv-OpTypeRuntimeArray-04680,VUID-RuntimeSpirv-vulkanMemoryModel-06265,VUID-StandaloneSpirv-None-10684,VUID-vkCmdTraceRaysKHR-None-08114,VUID-VkPresentInfoKHR-pImageIndices-01430 \
RUST_BACKTRACE=full \
SOLARI_OMM_CONSULT=1 SOLARI_TESS_SCALE=0.03 SOLARI_TESS_LEVEL=8 SOLARI_TESS_SPLIT=4 SOLARI_TESS_SMOOTH=1 \
BEVY_ASSET_ROOT=/mnt/code/p/solari_files \
  stdbuf -oL -eL cargo run -r -p san_miguel --features solari,bevy/debug,dlss > run.log 2>&1
