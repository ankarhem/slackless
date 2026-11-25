// Shared internal definitions for the modular macOS hardware media bridge.

#ifndef SLACKLESS_MACOS_COMMON_H
#define SLACKLESS_MACOS_COMMON_H

#import <AVFoundation/AVFoundation.h>
#import <Accelerate/Accelerate.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreGraphics/CoreGraphics.h>
#import <CoreImage/CoreImage.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <Vision/Vision.h>
#import <dispatch/dispatch.h>
#import <mach/mach_time.h>
#import <objc/runtime.h>
#import <pthread.h>
#import <stdatomic.h>
#import <stdbool.h>
#import <stdint.h>
#import <stdio.h>
#import <stdlib.h>
#import <string.h>

#ifndef kCMVideoCodecType_AV1
#define SLACKLESS_CODEC_AV1 ((CMVideoCodecType)0x61763031) // 'av01'
#else
#define SLACKLESS_CODEC_AV1 kCMVideoCodecType_AV1
#endif

// ---------------------------------------------------------------------------
// FFI Callback Types
// ---------------------------------------------------------------------------

typedef void (*SlacklessH264Callback)(
    void *ctx,
    const uint8_t *data,
    size_t len,
    uint32_t width,
    uint32_t height,
    uint64_t encode_us,
    bool is_keyframe);

typedef void (*SlacklessSurfaceCallback)(
    void *ctx,
    void *cv_pixel_buffer_retained,
    uint32_t width,
    uint32_t height);

typedef void (*SlacklessErrorCallback)(void *ctx, const char *message);
typedef void (*SlacklessCameraEnumCallback)(void *ctx, uint32_t index, const char *name);
typedef void (*SlacklessAudioDeviceChangeCallback)(void *ctx, bool is_input);

typedef void (*SlacklessThumbCallback)(
    void *ctx,
    const char *source_id,
    const uint8_t *bgra,
    size_t len,
    uint32_t width,
    uint32_t height);

typedef bool (*SlacklessWindowSkipCallback)(const char *app, const char *title);

typedef void (*SlacklessVpioCaptureCallback)(void *ctx, const float *samples, size_t count);
typedef void (*SlacklessVpioRenderCallback)(void *ctx, float *samples, size_t count);
typedef void (*SlacklessAudioTapCallback)(void *ctx, const int16_t *stereo_pcm, size_t count);

// ---------------------------------------------------------------------------
// Shared Utility Helpers
// ---------------------------------------------------------------------------

static inline uint64_t slackless_monotonic_us(void) {
    static mach_timebase_info_data_t tb = {0, 0};
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        mach_timebase_info(&tb);
    });
    uint64_t t = mach_absolute_time();
    if (tb.denom == 0) return 0;
    return (t * tb.numer) / (tb.denom * 1000ULL);
}

static inline void slackless_write_err(char *buf, size_t len, const char *msg) {
    if (!buf || len == 0) return;
    if (!msg) msg = "unknown macOS media error";
    snprintf(buf, len, "%s", msg);
}

static inline bool slackless_ensure_buf(uint8_t **buf, size_t *cap, size_t needed) {
    if (*cap >= needed) return true;
    size_t next = needed + (needed / 4) + 4096;
    uint8_t *p = (uint8_t *)realloc(*buf, next);
    if (!p) return false;
    *buf = p;
    *cap = next;
    return true;
}

static inline void slackless_compute_even_fit(
    uint32_t src_w,
    uint32_t src_h,
    uint32_t max_w,
    uint32_t max_h,
    uint32_t *out_w,
    uint32_t *out_h) {
    if (src_w == 0 || src_h == 0) {
        *out_w = max_w & ~1U;
        *out_h = max_h & ~1U;
        return;
    }
    double sx = max_w > 0 ? (double)max_w / (double)src_w : 1.0;
    double sy = max_h > 0 ? (double)max_h / (double)src_h : 1.0;
    double scale = sx < sy ? sx : sy;
    if (scale > 1.0) scale = 1.0;
    uint32_t w = ((uint32_t)((double)src_w * scale)) & ~1U;
    uint32_t h = ((uint32_t)((double)src_h * scale)) & ~1U;
    if (w < 2) w = 2;
    if (h < 2) h = 2;
    *out_w = w;
    *out_h = h;
}

// ---------------------------------------------------------------------------
// Internal Cross-Module Declarations
// ---------------------------------------------------------------------------

bool slackless_macos_supports_hw_av1_decode(void);

typedef struct VtEncoderCommon {
    void *ctx;
    SlacklessH264Callback on_h264;
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    atomic_bool stopped;
    atomic_bool force_keyframe;
    VTCompressionSessionRef vt;
    uint8_t *annex_b_buf;
    size_t annex_b_cap;
} VtEncoderCommon;

void slackless_vt_set_bitrate(VTCompressionSessionRef vt, uint32_t bitrate_bps);
OSStatus slackless_configure_vt_encoder(
    uint32_t width,
    uint32_t height,
    uint32_t fps,
    uint32_t bitrate_bps,
    double gop_seconds,
    VtEncoderCommon *enc,
    VTCompressionSessionRef *out_vt);
void slackless_encode_cvpb(VtEncoderCommon *enc, CVImageBufferRef pixelBuffer, CMTime pts);

typedef struct VtSurfaceDecodeCtx {
    void *user_ctx;
    SlacklessSurfaceCallback on_surface;
    bool got_frame;
} VtSurfaceDecodeCtx;

void slackless_vt_decompress_surface_cb(
    void *decompressionOutputRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTDecodeInfoFlags infoFlags,
    CVImageBufferRef imageBuffer,
    CMTime presentationTimeStamp,
    CMTime presentationDuration);
NSDictionary *slackless_vt_surface_output_attrs(void);

#endif // SLACKLESS_MACOS_COMMON_H
