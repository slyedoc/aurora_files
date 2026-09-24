#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "GFSDK_Aftermath_GpuCrashDumpDecoding.h"

int main(int argc, char** argv) {
    if (argc < 2) { fprintf(stderr, "usage: decode <dump>\n"); return 1; }
    FILE* f = fopen(argv[1], "rb");
    if (!f) { perror("open"); return 1; }
    fseek(f, 0, SEEK_END); long sz = ftell(f); fseek(f, 0, SEEK_SET);
    void* buf = malloc(sz); fread(buf, 1, sz, f); fclose(f);

    GFSDK_Aftermath_GpuCrashDump_Decoder dec;
    GFSDK_Aftermath_Result r = GFSDK_Aftermath_GpuCrashDump_CreateDecoder(
        GFSDK_Aftermath_Version_API, buf, (uint32_t)sz, &dec);
    if (!GFSDK_Aftermath_SUCCEED(r)) { fprintf(stderr, "CreateDecoder: 0x%x\n", r); return 1; }

    uint32_t jsz = 0;
    r = GFSDK_Aftermath_GpuCrashDump_GenerateJSON(
        dec, GFSDK_Aftermath_GpuCrashDumpDecoderFlags_ALL_INFO,
        GFSDK_Aftermath_GpuCrashDumpFormatterFlags_NONE,
        NULL, NULL, NULL, NULL, &jsz);
    if (!GFSDK_Aftermath_SUCCEED(r)) { fprintf(stderr, "GenerateJSON: 0x%x\n", r); return 1; }
    char* json = malloc(jsz);
    r = GFSDK_Aftermath_GpuCrashDump_GetJSON(dec, jsz, json);
    if (!GFSDK_Aftermath_SUCCEED(r)) { fprintf(stderr, "GetJSON: 0x%x\n", r); return 1; }
    fwrite(json, 1, jsz, stdout);
    GFSDK_Aftermath_GpuCrashDump_DestroyDecoder(dec);
    return 0;
}
