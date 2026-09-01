// Minimal C ABI over the NVIDIA OMM SDK CPU baker (omm-lib).
//
// The full `ommCpuBakeInputDesc` is large and union-laden; rather than mirror it
// in Rust FFI we keep all SDK contact in `omm_shim.cpp` (which uses the SDK's own
// `…DescDefault()` helpers) and expose only the few fields the importer drives.
// The result arrays are copied into shim-owned malloc buffers so the caller can
// read them after the SDK's bake result is destroyed; free with `omm_shim_free`.
#ifndef OMM_SHIM_H
#define OMM_SHIM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Mirrors ommCpuOpacityMicromapDesc — one per baked OMM in arrayData.
typedef struct OmmShimDesc {
    uint32_t offset;            // byte offset into arrayData
    uint16_t subdivisionLevel;  // micro-tri count = 4^level
    uint16_t format;            // 1 = OC1_2_State, 2 = OC1_4_State
} OmmShimDesc;

// Mirrors ommCpuOpacityMicromapUsageCount (== VkMicromapUsageEXT layout).
typedef struct OmmShimUsage {
    uint32_t count;
    uint16_t subdivisionLevel;
    uint16_t format;
} OmmShimUsage;

typedef struct OmmShimInput {
    const float*    alpha;       // width*height FP32 alpha, row-major
    uint32_t        width;
    uint32_t        height;
    const float*    uvs;         // vec2 (UV32_FLOAT) per vertex, packed
    uint32_t        uvCount;     // number of vec2 entries
    const uint32_t* indices;     // u32 triangle indices (post-cluster order)
    uint32_t        indexCount;  // == 3 * triangle count
    float           alphaCutoff; // texel opaque if alpha > cutoff
    uint32_t        format;      // 1 = OC1_2_State, 2 = OC1_4_State
    uint32_t        maxSubdivisionLevel; // [0,12]
    uint32_t        addressModeWrap;     // 1 = Wrap, 0 = Clamp
} OmmShimInput;

typedef struct OmmShimResult {
    uint8_t*      arrayData;            // OMM array build input (VkMicromapEXT data)
    uint32_t      arrayDataSize;
    OmmShimDesc*  descArray;            // micromap triangle array
    uint32_t      descCount;
    OmmShimUsage* descHistogram;        // usage counts for the micromap build
    uint32_t      descHistogramCount;
    uint8_t*      indexBuffer;          // per-triangle OMM index (BLAS/CLAS attach)
    uint32_t      indexCount;           // == triangle count
    uint32_t      indexFormat;          // omm enum: 0 = u16, 1 = u32, 2 = u8
    OmmShimUsage* indexHistogram;       // usage counts for the attach
    uint32_t      indexHistogramCount;
    // Debug stats (micro-triangle state counts) for bake-quality diagnosis.
    uint64_t      statOpaque;
    uint64_t      statTransparent;
    uint64_t      statUnknownOpaque;
    uint64_t      statUnknownTransparent;
    float         knownAreaMetric;      // [0,1] fraction of UV area resolved, -1 unknown
} OmmShimResult;

// Returns 0 (ommResult_SUCCESS) on success, else the ommResult error code.
int  omm_shim_bake(const OmmShimInput* in, OmmShimResult* out);
void omm_shim_free(OmmShimResult* out);

#ifdef __cplusplus
}
#endif

#endif // OMM_SHIM_H
