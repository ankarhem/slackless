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
    if (CVPixelBufferLockBaseAddress(buf, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return false;
    }

    size_t sw = CVPixelBufferGetWidth(buf);
    size_t sh = CVPixelBufferGetHeight(buf);
    if (sw == 0 || sh == 0) {
        CVPixelBufferUnlockBaseAddress(buf, kCVPixelBufferLock_ReadOnly);
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

    uint32_t dw = (uint32_t)sw;
    uint32_t dh = (uint32_t)sh;
    if (max_w > 0 && max_h > 0 && (sw > max_w || sh > max_h)) {
        compute_even_fit((uint32_t)sw, (uint32_t)sh, max_w, max_h, &dw, &dh);
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
            cb(ctx, idx, name ? name : "Camera");
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
    void *session_obj;     // retained AVCaptureSession*
    void *delegate_obj;    // retained SlacklessCamDelegate*
    void *ci_ctx_obj;      // retained CIContext*
    void *vn_handler_obj;  // retained VNSequenceRequestHandler*
    void *vn_req_obj;      // retained VNGeneratePersonSegmentationRequest*
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

    NSError *vnErr = nil;
    if (![handler performRequests:@[req] onCVPixelBuffer:srcPB error:&vnErr]) {
        return NULL;
    }
    VNPixelBufferObservation *obs = req.results.firstObject;
    CVPixelBufferRef maskPB = obs ? obs.pixelBuffer : NULL;
    if (!maskPB) return NULL;

    if (!ensure_blur_pool(cap, w, h)) return NULL;
    CVPixelBufferRef outPB = NULL;
    if (CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, cap->blur_pool, &outPB) != kCVReturnSuccess || !outPB) {
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

    CIImage *blurred = [[[srcImg imageByClampingToExtent]
        imageByApplyingGaussianBlurWithSigma:14.0]
        imageByCroppingToRect:extent];

    CIFilter *blend = [CIFilter filterWithName:@"CIBlendWithMask"];
    [blend setValue:srcImg forKey:kCIInputImageKey];
    [blend setValue:blurred forKey:kCIInputBackgroundImageKey];
    [blend setValue:maskImg forKey:kCIInputMaskImageKey];
    CIImage *composited = blend.outputImage;
    if (!composited) {
        CFRelease(outPB);
        return NULL;
    }

    [ciCtx render:composited toCVPixelBuffer:outPB];
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

        // Lazily create/recreate VT encoder on actual camera dimensions (always even).
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

        CVPixelBufferRef blurredPB = NULL;
        CVPixelBufferRef activePB = rawPB;
        if (atomic_load(&cap->blur_enabled)) {
            blurredPB = apply_vision_metal_blur(cap, rawPB);
            if (blurredPB) {
                activePB = blurredPB;
            }
        }

        CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
        encode_cvpb(&cap->enc, activePB, pts);

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
        if ([session canSetSessionPreset:AVCaptureSessionPreset1280x720]) {
            session.sessionPreset = AVCaptureSessionPreset1280x720;
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
        cap->sample_queue = dispatch_queue_create("dev.slackless.avcam.video", DISPATCH_QUEUE_SERIAL);
        [output setSampleBufferDelegate:delegate queue:cap->sample_queue];

        if (![session canAddOutput:output]) {
            [session commitConfiguration];
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
        if (cap->enc.vt) {
            VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
            VTCompressionSessionInvalidate(cap->enc.vt);
            CFRelease(cap->enc.vt);
            cap->enc.vt = NULL;
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
