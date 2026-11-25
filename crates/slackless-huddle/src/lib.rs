//! Native Amazon Chime SDK (`free_willy`) signaling, WebRTC DTLS-SRTP media, pure-Rust audio
//! processing (`cpal` + `opus` + `sonora` AEC3/NS/AGC), H.264 camera video (`AVFoundation` on
//! macOS; `nokhwa` + `openh264` on Linux/Windows), screen sharing (`ScreenCaptureKit` / `xcap` +
//! `openh264`), and Chime `DATA_MESSAGE` extras for Slack Huddles.

pub mod audio;
pub mod av1;
pub mod backend;
pub mod clips;
pub mod content_share;
#[cfg(target_os = "macos")]
pub mod macos_audio_tap;
#[cfg(target_os = "macos")]
pub mod macos_media;
pub mod metrics;
pub mod peer_message;
pub mod rtc;
pub mod screen;
pub mod session;
pub mod signaling;
pub mod video;

/// Generated prost types for Amazon Chime SDK signaling frames (`proto/SignalingProtocol.proto`).
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/chime.signal.rs"));
}

/// Generated prost types for official Slack Huddle `PeerMessage` frames (`proto/PeerMessage.proto`).
pub mod peer_proto {
    include!(concat!(env!("OUT_DIR"), "/slack.huddle.peer.rs"));
}

use slackless_types::{ChannelId, Ts, UserId};
use thiserror::Error;
use tokio::sync::mpsc;

pub use audio::{
    AmbientMusicPlayer, AudioDeviceInfo, AudioDevices, HuddleChime, MicLevelProbe,
    list_audio_devices, play_huddle_chime, play_speaker_test_tone,
};
pub use backend::{MediaCapabilities, platform_backend};
pub use clips::{
    AudioClipRecorder, ClipVideoSource, MAX_CLIP_DURATION_MS, RecordedClip, VideoClipRecorder,
    finish_audio_clip,
};
pub use content_share::{detect_system_audio_loopback, system_audio_source_label};
#[cfg(target_os = "macos")]
pub use macos_media::{
    MacVideoSurface, OverlayCursorFfi, OverlayMouseKind, OverlaySegmentFfi,
    configure_floating_panel, hide_desktop_draw_overlay, set_desktop_draw_pen_mode,
    update_desktop_draw_overlay,
};
pub use metrics::{HuddlePerfCounters, HuddlePerfSnapshot, ProcessResourceSampler};
pub use screen::{ScreenCapture, ScreenSource, capture_screen_thumbnails, list_screen_sources};
pub use session::spawn_huddle_session;
pub use signaling::install_default_crypto_provider;
pub use video::{
    CameraDeviceInfo, CameraPreviewProbe, VideoFrame, VideoFrameData, VideoTileKey, list_cameras,
};

/// Returns `true` if OS screen recording permission is granted (always `true` on non-macOS).
pub fn has_screen_capture_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_media::has_screen_capture_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Prompts for OS screen recording permission on macOS (registers the app in System Settings).
pub fn request_screen_capture_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos_media::request_screen_capture_permission()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Opens OS Screen Recording privacy settings on macOS (no-op on other platforms).
pub fn open_screen_capture_settings() {
    #[cfg(target_os = "macos")]
    macos_media::open_screen_capture_settings();
}

/// Errors returned by the native huddle engine.
#[derive(Debug, Error)]
pub enum HuddleError {
    #[error("signaling connection failed: {0}")]
    Signaling(String),
    #[error("chime protocol error ({status}): {description}")]
    Protocol { status: u32, description: String },
    #[error("webrtc error: {0}")]
    WebRtc(String),
    #[error("UDP media appears blocked on this network (ICE failed) — try Open in browser")]
    UdpBlocked,
    #[error("audio device error: {0}")]
    Audio(String),
    #[error("video/capture error: {0}")]
    Video(String),
    #[error("codec error: {0}")]
    Codec(String),
}

pub type Result<T, E = HuddleError> = std::result::Result<T, E>;

/// Credentials handed out by Slack's `rooms.join` (`call.free_willy`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuddleCredentials {
    pub channel: ChannelId,
    pub call_id: String,
    pub meeting_id: String,
    pub attendee_id: String,
    pub external_user_id: String,
    pub join_token: String,
    pub signaling_url: String,
    pub audio_host_url: String,
    pub turn_control_url: String,
    pub thread_root_ts: Option<Ts>,
    pub canvas_file_id: Option<String>,
    pub huddle_link: Option<String>,
}

/// User-selected audio & video settings for joining or running a huddle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuddleAudioConfig {
    pub muted: bool,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub camera_device: Option<usize>,
    pub blur_background: bool,
    pub aec: bool,
    pub ns: bool,
    pub agc: bool,
}

impl Default for HuddleAudioConfig {
    fn default() -> Self {
        Self {
            muted: false,
            input_device: None,
            output_device: None,
            camera_device: None,
            blur_background: false,
            aec: true,
            ns: true,
            agc: true,
        }
    }
}

/// Lifecycle phase of the active native huddle session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HuddlePhase {
    #[default]
    Idle,
    Joining,
    Connecting,
    Connected,
    Reconnecting,
    Leaving,
    Failed,
}

/// Immutable snapshot of the active huddle session forwarded to UI / CLI subscribers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HuddleSessionSnapshot {
    pub phase: HuddlePhase,
    pub channel: ChannelId,
    pub call_id: String,
    pub attendee_id: String,
    pub muted: bool,
    pub video: bool,
    pub blur_background: bool,
    pub audio_only: bool,
    pub sharing: bool,
    pub share_source_id: Option<String>,
    pub screen_audio_device: Option<String>,
    pub hand_raised: bool,
    pub sticky_emoji: Option<String>,
    pub captions_enabled: bool,
    pub topic: Option<String>,
    pub background_id: Option<String>,
    pub jukebox_song: Option<String>,
    pub started_at_secs: u64,
    pub thread_root_ts: Option<Ts>,
    pub canvas_file_id: Option<String>,
    pub huddle_link: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub aec: bool,
    pub ns: bool,
    pub agc: bool,
    pub active_video_codec: Option<String>,
    pub error: Option<String>,
    pub udp_blocked: bool,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub pongs_sent: u32,
    pub perf: HuddlePerfSnapshot,
}

/// Live speaking / audio-level update pushed at ~10 Hz while connected.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HuddleAudioLevels {
    /// Local microphone RMS level in `0.0..=1.0` (`0.0` while muted).
    pub local_rms: f32,
    /// Remote active speakers sorted loudest first (`(user_id, normalized_level)`).
    pub active_speakers: Vec<(UserId, f32)>,
    /// Remote peers reported muted by the Chime SFU.
    pub muted_peers: Vec<UserId>,
}

/// Reaction kind matching Slack's `PeerMessage.Reacji.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HuddleReactionKind {
    #[default]
    Standard,
    Effect,
    Gif,
}

/// Normalized cubic Bezier segment (`0.0..=1.0` screen-share coordinates) matching Slack's
/// `PeerMessage.Segment` for drawing on shared screens (`F-COLLAB-025`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HuddleDrawSegment {
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub c: [f64; 2],
    pub d: [f64; 2],
}

/// Screen-share drawing / live cursor message matching Slack's `PeerMessage.Draw`.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleDrawMessage {
    New {
        item_id: String,
        segment: HuddleDrawSegment,
    },
    Edit {
        item_id: String,
        segment: HuddleDrawSegment,
    },
    End {
        item_id: String,
    },
    ChangeHue {
        item_id: String,
        hue: i32,
    },
    Position {
        item_id: String,
        x: f64,
        y: f64,
    },
}

/// Interactive Chime `DATA_MESSAGE` event (`PeerMessage` on `"data-channel"` and AWS Transcribe
/// `SdkTranscriptFrame` on `"aws:chime:transcription"`).
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleDataEvent {
    Reaction {
        user: UserId,
        emoji: String,
        kind: HuddleReactionKind,
        emoji_url: Option<String>,
        at_ms: u64,
    },
    HandRaise {
        user: UserId,
        raised: bool,
    },
    StickyEmoji {
        user: UserId,
        emoji: Option<String>,
    },
    Topic {
        user: UserId,
        topic: String,
    },
    Caption {
        user: UserId,
        text: String,
        is_partial: bool,
        at_ms: u64,
    },
    Draw {
        user: UserId,
        message: HuddleDrawMessage,
        at_ms: u64,
    },
    Jukebox {
        user: UserId,
        song_key: Option<String>,
    },
    MutedByPeer {
        actor: UserId,
    },
}

/// Events emitted by a running huddle session back to `slackless-core`.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleUpdate {
    State(Box<HuddleSessionSnapshot>),
    AudioLevels(HuddleAudioLevels),
    VideoFrame {
        tile: VideoTileKey,
        frame: VideoFrame,
    },
    VideoTileClosed(VideoTileKey),
    DataEvent(HuddleDataEvent),
}

/// Live control commands accepted by an active huddle session.
#[derive(Debug, Clone)]
pub enum HuddleControl {
    SetMuted(bool),
    SetAudioDevices {
        input: Option<String>,
        output: Option<String>,
    },
    SetAudioProcessing {
        aec: bool,
        ns: bool,
        agc: bool,
    },
    SetCamera {
        enabled: bool,
        device_index: Option<usize>,
        blur: bool,
    },
    SetBlurBackground(bool),
    SetScreenShare {
        source_id: Option<String>,
        share_audio: bool,
    },
    SendReaction {
        emoji: String,
        kind: HuddleReactionKind,
        emoji_url: Option<String>,
    },
    SetHandRaised(bool),
    SetStickyEmoji(Option<String>),
    SendDraw(HuddleDrawMessage),
    SetJukebox(Option<String>),
    MutePeer(String),
    SetTopic(String),
    SetBackground(Option<String>),
    SetCaptionsEnabled(bool),
    /// Pause all remote video subscriptions & local camera for low-bandwidth audio-only mode (`F-COLLAB-075`).
    SetAudioOnly(bool),
    Leave,
}

/// Handle to a running huddle session inside the tokio runtime. Dropping it initiates a clean
/// Chime `LEAVE` teardown.
pub struct HuddleHandle {
    tx: mpsc::UnboundedSender<HuddleControl>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::Arc<tokio::sync::Notify>,
}

impl HuddleHandle {
    pub(crate) fn new(
        tx: mpsc::UnboundedSender<HuddleControl>,
        finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
        done: std::sync::Arc<tokio::sync::Notify>,
    ) -> Self {
        Self { tx, finished, done }
    }

    pub fn send(&self, control: HuddleControl) {
        let _ = self.tx.send(control);
    }

    pub fn leave(&self) {
        let _ = self.tx.send(HuddleControl::Leave);
    }

    /// Initiate a clean Chime `LEAVE` teardown and wait up to `timeout` for the session task
    /// (including any active `#content` screen-share session) to finish flushing and disconnect.
    pub async fn leave_and_wait(&self, timeout: std::time::Duration) {
        let _ = self.tx.send(HuddleControl::Leave);
        if self.finished.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let notified = self.done.notified();
        if self.finished.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let _ = tokio::time::timeout(timeout, notified).await;
    }
}

impl Drop for HuddleHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(HuddleControl::Leave);
    }
}
