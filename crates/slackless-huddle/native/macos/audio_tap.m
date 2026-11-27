// Zero-driver macOS 14.2+ CoreAudio Process Tap (`CATapDescription` + private aggregate device)
// with ARC memory management, ASBD format verification, unique aggregate UIDs, and zero-alloc
// real-time IOProc audio conversion.

#import "common.h"
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#import <unistd.h>

#define TAP_SCRATCH_FRAMES 16384

static atomic_uint_fast64_t g_tap_uid_seq = 0;

bool slackless_audio_tap_is_supported(void) {
    if (@available(macOS 14.2, *)) {
        return NSClassFromString(@"CATapDescription") != nil;
    }
    return false;
}

typedef struct SlacklessAudioTap {
    AudioObjectID tap_id;
    AudioDeviceID agg_device_id;
    AudioDeviceIOProcID io_proc_id;
    void *desc_obj; // retained CATapDescription*
    void *ctx;
    SlacklessAudioTapCallback on_pcm;
    bool non_interleaved;
    uint32_t channels;
    int16_t *scratch_i16;
    size_t scratch_cap_samples; // total int16_t elements (stereo pairs = cap / 2)
    atomic_bool stopped;
} SlacklessAudioTap;

static inline int16_t clamp_f32_to_i16(float v) {
    if (!(v == v)) return 0; // NaN guard
    if (v > 1.0f) v = 1.0f;
    if (v < -1.0f) v = -1.0f;
    return (int16_t)(v * 32767.0f);
}

static OSStatus tap_io_proc(
    AudioDeviceID inDevice,
    const AudioTimeStamp *inNow,
    const AudioBufferList *inInputData,
    const AudioTimeStamp *inInputTime,
    AudioBufferList *outOutputData,
    const AudioTimeStamp *inOutputTime,
    void *inClientData) {
    (void)inDevice;
    (void)inNow;
    (void)inInputTime;
    (void)outOutputData;
    (void)inOutputTime;

    SlacklessAudioTap *tap = (SlacklessAudioTap *)inClientData;
    if (!tap || atomic_load(&tap->stopped) || !inInputData || !tap->on_pcm || !tap->scratch_i16) {
        return noErr;
    }
    UInt32 num_bufs = inInputData->mNumberBuffers;
    if (num_bufs == 0) {
        return noErr;
    }

    size_t max_frames = tap->scratch_cap_samples / 2;
    size_t out_samples = 0;

    if (tap->non_interleaved && num_bufs >= 2) {
        const AudioBuffer *b0 = &inInputData->mBuffers[0];
        const AudioBuffer *b1 = &inInputData->mBuffers[1];
        if (b0->mData && b1->mData &&
            (((uintptr_t)b0->mData % _Alignof(float)) == 0) &&
            (((uintptr_t)b1->mData % _Alignof(float)) == 0)) {
            size_t n0 = (size_t)b0->mDataByteSize / sizeof(float);
            size_t n1 = (size_t)b1->mDataByteSize / sizeof(float);
            size_t frames = n0 < n1 ? n0 : n1;
            if (frames > max_frames) frames = max_frames;
            const float *left = (const float *)b0->mData;
            const float *right = (const float *)b1->mData;
            for (size_t i = 0; i < frames; i++) {
                tap->scratch_i16[i * 2] = clamp_f32_to_i16(left[i]);
                tap->scratch_i16[i * 2 + 1] = clamp_f32_to_i16(right[i]);
            }
            out_samples = frames * 2;
        }
    } else {
        const AudioBuffer *b0 = &inInputData->mBuffers[0];
        if (b0->mData && (((uintptr_t)b0->mData % _Alignof(float)) == 0)) {
            size_t total_f32 = (size_t)b0->mDataByteSize / sizeof(float);
            uint32_t ch = b0->mNumberChannels > 0 ? b0->mNumberChannels : tap->channels;
            if (ch == 0) ch = 1;
            size_t frames = total_f32 / (size_t)ch;
            if (frames > max_frames) frames = max_frames;
            const float *samples = (const float *)b0->mData;
            for (size_t i = 0; i < frames; i++) {
                float l = samples[i * ch];
                float r = ch >= 2 ? samples[i * ch + 1] : l;
                tap->scratch_i16[i * 2] = clamp_f32_to_i16(l);
                tap->scratch_i16[i * 2 + 1] = clamp_f32_to_i16(r);
            }
            out_samples = frames * 2;
        }
    }

    if (out_samples > 0) {
        tap->on_pcm(tap->ctx, tap->scratch_i16, out_samples);
    }
    return noErr;
}

SlacklessAudioTap *slackless_audio_tap_start(
    void *ctx,
    SlacklessAudioTapCallback on_pcm,
    uint32_t *out_sample_rate,
    char *err_buf,
    size_t err_buf_len) {
    if (out_sample_rate) *out_sample_rate = 48000;
    if (!on_pcm) {
        slackless_write_err(err_buf, err_buf_len, "missing audio tap callback");
        return NULL;
    }
    @autoreleasepool {
        if (@available(macOS 14.2, *)) {
            Class tapCls = NSClassFromString(@"CATapDescription");
            if (!tapCls) {
                slackless_write_err(err_buf, err_buf_len, "CATapDescription unavailable");
                return NULL;
            }
            // Exclude our own process so the tap never captures our playback of remote voices,
            // chimes and jukebox (otherwise every remote hears the room echoed back after the SFU
            // round trip). The exclusion list holds audio process *object* IDs, so translate our PID
            // through the system object first; the header notes an unknown PID yields
            // `kAudioObjectUnknown` rather than an error.
            AudioObjectID self_process = kAudioObjectUnknown;
            pid_t pid = getpid();
            UInt32 self_size = (UInt32)sizeof(self_process);
            AudioObjectPropertyAddress translate_pid = {
                kAudioHardwarePropertyTranslatePIDToProcessObject,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            OSStatus tr = AudioObjectGetPropertyData(
                kAudioObjectSystemObject, &translate_pid, (UInt32)sizeof(pid), &pid, &self_size, &self_process);
            if (tr != noErr || self_process == kAudioObjectUnknown) {
                NSLog(@"slackless: could not resolve own audio process object (OSStatus %d, id %u); "
                      @"the system audio tap will include our own playback",
                      (int)tr,
                      (unsigned)self_process);
            }
            NSArray *excluded = self_process != kAudioObjectUnknown ? @[ @(self_process) ] : @[];
            CATapDescription *desc = [[tapCls alloc] initStereoGlobalTapButExcludeProcesses:excluded];
            if (!desc) {
                slackless_write_err(err_buf, err_buf_len, "failed to initialize CATapDescription");
                return NULL;
            }
            desc.muteBehavior = CATapUnmuted;
            desc.privateTap = YES;

            AudioObjectID tap_id = kAudioObjectUnknown;
            OSStatus st = AudioHardwareCreateProcessTap(desc, &tap_id);
            if (st != noErr || tap_id == kAudioObjectUnknown) {
                char msg[128];
                snprintf(msg, sizeof(msg), "AudioHardwareCreateProcessTap failed (OSStatus %d)", (int)st);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }

            // Query tap UID.
            CFStringRef tapUidRef = NULL;
            UInt32 uidSize = (UInt32)sizeof(tapUidRef);
            AudioObjectPropertyAddress uidAddr = {
                kAudioTapPropertyUID,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            st = AudioObjectGetPropertyData(tap_id, &uidAddr, 0, NULL, &uidSize, &tapUidRef);
            if (st != noErr || !tapUidRef) {
                AudioHardwareDestroyProcessTap(tap_id);
                char msg[128];
                snprintf(msg, sizeof(msg), "failed to query tap UID (OSStatus %d)", (int)st);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }
            NSString *tapUid = (__bridge_transfer NSString *)tapUidRef;

            // Query and verify tap PCM format (must be 32-bit float LinearPCM).
            AudioStreamBasicDescription asbd;
            memset(&asbd, 0, sizeof(asbd));
            UInt32 asbdSize = (UInt32)sizeof(asbd);
            AudioObjectPropertyAddress fmtAddr = {
                kAudioTapPropertyFormat,
                kAudioObjectPropertyScopeGlobal,
                kAudioObjectPropertyElementMain,
            };
            st = AudioObjectGetPropertyData(tap_id, &fmtAddr, 0, NULL, &asbdSize, &asbd);
            if (st != noErr ||
                asbd.mFormatID != kAudioFormatLinearPCM ||
                (asbd.mFormatFlags & kAudioFormatFlagIsFloat) == 0 ||
                asbd.mBitsPerChannel != 32) {
                AudioHardwareDestroyProcessTap(tap_id);
                char msg[160];
                snprintf(
                    msg,
                    sizeof(msg),
                    "unsupported tap audio format (status=%d fmt=0x%x flags=0x%x bits=%u)",
                    (int)st,
                    (unsigned)asbd.mFormatID,
                    (unsigned)asbd.mFormatFlags,
                    (unsigned)asbd.mBitsPerChannel);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }

            bool non_interleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) != 0;
            uint32_t channels = asbd.mChannelsPerFrame > 0 ? asbd.mChannelsPerFrame : 2;
            uint32_t sample_rate =
                (asbd.mSampleRate >= 8000.0 && asbd.mSampleRate <= 384000.0)
                    ? (uint32_t)llround(asbd.mSampleRate)
                    : 48000;
            if (out_sample_rate) {
                *out_sample_rate = sample_rate;
            }

            // Unique aggregate device UID per start (`pid` + atomic counter) so rapid stop/restart
            // never collides in CoreAudio's device registry.
            uint64_t seq = atomic_fetch_add(&g_tap_uid_seq, 1) + 1;
            NSString *aggUid = [NSString stringWithFormat:@"dev.slackless.tap.%d.%llu",
                                                          (int)getpid(),
                                                          (unsigned long long)seq];
            NSDictionary *subTapDict = @{
                @(kAudioSubTapUIDKey) : tapUid,
                @(kAudioSubTapDriftCompensationKey) : @YES,
            };
            NSDictionary *aggDict = @{
                @(kAudioAggregateDeviceUIDKey) : aggUid,
                @(kAudioAggregateDeviceNameKey) : @"Slackless System Audio Tap",
                @(kAudioAggregateDeviceIsPrivateKey) : @YES,
                @(kAudioAggregateDeviceIsStackedKey) : @NO,
                @(kAudioAggregateDeviceTapListKey) : @[subTapDict],
            };

            AudioDeviceID agg_device_id = kAudioObjectUnknown;
            st = AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggDict, &agg_device_id);
            if (st != noErr || agg_device_id == kAudioObjectUnknown) {
                AudioHardwareDestroyProcessTap(tap_id);
                char msg[128];
                snprintf(msg, sizeof(msg), "AudioHardwareCreateAggregateDevice failed (OSStatus %d)", (int)st);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }

            SlacklessAudioTap *tap = (SlacklessAudioTap *)calloc(1, sizeof(SlacklessAudioTap));
            if (!tap) {
                AudioHardwareDestroyAggregateDevice(agg_device_id);
                AudioHardwareDestroyProcessTap(tap_id);
                slackless_write_err(err_buf, err_buf_len, "out of memory allocating audio tap");
                return NULL;
            }
            tap->scratch_cap_samples = TAP_SCRATCH_FRAMES * 2;
            tap->scratch_i16 = (int16_t *)calloc(tap->scratch_cap_samples, sizeof(int16_t));
            if (!tap->scratch_i16) {
                free(tap);
                AudioHardwareDestroyAggregateDevice(agg_device_id);
                AudioHardwareDestroyProcessTap(tap_id);
                slackless_write_err(err_buf, err_buf_len, "out of memory allocating audio tap scratch buffer");
                return NULL;
            }

            tap->tap_id = tap_id;
            tap->agg_device_id = agg_device_id;
            tap->ctx = ctx;
            tap->on_pcm = on_pcm;
            tap->non_interleaved = non_interleaved;
            tap->channels = channels;
            atomic_store(&tap->stopped, false);

            st = AudioDeviceCreateIOProcID(agg_device_id, tap_io_proc, tap, &tap->io_proc_id);
            if (st != noErr || !tap->io_proc_id) {
                free(tap->scratch_i16);
                free(tap);
                AudioHardwareDestroyAggregateDevice(agg_device_id);
                AudioHardwareDestroyProcessTap(tap_id);
                char msg[128];
                snprintf(msg, sizeof(msg), "AudioDeviceCreateIOProcID failed (OSStatus %d)", (int)st);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }

            st = AudioDeviceStart(agg_device_id, tap->io_proc_id);
            if (st != noErr) {
                AudioDeviceDestroyIOProcID(agg_device_id, tap->io_proc_id);
                free(tap->scratch_i16);
                free(tap);
                AudioHardwareDestroyAggregateDevice(agg_device_id);
                AudioHardwareDestroyProcessTap(tap_id);
                char msg[128];
                snprintf(msg, sizeof(msg), "AudioDeviceStart failed (OSStatus %d)", (int)st);
                slackless_write_err(err_buf, err_buf_len, msg);
                return NULL;
            }

            tap->desc_obj = (__bridge_retained void *)desc;
            return tap;
        } else {
            slackless_write_err(err_buf, err_buf_len, "CoreAudio Process Tap requires macOS 14.2+");
            return NULL;
        }
    }
}

void slackless_audio_tap_stop(SlacklessAudioTap *tap) {
    if (!tap) return;
    @autoreleasepool {
        atomic_store(&tap->stopped, true);
        if (tap->agg_device_id != kAudioObjectUnknown && tap->io_proc_id) {
            AudioDeviceStop(tap->agg_device_id, tap->io_proc_id);
            AudioDeviceDestroyIOProcID(tap->agg_device_id, tap->io_proc_id);
            tap->io_proc_id = NULL;
        }
        if (tap->agg_device_id != kAudioObjectUnknown) {
            AudioHardwareDestroyAggregateDevice(tap->agg_device_id);
            tap->agg_device_id = kAudioObjectUnknown;
        }
        if (@available(macOS 14.2, *)) {
            if (tap->tap_id != kAudioObjectUnknown) {
                AudioHardwareDestroyProcessTap(tap->tap_id);
                tap->tap_id = kAudioObjectUnknown;
            }
        }
        if (tap->desc_obj) {
            (void)(__bridge_transfer id)tap->desc_obj;
            tap->desc_obj = NULL;
        }
        free(tap->scratch_i16);
        free(tap);
    }
}
