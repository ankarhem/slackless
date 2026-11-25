// Shared VideoToolbox H.264 hardware encoder and H.264 Annex-B hardware decoder.

#import "common.h"

// ---------------------------------------------------------------------------
// Shared VideoToolbox H.264 Hardware Encoder Configuration & Annex-B Emitter
// ---------------------------------------------------------------------------

void slackless_vt_set_bitrate(VTCompressionSessionRef vt, uint32_t bitrate_bps) {
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
    uint64_t now = slackless_monotonic_us();
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
                if (!slackless_ensure_buf(&enc->annex_b_buf, &enc->annex_b_cap, out_len + 4 + ps_size)) {
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
        if (!slackless_ensure_buf(&enc->annex_b_buf, &enc->annex_b_cap, out_len + 4 + nal_size)) {
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

OSStatus slackless_configure_vt_encoder(
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
    if (VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel) != noErr) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Main_AutoLevel);
    }
    VTSessionSetProperty(vt, kVTCompressionPropertyKey_H264EntropyMode, kVTH264EntropyMode_CABAC);

    slackless_vt_set_bitrate(vt, bitrate_bps);

    int32_t fpsVal = (int32_t)(fps > 0 ? fps : 30);
    CFNumberRef fpsNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &fpsVal);
    if (fpsNum) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_ExpectedFrameRate, fpsNum);
        CFRelease(fpsNum);
    }

    int32_t gop = (int32_t)((double)fpsVal * gop_seconds);
    if (gop < fpsVal) gop = fpsVal;
    CFNumberRef gopNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &gop);
    if (gopNum) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameInterval, gopNum);
        CFRelease(gopNum);
    }

    CFNumberRef gopSecNum = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &gop_seconds);
    if (gopSecNum) {
        VTSessionSetProperty(vt, kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, gopSecNum);
        CFRelease(gopSecNum);
    }

    VTCompressionSessionPrepareToEncodeFrames(vt);
    return noErr;
}

void slackless_encode_cvpb(VtEncoderCommon *enc, CVImageBufferRef pixelBuffer, CMTime pts) {
    if (!enc || !enc->vt || !pixelBuffer) return;
    if (!CMTIME_IS_VALID(pts)) {
        pts = CMTimeMake((int64_t)slackless_monotonic_us(), 1000000);
    }
    CMTime dur = CMTimeMake(1, (int32_t)(enc->fps > 0 ? enc->fps : 30));

    CFDictionaryRef frameProps = NULL;
    if (atomic_exchange(&enc->force_keyframe, false)) {
        const void *keys[] = {kVTEncodeFrameOptionKey_ForceKeyFrame};
        const void *vals[] = {kCFBooleanTrue};
        frameProps = CFDictionaryCreate(
            kCFAllocatorDefault, keys, vals, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
    }

    uint64_t submit_us = slackless_monotonic_us();
    VTCompressionSessionEncodeFrame(
        enc->vt, pixelBuffer, pts, dur, frameProps, (void *)(uintptr_t)submit_us, NULL);
    if (frameProps) {
        CFRelease(frameProps);
    }
}

// ---------------------------------------------------------------------------
// Shared VideoToolbox Surface Decode Callback & Output Attributes
// ---------------------------------------------------------------------------

void slackless_vt_decompress_surface_cb(
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

NSDictionary *slackless_vt_surface_output_attrs(void) {
    return @{
        (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
        (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey : @YES,
    };
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
    VTDecompressionOutputCallbackRecord cb = {slackless_vt_decompress_surface_cb, NULL};
    st = VTDecompressionSessionCreate(
        kCFAllocatorDefault,
        dec->fmt,
        (__bridge CFDictionaryRef)decSpec,
        (__bridge CFDictionaryRef)slackless_vt_surface_output_attrs(),
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
                        if (!p) {
                            // Never decode with stale SPS after allocation failure.
                            return false;
                        }
                        dec->sps = p;
                        dec->sps_len = nal_len;
                        memcpy(dec->sps, nal, nal_len);
                        ps_changed = true;
                    }
                } else if (nal_type == 8) {
                    if (dec->pps_len != nal_len || !dec->pps || memcmp(dec->pps, nal, nal_len) != 0) {
                        uint8_t *p = (uint8_t *)realloc(dec->pps, nal_len);
                        if (!p) {
                            // Never decode with stale PPS after allocation failure.
                            return false;
                        }
                        dec->pps = p;
                        dec->pps_len = nal_len;
                        memcpy(dec->pps, nal, nal_len);
                        ps_changed = true;
                    }
                } else if (nal_type >= 1 && nal_type <= 5) {
                    if (!slackless_ensure_buf(&dec->avcc_buf, &dec->avcc_cap, avcc_len + 4 + nal_len)) {
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
        st = VTDecompressionSessionDecodeFrame(dec->session, sbuf, 0, &call, &flagsOut);
        CFRelease(sbuf);
        return st == noErr && call.got_frame;
    }
}
