// ScreenCaptureKit (`SCStream`) + VideoToolbox H.264 screen capture and `SCScreenshotManager`
// GPU-downscaled screen/window thumbnails.

#import "common.h"

// ---------------------------------------------------------------------------
// ScreenCaptureKit + VideoToolbox Hardware H.264 Screen Capture (`420f` Zero-Copy)
// ---------------------------------------------------------------------------

typedef struct SlacklessScCapture SlacklessScCapture;

void slackless_sc_capture_stop(SlacklessScCapture *cap, SlacklessCaptureReleasedCallback on_released);

@interface SlacklessStreamDelegate : NSObject <SCStreamOutput, SCStreamDelegate>
@property(nonatomic, assign) SlacklessScCapture *owner;
@end

struct SlacklessScCapture {
    VtEncoderCommon enc;
    SlacklessSurfaceCallback on_surface_preview;
    SlacklessErrorCallback on_error;
    SlacklessCaptureReleasedCallback on_released;

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
    slackless_encode_cvpb(&cap->enc, pixelBuffer, pts);

    // Zero-copy local preview throttled to ~20 fps.
    uint64_t now_us = slackless_monotonic_us();
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

// Frees `cap` and hands `ctx` back through `on_released`. Must run on the serial sample queue
// after every already-queued sample block (so none can dereference freed `cap`), or when the
// stream never started and nothing was queued.
static void slackless_sc_capture_release(SlacklessScCapture *cap) {
    if (!cap) return;
    cap->sample_queue = nil;
    if (cap->delegate_obj) {
        ((__bridge_transfer SlacklessStreamDelegate *)cap->delegate_obj).owner = NULL;
        cap->delegate_obj = NULL;
    }
    (void)(__bridge_transfer SCStream *)cap->stream_obj;
    cap->stream_obj = NULL;
    if (cap->enc.vt) {
        VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
        VTCompressionSessionInvalidate(cap->enc.vt);
        CFRelease(cap->enc.vt);
        cap->enc.vt = NULL;
    }
    free(cap->enc.annex_b_buf);
    if (cap->on_released) {
        cap->on_released(cap->enc.ctx);
    }
    free(cap);
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
                slackless_write_err(err_buf, err_buf_len, "timed out querying ScreenCaptureKit shareable content");
                return NULL;
            }
            if (!content) {
                slackless_write_err(
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
                    slackless_write_err(err_buf, err_buf_len, "no matching ScreenCaptureKit display found");
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
                    slackless_write_err(err_buf, err_buf_len, "matching ScreenCaptureKit window not found");
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
            slackless_compute_even_fit(src_w, src_h, max_w, max_h, &out_w, &out_h);

            SlacklessScCapture *cap = (SlacklessScCapture *)calloc(1, sizeof(SlacklessScCapture));
            if (!cap) {
                slackless_write_err(err_buf, err_buf_len, "out of memory allocating capture handle");
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

            OSStatus vt_st = slackless_configure_vt_encoder(
                out_w, out_h, cap->enc.fps, bitrate_bps, 10.0, &cap->enc, &cap->enc.vt);
            if (vt_st != noErr) {
                char msg[128];
                snprintf(msg, sizeof(msg), "VTCompressionSessionCreate failed (OSStatus %d)", (int)vt_st);
                slackless_write_err(err_buf, err_buf_len, msg);
                free(cap);
                return NULL;
            }

            SCStreamConfiguration *cfg = [[SCStreamConfiguration alloc] init];
            cfg.width = out_w;
            cfg.height = out_h;
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
            cap->stream_obj = (__bridge_retained void *)stream;
            cap->delegate_obj = (__bridge_retained void *)delegate;

            NSError *addErr = nil;
            if (![stream addStreamOutput:delegate
                                    type:SCStreamOutputTypeScreen
                      sampleHandlerQueue:cap->sample_queue
                                   error:&addErr]) {
                slackless_write_err(
                    err_buf,
                    err_buf_len,
                    addErr ? addErr.localizedDescription.UTF8String : "addStreamOutput failed");
                // Never started: no callback was queued, so the handle can be freed in place.
                slackless_sc_capture_release(cap);
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
                slackless_write_err(
                    err_buf,
                    err_buf_len,
                    startErr ? startErr.localizedDescription.UTF8String : "SCStream startCapture timed out");
                // A timed-out start may still come up: go through the stop path so a late sample
                // never reaches freed memory.
                slackless_sc_capture_stop(cap, NULL);
                return NULL;
            }

            return cap;
        } else {
            slackless_write_err(err_buf, err_buf_len, "ScreenCaptureKit requires macOS 12.3+");
            return NULL;
        }
    }
}

void slackless_sc_capture_force_keyframe(SlacklessScCapture *cap) {
    if (!cap) return;
    atomic_store(&cap->enc.force_keyframe, true);
}

void slackless_sc_capture_set_bitrate(SlacklessScCapture *cap, uint32_t bitrate_bps) {
    if (!cap || atomic_load(&cap->enc.stopped) || !cap->sample_queue) return;
    dispatch_async(cap->sample_queue, ^{
        if (!atomic_load(&cap->enc.stopped) && cap->enc.vt) {
            slackless_vt_set_bitrate(cap->enc.vt, bitrate_bps);
        }
    });
}

void slackless_sc_capture_stop(SlacklessScCapture *cap, SlacklessCaptureReleasedCallback on_released) {
    if (!cap) return;
    @autoreleasepool {
        atomic_store(&cap->enc.stopped, true);
        cap->on_released = on_released;
        if (@available(macOS 12.3, *)) {
            if (cap->stream_obj) {
                SCStream *stream = (__bridge SCStream *)cap->stream_obj;
                dispatch_semaphore_t sem = dispatch_semaphore_create(0);
                [stream stopCaptureWithCompletionHandler:^(NSError *e) {
                    (void)e;
                    // No further sample buffers are delivered once the stop completes; release on
                    // the serial sample queue so blocks already queued run (and see `stopped`) first.
                    if (cap->sample_queue) {
                        dispatch_async(cap->sample_queue, ^{ slackless_sc_capture_release(cap); });
                    } else {
                        slackless_sc_capture_release(cap);
                    }
                    dispatch_semaphore_signal(sem);
                }];
                if (dispatch_semaphore_wait(sem, dispatch_time(DISPATCH_TIME_NOW, 1500LL * NSEC_PER_MSEC)) != 0) {
                    // ScreenCaptureKit has not confirmed the stop. `cap` stays alive (every callback
                    // returns early on `stopped`) and the completion handler still releases it; if
                    // it never fires, this one handle and its callback context leak instead of being
                    // freed where a late sample callback could still reach them.
                    NSLog(@"slackless: SCStream stopCapture did not complete within 1.5s; deferring capture release");
                }
                return;
            }
        }
        slackless_sc_capture_release(cap);
    }
}

// ---------------------------------------------------------------------------
// GPU-Downscaled Screen & Window Thumbnails via SCScreenshotManager (macOS 14+)
// ---------------------------------------------------------------------------

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
    SlacklessThumbCallback cb,
    SlacklessWindowSkipCallback should_skip_window) {
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

            SlacklessThumbBatch *batch = [[SlacklessThumbBatch alloc] initWithCtx:ctx callback:cb];
            dispatch_group_t grp = dispatch_group_create();

            if (monitors_only) {
                for (SCDisplay *d in content.displays) {
                    uint32_t dw = 0, dh = 0;
                    slackless_compute_even_fit((uint32_t)d.width, (uint32_t)d.height, max_w, max_h, &dw, &dh);
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
                    if (should_skip_window && should_skip_window(appName.UTF8String, title.UTF8String)) {
                        continue;
                    }

                    count++;
                    uint32_t dw = 0, dh = 0;
                    slackless_compute_even_fit((uint32_t)fr.size.width, (uint32_t)fr.size.height, max_w, max_h, &dw, &dh);
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
