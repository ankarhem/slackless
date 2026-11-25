// AVFoundation webcam capture (`420f`), VideoToolbox H.264 encode, and Vision/CoreImage/Metal
// person-segmented background blur.

#import "common.h"

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

    pthread_mutex_t enc_lock;
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
        req.qualityLevel = VNGeneratePersonSegmentationRequestQualityLevelFast;
        req.outputPixelFormat = kCVPixelFormatType_OneComponent8;
        cap->vn_req_obj = (__bridge_retained void *)req;
    }

    VNSequenceRequestHandler *handler = (__bridge VNSequenceRequestHandler *)cap->vn_handler_obj;
    VNGeneratePersonSegmentationRequest *req = (__bridge VNGeneratePersonSegmentationRequest *)cap->vn_req_obj;
    CIContext *ciCtx = (__bridge CIContext *)cap->ci_ctx_obj;

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

        CVPixelBufferRef blurredPB = NULL;
        CVPixelBufferRef activePB = rawPB;
        if (atomic_load(&cap->blur_enabled)) {
            blurredPB = apply_vision_metal_blur(cap, rawPB);
            if (blurredPB) {
                activePB = blurredPB;
            }
        }

        // Synchronize `cap->enc.vt` and `cap->bitrate_bps` across bitrate/compact updates
        // and resolution changes; complete pending frames before invalidating on resize.
        if (cap->enc.on_h264) {
            uint32_t enc_w = w & ~1U;
            uint32_t enc_h = h & ~1U;
            CMTime pts = CMSampleBufferGetPresentationTimeStamp(sampleBuffer);
            pthread_mutex_lock(&cap->enc_lock);
            if (!atomic_load(&cap->enc.stopped)) {
                bool ready = true;
                if (!cap->enc.vt || cap->enc.width != enc_w || cap->enc.height != enc_h) {
                    if (cap->enc.vt) {
                        VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
                        VTCompressionSessionInvalidate(cap->enc.vt);
                        CFRelease(cap->enc.vt);
                        cap->enc.vt = NULL;
                    }
                    cap->enc.width = enc_w;
                    cap->enc.height = enc_h;
                    atomic_store(&cap->enc.force_keyframe, true);
                    if (slackless_configure_vt_encoder(
                            enc_w, enc_h, cap->enc.fps, cap->bitrate_bps, 8.0, &cap->enc, &cap->enc.vt) != noErr) {
                        ready = false;
                    }
                }
                if (ready && cap->enc.vt) {
                    slackless_encode_cvpb(&cap->enc, activePB, pts);
                }
            }
            pthread_mutex_unlock(&cap->enc_lock);
        }

        uint64_t now_us = slackless_monotonic_us();
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
            slackless_write_err(err_buf, err_buf_len, "no camera device found");
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
            slackless_write_err(
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
            slackless_write_err(err_buf, err_buf_len, "camera input could not be added to AVCaptureSession");
            return NULL;
        }
        [session addInput:input];

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
            slackless_write_err(err_buf, err_buf_len, "out of memory allocating camera handle");
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
        pthread_mutex_init(&cap->enc_lock, NULL);
        pthread_mutex_init(&cap->mask_lock, NULL);
        atomic_store(&cap->vision_in_flight, false);
        cap->sample_queue = dispatch_queue_create("dev.slackless.avcam.video", DISPATCH_QUEUE_SERIAL);
        cap->vision_queue = dispatch_queue_create("dev.slackless.avcam.vision", DISPATCH_QUEUE_SERIAL);
        [output setSampleBufferDelegate:delegate queue:cap->sample_queue];

        if (![session canAddOutput:output]) {
            [session commitConfiguration];
            delegate.owner = NULL;
            pthread_mutex_destroy(&cap->enc_lock);
            pthread_mutex_destroy(&cap->mask_lock);
            cap->sample_queue = nil;
            cap->vision_queue = nil;
            free(cap);
            slackless_write_err(err_buf, err_buf_len, "camera video output could not be added");
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
    pthread_mutex_lock(&cap->enc_lock);
    cap->bitrate_bps = bitrate_bps;
    if (!atomic_load(&cap->enc.stopped) && cap->enc.vt) {
        slackless_vt_set_bitrate(cap->enc.vt, bitrate_bps);
    }
    pthread_mutex_unlock(&cap->enc_lock);
}

void slackless_cam_capture_set_compact(SlacklessCamCapture *cap, bool compact) {
    if (!cap || atomic_load(&cap->enc.stopped) || !cap->session_obj || !cap->sample_queue) return;
    uint32_t target_bps = compact ? 500000U : 1200000U;
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
            pthread_mutex_lock(&cap->enc_lock);
            cap->bitrate_bps = target_bps;
            if (!atomic_load(&cap->enc.stopped) && cap->enc.vt) {
                slackless_vt_set_bitrate(cap->enc.vt, target_bps);
            }
            pthread_mutex_unlock(&cap->enc_lock);
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
        pthread_mutex_lock(&cap->enc_lock);
        if (cap->enc.vt) {
            VTCompressionSessionCompleteFrames(cap->enc.vt, kCMTimeInvalid);
            VTCompressionSessionInvalidate(cap->enc.vt);
            CFRelease(cap->enc.vt);
            cap->enc.vt = NULL;
        }
        pthread_mutex_unlock(&cap->enc_lock);
        pthread_mutex_destroy(&cap->enc_lock);

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
