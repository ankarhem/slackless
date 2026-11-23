//! Native Amazon Chime SDK (`free_willy`) signaling, WebRTC DTLS-SRTP media, and pure-Rust
//! audio processing (`cpal` + `opus` + `sonora` AEC3/NS/AGC) for Slack Huddles.

pub mod audio;
pub mod rtc;
pub mod session;
pub mod signaling;

/// Generated prost types for Amazon Chime SDK signaling frames (`proto/SignalingProtocol.proto`).
pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/chime.signal.rs"));
}

use slackless_types::{ChannelId, Ts, UserId};
use thiserror::Error;
use tokio::sync::mpsc;

pub use audio::{
    AudioDeviceInfo, AudioDevices, MicLevelProbe, list_audio_devices, play_speaker_test_tone,
};
pub use session::spawn_huddle_session;

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

/// User-selected audio settings for joining or running a huddle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuddleAudioConfig {
    pub muted: bool,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HuddleSessionSnapshot {
    pub phase: HuddlePhase,
    pub channel: ChannelId,
    pub call_id: String,
    pub muted: bool,
    pub video: bool,
    pub sharing: bool,
    pub started_at_secs: u64,
    pub thread_root_ts: Option<Ts>,
    pub canvas_file_id: Option<String>,
    pub huddle_link: Option<String>,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub error: Option<String>,
    pub udp_blocked: bool,
    pub packets_sent: u64,
    pub packets_received: u64,
    pub pongs_sent: u32,
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

/// Events emitted by a running huddle session back to `slackless-core`.
#[derive(Debug, Clone, PartialEq)]
pub enum HuddleUpdate {
    State(HuddleSessionSnapshot),
    AudioLevels(HuddleAudioLevels),
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
    Leave,
}

/// Handle to a running huddle session inside the tokio runtime. Dropping it initiates a clean
/// Chime `LEAVE` teardown.
pub struct HuddleHandle {
    tx: mpsc::UnboundedSender<HuddleControl>,
}

impl HuddleHandle {
    pub(crate) fn new(tx: mpsc::UnboundedSender<HuddleControl>) -> Self {
        Self { tx }
    }

    pub fn send(&self, control: HuddleControl) {
        let _ = self.tx.send(control);
    }

    pub fn leave(&self) {
        let _ = self.tx.send(HuddleControl::Leave);
    }
}

impl Drop for HuddleHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(HuddleControl::Leave);
    }
}
