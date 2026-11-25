// Native macOS in-app video player (`AVPlayer` + `AVPlayerItemVideoOutput`) producing zero-copy
// `420f` IOSurface-backed `CVPixelBufferRef` frames for GPUI's Metal surface renderer.

#import "common.h"
#import <math.h>

// ARC-managed atomic flag box shared with the `NSNotificationCenter` end-of-item observer so
// teardown can never UAF an in-flight notification block.
@interface SlacklessVideoEndFlag : NSObject {
@public
    atomic_bool ended;
}
@end

@implementation SlacklessVideoEndFlag
@end

typedef struct SlacklessVideoPlayer {
    pthread_mutex_t lock;
    void *asset_ref;    // __bridge_retained AVURLAsset *
    void *item_ref;     // __bridge_retained AVPlayerItem *
    void *player_ref;   // __bridge_retained AVPlayer *
    void *output_ref;   // __bridge_retained AVPlayerItemVideoOutput *
    void *observer_ref; // __bridge_retained id
    void *end_flag_ref; // __bridge_retained SlacklessVideoEndFlag *
    bool force_copy;
    CVPixelBufferRef last_pb;
    uint32_t last_w;
    uint32_t last_h;
    uint32_t track_w;
    uint32_t track_h;
} SlacklessVideoPlayer;

static void resolve_video_dimensions(
    AVAsset *asset,
    AVPlayerItem *item,
    uint32_t *out_w,
    uint32_t *out_h) {
    if (item) {
        CGSize ps = item.presentationSize;
        if (isfinite(ps.width) && isfinite(ps.height) && ps.width >= 1.0 && ps.height >= 1.0) {
            *out_w = (uint32_t)llround(ps.width);
            *out_h = (uint32_t)llround(ps.height);
            return;
        }
    }
    if (asset) {
        NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
        if (tracks.count > 0) {
            AVAssetTrack *track = tracks[0];
            CGSize natural = track.naturalSize;
            if (isfinite(natural.width) && isfinite(natural.height) &&
                natural.width >= 1.0 && natural.height >= 1.0) {
                CGSize rotated = CGSizeApplyAffineTransform(natural, track.preferredTransform);
                double w = fabs(rotated.width);
                double h = fabs(rotated.height);
                if (isfinite(w) && isfinite(h) && w >= 1.0 && h >= 1.0) {
                    *out_w = (uint32_t)llround(w);
                    *out_h = (uint32_t)llround(h);
                }
            }
        }
    }
}

SlacklessVideoPlayer *slackless_video_player_open(const char *path) {
    if (!path || path[0] == '\0') return NULL;
    @autoreleasepool {
        NSString *nsPath = [NSString stringWithUTF8String:path];
        if (!nsPath || nsPath.length == 0) return NULL;
        BOOL isDir = NO;
        if (![[NSFileManager defaultManager] fileExistsAtPath:nsPath isDirectory:&isDir] || isDir) {
            return NULL;
        }

        NSURL *url = [NSURL fileURLWithPath:nsPath];
        if (!url) return NULL;

        AVURLAsset *asset = [AVURLAsset
            URLAssetWithURL:url
                    options:@{AVURLAssetPreferPreciseDurationAndTimingKey : @YES}];
        if (!asset) return NULL;

        AVPlayerItem *item = [AVPlayerItem playerItemWithAsset:asset];
        if (!item) return NULL;

        NSDictionary *pbAttrs = @{
            (__bridge NSString *)kCVPixelBufferPixelFormatTypeKey :
                @(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange),
            (__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey : @{},
        };
        AVPlayerItemVideoOutput *output =
            [[AVPlayerItemVideoOutput alloc] initWithPixelBufferAttributes:pbAttrs];
        if (!output) return NULL;
        [item addOutput:output];

        AVPlayer *player = [AVPlayer playerWithPlayerItem:item];
        if (!player) {
            [item removeOutput:output];
            return NULL;
        }
        player.actionAtItemEnd = AVPlayerActionAtItemEndPause;

        SlacklessVideoEndFlag *endFlag = [[SlacklessVideoEndFlag alloc] init];
        atomic_init(&endFlag->ended, false);

        id observer = [[NSNotificationCenter defaultCenter]
            addObserverForName:AVPlayerItemDidPlayToEndTimeNotification
                        object:item
                         queue:nil
                    usingBlock:^(NSNotification *note) {
                        (void)note;
                        atomic_store_explicit(&endFlag->ended, true, memory_order_release);
                    }];

        SlacklessVideoPlayer *p = (SlacklessVideoPlayer *)calloc(1, sizeof(SlacklessVideoPlayer));
        if (!p) {
            [[NSNotificationCenter defaultCenter] removeObserver:observer];
            [item removeOutput:output];
            return NULL;
        }
        if (pthread_mutex_init(&p->lock, NULL) != 0) {
            [[NSNotificationCenter defaultCenter] removeObserver:observer];
            [item removeOutput:output];
            free(p);
            return NULL;
        }

        p->asset_ref = (__bridge_retained void *)asset;
        p->item_ref = (__bridge_retained void *)item;
        p->player_ref = (__bridge_retained void *)player;
        p->output_ref = (__bridge_retained void *)output;
        p->observer_ref = (__bridge_retained void *)observer;
        p->end_flag_ref = (__bridge_retained void *)endFlag;
        p->force_copy = true;
        resolve_video_dimensions(asset, item, &p->track_w, &p->track_h);
        return p;
    }
}

void slackless_video_player_play(SlacklessVideoPlayer *p) {
    if (!p) return;
    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        SlacklessVideoEndFlag *endFlag = (__bridge SlacklessVideoEndFlag *)p->end_flag_ref;
        if (player) {
            bool ended =
                endFlag ? atomic_load_explicit(&endFlag->ended, memory_order_acquire) : false;
            if (ended) {
                atomic_store_explicit(&endFlag->ended, false, memory_order_release);
                p->force_copy = true;
                [player seekToTime:kCMTimeZero
                    toleranceBefore:kCMTimeZero
                     toleranceAfter:kCMTimeZero];
            }
            [player play];
            if (player.rate == 0.0f) {
                player.rate = 1.0f;
            }
        }
    }
    pthread_mutex_unlock(&p->lock);
}

void slackless_video_player_pause(SlacklessVideoPlayer *p) {
    if (!p) return;
    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        if (player) {
            [player pause];
        }
    }
    pthread_mutex_unlock(&p->lock);
}

void slackless_video_player_set_muted(SlacklessVideoPlayer *p, bool muted) {
    if (!p) return;
    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        if (player) {
            player.muted = muted ? YES : NO;
        }
    }
    pthread_mutex_unlock(&p->lock);
}

void slackless_video_player_seek(SlacklessVideoPlayer *p, double secs) {
    if (!p) return;
    double clamped = (isfinite(secs) && secs > 0.0) ? secs : 0.0;
    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        SlacklessVideoEndFlag *endFlag = (__bridge SlacklessVideoEndFlag *)p->end_flag_ref;
        if (endFlag) {
            atomic_store_explicit(&endFlag->ended, false, memory_order_release);
        }
        p->force_copy = true;
        if (player) {
            CMTime target = CMTimeMakeWithSeconds(clamped, 600);
            [player seekToTime:target toleranceBefore:kCMTimeZero toleranceAfter:kCMTimeZero];
        }
    }
    pthread_mutex_unlock(&p->lock);
}

void slackless_video_player_state(
    SlacklessVideoPlayer *p,
    bool *is_playing,
    bool *is_muted,
    bool *has_ended,
    double *pos_secs,
    double *dur_secs,
    uint32_t *out_w,
    uint32_t *out_h,
    bool *is_failed) {
    if (is_playing) *is_playing = false;
    if (is_muted) *is_muted = false;
    if (has_ended) *has_ended = false;
    if (pos_secs) *pos_secs = 0.0;
    if (dur_secs) *dur_secs = 0.0;
    if (out_w) *out_w = 0;
    if (out_h) *out_h = 0;
    if (is_failed) *is_failed = false;
    if (!p) return;

    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        AVPlayerItem *item = (__bridge AVPlayerItem *)p->item_ref;
        AVURLAsset *asset = (__bridge AVURLAsset *)p->asset_ref;
        SlacklessVideoEndFlag *endFlag = (__bridge SlacklessVideoEndFlag *)p->end_flag_ref;

        bool playing = player && (player.rate > 0.0f);
        bool muted = player ? (player.muted ? true : false) : false;
        bool ended =
            endFlag ? atomic_load_explicit(&endFlag->ended, memory_order_acquire) : false;
        bool failed = (player && player.status == AVPlayerStatusFailed) ||
                      (item && item.status == AVPlayerItemStatusFailed);

        double pos = 0.0;
        if (player) {
            CMTime cur = player.currentTime;
            if (CMTIME_IS_NUMERIC(cur)) {
                double s = CMTimeGetSeconds(cur);
                if (isfinite(s) && s >= 0.0) pos = s;
            }
        }

        double dur = 0.0;
        CMTime d = item ? item.duration : kCMTimeInvalid;
        if (!CMTIME_IS_NUMERIC(d) && asset) {
            d = asset.duration;
        }
        if (CMTIME_IS_NUMERIC(d)) {
            double s = CMTimeGetSeconds(d);
            if (isfinite(s) && s > 0.0) dur = s;
        }

        if (p->track_w == 0 || p->track_h == 0) {
            resolve_video_dimensions(asset, item, &p->track_w, &p->track_h);
        }
        uint32_t w = p->track_w > 0 ? p->track_w : p->last_w;
        uint32_t h = p->track_h > 0 ? p->track_h : p->last_h;

        if (is_playing) *is_playing = playing;
        if (is_muted) *is_muted = muted;
        if (has_ended) *has_ended = ended;
        if (pos_secs) *pos_secs = pos;
        if (dur_secs) *dur_secs = dur;
        if (out_w) *out_w = w;
        if (out_h) *out_h = h;
        if (is_failed) *is_failed = failed;
    }
    pthread_mutex_unlock(&p->lock);
}

void *slackless_video_player_copy_frame(
    SlacklessVideoPlayer *p,
    uint32_t *out_w,
    uint32_t *out_h) {
    if (out_w) *out_w = 0;
    if (out_h) *out_h = 0;
    if (!p) return NULL;

    void *result = NULL;
    uint32_t res_w = 0;
    uint32_t res_h = 0;

    pthread_mutex_lock(&p->lock);
    @autoreleasepool {
        AVPlayer *player = (__bridge AVPlayer *)p->player_ref;
        AVPlayerItemVideoOutput *output = (__bridge AVPlayerItemVideoOutput *)p->output_ref;
        if (player && output) {
            CMTime t = player.currentTime;
            if (!CMTIME_IS_NUMERIC(t)) {
                t = kCMTimeZero;
            }
            bool has_new = [output hasNewPixelBufferForItemTime:t] ? true : false;
            if (has_new || p->force_copy || p->last_pb == NULL) {
                CVPixelBufferRef pb =
                    [output copyPixelBufferForItemTime:t itemTimeForDisplay:NULL];
                if (pb) {
                    uint32_t w = (uint32_t)CVPixelBufferGetWidth(pb);
                    uint32_t h = (uint32_t)CVPixelBufferGetHeight(pb);
                    if (p->last_pb) {
                        CVPixelBufferRelease(p->last_pb);
                    }
                    p->last_pb = CVPixelBufferRetain(pb);
                    p->last_w = w;
                    p->last_h = h;
                    p->force_copy = false;
                    result = (void *)pb; // +1 retained ownership transferred to caller
                    res_w = w;
                    res_h = h;
                }
            }
            if (!result && p->last_pb) {
                result = (void *)CVPixelBufferRetain(p->last_pb);
                res_w = p->last_w;
                res_h = p->last_h;
            }
        }
    }
    pthread_mutex_unlock(&p->lock);

    if (out_w) *out_w = res_w;
    if (out_h) *out_h = res_h;
    return result;
}

void slackless_video_player_destroy(SlacklessVideoPlayer *p) {
    if (!p) return;

    pthread_mutex_lock(&p->lock);
    void *player_ref = p->player_ref;
    void *item_ref = p->item_ref;
    void *asset_ref = p->asset_ref;
    void *output_ref = p->output_ref;
    void *observer_ref = p->observer_ref;
    void *end_flag_ref = p->end_flag_ref;
    CVPixelBufferRef pb = p->last_pb;

    p->player_ref = NULL;
    p->item_ref = NULL;
    p->asset_ref = NULL;
    p->output_ref = NULL;
    p->observer_ref = NULL;
    p->end_flag_ref = NULL;
    p->last_pb = NULL;
    pthread_mutex_unlock(&p->lock);

    pthread_mutex_destroy(&p->lock);
    free(p);

    @autoreleasepool {
        id observer = observer_ref ? (__bridge_transfer id)observer_ref : nil;
        AVPlayer *player = player_ref ? (__bridge_transfer AVPlayer *)player_ref : nil;
        AVPlayerItem *item = item_ref ? (__bridge_transfer AVPlayerItem *)item_ref : nil;
        AVURLAsset *asset = asset_ref ? (__bridge_transfer AVURLAsset *)asset_ref : nil;
        AVPlayerItemVideoOutput *output =
            output_ref ? (__bridge_transfer AVPlayerItemVideoOutput *)output_ref : nil;
        SlacklessVideoEndFlag *endFlag =
            end_flag_ref ? (__bridge_transfer SlacklessVideoEndFlag *)end_flag_ref : nil;

        if (observer) {
            [[NSNotificationCenter defaultCenter] removeObserver:observer];
        }
        if (player) {
            [player pause];
            [player replaceCurrentItemWithPlayerItem:nil];
        }
        if (item && output) {
            [item removeOutput:output];
        }
        if (pb) {
            CVPixelBufferRelease(pb);
        }
        (void)asset;
        (void)endFlag;
    }
}
