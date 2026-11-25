// Runtime hardware capability probes and zero-copy CVPixelBuffer / BGRA conversion helpers.

#import "common.h"

// ---------------------------------------------------------------------------
// Runtime Capabilities & Permissions
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Zero-Copy CVPixelBuffer Helpers & Pooled VTPixelTransfer / vImage BGRA Readback
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

#define XFER_POOL_CACHE_CAP 8

typedef struct XferPoolSlot {
    uint32_t w;
    uint32_t h;
    uint64_t last_used;
    CVPixelBufferPoolRef pool;
} XferPoolSlot;

static CVPixelBufferPoolRef get_or_create_xfer_pool_locked(uint32_t dw, uint32_t dh) {
    static XferPoolSlot slots[XFER_POOL_CACHE_CAP];
    static uint64_t use_clock = 0;
    use_clock++;

    int empty_idx = -1;
    int lru_idx = 0;
    uint64_t oldest = UINT64_MAX;

    for (int i = 0; i < XFER_POOL_CACHE_CAP; i++) {
        if (slots[i].pool && slots[i].w == dw && slots[i].h == dh) {
            slots[i].last_used = use_clock;
            return slots[i].pool;
        }
        if (!slots[i].pool && empty_idx < 0) {
            empty_idx = i;
        }
        if (slots[i].last_used < oldest) {
            oldest = slots[i].last_used;
            lru_idx = i;
        }
    }

    int target = empty_idx >= 0 ? empty_idx : lru_idx;
    if (slots[target].pool) {
        CVPixelBufferPoolRelease(slots[target].pool);
        slots[target].pool = NULL;
    }

    NSDictionary *attrs = @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA),
        (__bridge NSString *)kCVPixelBufferWidthKey : @(dw),
        (__bridge NSString *)kCVPixelBufferHeightKey : @(dh),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
    };
    CVPixelBufferPoolRef created = NULL;
    if (CVPixelBufferPoolCreate(
            kCFAllocatorDefault, NULL, (__bridge CFDictionaryRef)attrs, &created) == kCVReturnSuccess &&
        created) {
        slots[target].w = dw;
        slots[target].h = dh;
        slots[target].last_used = use_clock;
        slots[target].pool = created;
        return created;
    }
    return NULL;
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
    @autoreleasepool {
        CVPixelBufferRef buf = (CVPixelBufferRef)pb;
        size_t sw = CVPixelBufferGetWidth(buf);
        size_t sh = CVPixelBufferGetHeight(buf);
        if (sw == 0 || sh == 0) return false;

        uint32_t dw = (uint32_t)sw;
        uint32_t dh = (uint32_t)sh;
        if (max_w > 0 && max_h > 0 && (sw > max_w || sh > max_h)) {
            slackless_compute_even_fit((uint32_t)sw, (uint32_t)sh, max_w, max_h, &dw, &dh);
        }

        // Fast path: Apple Silicon hardware VTPixelTransferSession scales AND converts
        // 420f -> 32BGRA into an LRU-cached `(dw, dh)` pool (avoiding pool churn across
        // concurrent camera/screen/preview streams).
        {
            static pthread_mutex_t xfer_lock = PTHREAD_MUTEX_INITIALIZER;
            static VTPixelTransferSessionRef xfer_session = NULL;

            pthread_mutex_lock(&xfer_lock);
            if (!xfer_session) {
                if (VTPixelTransferSessionCreate(kCFAllocatorDefault, &xfer_session) == noErr && xfer_session) {
                    VTSessionSetProperty(
                        xfer_session, kVTPixelTransferPropertyKey_ScalingMode, kVTScalingMode_Normal);
                }
            }
            CVPixelBufferPoolRef pool = xfer_session ? get_or_create_xfer_pool_locked(dw, dh) : NULL;
            CVPixelBufferRef dstPB = NULL;
            bool xfer_ok = false;
            if (xfer_session && pool &&
                CVPixelBufferPoolCreatePixelBuffer(kCFAllocatorDefault, pool, &dstPB) == kCVReturnSuccess &&
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
}
