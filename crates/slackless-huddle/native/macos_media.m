// Native macOS hardware media bridge for Slackless Huddles:
// - ScreenCaptureKit (SCStream): GPU-accelerated, event-driven display/window capture with
//   Metal hardware downscaling directly to 1080p sRGB BGRA.
// - VideoToolbox (VTCompressionSession / VTDecompressionSession): Apple Silicon Media Engine
//   H.264 real-time encode & decode with Annex-B framing.

#import <CoreGraphics/CoreGraphics.h>
#import <CoreMedia/CoreMedia.h>
#import <CoreVideo/CoreVideo.h>
#import <Foundation/Foundation.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>
#import <VideoToolbox/VideoToolbox.h>
#import <dispatch/dispatch.h>
#import <mach/mach_time.h>
#import <stdatomic.h>
#import <stdbool.h>
#import <stdint.h>
#import <stdio.h>
#import <stdlib.h>
#import <string.h>

typedef void (*SlacklessH264Callback)(
    void *ctx,
    const uint8_t *data,
    size_t len,
    uint32_t width,
    uint32_t height,
    uint64_t encode_us,
    bool is_keyframe);

typedef void (*SlacklessPreviewCallback)(
    void *ctx,
    const uint8_t *bgra,
    size_t len,
    uint32_t width,
    uint32_t height);

typedef void (*SlacklessErrorCallback)(void *ctx, const char *message);

typedef void (*SlacklessDecodedFrameCallback)(
    void *ctx,
    const uint8_t *bgra,
    size_t len,
    uint32_t width,
    uint32_t height);

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
    double sx = (double)max_w / (double)src_w;
    double sy = (double)max_h / (double)src_h;
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
// ScreenCaptureKit + VideoToolbox Hardware H.264 Screen Capture
// ---------------------------------------------------------------------------

typedef struct SlacklessScCapture SlacklessScCapture;

@interface SlacklessStreamDelegate : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, assign) SlacklessScCapture *owner;
@end

struct SlacklessScCapture {
    void *ctx;
    SlacklessH264Callback on_h264;
    SlacklessPreviewCallback on_preview;
    SlacklessErrorCallback on_error;

    uint32_t width;
    uint32_t height;
    uint32_t fps;

    atomic_bool stopped;
    atomic_bool force_keyframe;
    uint64_t last_preview_us;

    VTCompressionSessionRef vt;
    dispatch_queue_t sample_queue;
    void *stream_obj;   // retained SCStream*
    void *delegate_obj; // retained SlacklessStreamDelegate*

    uint8_t *annex_b_buf;
    size_t annex_b_cap;
    uint8_t *preview_buf;
    size_t preview_cap;
};

static bool ensure_buf(uint8_t **buf, size_t *cap, size_t needed) {
    if (*cap >= needed) return true;
    size_t next = needed + (needed / 4) + 4096;
    uint8_t *p = (uint8_t *)realloc(*buf, next);
    if (!p) return false;
    *buf = p;
    *cap = next;
    return true;
}

static void vt_compress_output_cb(
    void *outputCallbackRefCon,
    void *sourceFrameRefCon,
    OSStatus status,
    VTEncodeInfoFlags infoFlags,
    CMSampleBufferRef sampleBuffer) {
    (void)infoFlags;
    SlacklessScCapture *cap = (SlacklessScCapture *)outputCallbackRefCon;
    if (!cap || atomic_load(&cap->stopped) || status != noErr || !sampleBuffer) {
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

    // Prepend SPS + PPS parameter sets before every IDR keyframe so SFU / receivers lock on immediately.
    if (is_keyframe) {
        for (size_t i = 0; i < ps_count; i++) {
            const uint8_t *ps_ptr = NULL;
            size_t ps_size = 0;
            if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(fmt, i, &ps_ptr, &ps_size, NULL, NULL) == noErr &&
                ps_ptr && ps_size > 0) {
                if (!ensure_buf(&cap->annex_b_buf, &cap->annex_b_cap, out_len + 4 + ps_size)) {
                    return;
                }
                memcpy(cap->annex_b_buf + out_len, start_code, 4);
                out_len += 4;
                memcpy(cap->annex_b_buf + out_len, ps_ptr, ps_size);
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
        if (!ensure_buf(&cap->annex_b_buf, &cap->annex_b_cap, out_len + 4 + nal_size)) {
            return;
        }
        memcpy(cap->annex_b_buf + out_len, start_code, 4);
        out_len += 4;
        memcpy(cap->annex_b_buf + out_len, data_ptr + offset, nal_size);
        out_len += nal_size;
        offset += nal_size;
    }

    if (out_len > 0 && cap->on_h264) {
        cap->on_h264(cap->ctx, cap->annex_b_buf, out_len, cap->width, cap->height, encode_us, is_keyframe);
    }
}

@implementation SlacklessStreamDelegate

- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
                   ofType:(SCStreamOutputType)type API_AVAILABLE(macos(12.3)) {
    (void)stream;
    SlacklessScCapture *cap = self.owner;
    if (!cap || atomic_load(&cap->stopped) || type != SCStreamOutputTypeScreen || !sampleBuffer) {
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
    if (!CMTIME_IS_VALID(pts)) {
        pts = CMTimeMake((int64_t)monotonic_us(), 1000000);
    }
    CMTime dur = CMTimeMake(1, (int32_t)(cap->fps > 0 ? cap->fps : 30));

    CFDictionaryRef frameProps = NULL;
    if (atomic_exchange(&cap->force_keyframe, false)) {
        const void *keys[] = {kVTEncodeFrameOptionKey_ForceKeyFrame};
        const void *vals[] = {kCFBooleanTrue};
        frameProps = CFDictionaryCreate(
            kCFAllocatorDefault, keys, vals, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    }

    uint64_t submit_us = monotonic_us();
    VTCompressionSessionEncodeFrame(
        cap->vt, pixelBuffer, pts, dur, frameProps, (void *)(uintptr_t)submit_us, NULL);
    if (frameProps) {
        CFRelease(frameProps);
    }

    // Throttle local UI preview copies to ~15 fps while full 30 fps H.264 streams over WebRTC.
    if (cap->on_preview && (submit_us - cap->last_preview_us) >= 66000ULL) {
        cap->last_preview_us = submit_us;
        if (CVPixelBufferLockBaseAddress(pixelBuffer, kCVPixelBufferLock_ReadOnly) == kCVReturnSuccess) {
            size_t w = CVPixelBufferGetWidth(pixelBuffer);
            size_t h = CVPixelBufferGetHeight(pixelBuffer);
            size_t bpr = CVPixelBufferGetBytesPerRow(pixelBuffer);
            const uint8_t *base = (const uint8_t *)CVPixelBufferGetBaseAddress(pixelBuffer);
            size_t row_bytes = w * 4;
            size_t tightly_packed = row_bytes * h;
            if (base && tightly_packed > 0) {
                if (bpr == row_bytes) {
                    cap->on_preview(cap->ctx, base, tightly_packed, (uint32_t)w, (uint32_t)h);
                } else if (ensure_buf(&cap->preview_buf, &cap->preview_cap, tightly_packed)) {
                    for (size_t row = 0; row < h; row++) {
                        memcpy(cap->preview_buf + row * row_bytes, base + row * bpr, row_bytes);
                    }
                    cap->on_preview(cap->ctx, cap->preview_buf, tightly_packed, (uint32_t)w, (uint32_t)h);
                }
            }
            CVPixelBufferUnlockBaseAddress(pixelBuffer, kCVPixelBufferLock_ReadOnly);
        }
    }
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error API_AVAILABLE(macos(12.3)) {
    (void)stream;
    SlacklessScCapture *cap = self.owner;
    if (!cap || atomic_load(&cap->stopped) || !error) return;
    if (cap->on_error) {
        const char *msg = error.localizedDescription.UTF8String;
        cap->on_error(cap->ctx, msg ? msg : "SCStream stopped with error");
    }
}

@end

static OSStatus configure_vt_encoder(
    uint32_t width,
    uint32_t height,
    uint32_t fps,
    uint32_t bitrate_bps,
    SlacklessScCapture *cap,
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
        vt_compress_output_cb,
        cap,
        out_vt);
    if (st != noErr || !*out_vt) {
        return st != noErr ? st : -1;
    }

    VTCompressionSessionRef vt = *out_vt;
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel);

    int32_t br = (int32_t)bitrate_bps;
    CFNumberRef brNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &br);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_AverageBitRate, brNum);
    CFRelease(brNum);

    // Smooth rate ceiling (~1.3x target over 1s window) prevents TURN packet bursts.
    int32_t bytes_per_sec = (int32_t)(((uint64_t)bitrate_bps * 13ULL) / 80ULL);
    double window_sec = 1.0;
    CFNumberRef limBytes = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_sec);
    CFNumberRef limSec = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &window_sec);
    const void *limVals[2] = {limBytes, limSec};
    CFArrayRef limits = CFArrayCreate(kCFAllocatorDefault, limVals, 2, &kCFTypeArrayCallBacks);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_DataRateLimits, limits);
    CFRelease(limits);
    CFRelease(limBytes);
    CFRelease(limSec);

    int32_t fpsVal = (int32_t)(fps > 0 ? fps : 30);
    CFNumberRef fpsNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &fpsVal);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_ExpectedFrameRate, fpsNum);
    CFRelease(fpsNum);

    int32_t gop = fpsVal * 3;
    CFNumberRef gopNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &gop);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameInterval, gopNum);
    CFRelease(gopNum);

    double gopSec = 3.0;
    CFNumberRef gopSecNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &gopSec);
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, gopSecNum);
    CFRelease(gopSecNum);

    VTCompressionSessionPrepareToEncodeFrames(vt);
    return noErr;
}

SlacklessScCapture *slackless_sc_capture_start(
    uint32_t is_monitor,
    uint32_t target_id,
    uint32_t max_w,
    uint32_t max_h,
    uint32_t max_fps,
    uint32_t bitrate_bps,
    void *ctx,
    SlacklessH264Callback on_h264,
    SlacklessPreviewCallback on_preview,
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
            if (contentErr || !content) {
                write_err(
                    err_buf,
                    err_buf_len,
                    contentErr ? contentErr.localizedDescription.UTF8String
                               : "ScreenCaptureKit unavailable (check Screen Recording permission)");
                return NULL;
            }

            SCContentFilter *filter = nil;
            uint32_t src_w = 0;
            uint32_t src_h = 0;

            if (is_monitor) {
                SCDisplay *picked = nil;
                for (SCDisplay *d in content.displays) {
                    if (target_id != 0 && d.displayID == (CGDirectDisplayID)target_id) {
                        picked = d;
                        break;
                    }
                }
                if (!picked) {
                    picked = content.displays.firstObject;
                }
                if (!picked) {
                    write_err(err_buf, err_buf_len, "no capturable display found");
                    return NULL;
                }
                filter = [[SCContentFilter alloc] initWithDisplay:picked excludingWindows:@[]];
                CGDisplayModeRef mode = CGDisplayCopyDisplayMode(picked.displayID);
                if (mode) {
                    src_w = (uint32_t)CGDisplayModeGetPixelWidth(mode);
                    src_h = (uint32_t)CGDisplayModeGetPixelHeight(mode);
                    CGDisplayModeRelease(mode);
                }
                if (src_w == 0 || src_h == 0) {
                    src_w = (uint32_t)(picked.width * 2);
                    src_h = (uint32_t)(picked.height * 2);
                }
            } else {
                SCWindow *picked = nil;
                for (SCWindow *w in content.windows) {
                    if (w.windowID == (CGWindowID)target_id) {
                        picked = w;
                        break;
                    }
                }
                if (!picked) {
                    write_err(err_buf, err_buf_len, "selected window is no longer available");
                    return NULL;
                }
                filter = [[SCContentFilter alloc] initWithDesktopIndependentWindow:picked];
                src_w = (uint32_t)(picked.frame.size.width * 2.0);
                src_h = (uint32_t)(picked.frame.size.height * 2.0);
            }

            uint32_t out_w = 0, out_h = 0;
            compute_even_fit(src_w, src_h, max_w, max_h, &out_w, &out_h);

            SlacklessScCapture *cap = (SlacklessScCapture *)calloc(1, sizeof(SlacklessScCapture));
            if (!cap) {
                write_err(err_buf, err_buf_len, "out of memory");
                return NULL;
            }
            cap->ctx = ctx;
            cap->on_h264 = on_h264;
            cap->on_preview = on_preview;
            cap->on_error = on_error;
            cap->width = out_w;
            cap->height = out_h;
            cap->fps = max_fps > 0 ? max_fps : 30;
            atomic_store(&cap->stopped, false);
            atomic_store(&cap->force_keyframe, true);

            OSStatus vt_st = configure_vt_encoder(out_w, out_h, cap->fps, bitrate_bps, cap, &cap->vt);
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
            cfg.pixelFormat = kCVPixelFormatType_32BGRA;
            cfg.minimumFrameInterval = CMTimeMake(1, (int32_t)cap->fps);
            cfg.queueDepth = 3;
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
                VTCompressionSessionInvalidate(cap->vt);
                CFRelease(cap->vt);
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
                VTCompressionSessionInvalidate(cap->vt);
                CFRelease(cap->vt);
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
    atomic_store(&cap->force_keyframe, true);
}

void slackless_sc_capture_stop(SlacklessScCapture *cap) {
    if (!cap) return;
    @autoreleasepool {
        atomic_store(&cap->stopped, true);
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
        // Drain sample queue so no callback still touches `cap` before tearing down VT + buffers.
        if (cap->sample_queue) {
            dispatch_sync(cap->sample_queue, ^{});
            cap->sample_queue = nil;
        }
        if (cap->vt) {
            VTCompressionSessionCompleteFrames(cap->vt, kCMTimeInvalid);
            VTCompressionSessionInvalidate(cap->vt);
            CFRelease(cap->vt);
            cap->vt = NULL;
        }
        free(cap->annex_b_buf);
        free(cap->preview_buf);
        free(cap);
    }
}

// ---------------------------------------------------------------------------
// VideoToolbox Hardware H.264 Decoder (Annex-B -> BGRA8888)
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
    uint8_t *out_bgra;
    size_t out_bgra_cap;
} SlacklessVtDecoder;

typedef struct VtDecodeCallCtx {
    SlacklessVtDecoder *dec;
    void *user_ctx;
    SlacklessDecodedFrameCallback on_frame;
    bool got_frame;
} VtDecodeCallCtx;

static void vt_decompress_cb(
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
    VtDecodeCallCtx *call = (VtDecodeCallCtx *)sourceFrameRefCon;
    if (!call || status != noErr || !imageBuffer) return;

    if (CVPixelBufferLockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly) != kCVReturnSuccess) {
        return;
    }
    size_t w = CVPixelBufferGetWidth(imageBuffer);
    size_t h = CVPixelBufferGetHeight(imageBuffer);
    size_t bpr = CVPixelBufferGetBytesPerRow(imageBuffer);
    const uint8_t *base = (const uint8_t *)CVPixelBufferGetBaseAddress(imageBuffer);
    size_t row_bytes = w * 4;
    size_t packed = row_bytes * h;
    if (base && packed > 0 && call->on_frame) {
        if (bpr == row_bytes) {
            call->on_frame(call->user_ctx, base, packed, (uint32_t)w, (uint32_t)h);
            call->got_frame = true;
        } else if (ensure_buf(&call->dec->out_bgra, &call->dec->out_bgra_cap, packed)) {
            for (size_t r = 0; r < h; r++) {
                memcpy(call->dec->out_bgra + r * row_bytes, base + r * bpr, row_bytes);
            }
            call->on_frame(call->user_ctx, call->dec->out_bgra, packed, (uint32_t)w, (uint32_t)h);
            call->got_frame = true;
        }
    }
    CVPixelBufferUnlockBaseAddress(imageBuffer, kCVPixelBufferLock_ReadOnly);
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

    NSDictionary *dstAttrs = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
    };
    VTDecompressionOutputCallbackRecord cb = {vt_decompress_cb, NULL};
    st = VTDecompressionSessionCreate(
        kCFAllocatorDefault,
        dec->fmt,
        NULL,
        (__bridge CFDictionaryRef)dstAttrs,
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
    free(dec->out_bgra);
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
    SlacklessDecodedFrameCallback on_frame) {
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

        VtDecodeCallCtx call = {dec, ctx, on_frame, false};
        VTDecodeInfoFlags flagsOut = 0;
        st = VTDecompressionSessionDecodeFrame(dec->session, sbuf, 0, &call, &flagsOut);
        if (st == noErr) {
            VTDecompressionSessionWaitForAsynchronousFrames(dec->session);
        }
        CFRelease(sbuf);
        return st == noErr && call.got_frame;
    }
}
