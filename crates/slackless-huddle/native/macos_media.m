// Native macOS hardware media bridge for Slackless Huddles:
// - Runtime capability probing (supports all M1/M2/M3/M4 & Intel Macs; never assumes M3+)
// - Zero-copy IOSurface / CVPixelBuffer (`420f` full-range NV12) surface sharing with GPUI Metal
// - Accelerate (vImage NEON) fast YCbCr->BGRA + high-quality downscaling when CPU pixels are needed
// - ScreenCaptureKit (SCStream, `420f`) + VideoToolbox HW H.264 High/CABAC encode with dynamic ABR
// - AVFoundation (AVCaptureSession, `420f`) + VideoToolbox HW H.264 webcam + Vision/Metal blur
// - VideoToolbox HW H.264 decode & runtime-gated M3+ HW AV1 decode (`420f` zero-copy output)
// - CoreAudio event-driven default input/output device hot-plug monitor (zero polling)

#import <AVFoundation/AVFoundation.h>
#import <Accelerate/Accelerate.h>
#import <AppKit/AppKit.h>
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

// ---------------------------------------------------------------------------
// Utilities & Runtime Capabilities
// ---------------------------------------------------------------------------

static uint64_t monotonic_us(void) {
    static mach_timebase_info_data_t tb = {0, 0};
    if (tb.denom == 0) {
        mach_timebase_info(&tb);
    }
    uint64_t t = mach_absolute_time();
    return (t * tb.numer) / (tb.denom * 1000ULL);
}

static void write_err(char *buf, size_t len, const char *msg) {
    if (!buf || len == 0) return;
    if (!msg) msg = "unknown macOS media error";
    snprintf(buf, len, "%s", msg);
}

static bool ensure_buf(uint8_t **buf, size_t *cap, size_t needed) {
    if (*cap >= needed) return true;
    size_t next = needed + (needed / 4) + 4096;
    uint8_t *p = (uint8_t *)realloc(*buf, next);
    if (!p) return false;
    *buf = p;
    *cap = next;
    return true;
}

bool slackless_has_screen_capture_permission(void) {
    return CGPreflightScreenCaptureAccess();
}

bool slackless_request_screen_capture_permission(void) {
    return CGRequestScreenCaptureAccess();
}

bool slackless_macos_supports_hw_h264_decode(void) {
    return VTIsHardwareDecodeSupported(kCMVideoCodecType_H264);
}

bool slackless_macos_supports_hw_av1_decode(void) {
    if (@available(macOS 14.0, *)) {
        return VTIsHardwareDecodeSupported(SLACKLESS_CODEC_AV1);
    }
    return false;
}

static void compute_even_fit(
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
// Zero-Copy CVPixelBuffer Helpers & Accelerate (vImage NEON) BGRA Conversion
// ---------------------------------------------------------------------------

void *slackless_cvpb_retain(void *pb) {
    if (!pb) return NULL;
    return (void *)CFRetain((CFTypeRef)pb);
}

void slackless_cvpb_release(void *pb) {
    if (!pb) return;
    CFRelease((CFTypeRef)pb);
}

void slackless_cvpb_dimensions(void *pb, uint32_t *out_w, uint32_t *out_h) {
    if (!pb) {
        if (out_w) *out_w = 0;
        if (out_h) *out_h = 0;
        return;
    }
    CVPixelBufferRef buf = (CVPixelBufferRef)pb;
    if (out_w) *out_w = (uint32_t)CVPixelBufferGetWidth(buf);
    if (out_h) *out_h = (uint32_t)CVPixelBufferGetHeight(buf);
}

void slackless_free_buf(uint8_t *buf) {
    free(buf);
}

bool slackless_cvpb_to_bgra(
    void *pb,
    uint32_t max_w,
    uint32_t max_h,
    uint8_t **out_buf,
    size_t *out_len,
    uint32_t *out_w,
    uint32_t *out_h) {
    if (!pb || !out_buf || !out_len || !out_w || !out_h) return false;
    CVPixelBufferRef buf = (CVPixelBufferRef)pb;
    size_t sw = CVPixelBufferGetWidth(buf);
    size_t sh = CVPixelBufferGetHeight(buf);
    if (sw == 0 || sh == 0) return false;

    uint32_t dw = (uint32_t)sw;
    uint32_t dh = (uint32_t)sh;
    if (max_w > 0 && max_h > 0 && (sw > max_w || sh > max_h)) {
        compute_even_fit((uint32_t)sw, (uint32_t)sh, max_w, max_h, &dw, &dh);
    }

    // Fast path: Apple Silicon hardware VTPixelTransferSession scales AND converts 420f -> 32BGRA
    // in a single GPU/Media-Engine pass into a small pooled destination buffer (zero full-res CPU
    // YCbCr conversion and zero CPU scaling!).
    {
        static pthread_mutex_t xfer_lock = PTHREAD_MUTEX_INITIALIZER;
        static VTPixelTransferSessionRef xfer_session = NULL;
        static CVPixelBufferPoolRef xfer_pool = NULL;
        static uint32_t xfer_pool_w = 0;
        static uint32_t xfer_pool_h = 0;

        pthread_mutex_lock(&xfer_lock);
        if (!xfer_session) {
            if (VTPixelTransferSessionCreate(kCFAllocatorDefault, &xfer_session) == noErr && xfer_session) {
                VTSessionSetProperty(
                    xfer_session, kVTPixelTransferPropertyKey_ScalingMode, kVTScalingMode_Normal);
            }
        }
        if (xfer_session && (!xfer_pool || xfer_pool_w != dw || xfer_pool_h != dh)) {
            if (xfer_pool) {
                CVPixelBufferPoolRelease(xfer_pool);
                xfer_pool = NULL;
            }
            NSDictionary *attrs = @{
                (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
                (__bridge NSString *)kCVPixelBufferWidthKey : @(dw),
                (__bridge NSString *)kCVPixelBufferHeightKey : @(dh),
                (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
            };
            if (CVPixelBufferPoolCreate(
                    kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)attrs, &xfer_pool) == kCVReturnSuccess) {
                xfer_pool_w = dw;
                xfer_pool_h = dh;
            }
        }
        CVPixelBufferRef dstPB = NULL;
        bool xfer_ok = false;
        if (xfer_session && xfer_pool &&
            CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, xfer_pool, &dstPB) == kCVReturnSuccess &&
            dstPB) {
            if (VTPixelTransferSessionTransferImage(xfer_session, buf, dstPB) == noErr) {
                xfer_ok = true;
            } else {
                CFRelease(dstPB);
                dstPB = NULL;
            }
        }
        pthread_mutex_unlock(&xfer_lock);

        if (xfer_ok && dstPB) {
            if (CVPixelBufferLockBaseAddress(dstPB, kCVPixelBufferLock_ReadOnly) == kCVReturnSuccess) {
                size_t row_bytes = (size_t)dw * 4;
                size_t total = row_bytes * (size_t)dh;
                uint8_t *packed = (uint8_t *)malloc(total);
                const uint8_t *base = (const uint8_t *)CVPixelBufferGetBaseAddress(dstPB);
                size_t bpr = CVPixelBufferGetBytesPerRow(dstPB);
                if (packed && base) {
                    if (bpr == row_bytes) {
                        memcpy(packed, base, total);
                    } else {
                        for (uint32_t y = 0; y < dh; y++) {
                            memcpy(packed + (size_t)y * row_bytes, base + (size_t)y * bpr, row_bytes);
                        }
                    }
                    CVPixelBufferUnlockBaseAddress(dstPB, kCVPixelBufferLock_ReadOnly);
                    CFRelease(dstPB);
                    *out_buf = packed;
                    *out_len = total;
                    *out_w = dw;
                    *out_h = dh;
                    return true;
                }
                free(packed);
                CVPixelBufferUnlockBaseAddress(dstPB, kCVPixelBufferLock_ReadOnly);
            }
            CFRelease(dstPB);
        }
    }

    // Fallback: NEON vImage conversion on CPU.
    if (CVPixelBufferLockBaseAddress(buf, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return false;
    }

    OSType fmt = CVPixelBufferGetPixelFormatType(buf);
    uint8_t *full_bgra = (uint8_t *)malloc(sw * sh * 4);
    if (!full_bgra) {
        CVPixelBufferUnlockBaseAddress(buf, kCVPixelBufferLock_ReadOnly);
        return false;
    }

    bool converted = false;
    if (fmt == kCVPixelFormatType_32BGRA) {
        const uint8_t *base = (const uint8_t *)CVPixelBufferGetBaseAddress(buf);
        size_t bpr = CVPixelBufferGetBytesPerRow(buf);
        size_t row_bytes = sw * 4;
        if (base) {
            if (bpr == row_bytes) {
                memcpy(full_bgra, base, row_bytes * sh);
            } else {
                for (size_t y = 0; y < sh; y++) {
                    memcpy(full_bgra + y * row_bytes, base + y * bpr, row_bytes);
                }
            }
            converted = true;
        }
    } else if (
        fmt == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange ||
        fmt == kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange) {
        vImage_Buffer y_plane = {
            .data = CVPixelBufferGetBaseAddressOfPlane(buf, 0),
            .height = CVPixelBufferGetHeightOfPlane(buf, 0),
            .width = CVPixelBufferGetWidthOfPlane(buf, 0),
            .rowBytes = CVPixelBufferGetBytesPerRowOfPlane(buf, 0),
        };
        vImage_Buffer cbcr_plane = {
            .data = CVPixelBufferGetBaseAddressOfPlane(buf, 1),
            .height = CVPixelBufferGetHeightOfPlane(buf, 1),
            .width = CVPixelBufferGetWidthOfPlane(buf, 1),
            .rowBytes = CVPixelBufferGetBytesPerRowOfPlane(buf, 1),
        };
        vImage_Buffer dst_bgra = {
            .data = full_bgra,
            .height = sh,
            .width = sw,
            .rowBytes = sw * 4,
        };
        if (y_plane.data && cbcr_plane.data) {
            static vImage_YpCbCrToARGB full_info;
            static vImage_YpCbCrToARGB video_info;
            static dispatch_once_t once;
            dispatch_once(&once, ^{
                vImage_YpCbCrPixelRange full_range = {0, 128, 255, 255, 255, 1, 255, 0};
                vImage_YpCbCrPixelRange video_range = {16, 128, 235, 240, 255, 0, 255, 0};
                vImageConvert_YpCbCrToARGB_GenerateConversion(
                    kvImage_YpCbCrToARGBMatrix_ITU_R_601_4,
                    &full_range,
                    &full_info,
                    kvImage420Yp8_CbCr8,
                    kvImageARGB8888,
                    kvImageNoFlags);
                vImageConvert_YpCbCrToARGB_GenerateConversion(
                    kvImage_YpCbCrToARGBMatrix_ITU_R_709_2,
                    &video_range,
                    &video_info,
                    kvImage420Yp8_CbCr8,
                    kvImageARGB8888,
                    kvImageNoFlags);
            });
            const vImage_YpCbCrToARGB *conv =
                (fmt == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange) ? &full_info : &video_info;
            // Permute ARGB (0=A,1=R,2=G,3=B) -> BGRA (0=B,1=G,2=R,3=A) directly in vImage NEON!
            const uint8_t permute_map[4] = {3, 2, 1, 0};
            vImage_Error err = vImageConvert_420Yp8_CbCr8ToARGB8888(
                &y_plane, &cbcr_plane, &dst_bgra, conv, permute_map, 255, kvImageNoFlags);
            converted = (err == kvImageNoError);
        }
    }

    CVPixelBufferUnlockBaseAddress(buf, kCVPixelBufferLock_ReadOnly);

    if (!converted) {
        free(full_bgra);
        return false;
    }

    if (dw != (uint32_t)sw || dh != (uint32_t)sh) {
        uint8_t *scaled = (uint8_t *)malloc((size_t)dw * (size_t)dh * 4);
        if (scaled) {
            vImage_Buffer src_v = {.data = full_bgra, .height = sh, .width = sw, .rowBytes = sw * 4};
            vImage_Buffer dst_v = {.data = scaled, .height = dh, .width = dw, .rowBytes = (size_t)dw * 4};
            if (vImageScale_ARGB8888(&src_v, &dst_v, NULL, kvImageNoFlags) == kvImageNoError) {
                free(full_bgra);
                *out_buf = scaled;
                *out_len = (size_t)dw * (size_t)dh * 4;
                *out_w = dw;
                *out_h = dh;
                return true;
            }
            free(scaled);
        }
        dw = (uint32_t)sw;
        dh = (uint32_t)sh;
    }

    *out_buf = full_bgra;
    *out_len = (size_t)dw * (size_t)dh * 4;
    *out_w = dw;
    *out_h = dh;
    return true;
}

// ---------------------------------------------------------------------------
// Shared VideoToolbox H.264 Hardware Encoder Configuration & Annex-B Emitter
// ---------------------------------------------------------------------------

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

static void vt_set_bitrate(VTCompressionSessionRef vt, uint32_t bitrate_bps) {
    if (!vt || bitrate_bps == 0) return;
    int32_t br = (int32_t)bitrate_bps;
    CFNumberRef brNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &br);
    if (brNum) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_AverageBitRate, brNum);
        CFRelease(brNum);
    }
    // Smooth rate ceiling (~1.3x target over 1s window) prevents TURN packet bursts.
    int32_t bytes_per_sec = (int32_t)(((uint64_t)bitrate_bps * 13ULL) / 80ULL);
    double window_sec = 1.0;
    CFNumberRef limBytes = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_sec);
    CFNumberRef limSec = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &window_sec);
    if (limBytes && limSec) {
        const void *limVals[2] = {limBytes, limSec};
        CFArrayRef limits = CFArrayCreate(kCFAllocatorDefault, limVals, 2, &kCFTypeArrayCallBacks);
        if (limits) {
            VTSessionSetProperty(vt, kVTCompressionPropertyKey_DataRateLimits, limits);
            CFRelease(limits);
        }
    }
    if (limBytes) CFRelease(limBytes);
    if (limSec) CFRelease(limSec);
}

static void vt_common_compress_cb(
    void *outputCallbackRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTEncodeInfoFlags infoFlags,
    CMSampleBufferRef sampleBuffer) {
    (void)infoFlags;
    VtEncoderCommon *enc = (VtEncoderCommon *)outputCallbackRefCon;
    if (!enc || atomic_load(&enc->stopped) || status != noErr || !sampleBuffer) {
        return;
    }
    if (!CMSampleBufferDataIsReady(sampleBuffer)) {
        return;
    }

    uint64_t submit_us = (uint64_t)(uintptr_t)sourceFrameRefCon;
    uint64_t now = monotonic_us();
    uint64_t encode_us = now >= submit_us ? (now - submit_us) : 0;

    bool is_keyframe = true;
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        CFDictionaryRef dict = (CFDictionaryRef)CFArrayGetValueAtIndex(attachments, 0);
        if (dict && CFDictionaryContainsKey(dict, kCMSampleAttachmentKey_NotSync)) {
            CFBooleanRef notSync = (CFBooleanRef)CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync);
            if (notSync && CFBooleanGetValue(notSync)) {
                is_keyframe = false;
            }
        }
    }

    CMFormatDescriptionRef fmt = CMSampleBufferGetFormatDescription(sampleBuffer);
    if (!fmt) return;

    int nal_len_size = 4;
    size_t ps_count = 0;
    CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, 0, NULL, NULL, &ps_count, &nal_len_size);
    if (nal_len_size <= 0 || nal_len_size > 4) nal_len_size = 4;

    CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sampleBuffer);
    if (!block) return;

    size_t total_len = 0;
    char *data_ptr = NULL;
    if (CMBlockBufferGetDataPointer(block, 0, NULL, &total_len, &data_ptr) != kCMBlockBufferNoErr || !data_ptr) {
        return;
    }

    static const uint8_t start_code[4] = {0, 0, 0, 1};
    size_t out_len = 0;

    if (is_keyframe) {
        for (size_t i = 0; i < ps_count; i++) {
            const uint8_t *ps_ptr = NULL;
            size_t ps_size = 0;
            if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, i, &ps_ptr, &ps_size, NULL, NULL) == noErr &&
                ps_ptr && ps_size > 0) {
                if (!ensure_buf(&enc->annex_b_buf, &enc->annex_b_cap, out_len + 4 + ps_size)) {
                    return;
                }
                memcpy(enc->annex_b_buf + out_len, start_code, 4);
                out_len += 4;
                memcpy(enc->annex_b_buf + out_len, ps_ptr, ps_size);
                out_len += ps_size;
            }
        }
    }

    size_t offset = 0;
    while (offset + (size_t)nal_len_size <= total_len) {
        uint32_t nal_size = 0;
        for (int b = 0; b < nal_len_size; b++) {
            nal_size = (nal_size << 8) | (uint8_t)data_ptr[offset + (size_t)b];
        }
        offset += (size_t)nal_len_size;
        if (nal_size == 0 || offset + nal_size > total_len) {
            break;
        }
        if (!ensure_buf(&enc->annex_b_buf, &enc->annex_b_cap, out_len + 4 + nal_size)) {
            return;
        }
        memcpy(enc->annex_b_buf + out_len, start_code, 4);
        out_len += 4;
        memcpy(enc->annex_b_buf + out_len, data_ptr + offset, nal_size);
        out_len += nal_size;
        offset += nal_size;
    }

    if (out_len > 0 && enc->on_h264) {
        enc->on_h264(enc->ctx, enc->annex_b_buf, out_len, enc->width, enc->height, encode_us, is_keyframe);
    }
}

static OSStatus configure_vt_encoder(
    uint32_t width,
    uint32_t height,
    uint32_t fps,
    uint32_t bitrate_bps,
    double gop_seconds,
    VtEncoderCommon *enc,
    VTCompressionSessionRef *out_vt) {
    NSDictionary *encSpec = @{
        (__bridge NSString *)kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder : @YES,
    };
    OSStatus st = VTCompressionSessionCreate(
        kCFAllocatorDefault,
        (int32_t)width,
        (int32_t)height,
        kCMVideoCodecType_H264,
        (__bridge CFDictionaryRef)encSpec,
        NULL,
        kCFAllocatorDefault,
        vt_common_compress_cb,
        enc,
        out_vt);
    if (st != noErr || !*out_vt) {
        return st != noErr ? st : -1;
    }

    VTCompressionSessionRef vt = *out_vt;
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    // High profile + CABAC (no B-frames) gives ~15-25% sharper text & detail at identical bitrate
    // with zero added latency; decoded natively by Chrome/Slack, Safari, Firefox, OpenH264 & VT.
    if (VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel) != noErr) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Main_AutoLevel);
    }
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_H264EntropyMode, kVTH264EntropyMode_CABAC);

    vt_set_bitrate(vt, bitrate_bps);

    int32_t fpsVal = (int32_t)(fps > 0 ? fps : 30);
    CFNumberRef fpsNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &fpsVal);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_ExpectedFrameRate, fpsNum);
    CFRelease(fpsNum);

    int32_t gop = (int32_t)((double)fpsVal * gop_seconds);
    if (gop < fpsVal) gop = fpsVal;
    CFNumberRef gopNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &gop);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameInterval, gopNum);
    CFRelease(gopNum);

    CFNumberRef gopSecNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &gop_seconds);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, gopSecNum);
    CFRelease(gopSecNum);

    VTCompressionSessionPrepareToEncodeFrames(vt);
    return noErr;
}

static void encode_cvpb(VtEncoderCommon *enc, CVImageBufferRef pixelBuffer, CMTime pts) {
    if (!enc || !enc->vt || !pixelBuffer) return;
    if (!CMTIME_IS_VALID(pts)) {
        pts = CMTimeMake((int64_t)monotonic_us(), 1000000);
    }
    CMTime dur = CMTimeMake(1, (int32_t)(enc->fps > 0 ? enc->fps : 30));

    CFDictionaryRef frameProps = NULL;
    if (atomic_exchange(&enc->force_keyframe, false)) {
        const void *keys[] = {kVTEncodeFrameOptionKey_ForceKeyFrame};
        const void *vals[] = {kCFBooleanTrue};
        frameProps = CFDictionaryCreate(
            kCFAllocatorDefault, keys, vals, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    }

    uint64_t submit_us = monotonic_us();
    VTCompressionSessionEncodeFrame(
        enc->vt, pixelBuffer, pts, dur, frameProps, (void *)(uintptr_t)submit_us, NULL);
    if (frameProps) {
        CFRelease(frameProps);
    }
}

// ---------------------------------------------------------------------------
// ScreenCaptureKit + VideoToolbox Hardware H.264 Screen Capture (`420f` Zero-Copy)
// ---------------------------------------------------------------------------

typedef struct SlacklessScCapture SlacklessScCapture;

@interface SlacklessStreamDelegate : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, assign) SlacklessScCapture *owner;
@end

struct SlacklessScCapture {
    VtEncoderCommon enc;
    SlacklessSurfaceCallback on_surface_preview;
    SlacklessErrorCallback on_error;

    uint64_t last_preview_us;
    dispatch_queue_t sample_queue;
    void *stream_obj;   // retained SCStream*
    void *delegate_obj; // retained SlacklessStreamDelegate*
};

@implementation SlacklessStreamDelegate

- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
                   ofType:(SCStreamOutputType)type API_AVAILABLE(macos(12.3)) {
    (void)stream;
    SlacklessScCapture *cap = self.owner;
    if (!cap || atomic_load(&cap->enc.stopped) || type != SCStreamOutputTypeScreen || !sampleBuffer) {
        return;
    }

    // Ignore blank/idle/suspended frames from ScreenCaptureKit.
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sampleBuffer, false);
    if (attachments && CFArrayGetCount(attachments) > 0) {
        NSDictionary *dict = (__bridge NSDictionary *)CFArrayGetValueAtIndex(attachments, 0);
        NSNumber *statusNum = dict[SCStreamFrameInfoStatus];
        if (statusNum && (SCFrameStatus)statusNum.integerValue != SCFrameStatusComplete) {
            return;
        }
    }

    CVImageBufferRef pixelBuffer = CMSampleBufferGetImageBuffer(sampleBuffer);
    if (!pixelBuffer) {
        return;
    }

    CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
    encode_cvpb(&cap->enc, pixelBuffer, pts);

    // Zero-copy local preview: just CFRetain the IOSurface-backed `420f` CVPixelBuffer (throttled
    // to ~20 fps so GPUI layout stays light while visual preview is buttery smooth).
    uint64_t now_us = monotonic_us();
    if (cap->on_surface_preview && (now_us - cap->last_preview_us) >= 50000ULL) {
        cap->last_preview_us = now_us;
        size_t w = CVPixelBufferGetWidth(pixelBuffer);
        size_t h = CVPixelBufferGetHeight(pixelBuffer);
        if (w > 0 && h > 0) {
            void *retained = (void *)CFRetain(pixelBuffer);
            cap->on_surface_preview(cap->enc.ctx, retained, (uint32_t)w, (uint32_t)h);
        }
    }
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error API_AVAILABLE(macos(12.3)) {
    (void)stream;
    SlacklessScCapture *cap = self.owner;
    if (!cap || atomic_load(&cap->enc.stopped) || !error) return;
    if (cap->on_error) {
        const char *msg = error.localizedDescription.UTF8String;
        cap->on_error(cap->enc.ctx, msg ? msg : "SCStream stopped with error");
    }
}

@end

SlacklessScCapture *slackless_sc_capture_start(
    uint32_t is_monitor,
    uint32_t target_id,
    uint32_t max_w,
    uint32_t max_h,
    uint32_t max_fps,
    uint32_t bitrate_bps,
    void *ctx,
    SlacklessH264Callback on_h264,
    SlacklessSurfaceCallback on_surface_preview,
    SlacklessErrorCallback on_error,
    char *err_buf,
    size_t err_buf_len) {
    @autoreleasepool {
        if (@available(macOS 12.3, *)) {
            __block SCShareableContent *content = nil;
            __block NSError *contentErr = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                                       onScreenWindowsOnly:YES
                                                         completionHandler:^(SCShareableContent *c, NSError *e) {
                                                             content = c;
                                                             contentErr = e;
                                                             dispatch_semaphore_signal(sem);
                                                         }];
            if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 5LL * NSEC_PER_SEC)) != 0) {
                write_err(err_buf, err_buf_len, "timed out querying ScreenCaptureKit shareable content");
                return NULL;
            }
            if (!content) {
                write_err(
                    err_buf,
                    err_buf_len,
                    contentErr ? contentErr.localizedDescription.UTF8String
                               : "ScreenCaptureKit unavailable (check Screen Recording permission)");
                return NULL;
            }

            SCContentFilter *filter = nil;
            uint32_t src_w = max_w;
            uint32_t src_h = max_h;

            if (is_monitor) {
                SCDisplay *picked = nil;
                for (SCDisplay *d in content.displays) {
                    if (target_id == 0 || (uint32_t)d.displayID == target_id) {
                        picked = d;
                        break;
                    }
                }
                if (!picked && content.displays.count > 0) {
                    picked = content.displays.firstObject;
                }
                if (!picked) {
                    write_err(err_buf, err_buf_len, "no matching ScreenCaptureKit display found");
                    return NULL;
                }
                filter = [[SCContentFilter alloc] initWithDisplay:picked excludingWindows:@[]];
                src_w = (uint32_t)picked.width;
                src_h = (uint32_t)picked.height;
                CGDisplayModeRef mode = CGDisplayCopyDisplayMode(picked.displayID);
                if (mode) {
                    size_t pw = CGDisplayModeGetPixelWidth(mode);
                    size_t ph = CGDisplayModeGetPixelHeight(mode);
                    if (pw > 0 && ph > 0) {
                        src_w = (uint32_t)pw;
                        src_h = (uint32_t)ph;
                    }
                    CGDisplayModeRelease(mode);
                }
            } else {
                SCWindow *picked = nil;
                for (SCWindow *w in content.windows) {
                    if ((uint32_t)w.windowID == target_id) {
                        picked = w;
                        break;
                    }
                }
                if (!picked) {
                    write_err(err_buf, err_buf_len, "matching ScreenCaptureKit window not found");
                    return NULL;
                }
                filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:picked];
                CGRect frame = picked.frame;
                double scale = 2.0;
                if (content.displays.count > 0) {
                    SCDisplay *d0 = content.displays.firstObject;
                    CGDisplayModeRef mode = CGDisplayCopyDisplayMode(d0.displayID);
                    if (mode && d0.width > 0) {
                        double s = (double)CGDisplayModeGetPixelWidth(mode) / (double)d0.width;
                        if (s >= 1.0 && s <= 4.0) scale = s;
                    }
                    if (mode) CGDisplayModeRelease(mode);
                }
                src_w = (uint32_t)(frame.size.width * scale);
                src_h = (uint32_t)(frame.size.height * scale);
            }

            uint32_t out_w = 0, out_h = 0;
            compute_even_fit(src_w, src_h, max_w, max_h, &out_w, &out_h);

            SlacklessScCapture *cap = (SlacklessScCapture *)calloc(1, sizeof(SlacklessScCapture));
            if (!cap) {
                write_err(err_buf, err_buf_len, "out of memory allocating capture handle");
                return NULL;
            }
            cap->enc.ctx = ctx;
            cap->enc.on_h264 = on_h264;
            cap->enc.width = out_w;
            cap->enc.height = out_h;
            cap->enc.fps = max_fps > 0 ? max_fps : 30;
            atomic_store(&cap->enc.stopped, false);
            atomic_store(&cap->enc.force_keyframe, true);
            cap->on_surface_preview = on_surface_preview;
            cap->on_error = on_error;

            // 10s GOP avoids 3s IDR bitrate spikes/pulsing; RTCP PLI/FIR triggers immediate IDR.
            OSStatus vt_st = configure_vt_encoder(
                out_w, out_h, cap->enc.fps, bitrate_bps, 10.0, &cap->enc, &cap->enc.vt);
            if (vt_st != noErr) {
                char msg[128];
                snprintf(msg, sizeof(msg), "VTCompressionSessionCreate failed (OSStatus %d)", (int)vt_st);
                write_err(err_buf, err_buf_len, msg);
                free(cap);
                return NULL;
            }

            SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
            cfg.width = out_w;
            cfg.height = out_h;
            // Full-range NV12 (`420f`): 62% less bandwidth than 32BGRA, native to VideoToolbox,
            // AND directly renderable zero-copy by GPUI's Metal `paint_surface` shader!
            cfg.pixelFormat = kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
            cfg.minimumFrameInterval = CMTimeMake(1, (int32_t)cap->enc.fps);
            cfg.queueDepth = 6;
            cfg.showsCursor = YES;
            cfg.scalesToFit = YES;
            cfg.colorSpaceName = kCGColorSpaceSRGB;

            SlacklessStreamDelegate *delegate = [[SlacklessStreamDelegate alloc] init];
            delegate.owner = cap;

            SCStream *stream = [[SCStream alloc] initWithFilter:filter configuration:cfg delegate:delegate];
            cap->sample_queue = dispatch_queue_create("dev.slackless.sckit.video", DISPATCH_QUEUE_SERIAL);

            NSError *addErr = nil;
            if (![stream addStreamOutput:delegate
                                    type:SCStreamOutputTypeScreen
                      sampleHandlerQueue:cap->sample_queue
                                   error:&addErr]) {
                write_err(
                    err_buf,
                    err_buf_len,
                    addErr ? addErr.localizedDescription.UTF8String : "addStreamOutput failed");
                VTCompressionSessionInvalidate(cap->enc.vt);
                CFRelease(cap->enc.vt);
                free(cap);
                return NULL;
            }

            __block NSError *startErr = nil;
            dispatch_semaphore_t startSem = dispatch_semaphore_create(0);
            [stream startCaptureWithCompletionHandler:^(NSError *e) {
                startErr = e;
                dispatch_semaphore_signal(startSem);
            }];
            if (dispatch_semaphore_wait(startSem, dispatch_time(DISPATCH_TIME_NOW, 5LL * NSEC_PER_SEC)) != 0 ||
                startErr) {
                write_err(
                    err_buf,
                    err_buf_len,
                    startErr ? startErr.localizedDescription.UTF8String : "SCStream startCapture timed out");
                VTCompressionSessionInvalidate(cap->enc.vt);
                CFRelease(cap->enc.vt);
                free(cap);
                return NULL;
            }

            cap->stream_obj = (__bridge_retained void *)stream;
            cap->delegate_obj = (__bridge_retained void *)delegate;
            return cap;
        } else {
            write_err(err_buf, err_buf_len, "ScreenCaptureKit requires macOS 12.3+");
            return NULL;
        }
    }
}

void slackless_sc_capture_force_keyframe(SlacklessScCapture *cap) {
    if (!cap) return;
    atomic_store(&cap->enc.force_keyframe, true);
}

void slackless_sc_capture_set_bitrate(SlacklessScCapture *cap, uint32_t bitrate_bps) {
    if (!cap || atomic_load(&cap->enc.stopped) || !cap->enc.vt) return;
    vt_set_bitrate(cap->enc.vt, bitrate_bps);
}

void slackless_sc_capture_stop(SlacklessScCapture *cap) {
    if (!cap) return;
    @autoreleasepool {
        atomic_store(&cap->enc.stopped, true);
        if (@available(macOS 12.3, *)) {
            if (cap->stream_obj) {
                SCStream *stream = (__bridge_transfer SCStream *)cap->stream_obj;
                cap->stream_obj = NULL;
                dispatch_semaphore_t sem = dispatch_semaphore_create(0);
                [stream stopCaptureWithCompletionHandler:^(NSError *e) {
                    (void)e;
                    dispatch_semaphore_signal(sem);
                }];
                dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 1500LL * NSEC_PER_MSEC));
            }
            if (cap->delegate_obj) {
                SlacklessStreamDelegate *del = (__bridge_transfer SlacklessStreamDelegate *)cap->delegate_obj;
                del.owner = NULL;
                cap->delegate_obj = NULL;
            }
        }
        if (cap->sample_queue) {
            dispatch_sync(cap->sample_queue, ^{});
            cap->sample_queue = nil;
        }
        if (cap->enc.vt) {
            VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
            VTCompressionSessionInvalidate(cap->enc.vt);
            CFRelease(cap->enc.vt);
            cap->enc.vt = NULL;
        }
        free(cap->enc.annex_b_buf);
        free(cap);
    }
}

// ---------------------------------------------------------------------------
// AVFoundation + VideoToolbox Hardware H.264 Webcam + Vision/Metal Person Blur
// ---------------------------------------------------------------------------

static NSArray<AVCaptureDevice *> *discover_video_devices(void) {
    NSMutableArray<AVCaptureDeviceType> *types = [NSMutableArray arrayWithObjects:
        AVCaptureDeviceTypeBuiltInWideAngleCamera,
        nil];
    if (@available(macOS 14.0, *)) {
        [types addObject:AVCaptureDeviceTypeExternal];
        [types addObject:AVCaptureDeviceTypeContinuityCamera];
    } else {
        [types addObject:AVCaptureDeviceTypeExternalUnknown];
    }
    AVCaptureDeviceDiscoverySession *session =
        [AVCaptureDeviceDiscoverySession discoverySessionWithDeviceTypes:types
                                                               mediaType:AVMediaTypeVideo
                                                                position:AVCaptureDevicePositionUnspecified];
    return session.devices ?: @[];
}

void slackless_list_cameras(void *ctx, SlacklessCameraEnumCallback cb) {
    if (!cb) return;
    @autoreleasepool {
        NSArray<AVCaptureDevice *> *devices = discover_video_devices();
        uint32_t idx = 0;
        for (AVCaptureDevice *dev in devices) {
            const char *name = dev.localizedName.UTF8String;
            cb(ctx, idx, name);
            idx++;
        }
    }
}

typedef struct SlacklessCamCapture SlacklessCamCapture;

@interface SlacklessCamDelegate : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate>
@property(nonatomic, assign) SlacklessCamCapture *owner;
@end

struct SlacklessCamCapture {
    VtEncoderCommon enc;
    SlacklessSurfaceCallback on_surface_preview;
    SlacklessErrorCallback on_error;

    uint32_t max_w;
    uint32_t max_h;
    uint32_t bitrate_bps;
    atomic_bool blur_enabled;
    uint64_t last_preview_us;

    dispatch_queue_t sample_queue;
    dispatch_queue_t vision_queue;
    pthread_mutex_t mask_lock;
    atomic_bool vision_in_flight;
    void *session_obj;      // retained AVCaptureSession*
    void *delegate_obj;     // retained SlacklessCamDelegate*
    void *ci_ctx_obj;       // retained CIContext*
    void *vn_handler_obj;   // retained VNSequenceRequestHandler*
    void *vn_req_obj;       // retained VNGeneratePersonSegmentationRequest*
    void *blend_filter_obj; // retained CIFilter* (CIBlendWithMask)
    CVPixelBufferRef last_mask_pb;
    uint32_t blur_frame_idx;
    CVPixelBufferPoolRef blur_pool;
    uint32_t blur_pool_w;
    uint32_t blur_pool_h;
};

static bool ensure_blur_pool(SlacklessCamCapture *cap, uint32_t w, uint32_t h) {
    if (cap->blur_pool && cap->blur_pool_w == w && cap->blur_pool_h == h) {
        return true;
    }
    if (cap->blur_pool) {
        CVPixelBufferPoolRelease(cap->blur_pool);
        cap->blur_pool = NULL;
    }
    NSDictionary *pbAttrs = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
        (__bridge NSString *)kCVPixelBufferWidthKey : @(w),
        (__bridge NSString *)kCVPixelBufferHeightKey : @(h),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
    };
    CVReturn rv = CVPixelBufferPoolCreate(
        kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)pbAttrs, &cap->blur_pool);
    if (rv == kCVReturnSuccess && cap->blur_pool) {
        cap->blur_pool_w = w;
        cap->blur_pool_h = h;
        return true;
    }
    return false;
}

static CVPixelBufferRef apply_vision_metal_blur(SlacklessCamCapture *cap, CVPixelBufferRef srcPB) {
    if (!cap || !srcPB) return NULL;
    uint32_t w = (uint32_t)CVPixelBufferGetWidth(srcPB);
    uint32_t h = (uint32_t)CVPixelBufferGetHeight(srcPB);
    if (w < 2 || h < 2) return NULL;

    if (!cap->ci_ctx_obj) {
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        NSDictionary *opts = @{
            kCIContextWorkingColorSpace : [NSNull null],
            kCIContextOutputColorSpace : [NSNull null],
            kCIContextCacheIntermediates : @NO,
        };
        CIContext *ctx = device
            ? [CIContext contextWithMTLDevice:device options:opts]
            : [CIContext contextWithOptions:opts];
        if (!ctx) return NULL;
        cap->ci_ctx_obj = (__bridge_retained void *)ctx;
    }
    if (!cap->vn_handler_obj) {
        VNSequenceRequestHandler *handler = [[VNSequenceRequestHandler alloc] init];
        cap->vn_handler_obj = (__bridge_retained void *)handler;
    }
    if (!cap->vn_req_obj) {
        VNGeneratePersonSegmentationRequest *req = [[VNGeneratePersonSegmentationRequest alloc] init];
        // Apple's recommended live video-conferencing level (fast Neural Engine inference with
        // smooth mask edges well-suited for Gaussian background blur).
        req.qualityLevel = VNGeneratePersonSegmentationRequestQualityLevelFast;
        req.outputPixelFormat = kCVPixelFormatType_OneComponent8;
        cap->vn_req_obj = (__bridge_retained void *)req;
    }

    VNSequenceRequestHandler *handler = (__bridge VNSequenceRequestHandler *)cap->vn_handler_obj;
    VNGeneratePersonSegmentationRequest *req = (__bridge VNGeneratePersonSegmentationRequest *)cap->vn_req_obj;
    CIContext *ciCtx = (__bridge CIContext *)cap->ci_ctx_obj;

    // 4-way hardware pipeline parallelism:
    // - Frame 0 runs Vision synchronously so there is never an unblurred startup flash.
    // - Subsequent even frames (12 fps) dispatch Apple Neural Engine person segmentation
    //   asynchronously onto `vision_queue` (coalesced by `vision_in_flight`), while the capture
    //   thread immediately composites on Metal GPU with the latest completed mask and feeds
    //   VideoToolbox H.264 with zero ANE stalls.
    CVPixelBufferRef maskPB = NULL;
    pthread_mutex_lock(&cap->mask_lock);
    if (cap->last_mask_pb) {
        maskPB = (CVPixelBufferRef)CFRetain(cap->last_mask_pb);
    }
    pthread_mutex_unlock(&cap->mask_lock);

    bool need_sync = (maskPB == NULL);
    bool schedule_async = (!need_sync) && ((cap->blur_frame_idx & 1U) == 0);
    cap->blur_frame_idx++;

    if (need_sync) {
        NSError *vnErr = nil;
        if ([handler performRequests:@[req] onCVPixelBuffer:srcPB error:&vnErr]) {
            VNPixelBufferObservation *obs = req.results.firstObject;
            CVPixelBufferRef freshMask = obs ? obs.pixelBuffer : NULL;
            if (freshMask) {
                pthread_mutex_lock(&cap->mask_lock);
                if (cap->last_mask_pb) {
                    CFRelease(cap->last_mask_pb);
                }
                cap->last_mask_pb = (CVPixelBufferRef)CFRetain(freshMask);
                maskPB = (CVPixelBufferRef)CFRetain(freshMask);
                pthread_mutex_unlock(&cap->mask_lock);
            }
        }
    } else if (schedule_async && cap->vision_queue && !atomic_exchange(&cap->vision_in_flight, true)) {
        CVPixelBufferRef retainedSrc = (CVPixelBufferRef)CFRetain(srcPB);
        dispatch_async(cap->vision_queue, ^{
            @autoreleasepool {
                if (!atomic_load(&cap->enc.stopped)) {
                    NSError *vnErr = nil;
                    if ([handler performRequests:@[req] onCVPixelBuffer:retainedSrc error:&vnErr]) {
                        VNPixelBufferObservation *obs = req.results.firstObject;
                        CVPixelBufferRef freshMask = obs ? obs.pixelBuffer : NULL;
                        if (freshMask) {
                            pthread_mutex_lock(&cap->mask_lock);
                            if (cap->last_mask_pb) {
                                CFRelease(cap->last_mask_pb);
                            }
                            cap->last_mask_pb = (CVPixelBufferRef)CFRetain(freshMask);
                            pthread_mutex_unlock(&cap->mask_lock);
                        }
                    }
                }
                CFRelease(retainedSrc);
                atomic_store(&cap->vision_in_flight, false);
            }
        });
    }
    if (!maskPB) return NULL;

    if (!ensure_blur_pool(cap, w, h)) {
        CFRelease(maskPB);
        return NULL;
    }
    CVPixelBufferRef outPB = NULL;
    if (CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, cap->blur_pool, &outPB) != kCVReturnSuccess || !outPB) {
        CFRelease(maskPB);
        return NULL;
    }

    CIImage *srcImg = [CIImage imageWithCVPixelBuffer:srcPB];
    CIImage *maskImg = [CIImage imageWithCVPixelBuffer:maskPB];
    CGRect extent = srcImg.extent;
    CGRect maskExtent = maskImg.extent;
    if (maskExtent.size.width > 0 && maskExtent.size.height > 0) {
        CGFloat sx = extent.size.width / maskExtent.size.width;
        CGFloat sy = extent.size.height / maskExtent.size.height;
        maskImg = [maskImg imageByApplyingTransform:CGAffineTransformMakeScale(sx, sy)];
    }

    // 4x downsample -> Gaussian blur (sigma 3.5) -> 4x upscale on Metal GPU:
    // ~16x fewer shader ops and memory bandwidth than full-res sigma 14.0 with identical bokeh!
    CIImage *smallBg = [srcImg imageByApplyingTransform:CGAffineTransformMakeScale(0.25, 0.25)];
    CIImage *blurredSmall = [[smallBg imageByClampingToExtent]
        imageByApplyingGaussianBlurWithSigma:3.5];
    CIImage *blurred = [[blurredSmall imageByApplyingTransform:CGAffineTransformMakeScale(4.0, 4.0)]
        imageByCroppingToRect:extent];

    if (!cap->blend_filter_obj) {
        CIFilter *f = [CIFilter filterWithName:@"CIBlendWithMask"];
        if (f) {
            cap->blend_filter_obj = (__bridge_retained void *)f;
        }
    }
    CIFilter *blend = (__bridge CIFilter *)cap->blend_filter_obj;
    if (!blend) {
        CFRelease(maskPB);
        CFRelease(outPB);
        return NULL;
    }
    [blend setValue:srcImg forKey:kCIInputImageKey];
    [blend setValue:blurred forKey:kCIInputBackgroundImageKey];
    [blend setValue:maskImg forKey:kCIInputMaskImageKey];
    CIImage *composited = blend.outputImage;
    if (!composited) {
        CFRelease(maskPB);
        CFRelease(outPB);
        return NULL;
    }

    [ciCtx render:composited toCVPixelBuffer:outPB];
    CFRelease(maskPB);
    return outPB;
}

@implementation SlacklessCamDelegate

- (void)captureOutput:(AVCaptureOutput *)output
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
           fromConnection:(AVCaptureConnection *)connection {
    (void)output;
    (void)connection;
    SlacklessCamCapture *cap = self.owner;
    if (!cap || atomic_load(&cap->enc.stopped) || !sampleBuffer) {
        return;
    }

    @autoreleasepool {
        CVImageBufferRef rawPB = CMSampleBufferGetImageBuffer(sampleBuffer);
        if (!rawPB) return;

        uint32_t w = (uint32_t)CVPixelBufferGetWidth(rawPB);
        uint32_t h = (uint32_t)CVPixelBufferGetHeight(rawPB);
        if (w < 2 || h < 2) return;

        // Lazily create/recreate VT encoder only when an H.264 consumer is attached.
        if (cap->enc.on_h264) {
            uint32_t enc_w = w & ~1U;
            uint32_t enc_h = h & ~1U;
            if (!cap->enc.vt || cap->enc.width != enc_w || cap->enc.height != enc_h) {
                if (cap->enc.vt) {
                    VTCompressionSessionInvalidate(cap->enc.vt);
                    CFRelease(cap->enc.vt);
                    cap->enc.vt = NULL;
                }
                cap->enc.width = enc_w;
                cap->enc.height = enc_h;
                atomic_store(&cap->enc.force_keyframe, true);
                if (configure_vt_encoder(
                        enc_w, enc_h, cap->enc.fps, cap->bitrate_bps, 8.0, &cap->enc, &cap->enc.vt) != noErr) {
                    return;
                }
            }
        }

        CVPixelBufferRef blurredPB = NULL;
        CVPixelBufferRef activePB = rawPB;
        if (atomic_load(&cap->blur_enabled)) {
            blurredPB = apply_vision_metal_blur(cap, rawPB);
            if (blurredPB) {
                activePB = blurredPB;
            }
        }

        if (cap->enc.on_h264) {
            CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
            encode_cvpb(&cap->enc, activePB, pts);
        }

        uint64_t now_us = monotonic_us();
        if (cap->on_surface_preview && (now_us - cap->last_preview_us) >= 50000ULL) {
            cap->last_preview_us = now_us;
            void *retained = (void *)CFRetain(activePB);
            cap->on_surface_preview(cap->enc.ctx, retained, w, h);
        }

        if (blurredPB) {
            CFRelease(blurredPB);
        }
    }
}

@end

SlacklessCamCapture *slackless_cam_capture_start(
    int32_t device_index,
    uint32_t max_w,
    uint32_t max_h,
    uint32_t fps,
    uint32_t bitrate_bps,
    bool blur_enabled,
    void *ctx,
    SlacklessH264Callback on_h264,
    SlacklessSurfaceCallback on_surface_preview,
    SlacklessErrorCallback on_error,
    char *err_buf,
    size_t err_buf_len) {
    @autoreleasepool {
        NSArray<AVCaptureDevice *> *devices = discover_video_devices();
        if (devices.count == 0) {
            write_err(err_buf, err_buf_len, "no camera device found");
            return NULL;
        }
        NSUInteger pickedIdx = 0;
        if (device_index >= 0 && (NSUInteger)device_index < devices.count) {
            pickedIdx = (NSUInteger)device_index;
        }
        AVCaptureDevice *device = devices[pickedIdx];

        NSError *inputErr = nil;
        AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:device error:&inputErr];
        if (!input) {
            write_err(
                err_buf,
                err_buf_len,
                inputErr ? inputErr.localizedDescription.UTF8String : "failed to open camera device");
            return NULL;
        }

        AVCaptureSession *session = [[AVCaptureSession alloc] init];
        [session beginConfiguration];
        bool want_compact = (max_w > 0 && max_w <= 640);
        if (want_compact && [session canSetSessionPreset:AVCaptureSessionPreset640x480]) {
            session.sessionPreset = AVCaptureSessionPreset640x480;
        } else if ([session canSetSessionPreset:AVCaptureSessionPreset1280x720]) {
            session.sessionPreset = AVCaptureSessionPreset1280x720;
        } else if ([session canSetSessionPreset:AVCaptureSessionPreset640x480]) {
            session.sessionPreset = AVCaptureSessionPreset640x480;
        } else if ([session canSetSessionPreset:AVCaptureSessionPresetMedium]) {
            session.sessionPreset = AVCaptureSessionPresetMedium;
        }
        if (![session canAddInput:input]) {
            [session commitConfiguration];
            write_err(err_buf, err_buf_len, "camera input could not be added to AVCaptureSession");
            return NULL;
        }
        [session addInput:input];

        // Cap device framerate cleanly to target fps (24-30 fps) to reduce ISP + encoder load.
        if (fps > 0 && [device lockForConfiguration:nil]) {
            CMTime targetDur = CMTimeMake(1, (int32_t)fps);
            for (AVFrameRateRange *r in device.activeFormat.videoSupportedFrameRateRanges) {
                if ((double)fps >= r.minFrameRate - 0.5 && (double)fps <= r.maxFrameRate + 0.5) {
                    device.activeVideoMinFrameDuration = targetDur;
                    device.activeVideoMaxFrameDuration = targetDur;
                    break;
                }
            }
            [device unlockForConfiguration];
        }

        SlacklessCamCapture *cap = (SlacklessCamCapture *)calloc(1, sizeof(SlacklessCamCapture));
        if (!cap) {
            [session commitConfiguration];
            write_err(err_buf, err_buf_len, "out of memory allocating camera handle");
            return NULL;
        }
        cap->enc.ctx = ctx;
        cap->enc.on_h264 = on_h264;
        cap->enc.fps = fps > 0 ? fps : 24;
        atomic_store(&cap->enc.stopped, false);
        atomic_store(&cap->enc.force_keyframe, true);
        cap->on_surface_preview = on_surface_preview;
        cap->on_error = on_error;
        cap->max_w = max_w > 0 ? max_w : 1280;
        cap->max_h = max_h > 0 ? max_h : 720;
        cap->bitrate_bps = bitrate_bps > 0 ? bitrate_bps : 1200000;
        atomic_store(&cap->blur_enabled, blur_enabled);

        AVCaptureVideoDataOutput *output = [[AVCaptureVideoDataOutput alloc] init];
        output.alwaysDiscardsLateVideoFrames = YES;
        output.videoSettings = @{
            (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
        };

        SlacklessCamDelegate *delegate = [[SlacklessCamDelegate alloc] init];
        delegate.owner = cap;
        pthread_mutex_init(&cap->mask_lock, NULL);
        atomic_store(&cap->vision_in_flight, false);
        cap->sample_queue = dispatch_queue_create("dev.slackless.avcam.video", DISPATCH_QUEUE_SERIAL);
        cap->vision_queue = dispatch_queue_create("dev.slackless.avcam.vision", DISPATCH_QUEUE_SERIAL);
        [output setSampleBufferDelegate:delegate queue:cap->sample_queue];

        if (![session canAddOutput:output]) {
            [session commitConfiguration];
            pthread_mutex_destroy(&cap->mask_lock);
            cap->sample_queue = nil;
            cap->vision_queue = nil;
            free(cap);
            write_err(err_buf, err_buf_len, "camera video output could not be added");
            return NULL;
        }
        [session addOutput:output];
        [session commitConfiguration];

        cap->session_obj = (__bridge_retained void *)session;
        cap->delegate_obj = (__bridge_retained void *)delegate;

        [session startRunning];
        return cap;
    }
}

void slackless_cam_capture_set_blur(SlacklessCamCapture *cap, bool enabled) {
    if (!cap) return;
    atomic_store(&cap->blur_enabled, enabled);
}

void slackless_cam_capture_force_keyframe(SlacklessCamCapture *cap) {
    if (!cap) return;
    atomic_store(&cap->enc.force_keyframe, true);
}

void slackless_cam_capture_set_bitrate(SlacklessCamCapture *cap, uint32_t bitrate_bps) {
    if (!cap || atomic_load(&cap->enc.stopped)) return;
    cap->bitrate_bps = bitrate_bps;
    if (cap->enc.vt) {
        vt_set_bitrate(cap->enc.vt, bitrate_bps);
    }
}

void slackless_cam_capture_set_compact(SlacklessCamCapture *cap, bool compact) {
    if (!cap || atomic_load(&cap->enc.stopped) || !cap->session_obj || !cap->sample_queue) return;
    uint32_t target_bps = compact ? 500000U : 1200000U;
    cap->bitrate_bps = target_bps;
    dispatch_async(cap->sample_queue, ^{
        @autoreleasepool {
            if (atomic_load(&cap->enc.stopped) || !cap->session_obj) return;
            AVCaptureSession *session = (__bridge AVCaptureSession *)cap->session_obj;
            AVCaptureSessionPreset desired = compact
                ? AVCaptureSessionPreset640x480
                : AVCaptureSessionPreset1280x720;
            if ([session canSetSessionPreset:desired] && ![session.sessionPreset isEqualToString:desired]) {
                [session beginConfiguration];
                session.sessionPreset = desired;
                [session commitConfiguration];
            }
            if (cap->enc.vt) {
                vt_set_bitrate(cap->enc.vt, target_bps);
            }
            atomic_store(&cap->enc.force_keyframe, true);
        }
    });
}

void slackless_cam_capture_stop(SlacklessCamCapture *cap) {
    if (!cap) return;
    @autoreleasepool {
        atomic_store(&cap->enc.stopped, true);
        if (cap->session_obj) {
            AVCaptureSession *session = (__bridge_transfer AVCaptureSession *)cap->session_obj;
            cap->session_obj = NULL;
            [session stopRunning];
        }
        if (cap->delegate_obj) {
            SlacklessCamDelegate *del = (__bridge_transfer SlacklessCamDelegate *)cap->delegate_obj;
            del.owner = NULL;
            cap->delegate_obj = NULL;
        }
        if (cap->sample_queue) {
            dispatch_sync(cap->sample_queue, ^{});
            cap->sample_queue = nil;
        }
        if (cap->vision_queue) {
            dispatch_sync(cap->vision_queue, ^{});
            cap->vision_queue = nil;
        }
        if (cap->enc.vt) {
            VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
            VTCompressionSessionInvalidate(cap->enc.vt);
            CFRelease(cap->enc.vt);
            cap->enc.vt = NULL;
        }
        pthread_mutex_lock(&cap->mask_lock);
        if (cap->last_mask_pb) {
            CFRelease(cap->last_mask_pb);
            cap->last_mask_pb = NULL;
        }
        pthread_mutex_unlock(&cap->mask_lock);
        pthread_mutex_destroy(&cap->mask_lock);
        if (cap->blend_filter_obj) {
            (void)(__bridge_transfer CIFilter *)cap->blend_filter_obj;
            cap->blend_filter_obj = NULL;
        }
        if (cap->blur_pool) {
            CVPixelBufferPoolRelease(cap->blur_pool);
            cap->blur_pool = NULL;
        }
        if (cap->ci_ctx_obj) {
            (void)(__bridge_transfer CIContext *)cap->ci_ctx_obj;
            cap->ci_ctx_obj = NULL;
        }
        if (cap->vn_handler_obj) {
            (void)(__bridge_transfer VNSequenceRequestHandler *)cap->vn_handler_obj;
            cap->vn_handler_obj = NULL;
        }
        if (cap->vn_req_obj) {
            (void)(__bridge_transfer VNGeneratePersonSegmentationRequest *)cap->vn_req_obj;
            cap->vn_req_obj = NULL;
        }
        free(cap->enc.annex_b_buf);
        free(cap);
    }
}

// ---------------------------------------------------------------------------
// VideoToolbox Hardware H.264 Decoder (Annex-B -> Zero-Copy `420f` Surface)
// ---------------------------------------------------------------------------

typedef struct SlacklessVtDecoder {
    VTDecompressionSessionRef session;
    CMVideoFormatDescriptionRef fmt;
    uint8_t *sps;
    size_t sps_len;
    uint8_t *pps;
    size_t pps_len;
    uint8_t *avcc_buf;
    size_t avcc_cap;
} SlacklessVtDecoder;

typedef struct VtSurfaceDecodeCtx {
    void *user_ctx;
    SlacklessSurfaceCallback on_surface;
    bool got_frame;
} VtSurfaceDecodeCtx;

static void vt_decompress_surface_cb(
    void *decompressionOutputRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTDecodeInfoFlags infoFlags,
    CVImageBufferRef imageBuffer,
    CMTime presentationTimeStamp,
    CMTime presentationDuration) {
    (void)decompressionOutputRefCon;
    (void)infoFlags;
    (void)presentationTimeStamp;
    (void)presentationDuration;
    VtSurfaceDecodeCtx *call = (VtSurfaceDecodeCtx *)sourceFrameRefCon;
    if (!call || status != noErr || !imageBuffer || !call->on_surface) return;

    size_t w = CVPixelBufferGetWidth(imageBuffer);
    size_t h = CVPixelBufferGetHeight(imageBuffer);
    if (w == 0 || h == 0) return;

    // Zero-copy handoff: CFRetain the IOSurface-backed `420f` CVPixelBuffer directly!
    void *retained = (void *)CFRetain(imageBuffer);
    call->on_surface(call->user_ctx, retained, (uint32_t)w, (uint32_t)h);
    call->got_frame = true;
}

static NSDictionary *vt_surface_output_attrs(void) {
    return @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
    };
}

static bool vt_decoder_rebuild(SlacklessVtDecoder *dec) {
    if (!dec->sps || dec->sps_len == 0 || !dec->pps || dec->pps_len == 0) {
        return false;
    }
    if (dec->session) {
        VTDecompressionSessionInvalidate(dec->session);
        CFRelease(dec->session);
        dec->session = NULL;
    }
    if (dec->fmt) {
        CFRelease(dec->fmt);
        dec->fmt = NULL;
    }

    const uint8_t *ps_ptrs[2] = {dec->sps, dec->pps};
    size_t ps_sizes[2] = {dec->sps_len, dec->pps_len};
    OSStatus st = CMVideoFormatDescriptionCreateFromH264ParameterSets(
        kCFAllocatorDefault, 2, ps_ptrs, ps_sizes, 4, &dec->fmt);
    if (st != noErr || !dec->fmt) {
        return false;
    }

    NSDictionary *decSpec = @{
        (__bridge NSString *)kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder : @YES,
    };
    VTDecompressionOutputCallbackRecord cb = {vt_decompress_surface_cb, NULL};
    st = VTDecompressionSessionCreate(
        kCFAllocatorDefault,
        dec->fmt,
        (__bridge CFDictionaryRef)decSpec,
        (__bridge CFDictionaryRef)vt_surface_output_attrs(),
        &cb,
        &dec->session);
    if (st != noErr || !dec->session) {
        CFRelease(dec->fmt);
        dec->fmt = NULL;
        return false;
    }
    VTSessionSetProperty(dec->session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
    return true;
}

SlacklessVtDecoder *slackless_vt_decoder_new(void) {
    return (SlacklessVtDecoder *)calloc(1, sizeof(SlacklessVtDecoder));
}

void slackless_vt_decoder_free(SlacklessVtDecoder *dec) {
    if (!dec) return;
    if (dec->session) {
        VTDecompressionSessionInvalidate(dec->session);
        CFRelease(dec->session);
    }
    if (dec->fmt) {
        CFRelease(dec->fmt);
    }
    free(dec->sps);
    free(dec->pps);
    free(dec->avcc_buf);
    free(dec);
}

static size_t find_start_code(const uint8_t *buf, size_t len, size_t from, size_t *sc_len) {
    for (size_t i = from; i + 3 <= len; i++) {
        if (buf[i] == 0 && buf[i + 1] == 0) {
            if (buf[i + 2] == 1) {
                *sc_len = 3;
                return i;
            }
            if (i + 4 <= len && buf[i + 2] == 0 && buf[i + 3] == 1) {
                *sc_len = 4;
                return i;
            }
        }
    }
    *sc_len = 0;
    return len;
}

bool slackless_vt_decoder_decode(
    SlacklessVtDecoder *dec,
    const uint8_t *annex_b,
    size_t len,
    void *ctx,
    SlacklessSurfaceCallback on_surface) {
    if (!dec || !annex_b || len < 4) return false;
    @autoreleasepool {
        bool ps_changed = false;
        size_t avcc_len = 0;

        size_t sc_len = 0;
        size_t pos = find_start_code(annex_b, len, 0, &sc_len);
        while (pos < len) {
            size_t nal_start = pos + sc_len;
            size_t next_sc = 0;
            size_t next_pos = find_start_code(annex_b, len, nal_start, &next_sc);
            size_t nal_len = next_pos > nal_start ? (next_pos - nal_start) : 0;

            if (nal_len > 0) {
                const uint8_t *nal = annex_b + nal_start;
                uint8_t nal_type = nal[0] & 0x1F;
                if (nal_type == 7) {
                    if (dec->sps_len != nal_len || !dec->sps || memcmp(dec->sps, nal, nal_len) != 0) {
                        uint8_t *p = (uint8_t *)realloc(dec->sps, nal_len);
                        if (p) {
                            dec->sps = p;
                            dec->sps_len = nal_len;
                            memcpy(dec->sps, nal, nal_len);
                            ps_changed = true;
                        }
                    }
                } else if (nal_type == 8) {
                    if (dec->pps_len != nal_len || !dec->pps || memcmp(dec->pps, nal, nal_len) != 0) {
                        uint8_t *p = (uint8_t *)realloc(dec->pps, nal_len);
                        if (p) {
                            dec->pps = p;
                            dec->pps_len = nal_len;
                            memcpy(dec->pps, nal, nal_len);
                            ps_changed = true;
                        }
                    }
                } else if (nal_type >= 1 && nal_type <= 5) {
                    if (!ensure_buf(&dec->avcc_buf, &dec->avcc_cap, avcc_len + 4 + nal_len)) {
                        return false;
                    }
                    uint32_t sz = (uint32_t)nal_len;
                    dec->avcc_buf[avcc_len] = (uint8_t)((sz >> 24) & 0xFF);
                    dec->avcc_buf[avcc_len + 1] = (uint8_t)((sz >> 16) & 0xFF);
                    dec->avcc_buf[avcc_len + 2] = (uint8_t)((sz >> 8) & 0xFF);
                    dec->avcc_buf[avcc_len + 3] = (uint8_t)(sz & 0xFF);
                    memcpy(dec->avcc_buf + avcc_len + 4, nal, nal_len);
                    avcc_len += 4 + nal_len;
                }
            }
            pos = next_pos;
            sc_len = next_sc;
        }

        if (ps_changed || !dec->session) {
            if (!vt_decoder_rebuild(dec)) {
                return false;
            }
        }
        if (avcc_len == 0 || !dec->session || !dec->fmt) {
            return false;
        }

        CMBlockBufferRef block = NULL;
        OSStatus st = CMBlockBufferCreateWithMemoryBlock(
            kCFAllocatorDefault,
            dec->avcc_buf,
            avcc_len,
            kCFAllocatorNull,
            NULL,
            0,
            avcc_len,
            0,
            &block);
        if (st != noErr || !block) return false;

        CMSampleBufferRef sbuf = NULL;
        const size_t sample_sizes[1] = {avcc_len};
        st = CMSampleBufferCreateReady(
            kCFAllocatorDefault, block, dec->fmt, 1, 0, NULL, 1, sample_sizes, &sbuf);
        CFRelease(block);
        if (st != noErr || !sbuf) return false;

        VtSurfaceDecodeCtx call = {ctx, on_surface, false};
        VTDecodeInfoFlags flagsOut = 0;
        // Synchronous flag (0) with RealTime=true delivers the decoded IOSurface immediately
        // without blocking on an extra WaitForAsynchronousFrames barrier!
        st = VTDecompressionSessionDecodeFrame(dec->session, sbuf, 0, &call, &flagsOut);
        CFRelease(sbuf);
        return st == noErr && call.got_frame;
    }
}

// ---------------------------------------------------------------------------
// VideoToolbox Hardware AV1 Decoder (Runtime-Gated M3+ Only -> Zero-Copy `420f`)
// ---------------------------------------------------------------------------

typedef struct BitReader {
    const uint8_t *data;
    size_t len;
    size_t bit_pos;
} BitReader;

static bool br_read_bits(BitReader *br, int n, uint32_t *out) {
    if (n == 0) {
        *out = 0;
        return true;
    }
    if (n > 32 || br->bit_pos + (size_t)n > br->len * 8) return false;
    uint32_t v = 0;
    for (int i = 0; i < n; i++) {
        size_t byte_ix = br->bit_pos >> 3;
        int bit_ix = 7 - (int)(br->bit_pos & 7);
        v = (v << 1) | ((br->data[byte_ix] >> bit_ix) & 1U);
        br->bit_pos++;
    }
    *out = v;
    return true;
}

static bool br_skip_uvlc(BitReader *br) {
    int leading = 0;
    uint32_t bit = 0;
    while (leading < 32) {
        if (!br_read_bits(br, 1, &bit)) return false;
        if (bit) break;
        leading++;
    }
    if (leading >= 32) return true;
    uint32_t dummy = 0;
    return br_read_bits(br, leading, &dummy);
}

typedef struct Av1SeqInfo {
    uint8_t seq_profile;
    uint8_t seq_level_idx_0;
    uint8_t seq_tier_0;
    uint32_t max_w;
    uint32_t max_h;
} Av1SeqInfo;

static bool parse_av1_seq_header(const uint8_t *payload, size_t len, Av1SeqInfo *info) {
    BitReader br = {payload, len, 0};
    uint32_t v = 0, still_pic = 0, reduced = 0;
    if (!br_read_bits(&br, 3, &v)) return false;
    info->seq_profile = (uint8_t)v;
    if (!br_read_bits(&br, 1, &still_pic)) return false;
    if (!br_read_bits(&br, 1, &reduced)) return false;
    info->seq_level_idx_0 = 0;
    info->seq_tier_0 = 0;

    if (reduced) {
        if (!br_read_bits(&br, 5, &v)) return false;
        info->seq_level_idx_0 = (uint8_t)v;
    } else {
        uint32_t timing_present = 0, dec_model_present = 0, buf_delay_len = 0;
        if (!br_read_bits(&br, 1, &timing_present)) return false;
        if (timing_present) {
            if (!br_read_bits(&br, 32, &v)) return false;
            if (!br_read_bits(&br, 32, &v)) return false;
            uint32_t eq_int = 0;
            if (!br_read_bits(&br, 1, &eq_int)) return false;
            if (eq_int && !br_skip_uvlc(&br)) return false;
            if (!br_read_bits(&br, 1, &dec_model_present)) return false;
            if (dec_model_present) {
                if (!br_read_bits(&br, 5, &buf_delay_len)) return false;
                if (!br_read_bits(&br, 32, &v)) return false;
                if (!br_read_bits(&br, 10, &v)) return false;
            }
        }
        uint32_t init_disp_present = 0, op_cnt_m1 = 0;
        if (!br_read_bits(&br, 1, &init_disp_present)) return false;
        if (!br_read_bits(&br, 5, &op_cnt_m1)) return false;
        for (uint32_t i = 0; i <= op_cnt_m1; i++) {
            uint32_t idc = 0, lvl = 0, tier = 0;
            if (!br_read_bits(&br, 12, &idc)) return false;
            if (!br_read_bits(&br, 5, &lvl)) return false;
            if (lvl > 7) {
                if (!br_read_bits(&br, 1, &tier)) return false;
            }
            if (i == 0) {
                info->seq_level_idx_0 = (uint8_t)lvl;
                info->seq_tier_0 = (uint8_t)tier;
            }
            if (dec_model_present) {
                uint32_t present_op = 0;
                if (!br_read_bits(&br, 1, &present_op)) return false;
                if (present_op) {
                    int n = (int)buf_delay_len + 1;
                    if (!br_read_bits(&br, n, &v)) return false;
                    if (!br_read_bits(&br, n, &v)) return false;
                    if (!br_read_bits(&br, 1, &v)) return false;
                }
            }
            if (init_disp_present) {
                uint32_t disp_op = 0;
                if (!br_read_bits(&br, 1, &disp_op)) return false;
                if (disp_op && !br_read_bits(&br, 4, &v)) return false;
            }
        }
    }

    uint32_t w_bits_m1 = 0, h_bits_m1 = 0, mw_m1 = 0, mh_m1 = 0;
    if (!br_read_bits(&br, 4, &w_bits_m1)) return false;
    if (!br_read_bits(&br, 4, &h_bits_m1)) return false;
    if (!br_read_bits(&br, (int)w_bits_m1 + 1, &mw_m1)) return false;
    if (!br_read_bits(&br, (int)h_bits_m1 + 1, &mh_m1)) return false;
    info->max_w = mw_m1 + 1;
    info->max_h = mh_m1 + 1;
    return info->max_w >= 2 && info->max_h >= 2;
}

static bool read_leb128_c(const uint8_t *buf, size_t len, size_t *out_val, size_t *out_n) {
    size_t val = 0;
    for (size_t i = 0; i < len && i < 8; i++) {
        val |= ((size_t)(buf[i] & 0x7F)) << (i * 7);
        if ((buf[i] & 0x80) == 0) {
            *out_val = val;
            *out_n = i + 1;
            return true;
        }
    }
    return false;
}

static bool find_av1_seq_obu(
    const uint8_t *tu,
    size_t len,
    const uint8_t **out_obu,
    size_t *out_obu_len,
    const uint8_t **out_payload,
    size_t *out_payload_len) {
    size_t pos = 0;
    while (pos < len) {
        uint8_t hdr = tu[pos];
        uint8_t obu_type = (hdr >> 3) & 0x0F;
        bool has_ext = (hdr & 0x04) != 0;
        bool has_size = (hdr & 0x02) != 0;
        size_t hdr_len = has_ext ? 2 : 1;
        if (pos + hdr_len > len) return false;

        size_t payload_len = 0;
        size_t size_len = 0;
        if (has_size) {
            if (!read_leb128_c(tu + pos + hdr_len, len - (pos + hdr_len), &payload_len, &size_len)) {
                return false;
            }
        } else {
            payload_len = len - (pos + hdr_len);
        }
        size_t total_obu = hdr_len + size_len + payload_len;
        if (pos + total_obu > len) return false;

        if (obu_type == 1) { // OBU_SEQUENCE_HEADER
            *out_obu = tu + pos;
            *out_obu_len = total_obu;
            *out_payload = tu + pos + hdr_len + size_len;
            *out_payload_len = payload_len;
            return true;
        }
        pos += total_obu;
    }
    return false;
}

typedef struct SlacklessVtAv1Decoder {
    VTDecompressionSessionRef session;
    CMVideoFormatDescriptionRef fmt;
    uint8_t *seq_obu;
    size_t seq_obu_len;
} SlacklessVtAv1Decoder;

SlacklessVtAv1Decoder *slackless_vt_av1_decoder_new(void) {
    if (!slackless_macos_supports_hw_av1_decode()) {
        return NULL;
    }
    return (SlacklessVtAv1Decoder *)calloc(1, sizeof(SlacklessVtAv1Decoder));
}

void slackless_vt_av1_decoder_free(SlacklessVtAv1Decoder *dec) {
    if (!dec) return;
    if (dec->session) {
        VTDecompressionSessionInvalidate(dec->session);
        CFRelease(dec->session);
    }
    if (dec->fmt) {
        CFRelease(dec->fmt);
    }
    free(dec->seq_obu);
    free(dec);
}

static bool vt_av1_rebuild(
    SlacklessVtAv1Decoder *dec,
    const uint8_t *seq_obu,
    size_t seq_obu_len,
    const Av1SeqInfo *info) {
    if (dec->session) {
        VTDecompressionSessionInvalidate(dec->session);
        CFRelease(dec->session);
        dec->session = NULL;
    }
    if (dec->fmt) {
        CFRelease(dec->fmt);
        dec->fmt = NULL;
    }

    // Standard ISO BMFF av1C configuration record (4-byte header + configOBUs).
    NSMutableData *av1c = [NSMutableData dataWithCapacity:4 + seq_obu_len];
    uint8_t hdr[4] = {
        0x81, // marker=1, version=1
        (uint8_t)(((info->seq_profile & 0x07) << 5) | (info->seq_level_idx_0 & 0x1F)),
        // tier, high_bitdepth=0, twelve_bit=0, mono=0, chroma_subsampling_x=1, y=1, sample_pos=0
        (uint8_t)(((info->seq_tier_0 & 0x01) << 7) | 0x0C),
        0x00,
    };
    [av1c appendBytes:hdr length:4];
    [av1c appendBytes:seq_obu length:seq_obu_len];

    NSDictionary *ext = @{
        (__bridge NSString *)kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms : @{
            @"av1C" : av1c,
        },
    };
    OSStatus st = CMVideoFormatDescriptionCreate(
        kCFAllocatorDefault,
        SLACKLESS_CODEC_AV1,
        (int32_t)info->max_w,
        (int32_t)info->max_h,
        (__bridge CFDictionaryRef)ext,
        &dec->fmt);
    if (st != noErr || !dec->fmt) {
        return false;
    }

    NSDictionary *decSpec = @{
        (__bridge NSString *)kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder : @YES,
    };
    VTDecompressionOutputCallbackRecord cb = {vt_decompress_surface_cb, NULL};
    st = VTDecompressionSessionCreate(
        kCFAllocatorDefault,
        dec->fmt,
        (__bridge CFDictionaryRef)decSpec,
        (__bridge CFDictionaryRef)vt_surface_output_attrs(),
        &cb,
        &dec->session);
    if (st != noErr || !dec->session) {
        CFRelease(dec->fmt);
        dec->fmt = NULL;
        return false;
    }
    VTSessionSetProperty(dec->session, kVTDecompressionPropertyKey_RealTime, kCFBooleanTrue);
    return true;
}

bool slackless_vt_av1_decoder_decode(
    SlacklessVtAv1Decoder *dec,
    const uint8_t *tu,
    size_t len,
    void *ctx,
    SlacklessSurfaceCallback on_surface) {
    if (!dec || !tu || len < 2) return false;
    @autoreleasepool {
        const uint8_t *seq_obu = NULL;
        size_t seq_obu_len = 0;
        const uint8_t *seq_payload = NULL;
        size_t seq_payload_len = 0;
        if (find_av1_seq_obu(tu, len, &seq_obu, &seq_obu_len, &seq_payload, &seq_payload_len)) {
            bool changed = !dec->seq_obu || dec->seq_obu_len != seq_obu_len ||
                           memcmp(dec->seq_obu, seq_obu, seq_obu_len) != 0;
            if (changed || !dec->session) {
                Av1SeqInfo info;
                if (!parse_av1_seq_header(seq_payload, seq_payload_len, &info)) {
                    return false;
                }
                if (!vt_av1_rebuild(dec, seq_obu, seq_obu_len, &info)) {
                    return false;
                }
                uint8_t *copy = (uint8_t *)realloc(dec->seq_obu, seq_obu_len);
                if (copy) {
                    dec->seq_obu = copy;
                    dec->seq_obu_len = seq_obu_len;
                    memcpy(dec->seq_obu, seq_obu, seq_obu_len);
                }
            }
        }

        if (!dec->session || !dec->fmt) {
            return false;
        }

        CMBlockBufferRef block = NULL;
        OSStatus st = CMBlockBufferCreateWithMemoryBlock(
            kCFAllocatorDefault,
            (void *)tu,
            len,
            kCFAllocatorNull,
            NULL,
            0,
            len,
            0,
            &block);
        if (st != noErr || !block) return false;

        CMSampleBufferRef sbuf = NULL;
        const size_t sample_sizes[1] = {len};
        st = CMSampleBufferCreateReady(
            kCFAllocatorDefault, block, dec->fmt, 1, 0, NULL, 1, sample_sizes, &sbuf);
        CFRelease(block);
        if (st != noErr || !sbuf) return false;

        VtSurfaceDecodeCtx call = {ctx, on_surface, false};
        VTDecodeInfoFlags flagsOut = 0;
        st = VTDecompressionSessionDecodeFrame(dec->session, sbuf, 0, &call, &flagsOut);
        CFRelease(sbuf);
        return st == noErr && call.got_frame;
    }
}

// ---------------------------------------------------------------------------
// CoreAudio Event-Driven Default Input/Output Device Hot-Plug Monitor
// ---------------------------------------------------------------------------

typedef struct SlacklessAudioDeviceMonitor {
    dispatch_queue_t queue;
    AudioObjectPropertyListenerBlock in_block;
    AudioObjectPropertyListenerBlock out_block;
    atomic_bool stopped;
} SlacklessAudioDeviceMonitor;

SlacklessAudioDeviceMonitor *slackless_audio_device_monitor_start(
    void *ctx,
    SlacklessAudioDeviceChangeCallback cb) {
    if (!cb) return NULL;
    SlacklessAudioDeviceMonitor *mon =
        (SlacklessAudioDeviceMonitor *)calloc(1, sizeof(SlacklessAudioDeviceMonitor));
    if (!mon) return NULL;
    atomic_store(&mon->stopped, false);
    mon->queue = dispatch_queue_create("dev.slackless.coreaudio.hotplug", DISPATCH_QUEUE_SERIAL);

    AudioObjectPropertyAddress inAddr = {
        kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    AudioObjectPropertyAddress outAddr = {
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };

    mon->in_block = ^(UInt32 numAddrs, const AudioObjectPropertyAddress *addrs) {
        (void)numAddrs;
        (void)addrs;
        if (!atomic_load(&mon->stopped)) {
            cb(ctx, true);
        }
    };
    mon->out_block = ^(UInt32 numAddrs, const AudioObjectPropertyAddress *addrs) {
        (void)numAddrs;
        (void)addrs;
        if (!atomic_load(&mon->stopped)) {
            cb(ctx, false);
        }
    };

    AudioObjectAddPropertyListenerBlock(kAudioObjectSystemObject, &inAddr, mon->queue, mon->in_block);
    AudioObjectAddPropertyListenerBlock(kAudioObjectSystemObject, &outAddr, mon->queue, mon->out_block);
    return mon;
}

void slackless_audio_device_monitor_stop(SlacklessAudioDeviceMonitor *mon) {
    if (!mon) return;
    atomic_store(&mon->stopped, true);
    AudioObjectPropertyAddress inAddr = {
        kAudioHardwarePropertyDefaultInputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    AudioObjectPropertyAddress outAddr = {
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    if (mon->in_block) {
        AudioObjectRemovePropertyListenerBlock(kAudioObjectSystemObject, &inAddr, mon->queue, mon->in_block);
    }
    if (mon->out_block) {
        AudioObjectRemovePropertyListenerBlock(kAudioObjectSystemObject, &outAddr, mon->queue, mon->out_block);
    }
    if (mon->queue) {
        dispatch_sync(mon->queue, ^{});
        mon->queue = nil;
    }
    mon->in_block = nil;
    mon->out_block = nil;
    free(mon);
}

// ---------------------------------------------------------------------------
// GPU-Downscaled Screen & Window Thumbnails via SCScreenshotManager (macOS 14+)
// ---------------------------------------------------------------------------

typedef void (*SlacklessThumbCallback)(
    void *ctx,
    const char *source_id,
    const uint8_t *bgra,
    size_t len,
    uint32_t width,
    uint32_t height);

static bool is_system_chrome_name(NSString *app, NSString *title) {
    NSString *al = [[app ?: @"" stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]] lowercaseString];
    NSString *tl = [[title ?: @"" stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]] lowercaseString];
    if ([al isEqualToString:@"dock"] || [al isEqualToString:@"notificationcenter"] ||
        [al isEqualToString:@"notiscenter"] || [al isEqualToString:@"window server"] ||
        [al isEqualToString:@"control center"] || [al isEqualToString:@"kontrollcenter"] ||
        [al isEqualToString:@"systemuiserver"] || [al isEqualToString:@"spotlight"]) {
        return true;
    }
    if ([tl isEqualToString:@"dock"] || [tl isEqualToString:@"menubar"] || [tl isEqualToString:@"statusindicator"]) {
        return true;
    }
    return false;
}

static void emit_cgimage_bgra(
    CGImageRef cgImg,
    const char *source_id,
    void *ctx,
    SlacklessThumbCallback cb) {
    if (!cgImg || !cb) return;
    size_t w = CGImageGetWidth(cgImg);
    size_t h = CGImageGetHeight(cgImg);
    if (w < 2 || h < 2) return;
    size_t len = w * h * 4;
    uint8_t *buf = (uint8_t *)calloc(1, len);
    if (!buf) return;
    CGColorSpaceRef cs = CGColorSpaceCreateDeviceRGB();
    CGBitmapInfo info = (CGBitmapInfo)kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little; // BGRA8888
    CGContextRef cgCtx = CGBitmapContextCreate(buf, w, h, 8, w * 4, cs, info);
    CGColorSpaceRelease(cs);
    if (cgCtx) {
        CGContextDrawImage(cgCtx, CGRectMake(0, 0, (CGFloat)w, (CGFloat)h), cgImg);
        CGContextRelease(cgCtx);
        cb(ctx, source_id, buf, len, (uint32_t)w, (uint32_t)h);
    }
    free(buf);
}

@interface SlacklessThumbBatch : NSObject {
@public
    pthread_mutex_t lock;
    bool cancelled;
    void *ctx;
    SlacklessThumbCallback cb;
}
@end

@implementation SlacklessThumbBatch
- (instancetype)initWithCtx:(void *)c callback:(SlacklessThumbCallback)f {
    if ((self = [super init])) {
        pthread_mutex_init(&lock, NULL);
        cancelled = false;
        ctx = c;
        cb = f;
    }
    return self;
}
- (void)dealloc {
    pthread_mutex_destroy(&lock);
}
- (void)emitImage:(CGImageRef)img sourceId:(const char *)sid {
    if (!img || !sid) return;
    pthread_mutex_lock(&lock);
    if (!cancelled && cb) {
        emit_cgimage_bgra(img, sid, ctx, cb);
    }
    pthread_mutex_unlock(&lock);
}
- (void)cancel {
    pthread_mutex_lock(&lock);
    cancelled = true;
    pthread_mutex_unlock(&lock);
}
@end

bool slackless_capture_thumbnails(
    bool monitors_only,
    uint32_t max_w,
    uint32_t max_h,
    void *ctx,
    SlacklessThumbCallback cb) {
    if (!cb || max_w < 2 || max_h < 2) return false;
    @autoreleasepool {
        if (@available(macOS 14.0, *)) {
            __block SCShareableContent *content = nil;
            dispatch_semaphore_t sem = dispatch_semaphore_create(0);
            [SCShareableContent getShareableContentExcludingDesktopWindows:NO
                                                       onScreenWindowsOnly:YES
                                                         completionHandler:^(SCShareableContent *c, NSError *e) {
                                                             (void)e;
                                                             content = c;
                                                             dispatch_semaphore_signal(sem);
                                                         }];
            if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 3LL * NSEC_PER_SEC)) != 0 || !content) {
                return false;
            }

            // Fan out all SCScreenshotManager GPU captures concurrently under a dispatch_group
            // so WindowServer/Metal renders all displays or up to 18 windows in parallel (~8x faster).
            SlacklessThumbBatch *batch = [[SlacklessThumbBatch alloc] initWithCtx:ctx callback:cb];
            dispatch_group_t grp = dispatch_group_create();

            if (monitors_only) {
                for (SCDisplay *d in content.displays) {
                    uint32_t dw = 0, dh = 0;
                    compute_even_fit((uint32_t)d.width, (uint32_t)d.height, max_w, max_h, &dw, &dh);
                    SCContentFilter *filter = [[SCContentFilter alloc] initWithDisplay:d excludingWindows:@[]];
                    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
                    cfg.width = dw;
                    cfg.height = dh;
                    cfg.scalesToFit = YES;
                    cfg.showsCursor = NO;
                    CGDirectDisplayID dispId = d.displayID;
                    dispatch_group_enter(grp);
                    [SCScreenshotManager captureImageWithFilter:filter
                                                  configuration:cfg
                                              completionHandler:^(CGImageRef img, NSError *err) {
                                                  (void)err;
                                                  if (img) {
                                                      char sid[64];
                                                      snprintf(sid, sizeof(sid), "monitor:%u", (unsigned)dispId);
                                                      [batch emitImage:img sourceId:sid];
                                                  }
                                                  dispatch_group_leave(grp);
                                              }];
                }
            } else {
                int count = 0;
                for (SCWindow *w in content.windows) {
                    if (count >= 18) break;
                    if (!w.isOnScreen) continue;
                    CGRect fr = w.frame;
                    if (fr.size.width < 120.0 || fr.size.height < 120.0) continue;
                    NSString *appName = w.owningApplication ? w.owningApplication.applicationName : @"";
                    NSString *title = w.title ?: @"";
                    if (appName.length == 0 && title.length == 0) continue;
                    if (is_system_chrome_name(appName, title)) continue;

                    count++;
                    uint32_t dw = 0, dh = 0;
                    compute_even_fit((uint32_t)fr.size.width, (uint32_t)fr.size.height, max_w, max_h, &dw, &dh);
                    SCContentFilter *filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:w];
                    SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
                    cfg.width = dw;
                    cfg.height = dh;
                    cfg.scalesToFit = YES;
                    cfg.showsCursor = NO;
                    CGWindowID winId = w.windowID;
                    dispatch_group_enter(grp);
                    [SCScreenshotManager captureImageWithFilter:filter
                                                  configuration:cfg
                                              completionHandler:^(CGImageRef img, NSError *err) {
                                                  (void)err;
                                                  if (img) {
                                                      char sid[64];
                                                      snprintf(sid, sizeof(sid), "window:%u", (unsigned)winId);
                                                      [batch emitImage:img sourceId:sid];
                                                  }
                                                  dispatch_group_leave(grp);
                                              }];
                }
            }

            dispatch_group_wait(grp, dispatch_time(DISPATCH_TIME_NOW, 2500LL * NSEC_PER_MSEC));
            [batch cancel];
            return true;
        }
        return false;
    }
}

// ---------------------------------------------------------------------------
// CoreAudio Native VoiceProcessingIO (Hardware-Tuned AEC / NS / AGC AudioUnit)
// ---------------------------------------------------------------------------

typedef void (*SlacklessVpioCaptureCallback)(void *ctx, const float *samples, size_t count);
typedef void (*SlacklessVpioRenderCallback)(void *ctx, float *samples, size_t count);

typedef struct SlacklessVpio {
    AudioComponentInstance au;
    void *ctx;
    SlacklessVpioCaptureCallback on_capture;
    SlacklessVpioRenderCallback on_render;
    float *capture_buf;
    size_t capture_cap;
    atomic_bool stopped;
} SlacklessVpio;

static OSStatus vpio_input_cb(
    void *inRefCon,
    AudioUnitRenderActionFlags *ioActionFlags,
    const AudioTimeStamp *inTimeStamp,
    UInt32 inBusNumber,
    UInt32 inNumberFrames,
    AudioBufferList *ioData) {
    (void)ioData;
    SlacklessVpio *vpio = (SlacklessVpio *)inRefCon;
    if (!vpio || atomic_load(&vpio->stopped) || !vpio->au || inNumberFrames == 0) {
        return noErr;
    }
    if (inNumberFrames > vpio->capture_cap) {
        // Real-time thread: avoid malloc if preallocated capacity fits; grow safely only if driver
        // delivers an unexpectedly huge buffer.
        float *p = (float *)realloc(vpio->capture_buf, (size_t)inNumberFrames * sizeof(float));
        if (!p) return noErr;
        vpio->capture_buf = p;
        vpio->capture_cap = inNumberFrames;
    }
    AudioBufferList abl;
    abl.mNumberBuffers = 1;
    abl.mBuffers[0].mNumberChannels = 1;
    abl.mBuffers[0].mDataByteSize = inNumberFrames * (UInt32)sizeof(float);
    abl.mBuffers[0].mData = vpio->capture_buf;

    OSStatus st = AudioUnitRender(vpio->au, ioActionFlags, inTimeStamp, inBusNumber, inNumberFrames, &abl);
    if (st == noErr && vpio->on_capture) {
        vpio->on_capture(vpio->ctx, vpio->capture_buf, (size_t)inNumberFrames);
    }
    return noErr;
}

static OSStatus vpio_render_cb(
    void *inRefCon,
    AudioUnitRenderActionFlags *ioActionFlags,
    const AudioTimeStamp *inTimeStamp,
    UInt32 inBusNumber,
    UInt32 inNumberFrames,
    AudioBufferList *ioData) {
    (void)ioActionFlags;
    (void)inTimeStamp;
    (void)inBusNumber;
    SlacklessVpio *vpio = (SlacklessVpio *)inRefCon;
    if (!ioData || ioData->mNumberBuffers == 0) return noErr;
    float *dst = (float *)ioData->mBuffers[0].mData;
    if (!dst) return noErr;
    if (!vpio || atomic_load(&vpio->stopped) || !vpio->on_render) {
        memset(dst, 0, (size_t)inNumberFrames * sizeof(float));
        return noErr;
    }
    vpio->on_render(vpio->ctx, dst, (size_t)inNumberFrames);
    return noErr;
}

void slackless_vpio_set_processing(SlacklessVpio *vpio, bool enable_vp, bool enable_agc) {
    if (!vpio || !vpio->au || atomic_load(&vpio->stopped)) return;
    UInt32 bypass = enable_vp ? 0 : 1;
    AudioUnitSetProperty(
        vpio->au,
        kAUVoiceIOProperty_BypassVoiceProcessing,
        kAudioUnitScope_Global,
        0,
        &bypass,
        sizeof(bypass));
    UInt32 agc = enable_agc ? 1 : 0;
    AudioUnitSetProperty(
        vpio->au,
        kAUVoiceIOProperty_VoiceProcessingEnableAGC,
        kAudioUnitScope_Global,
        0,
        &agc,
        sizeof(agc));
}

SlacklessVpio *slackless_vpio_start(
    uint32_t sample_rate,
    bool enable_vp,
    bool enable_agc,
    void *ctx,
    SlacklessVpioCaptureCallback on_capture,
    SlacklessVpioRenderCallback on_render) {
    AudioComponentDescription desc = {0};
    desc.componentType = kAudioUnitType_Output;
    desc.componentSubType = kAudioUnitSubType_VoiceProcessingIO;
    desc.componentManufacturer = kAudioUnitManufacturer_Apple;

    AudioComponent comp = AudioComponentFindNext(NULL, &desc);
    if (!comp) return NULL;

    SlacklessVpio *vpio = (SlacklessVpio *)calloc(1, sizeof(SlacklessVpio));
    if (!vpio) return NULL;
    vpio->ctx = ctx;
    vpio->on_capture = on_capture;
    vpio->on_render = on_render;
    vpio->capture_cap = 4096;
    vpio->capture_buf = (float *)calloc(vpio->capture_cap, sizeof(float));
    atomic_store(&vpio->stopped, false);

    if (AudioComponentInstanceNew(comp, &vpio->au) != noErr || !vpio->au) {
        free(vpio->capture_buf);
        free(vpio);
        return NULL;
    }

    UInt32 one = 1;
    AudioUnitSetProperty(vpio->au, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1, &one, sizeof(one));
    AudioUnitSetProperty(vpio->au, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0, &one, sizeof(one));

    AudioStreamBasicDescription fmt = {0};
    fmt.mSampleRate = (Float64)(sample_rate > 0 ? sample_rate : 48000);
    fmt.mFormatID = kAudioFormatLinearPCM;
    fmt.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    fmt.mFramesPerPacket = 1;
    fmt.mChannelsPerFrame = 1;
    fmt.mBitsPerChannel = 32;
    fmt.mBytesPerFrame = 4;
    fmt.mBytesPerPacket = 4;

    OSStatus st1 = AudioUnitSetProperty(
        vpio->au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Output, 1, &fmt, sizeof(fmt));
    OSStatus st2 = AudioUnitSetProperty(
        vpio->au, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &fmt, sizeof(fmt));
    if (st1 != noErr || st2 != noErr) {
        AudioComponentInstanceDispose(vpio->au);
        free(vpio->capture_buf);
        free(vpio);
        return NULL;
    }

    // Avoid unexpectedly ducking YouTube/Spotify/system audio during huddles (macOS 14+).
    if (@available(macOS 14.0, *)) {
        AUVoiceIOOtherAudioDuckingConfiguration duck = {
            .mEnableAdvancedDucking = false,
            .mDuckingLevel = kAUVoiceIOOtherAudioDuckingLevelMin,
        };
        AudioUnitSetProperty(
            vpio->au,
            kAUVoiceIOProperty_OtherAudioDuckingConfiguration,
            kAudioUnitScope_Global,
            0,
            &duck,
            sizeof(duck));
    }

    AURenderCallbackStruct inCb = {vpio_input_cb, vpio};
    AudioUnitSetProperty(
        vpio->au, kAudioOutputUnitProperty_SetInputCallback, kAudioUnitScope_Global, 1, &inCb, sizeof(inCb));

    AURenderCallbackStruct outCb = {vpio_render_cb, vpio};
    AudioUnitSetProperty(
        vpio->au, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &outCb, sizeof(outCb));

    slackless_vpio_set_processing(vpio, enable_vp, enable_agc);

    if (AudioUnitInitialize(vpio->au) != noErr || AudioOutputUnitStart(vpio->au) != noErr) {
        AudioUnitUninitialize(vpio->au);
        AudioComponentInstanceDispose(vpio->au);
        free(vpio->capture_buf);
        free(vpio);
        return NULL;
    }
    return vpio;
}

void slackless_vpio_stop(SlacklessVpio *vpio) {
    if (!vpio) return;
    atomic_store(&vpio->stopped, true);
    if (vpio->au) {
        AudioOutputUnitStop(vpio->au);
        AudioUnitUninitialize(vpio->au);
        AudioComponentInstanceDispose(vpio->au);
        vpio->au = NULL;
    }
    free(vpio->capture_buf);
    free(vpio);
}

// ---------------------------------------------------------------------------
// CoreAudio Output-Device-Routed Native Audio Player (`AVAudioEngine` + Gain)
// ---------------------------------------------------------------------------

static AudioDeviceID resolve_output_device_id_by_name(const char *device_name) {
    if (!device_name || device_name[0] == '\0') return kAudioObjectUnknown;
    NSString *target = [NSString stringWithUTF8String:device_name];
    if (!target || target.length == 0) return kAudioObjectUnknown;

    AudioObjectPropertyAddress prop = {
        kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    UInt32 dataSize = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &prop, 0, NULL, &dataSize) != noErr ||
        dataSize == 0) {
        return kAudioObjectUnknown;
    }
    UInt32 count = dataSize / (UInt32)sizeof(AudioDeviceID);
    AudioDeviceID *devs = (AudioDeviceID *)malloc(dataSize);
    if (!devs) return kAudioObjectUnknown;
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &prop, 0, NULL, &dataSize, devs) != noErr) {
        free(devs);
        return kAudioObjectUnknown;
    }

    AudioDeviceID matched = kAudioObjectUnknown;
    AudioObjectPropertyAddress nameAddr = {
        kAudioObjectPropertyName,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    AudioObjectPropertyAddress outStreamsAddr = {
        kAudioDevicePropertyStreams,
        kAudioDevicePropertyScopeOutput,
        kAudioObjectPropertyElementMain,
    };
    for (UInt32 i = 0; i < count; i++) {
        UInt32 streamBytes = 0;
        if (AudioObjectGetPropertyDataSize(devs[i], &outStreamsAddr, 0, NULL, &streamBytes) != noErr ||
            streamBytes == 0) {
            continue;
        }
        CFStringRef cfName = NULL;
        UInt32 sz = (UInt32)sizeof(cfName);
        if (AudioObjectGetPropertyData(devs[i], &nameAddr, 0, NULL, &sz, &cfName) == noErr && cfName) {
            NSString *name = (__bridge_transfer NSString *)cfName;
            if ([name isEqualToString:target]) {
                matched = devs[i];
                break;
            }
        }
    }
    free(devs);
    return matched;
}

static NSString *resolve_output_device_uid_by_name(const char *device_name) {
    AudioDeviceID dev = resolve_output_device_id_by_name(device_name);
    if (dev == kAudioObjectUnknown) return nil;
    AudioObjectPropertyAddress uidAddr = {
        kAudioDevicePropertyDeviceUID,
        kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain,
    };
    CFStringRef cfUid = NULL;
    UInt32 uidSz = (UInt32)sizeof(cfUid);
    if (AudioObjectGetPropertyData(dev, &uidAddr, 0, NULL, &uidSz, &cfUid) == noErr && cfUid) {
        return (__bridge_transfer NSString *)cfUid;
    }
    return nil;
}

@interface SlacklessEnginePlayer : NSObject
@property(nonatomic, strong) AVAudioEngine *engine;
@property(nonatomic, strong) AVAudioPlayerNode *player;
@property(nonatomic, strong) AVAudioUnitEQ *eq;
@property(nonatomic, strong) AVAudioFile *file;
@property(nonatomic, assign) bool loops;
@property(nonatomic, assign) bool stopped;
- (void)scheduleLoop;
- (void)stopNow;
@end

static NSMutableSet<SlacklessEnginePlayer *> *g_active_oneshot_engines = nil;

@implementation SlacklessEnginePlayer

- (void)scheduleLoop {
    if (self.stopped || !self.player || !self.file) return;
    __weak SlacklessEnginePlayer *weakSelf = self;
    [self.player scheduleFile:self.file
                       atTime:nil
               completionCallbackType:AVAudioPlayerNodeCompletionDataPlayedBack
                    completionHandler:^(AVAudioPlayerNodeCompletionCallbackType type) {
                        (void)type;
                        dispatch_async(dispatch_get_main_queue(), ^{
                            SlacklessEnginePlayer *strongSelf = weakSelf;
                            if (!strongSelf || strongSelf.stopped) return;
                            if (strongSelf.loops) {
                                [strongSelf scheduleLoop];
                            } else {
                                [strongSelf stopNow];
                                [g_active_oneshot_engines removeObject:strongSelf];
                            }
                        });
                    }];
}

- (void)stopNow {
    self.stopped = true;
    if (self.player) {
        [self.player stop];
        self.player = nil;
    }
    if (self.engine) {
        [self.engine stop];
        self.engine = nil;
    }
    self.eq = nil;
    self.file = nil;
}

@end

// Populate and start a CoreAudio-routed AVAudioEngine player inside `holder`, supporting true
// linear gain from -48 dB to +24 dB via AVAudioUnitEQ.globalGain (unlike NSSound.volume which
// caps at 1.0 and cannot lift quiet tracks like -46 dBFS piano_concerto).
static bool start_engine_audio_player_into(
    SlacklessEnginePlayer *holder,
    NSString *nsPath,
    AudioDeviceID deviceId,
    float linearGain,
    bool loops) {
    NSURL *url = [NSURL fileURLWithPath:nsPath];
    NSError *err = nil;
    AVAudioFile *file = [[AVAudioFile alloc] initForReading:url error:&err];
    if (!file || err) return false;

    AVAudioEngine *engine = [[AVAudioEngine alloc] init];
    if (deviceId != kAudioObjectUnknown && engine.outputNode.audioUnit) {
        AudioUnitSetProperty(
            engine.outputNode.audioUnit,
            kAudioOutputUnitProperty_CurrentDevice,
            kAudioUnitScope_Global,
            0,
            &deviceId,
            sizeof(deviceId));
    }

    AVAudioPlayerNode *player = [[AVAudioPlayerNode alloc] init];
    AVAudioUnitEQ *eq = [[AVAudioUnitEQ alloc] initWithNumberOfBands:0];
    float gain = linearGain > 1e-4f ? linearGain : 0.32f;
    float gainDb = 20.0f * log10f(gain);
    if (gainDb < -48.0f) gainDb = -48.0f;
    if (gainDb > 24.0f) gainDb = 24.0f;
    eq.globalGain = gainDb;

    [engine attachNode:player];
    [engine attachNode:eq];
    [engine connect:player to:eq format:file.processingFormat];
    [engine connect:eq to:engine.mainMixerNode format:file.processingFormat];

    if (![engine startAndReturnError:&err] || err) {
        return false;
    }

    holder.engine = engine;
    holder.player = player;
    holder.eq = eq;
    holder.file = file;
    holder.loops = loops;
    holder.stopped = false;
    [holder scheduleLoop];
    [player play];
    return true;
}

typedef struct SlacklessAudioHandle {
    void *holder_obj; // retained SlacklessEnginePlayer*
} SlacklessAudioHandle;

bool slackless_audio_file_play_oneshot(const char *path, const char *device_name, float volume) {
    if (!path || path[0] == '\0') return false;
    NSString *nsPath = [NSString stringWithUTF8String:path];
    AudioDeviceID devId = resolve_output_device_id_by_name(device_name);
    NSString *uid = resolve_output_device_uid_by_name(device_name);
    if (!nsPath) return false;
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (!g_active_oneshot_engines) {
                g_active_oneshot_engines = [NSMutableSet set];
            }
            SlacklessEnginePlayer *holder = [[SlacklessEnginePlayer alloc] init];
            if (start_engine_audio_player_into(holder, nsPath, devId, volume, false)) {
                [g_active_oneshot_engines addObject:holder];
                return;
            }
            // Fallback to NSSound if AVAudioEngine fails to start.
            NSSound *sound = [[NSSound alloc] initWithContentsOfFile:nsPath byReference:YES];
            if (!sound) return;
            sound.volume = volume > 1.0f ? 1.0f : (volume > 0.0f ? volume : 0.85f);
            sound.loops = NO;
            if (uid) sound.playbackDeviceIdentifier = uid;
            [sound play];
        }
    });
    return true;
}

SlacklessAudioHandle *slackless_audio_file_start_loop(
    const char *path,
    const char *device_name,
    float volume) {
    if (!path || path[0] == '\0') return NULL;
    NSString *nsPath = [NSString stringWithUTF8String:path];
    AudioDeviceID devId = resolve_output_device_id_by_name(device_name);
    if (!nsPath) return NULL;
    SlacklessEnginePlayer *holder = [[SlacklessEnginePlayer alloc] init];
    holder.loops = true;
    holder.stopped = false;
    SlacklessAudioHandle *h = (SlacklessAudioHandle *)calloc(1, sizeof(SlacklessAudioHandle));
    if (!h) return NULL;
    h->holder_obj = (__bridge_retained void *)holder;

    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (holder.stopped) return;
            start_engine_audio_player_into(holder, nsPath, devId, volume, true);
        }
    });
    return h;
}

// Measure full-track perceptually weighted (200 Hz high-pass) gated RMS loudness of an audio file
// via CoreAudio ExtAudioFile so quiet tracks (-46 dBFS piano) and hot tracks (-16 dBFS lofi) are
// normalized to the exact same perceived level. Also caps gain against peak headroom so playback
// never clips.
// Returns linear weighted RMS in (0.0, 1.0], or -1.0f if decoding fails.
float slackless_audio_file_measure_rms(const char *path) {
    if (!path || path[0] == '\0') return -1.0f;
    @autoreleasepool {
        NSString *nsPath = [NSString stringWithUTF8String:path];
        if (!nsPath) return -1.0f;
        NSURL *url = [NSURL fileURLWithPath:nsPath];
        ExtAudioFileRef extFile = NULL;
        if (ExtAudioFileOpenURL((__bridge CFURLRef)url, &extFile) != noErr || !extFile) {
            return -1.0f;
        }
        AudioStreamBasicDescription clientFmt;
        memset(&clientFmt, 0, sizeof(clientFmt));
        clientFmt.mSampleRate = 24000.0;
        clientFmt.mFormatID = kAudioFormatLinearPCM;
        clientFmt.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
        clientFmt.mFramesPerPacket = 1;
        clientFmt.mChannelsPerFrame = 1;
        clientFmt.mBitsPerChannel = 32;
        clientFmt.mBytesPerFrame = 4;
        clientFmt.mBytesPerPacket = 4;
        if (ExtAudioFileSetProperty(extFile,
                                    kExtAudioFileProperty_ClientDataFormat,
                                    sizeof(clientFmt),
                                    &clientFmt) != noErr) {
            ExtAudioFileDispose(extFile);
            return -1.0f;
        }

        // Decode the full track (up to 6 minutes at 24 kHz mono) with a 1st-order 200 Hz high-pass
        // weighting filter (approximating ITU-R BS.1770 K-weighting so bass-heavy tracks aren't
        // over-attenuated relative to bright mid-range tracks).
        const UInt32 chunkFrames = 8192;
        float buf[8192];
        double gatedSumSq = 0.0;
        uint64_t gatedSamples = 0;
        double allSumSq = 0.0;
        uint64_t allSamples = 0;
        const uint64_t maxFrames = 24000ULL * 360ULL;
        const double rc = 1.0 / (2.0 * M_PI * 200.0);
        const double dt = 1.0 / 24000.0;
        const double hpAlpha = rc / (rc + dt);
        double hpY = 0.0;
        double hpPrevX = 0.0;

        while (allSamples < maxFrames) {
            AudioBufferList abl;
            abl.mNumberBuffers = 1;
            abl.mBuffers[0].mNumberChannels = 1;
            abl.mBuffers[0].mDataByteSize = chunkFrames * sizeof(float);
            abl.mBuffers[0].mData = buf;
            UInt32 nFrames = chunkFrames;
            OSStatus st = ExtAudioFileRead(extFile, &nFrames, &abl);
            if (st != noErr || nFrames == 0) break;

            double blockSq = 0.0;
            for (UInt32 i = 0; i < nFrames; i++) {
                double x = (double)buf[i];
                hpY = hpAlpha * (hpY + x - hpPrevX);
                hpPrevX = x;
                blockSq += hpY * hpY;
            }
            double blockRms = sqrt(blockSq / (double)nFrames);
            allSumSq += blockSq;
            allSamples += nFrames;
            // Gate only true silence (below -60 dBFS = 0.001) so ultra-quiet tracks like
            // piano_concerto (-46 dBFS) are measured accurately across the entire track.
            if (blockRms > 0.001) {
                gatedSumSq += blockSq;
                gatedSamples += nFrames;
            }
        }
        ExtAudioFileDispose(extFile);

        if (gatedSamples > 0) {
            return (float)sqrt(gatedSumSq / (double)gatedSamples);
        }
        if (allSamples > 0) {
            return (float)sqrt(allSumSq / (double)allSamples);
        }
        return -1.0f;
    }
}

void slackless_audio_file_stop_loop(SlacklessAudioHandle *handle) {
    if (!handle) return;
    void *obj = handle->holder_obj;
    handle->holder_obj = NULL;
    free(handle);
    if (!obj) return;
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            SlacklessEnginePlayer *holder = (__bridge_transfer SlacklessEnginePlayer *)obj;
            [holder stopNow];
        }
    });
}

// ---------------------------------------------------------------------------
// Click-Through Transparent Desktop Drawing Overlay (`NSWindowSharingNone`)
// ---------------------------------------------------------------------------

typedef struct SlacklessOverlaySegment {
    double ax, ay;
    double bx, by;
    double cx, cy;
    double dx, dy;
    int32_t hue;
    float alpha;
    float width;
} SlacklessOverlaySegment;

typedef struct SlacklessOverlayCursor {
    double x, y;
    int32_t hue;
    float alpha;
} SlacklessOverlayCursor;

static void hsl_hue_to_rgb(int32_t hue_deg, CGFloat *r, CGFloat *g, CGFloat *b) {
    double h = fmod((double)hue_deg, 360.0);
    if (h < 0.0) h += 360.0;
    double s = 0.85, l = 0.56;
    double c = (1.0 - fabs(2.0 * l - 1.0)) * s;
    double x = c * (1.0 - fabs(fmod(h / 60.0, 2.0) - 1.0));
    double m = l - c / 2.0;
    double rr = 0, gg = 0, bb = 0;
    if (h < 60) { rr = c; gg = x; }
    else if (h < 120) { rr = x; gg = c; }
    else if (h < 180) { gg = c; bb = x; }
    else if (h < 240) { gg = x; bb = c; }
    else if (h < 300) { rr = x; bb = c; }
    else { rr = c; bb = x; }
    *r = (CGFloat)(rr + m);
    *g = (CGFloat)(gg + m);
    *b = (CGFloat)(bb + m);
}

typedef void (*SlacklessOverlayMouseCallback)(void *ctx, uint8_t kind, double nx, double ny);

@interface SlacklessDrawOverlayView : NSView {
@public
    SlacklessOverlaySegment *segments;
    size_t segment_count;
    SlacklessOverlayCursor *cursors;
    size_t cursor_count;
    bool pen_enabled;
    int32_t pen_hue;
    SlacklessOverlayMouseCallback mouse_cb;
    void *mouse_ctx;
    NSTrackingArea *tracking_area;
}
- (void)syncTrackingArea;
@end

@implementation SlacklessDrawOverlayView

- (BOOL)isFlipped {
    return YES;
}

- (BOOL)isOpaque {
    return NO;
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return pen_enabled ? YES : NO;
}

- (void)dealloc {
    free(segments);
    free(cursors);
}

- (void)syncTrackingArea {
    if (tracking_area) {
        [self removeTrackingArea:tracking_area];
        tracking_area = nil;
    }
    if (pen_enabled) {
        NSTrackingAreaOptions opts = NSTrackingMouseMoved | NSTrackingActiveAlways |
                                     NSTrackingInVisibleRect | NSTrackingCursorUpdate;
        tracking_area = [[NSTrackingArea alloc] initWithRect:self.bounds
                                                     options:opts
                                                       owner:self
                                                    userInfo:nil];
        [self addTrackingArea:tracking_area];
    }
    if (self.window) {
        [self.window invalidateCursorRectsForView:self];
    }
}

- (void)resetCursorRects {
    [super resetCursorRects];
    if (pen_enabled) {
        [self addCursorRect:self.bounds cursor:[NSCursor crosshairCursor]];
    }
}

- (void)cursorUpdate:(NSEvent *)event {
    (void)event;
    if (pen_enabled) {
        [[NSCursor crosshairCursor] set];
    }
}

- (void)emitMouseKind:(uint8_t)kind event:(NSEvent *)event {
    if (!pen_enabled || !mouse_cb) return;
    NSPoint p = [self convertPoint:event.locationInWindow fromView:nil];
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    if (w < 2.0 || h < 2.0) return;
    double nx = (double)(p.x / w);
    double ny = (double)(p.y / h);
    if (nx < 0.0) nx = 0.0;
    if (nx > 1.0) nx = 1.0;
    if (ny < 0.0) ny = 0.0;
    if (ny > 1.0) ny = 1.0;
    mouse_cb(mouse_ctx, kind, nx, ny);
}

- (void)mouseDown:(NSEvent *)event {
    [self emitMouseKind:0 event:event];
}

- (void)mouseDragged:(NSEvent *)event {
    [self emitMouseKind:1 event:event];
}

- (void)mouseUp:(NSEvent *)event {
    [self emitMouseKind:2 event:event];
}

- (void)mouseMoved:(NSEvent *)event {
    [self emitMouseKind:3 event:event];
}

- (void)drawRect:(NSRect)dirtyRect {
    (void)dirtyRect;
    CGContextRef ctx = [[NSGraphicsContext currentContext] CGContext];
    if (!ctx) return;

    CGContextClearRect(ctx, self.bounds);
    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    if (w < 2.0 || h < 2.0) return;

    if (pen_enabled) {
        // Subtle glowing perimeter frame while desktop pen mode is active so the presenter knows
        // annotations are armed without obstructing the shared app.
        CGFloat br = 0, bg = 0, bb = 0;
        hsl_hue_to_rgb(pen_hue, &br, &bg, &bb);
        CGContextSetLineWidth(ctx, 4.0);
        CGContextSetRGBStrokeColor(ctx, br, bg, bb, 0.55);
        CGContextStrokeRect(ctx, CGRectInset(self.bounds, 2.0, 2.0));
    }

    CGContextSetLineCap(ctx, kCGLineCapRound);
    CGContextSetLineJoin(ctx, kCGLineJoinRound);

    for (size_t i = 0; i < segment_count; i++) {
        SlacklessOverlaySegment s = segments[i];
        if (s.alpha <= 0.01f) continue;
        CGFloat r = 0, g = 0, b = 0;
        hsl_hue_to_rgb(s.hue, &r, &g, &b);
        CGFloat lw = (CGFloat)(s.width > 0.5f ? s.width : 5.0f);

        CGPoint p0 = CGPointMake((CGFloat)s.ax * w, (CGFloat)s.ay * h);
        CGPoint p1 = CGPointMake((CGFloat)s.bx * w, (CGFloat)s.by * h);
        CGPoint p2 = CGPointMake((CGFloat)s.cx * w, (CGFloat)s.cy * h);
        CGPoint p3 = CGPointMake((CGFloat)s.dx * w, (CGFloat)s.dy * h);

        // Soft outer contrast halo so strokes stay legible over both dark and light apps.
        CGContextBeginPath(ctx);
        CGContextMoveToPoint(ctx, p0.x, p0.y);
        CGContextAddCurveToPoint(ctx, p1.x, p1.y, p2.x, p2.y, p3.x, p3.y);
        CGContextSetLineWidth(ctx, lw + 3.0);
        CGContextSetRGBStrokeColor(ctx, 1.0, 1.0, 1.0, (CGFloat)s.alpha * 0.45);
        CGContextStrokePath(ctx);

        // Primary colored stroke.
        CGContextBeginPath(ctx);
        CGContextMoveToPoint(ctx, p0.x, p0.y);
        CGContextAddCurveToPoint(ctx, p1.x, p1.y, p2.x, p2.y, p3.x, p3.y);
        CGContextSetLineWidth(ctx, lw);
        CGContextSetRGBStrokeColor(ctx, r, g, b, (CGFloat)s.alpha);
        CGContextStrokePath(ctx);
    }

    for (size_t i = 0; i < cursor_count; i++) {
        SlacklessOverlayCursor c = cursors[i];
        if (c.alpha <= 0.01f) continue;
        CGFloat r = 0, g = 0, b = 0;
        hsl_hue_to_rgb(c.hue, &r, &g, &b);
        CGFloat cx = (CGFloat)c.x * w;
        CGFloat cy = (CGFloat)c.y * h;
        CGContextSetRGBFillColor(ctx, r, g, b, (CGFloat)c.alpha);
        CGContextFillEllipseInRect(ctx, CGRectMake(cx - 6.0, cy - 6.0, 12.0, 12.0));
        CGContextSetLineWidth(ctx, 2.0);
        CGContextSetRGBStrokeColor(ctx, 1.0, 1.0, 1.0, (CGFloat)c.alpha * 0.9);
        CGContextStrokeEllipseInRect(ctx, CGRectMake(cx - 6.0, cy - 6.0, 12.0, 12.0));
    }
}

@end

static NSPanel *g_draw_overlay_win = nil;
static SlacklessDrawOverlayView *g_draw_overlay_view = nil;

static void ensure_draw_overlay_window(NSRect frame) {
    if (!g_draw_overlay_win) {
        // Non-activating NSPanel with `hidesOnDeactivate = NO`: stays live across Cmd+Tab so
        // desktop pen mode never silently disables when switching apps, and drawing over another
        // app never steals focus away from it.
        NSPanel *win = [[NSPanel alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        win.opaque = NO;
        win.backgroundColor = [NSColor clearColor];
        win.hasShadow = NO;
        win.hidesOnDeactivate = NO;
        win.floatingPanel = YES;
        win.becomesKeyOnlyIfNeeded = YES;
        win.ignoresMouseEvents = YES;
        win.acceptsMouseMovedEvents = YES;
        win.level = NSFloatingWindowLevel;
        // Critical: exclude the overlay from ScreenCaptureKit so teammates' strokes and our pen
        // border never feed back into the captured video stream!
        win.sharingType = NSWindowSharingNone;
        win.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                 NSWindowCollectionBehaviorFullScreenAuxiliary |
                                 NSWindowCollectionBehaviorStationary |
                                 NSWindowCollectionBehaviorIgnoresCycle;
        SlacklessDrawOverlayView *view =
            [[SlacklessDrawOverlayView alloc] initWithFrame:NSMakeRect(0, 0, frame.size.width, frame.size.height)];
        view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        win.contentView = view;
        g_draw_overlay_win = win;
        g_draw_overlay_view = view;
    } else if (!NSEqualRects(g_draw_overlay_win.frame, frame)) {
        [g_draw_overlay_win setFrame:frame display:NO];
    }
}

static bool resolve_share_target_nsrect(bool is_monitor, uint32_t target_id, NSRect *out_rect) {
    CGRect primary = CGDisplayBounds(CGMainDisplayID());
    CGFloat primary_h = primary.size.height;
    CGRect cg_rect = CGRectZero;
    bool found = false;

    if (is_monitor) {
        cg_rect = CGDisplayBounds((CGDirectDisplayID)target_id);
        found = cg_rect.size.width > 10.0 && cg_rect.size.height > 10.0;
    } else {
        CFArrayRef list = CGWindowListCopyWindowInfo(
            kCGWindowListOptionIncludingWindow, (CGWindowID)target_id);
        if (list) {
            NSArray *arr = (__bridge_transfer NSArray *)list;
            if (arr.count > 0) {
                NSDictionary *info = arr[0];
                NSDictionary *b = info[(id)kCGWindowBounds];
                if (b && CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)b, &cg_rect)) {
                    found = cg_rect.size.width > 10.0 && cg_rect.size.height > 10.0;
                }
            }
        }
    }
    if (!found) return false;
    // Convert top-left CG coordinates to AppKit bottom-left screen coordinates.
    CGFloat ns_y = primary_h - cg_rect.origin.y - cg_rect.size.height;
    *out_rect = NSMakeRect(cg_rect.origin.x, ns_y, cg_rect.size.width, cg_rect.size.height);
    return true;
}

void slackless_draw_overlay_update(
    bool is_monitor,
    uint32_t target_id,
    const SlacklessOverlaySegment *segs,
    size_t seg_count,
    const SlacklessOverlayCursor *cur,
    size_t cur_count) {
    SlacklessOverlaySegment *segs_copy = NULL;
    if (segs && seg_count > 0) {
        segs_copy = (SlacklessOverlaySegment *)malloc(seg_count * sizeof(SlacklessOverlaySegment));
        if (segs_copy) memcpy(segs_copy, segs, seg_count * sizeof(SlacklessOverlaySegment));
    }
    SlacklessOverlayCursor *cur_copy = NULL;
    if (cur && cur_count > 0) {
        cur_copy = (SlacklessOverlayCursor *)malloc(cur_count * sizeof(SlacklessOverlayCursor));
        if (cur_copy) memcpy(cur_copy, cur, cur_count * sizeof(SlacklessOverlayCursor));
    }

    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            bool pen_on = g_draw_overlay_view && g_draw_overlay_view->pen_enabled;
            if ((!segs_copy || seg_count == 0) && (!cur_copy || cur_count == 0)) {
                free(segs_copy);
                free(cur_copy);
                if (g_draw_overlay_view) {
                    free(g_draw_overlay_view->segments);
                    g_draw_overlay_view->segments = NULL;
                    g_draw_overlay_view->segment_count = 0;
                    free(g_draw_overlay_view->cursors);
                    g_draw_overlay_view->cursors = NULL;
                    g_draw_overlay_view->cursor_count = 0;
                    [g_draw_overlay_view setNeedsDisplay:YES];
                }
                if (g_draw_overlay_win && !pen_on) {
                    [g_draw_overlay_win orderOut:nil];
                }
                return;
            }
            NSRect frame = NSZeroRect;
            if (!resolve_share_target_nsrect(is_monitor, target_id, &frame)) {
                free(segs_copy);
                free(cur_copy);
                return;
            }
            ensure_draw_overlay_window(frame);
            free(g_draw_overlay_view->segments);
            g_draw_overlay_view->segments = segs_copy;
            g_draw_overlay_view->segment_count = segs_copy ? seg_count : 0;
            free(g_draw_overlay_view->cursors);
            g_draw_overlay_view->cursors = cur_copy;
            g_draw_overlay_view->cursor_count = cur_copy ? cur_count : 0;
            [g_draw_overlay_view setNeedsDisplay:YES];
            if (!g_draw_overlay_win.isVisible) {
                [g_draw_overlay_win orderFrontRegardless];
            }
        }
    });
}

void slackless_draw_overlay_set_pen_mode(
    bool enabled,
    bool is_monitor,
    uint32_t target_id,
    int32_t hue,
    void *ctx,
    SlacklessOverlayMouseCallback cb) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (!enabled) {
                if (g_draw_overlay_view) {
                    g_draw_overlay_view->pen_enabled = false;
                    g_draw_overlay_view->mouse_cb = NULL;
                    g_draw_overlay_view->mouse_ctx = NULL;
                    [g_draw_overlay_view syncTrackingArea];
                    [g_draw_overlay_view setNeedsDisplay:YES];
                }
                if (g_draw_overlay_win) {
                    g_draw_overlay_win.ignoresMouseEvents = YES;
                    if (!g_draw_overlay_view ||
                        (g_draw_overlay_view->segment_count == 0 && g_draw_overlay_view->cursor_count == 0)) {
                        [g_draw_overlay_win orderOut:nil];
                    }
                }
                return;
            }

            NSRect frame = NSZeroRect;
            if (!resolve_share_target_nsrect(is_monitor, target_id, &frame)) {
                return;
            }
            ensure_draw_overlay_window(frame);
            g_draw_overlay_view->pen_enabled = true;
            g_draw_overlay_view->pen_hue = hue;
            g_draw_overlay_view->mouse_cb = cb;
            g_draw_overlay_view->mouse_ctx = ctx;
            g_draw_overlay_win.ignoresMouseEvents = NO;
            [g_draw_overlay_view syncTrackingArea];
            [g_draw_overlay_view setNeedsDisplay:YES];
            [g_draw_overlay_win orderFrontRegardless];
        }
    });
}

void slackless_draw_overlay_hide(void) {
    dispatch_async(dispatch_get_main_queue(), ^{
        @autoreleasepool {
            if (g_draw_overlay_view) {
                g_draw_overlay_view->pen_enabled = false;
                g_draw_overlay_view->mouse_cb = NULL;
                g_draw_overlay_view->mouse_ctx = NULL;
                [g_draw_overlay_view syncTrackingArea];
                free(g_draw_overlay_view->segments);
                g_draw_overlay_view->segments = NULL;
                g_draw_overlay_view->segment_count = 0;
                free(g_draw_overlay_view->cursors);
                g_draw_overlay_view->cursors = NULL;
                g_draw_overlay_view->cursor_count = 0;
            }
            if (g_draw_overlay_win) {
                g_draw_overlay_win.ignoresMouseEvents = YES;
                [g_draw_overlay_win orderOut:nil];
            }
        }
    });
}

static void clamp_floating_panel_to_visible_frame(NSWindow *win, CGFloat bottomMargin) {
    if (!win) return;
    NSScreen *scr = win.screen ?: [NSScreen mainScreen];
    if (!scr) return;
    // `visibleFrame` excludes the macOS Dock (bottom/left/right) and the top menu bar, so
    // `vf.origin.y` is the exact top edge of a bottom-anchored Dock in AppKit coordinates.
    NSRect vf = scr.visibleFrame;
    NSRect f = win.frame;
    CGFloat minY = vf.origin.y + bottomMargin;
    CGFloat maxY = NSMaxY(vf) - f.size.height;
    CGFloat minX = vf.origin.x;
    CGFloat maxX = NSMaxX(vf) - f.size.width;
    if (maxY < minY) maxY = minY;
    if (maxX < minX) maxX = minX;

    CGFloat nx = f.origin.x;
    CGFloat ny = f.origin.y;
    if (ny < minY) ny = minY;
    if (ny > maxY) ny = maxY;
    if (nx < minX) nx = minX;
    if (nx > maxX) nx = maxX;

    if (fabs(nx - f.origin.x) > 0.5 || fabs(ny - f.origin.y) > 0.5) {
        [win setFrameOrigin:NSMakePoint(nx, ny)];
    }
}

void slackless_configure_floating_panel(void *ns_view_ptr) {
    if (!ns_view_ptr) return;
    NSView *view = (__bridge NSView *)ns_view_ptr;
    void (^apply)(void) = ^{
        @autoreleasepool {
            NSWindow *win = view.window;
            if (!win) return;
            // One level above `NSFloatingWindowLevel` so the movable control panel ALWAYS sits
            // above the transparent desktop drawing overlay even while pen mode is active!
            win.level = NSFloatingWindowLevel + 1;
            win.hidesOnDeactivate = NO;
            win.sharingType = NSWindowSharingNone;
            win.movableByWindowBackground = YES;
            win.opaque = NO;
            win.backgroundColor = [NSColor clearColor];
            win.hasShadow = YES;
            win.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                                     NSWindowCollectionBehaviorFullScreenAuxiliary;

            // Clamp initial placement comfortably above the macOS Dock (`visibleFrame.origin.y`).
            clamp_floating_panel_to_visible_frame(win, 12.0);

            // Prevent dragging or resizing the floating panel below the top edge of the Dock.
            NSNotificationCenter *nc = [NSNotificationCenter defaultCenter];
            __weak NSWindow *weakWin = win;
            id moveObs = [nc addObserverForName:NSWindowDidMoveNotification
                                         object:win
                                          queue:[NSOperationQueue mainQueue]
                                     usingBlock:^(NSNotification *note) {
                                         (void)note;
                                         clamp_floating_panel_to_visible_frame(weakWin, 0.0);
                                     }];
            id resizeObs = [nc addObserverForName:NSWindowDidResizeNotification
                                           object:win
                                            queue:[NSOperationQueue mainQueue]
                                       usingBlock:^(NSNotification *note) {
                                           (void)note;
                                           clamp_floating_panel_to_visible_frame(weakWin, 0.0);
                                       }];
            __block id closeObs = nil;
            closeObs = [nc addObserverForName:NSWindowWillCloseNotification
                                       object:win
                                        queue:[NSOperationQueue mainQueue]
                                   usingBlock:^(NSNotification *note) {
                                       (void)note;
                                       NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
                                       [center removeObserver:moveObs];
                                       [center removeObserver:resizeObs];
                                       if (closeObs) {
                                           [center removeObserver:closeObs];
                                           closeObs = nil;
                                       }
                                   }];
        }
    };
    if ([NSThread isMainThread]) {
        apply();
    } else {
        dispatch_async(dispatch_get_main_queue(), apply);
    }
}
