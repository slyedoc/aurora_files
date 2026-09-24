# Aftermath dump decoder

Decodes `.nv-gpudump` GPU crash dumps to JSON (faulting shader, page fault info, warp state).

```sh
gcc -o decode decode.c -I ~/nvidia/aftermath/include -L ~/nvidia/aftermath/lib/x64 \
    -lGFSDK_Aftermath_Lib.x64 -Wl,-rpath,$HOME/nvidia/aftermath/lib/x64
./decode some-crash.nv-gpudump | python3 -m json.tool
```

With `VK_NV_device_diagnostics_config` enabled (aurora render_device.rs, armed when the
`dev` feature + SDK are present), dumps also carry resource tracking (which buffer owned
the faulting VA, and whether it was already destroyed) and shader source mapping via the
`.nvdbg` blobs aurora writes next to the dumps.
