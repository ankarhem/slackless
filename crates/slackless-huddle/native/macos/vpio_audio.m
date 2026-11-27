// CoreAudio event-driven default device hot-plug monitor and native VoiceProcessingIO
// full-duplex hardware AEC/NS/AGC audio engine.

#import "common.h"

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
// CoreAudio Native VoiceProcessingIO (Hardware-Tuned AEC / NS / AGC AudioUnit)
// ---------------------------------------------------------------------------

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
    if (!vpio || atomic_load(&vpio->stopped) || !vpio->au || !vpio->capture_buf || inNumberFrames == 0) {
        return noErr;
    }
    // Real-time audio callback: NEVER call realloc()/malloc() here. Preallocated capacity
    // exceeds kAudioUnitProperty_MaximumFramesPerSlice; skip any out-of-range slice safely.
    if ((size_t)inNumberFrames > vpio->capture_cap) {
        return noErr;
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
    size_t max_frames = (size_t)ioData->mBuffers[0].mDataByteSize / sizeof(float);
    size_t count = (size_t)inNumberFrames < max_frames ? (size_t)inNumberFrames : max_frames;
    if (count == 0) return noErr;
    if (!vpio || atomic_load(&vpio->stopped) || !vpio->on_render) {
        memset(dst, 0, count * sizeof(float));
        return noErr;
    }
    vpio->on_render(vpio->ctx, dst, count);
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
    atomic_store(&vpio->stopped, false);

    if (AudioComponentInstanceNew(comp, &vpio->au) != noErr || !vpio->au) {
        free(vpio);
        return NULL;
    }

    UInt32 one = 1;
    OSStatus en_in = AudioUnitSetProperty(
        vpio->au, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Input, 1, &one, sizeof(one));
    OSStatus en_out = AudioUnitSetProperty(
        vpio->au, kAudioOutputUnitProperty_EnableIO, kAudioUnitScope_Output, 0, &one, sizeof(one));
    if (en_in != noErr || en_out != noErr) {
        AudioComponentInstanceDispose(vpio->au);
        free(vpio);
        return NULL;
    }

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
        free(vpio);
        return NULL;
    }

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
    OSStatus cb_in = AudioUnitSetProperty(
        vpio->au, kAudioOutputUnitProperty_SetInputCallback, kAudioUnitScope_Global, 1, &inCb, sizeof(inCb));

    AURenderCallbackStruct outCb = {vpio_render_cb, vpio};
    OSStatus cb_out = AudioUnitSetProperty(
        vpio->au, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &outCb, sizeof(outCb));
    if (cb_in != noErr || cb_out != noErr) {
        AudioComponentInstanceDispose(vpio->au);
        free(vpio);
        return NULL;
    }

    slackless_vpio_set_processing(vpio, enable_vp, enable_agc);

    if (AudioUnitInitialize(vpio->au) != noErr) {
        AudioComponentInstanceDispose(vpio->au);
        free(vpio);
        return NULL;
    }

    // Query max frames per slice after initialization and pre-allocate capture buffer generously
    // so `vpio_input_cb` never allocates or reallocates on the real-time audio thread.
    UInt32 max_frames = 0;
    UInt32 prop_size = sizeof(max_frames);
    if (AudioUnitGetProperty(
            vpio->au,
            kAudioUnitProperty_MaximumFramesPerSlice,
            kAudioUnitScope_Global,
            0,
            &max_frames,
            &prop_size) != noErr) {
        max_frames = 4096;
    }
    size_t cap = (size_t)max_frames;
    if (cap < 16384) cap = 16384;
    vpio->capture_cap = cap;
    vpio->capture_buf = (float *)calloc(vpio->capture_cap, sizeof(float));
    if (!vpio->capture_buf) {
        AudioUnitUninitialize(vpio->au);
        AudioComponentInstanceDispose(vpio->au);
        free(vpio);
        return NULL;
    }

    if (AudioOutputUnitStart(vpio->au) != noErr) {
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
