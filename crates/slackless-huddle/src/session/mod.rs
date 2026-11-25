//! Full Amazon Chime huddle session orchestrator:
//! - Resilient signaling + auto-reconnect supervisor (1s -> 2s -> 4s backoff on unexpected drop)
//! - WebRTC DTLS-SRTP audio + multi-codec video (`AV1` via `rav1d` + `H.264` via `openh264`)
//! - Official `#content` second Chime session for native screen sharing (simultaneous with camera!)
//! - Official Chime `INDEX` + mid-2+ `Recvonly` transceivers + `SUBSCRIBE` + `REMOTE_VIDEO_UPDATE`
//! - Official Slack `PeerMessage` (`calls/client/peer_message.proto`) on `"data-channel"`
//!   (reactions, effects, GIFs, sticky emoji / hand raise, drawing on shared screens, jukebox, mute)
//! - Official AWS Chime `SdkTranscriptFrame` live captions on `"aws:chime:transcription"`

mod connected;
mod controls;
mod handshake;
mod signaling_handlers;
mod supervisor;
#[cfg(test)]
mod tests;
mod video_rx;

pub use supervisor::spawn_huddle_session;
