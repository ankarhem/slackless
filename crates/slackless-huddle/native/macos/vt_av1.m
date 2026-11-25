// VideoToolbox Hardware AV1 Decoder (Runtime-Gated M3+ Only -> Zero-Copy `420f`).

#import "common.h"

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
                uint8_t *copy = (uint8_t *)realloc(dec->seq_obu, seq_obu_len);
                if (!copy) {
                    return false;
                }
                dec->seq_obu = copy;
                dec->seq_obu_len = seq_obu_len;
                memcpy(dec->seq_obu, seq_obu, seq_obu_len);
                if (!vt_av1_rebuild(dec, seq_obu, seq_obu_len, &info)) {
                    return false;
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
