// CoreAudio output-device-routed native audio player (`AVAudioEngine` + `NSSound` fallback)
// and `ExtAudioFile` perceptually weighted RMS loudness measurement.

#import "common.h"

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

// Retains fallback NSSound instances until playback finishes so ARC never releases them prematurely.
@interface SlacklessSoundDelegate : NSObject <NSSoundDelegate>
@end

static NSMutableSet<NSSound *> *g_active_oneshot_sounds = nil;
static SlacklessSoundDelegate *g_sound_delegate = nil;

@implementation SlacklessSoundDelegate

- (void)sound:(NSSound *)sound didFinishPlaying:(BOOL)flag {
    (void)flag;
    dispatch_async(dispatch_get_main_queue(), ^{
        if (sound) {
            sound.delegate = nil;
            [g_active_oneshot_sounds removeObject:sound];
        }
    });
}

@end

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
            // Fallback to NSSound if AVAudioEngine fails to start; retain until playback completes.
            NSSound *sound = [[NSSound alloc] initWithContentsOfFile:nsPath byReference:YES];
            if (!sound) return;
            sound.volume = volume > 1.0f ? 1.0f : (volume > 0.0f ? volume : 0.85f);
            sound.loops = NO;
            if (uid) sound.playbackDeviceIdentifier = uid;
            if (!g_active_oneshot_sounds) {
                g_active_oneshot_sounds = [NSMutableSet set];
            }
            if (!g_sound_delegate) {
                g_sound_delegate = [[SlacklessSoundDelegate alloc] init];
            }
            sound.delegate = g_sound_delegate;
            [g_active_oneshot_sounds addObject:sound];
            if (![sound play]) {
                sound.delegate = nil;
                [g_active_oneshot_sounds removeObject:sound];
            }
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
