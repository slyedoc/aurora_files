// CPU OMM bake shim — see omm_shim.h. All NVIDIA OMM SDK contact lives here.
#include "omm_shim.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>

#include "omm.h"

namespace {

// memcpy `bytes` into a fresh malloc buffer (the SDK frees its own copy on
// DestroyBakeResult, so the caller needs an independent one).
void* dup(const void* src, size_t bytes) {
    if (!src || bytes == 0) {
        return nullptr;
    }
    void* dst = std::malloc(bytes);
    if (dst) {
        std::memcpy(dst, src, bytes);
    }
    return dst;
}

void messageCallback(ommMessageSeverity severity, const char* message, void*) {
    const char* sev = severity == ommMessageSeverity_Fatal ? "FATAL"
                    : severity == ommMessageSeverity_Error ? "ERROR"
                    : severity == ommMessageSeverity_PerfWarning ? "PERF"
                                                                 : "INFO";
    std::fprintf(stderr, "[omm %s] %s\n", sev, message);
}

} // namespace

extern "C" int omm_shim_bake(const OmmShimInput* in, OmmShimResult* out) {
    std::memset(out, 0, sizeof(*out));

    ommBakerCreationDesc bakerDesc = ommBakerCreationDescDefault();
    bakerDesc.type = ommBakerType_CPU;
    bakerDesc.messageInterface.messageCallback = messageCallback;

    ommBaker baker = 0;
    ommResult res = ommCreateBaker(&bakerDesc, &baker);
    if (res != ommResult_SUCCESS) {
        return (int)res;
    }

    ommCpuTextureMipDesc mip = {};
    mip.width = in->width;
    mip.height = in->height;
    mip.textureData = in->alpha;

    ommCpuTextureDesc texDesc = {};
    texDesc.format = ommCpuTextureFormat_FP32;
    texDesc.mipCount = 1;
    texDesc.mips = &mip;
    // If embedded in the texture object it must match the bake desc exactly.
    texDesc.alphaCutoff = in->alphaCutoff;

    ommCpuTexture texture = 0;
    res = ommCpuCreateTexture(baker, &texDesc, &texture);
    if (res != ommResult_SUCCESS) {
        ommDestroyBaker(baker);
        return (int)res;
    }

    ommCpuBakeInputDesc bakeDesc = ommCpuBakeInputDescDefault();
    bakeDesc.texture = texture;
    bakeDesc.alphaMode = ommAlphaMode_Test;
    bakeDesc.alphaCutoff = in->alphaCutoff;
    bakeDesc.runtimeSamplerDesc.addressingMode =
        in->addressModeWrap ? ommTextureAddressMode_Wrap : ommTextureAddressMode_Clamp;
    bakeDesc.runtimeSamplerDesc.filter = ommTextureFilterMode_Linear;
    bakeDesc.texCoordFormat = ommTexCoordFormat_UV32_FLOAT;
    bakeDesc.texCoords = in->uvs;
    bakeDesc.texCoordStrideInBytes = 0; // packed
    bakeDesc.indexFormat = ommIndexFormat_UINT_32;
    bakeDesc.indexBuffer = in->indices;
    bakeDesc.indexCount = in->indexCount;
    bakeDesc.format = in->format == 1 ? ommFormat_OC1_2_State : ommFormat_OC1_4_State;
    // Per-triangle subdivision from texel area (SDK default dynamicSubdivisionScale=2,
    // left untouched), capped by maxSubdivisionLevel — keeps array data proportional to
    // texel density instead of paying 4^maxLevel on every triangle. Deterministic at
    // bake time (the ray-cone mip-bias that would make it view-dependent is runtime-only).
    bakeDesc.maxSubdivisionLevel = (uint8_t)in->maxSubdivisionLevel;
    bakeDesc.bakeFlags = ommCpuBakeFlags_EnableInternalThreads;

    ommCpuBakeResult bakeResult = 0;
    res = ommCpuBake(baker, &bakeDesc, &bakeResult);
    if (res != ommResult_SUCCESS) {
        ommCpuDestroyTexture(baker, texture);
        ommDestroyBaker(baker);
        return (int)res;
    }

    const ommCpuBakeResultDesc* desc = nullptr;
    res = ommCpuGetBakeResultDesc(bakeResult, &desc);
    if (res == ommResult_SUCCESS && desc) {
        out->arrayData = (uint8_t*)dup(desc->arrayData, desc->arrayDataSize);
        out->arrayDataSize = desc->arrayDataSize;

        out->descArray = (OmmShimDesc*)dup(
            desc->descArray, sizeof(OmmShimDesc) * desc->descArrayCount);
        out->descCount = desc->descArrayCount;

        out->descHistogram = (OmmShimUsage*)dup(
            desc->descArrayHistogram, sizeof(OmmShimUsage) * desc->descArrayHistogramCount);
        out->descHistogramCount = desc->descArrayHistogramCount;

        // CPU baker always emits a per-triangle index buffer; width depends on
        // the chosen indexFormat (omm picks the narrowest that fits).
        uint32_t idxBytes = desc->indexFormat == ommIndexFormat_UINT_16 ? 2
                          : desc->indexFormat == ommIndexFormat_UINT_8  ? 1
                                                                        : 4;
        out->indexBuffer = (uint8_t*)dup(desc->indexBuffer, (size_t)idxBytes * desc->indexCount);
        out->indexCount = desc->indexCount;
        out->indexFormat = (uint32_t)desc->indexFormat;

        out->indexHistogram = (OmmShimUsage*)dup(
            desc->indexHistogram, sizeof(OmmShimUsage) * desc->indexHistogramCount);
        out->indexHistogramCount = desc->indexHistogramCount;

        // Bake-quality stats: micro-triangle state breakdown.
        ommDebugStats stats = {};
        if (ommDebugGetStats(baker, desc, &stats) == ommResult_SUCCESS) {
            out->statOpaque = stats.totalOpaque;
            out->statTransparent = stats.totalTransparent;
            out->statUnknownOpaque = stats.totalUnknownOpaque;
            out->statUnknownTransparent = stats.totalUnknownTransparent;
            out->knownAreaMetric = stats.knownAreaMetric;
        }

        // Ground-truth dump: OMM_DEBUG_DUMP=<dir> overlays the baked OMM states
        // onto the alpha texture (one PNG per bake) so we can eyeball whether the
        // cutout matches the leaf. Each bake gets a unique postfix.
        if (const char* dumpDir = std::getenv("OMM_DEBUG_DUMP")) {
            static int s_dumpCounter = 0;
            char postfix[32];
            std::snprintf(postfix, sizeof(postfix), "_%04d", s_dumpCounter++);
            ommDebugSaveImagesDesc save = ommDebugSaveImagesDescDefault();
            save.path = dumpDir;
            save.filePostfix = postfix;
            save.oneFile = 1;            // all primitives in one image
            save.monochromeUnknowns = 0; // distinct colors per state
            ommResult dres = ommDebugSaveAsImages(baker, &bakeDesc, desc, &save);
            std::fprintf(stderr, "[omm dump] %s/*%s.png -> result %d\n",
                         dumpDir, postfix, (int)dres);
        }
    }

    ommCpuDestroyBakeResult(bakeResult);
    ommCpuDestroyTexture(baker, texture);
    ommDestroyBaker(baker);
    return (int)res;
}

extern "C" void omm_shim_free(OmmShimResult* out) {
    std::free(out->arrayData);
    std::free(out->descArray);
    std::free(out->descHistogram);
    std::free(out->indexBuffer);
    std::free(out->indexHistogram);
    std::memset(out, 0, sizeof(*out));
}
