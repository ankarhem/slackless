//! Amazon Chime huddle signaling and room lifecycle wire payloads.

use serde::Deserialize;

use crate::types::{ChannelId, Ts};

/// Chime attendee capabilities returned inside `rooms.join`'s `call.free_willy.attendee`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeCapabilities {
    #[serde(default, rename = "Audio")]
    pub audio: String,
    #[serde(default, rename = "Video")]
    pub video: String,
    #[serde(default, rename = "Content")]
    pub content: String,
}

/// Chime attendee credentials returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeAttendee {
    #[serde(default, rename = "AttendeeId")]
    pub attendee_id: String,
    #[serde(default, rename = "ExternalUserId")]
    pub external_user_id: String,
    #[serde(default, rename = "JoinToken")]
    pub join_token: String,
    #[serde(default, rename = "Capabilities")]
    pub capabilities: ChimeCapabilities,
}

/// Chime media placement URLs returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeMediaPlacement {
    #[serde(default, rename = "SignalingUrl")]
    pub signaling_url: String,
    #[serde(default, rename = "AudioHostUrl")]
    pub audio_host_url: String,
    #[serde(default, rename = "AudioFallbackUrl")]
    pub audio_fallback_url: String,
    #[serde(default, rename = "TurnControlUrl")]
    pub turn_control_url: String,
    #[serde(default, rename = "ScreenSharingUrl")]
    pub screen_sharing_url: String,
    #[serde(default, rename = "ScreenViewingUrl")]
    pub screen_viewing_url: String,
    #[serde(default, rename = "ScreenDataUrl")]
    pub screen_data_url: String,
    #[serde(default, rename = "EventIngestionUrl")]
    pub event_ingestion_url: String,
}

/// Chime meeting descriptor returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeMeeting {
    #[serde(default, rename = "MeetingId")]
    pub meeting_id: String,
    #[serde(default, rename = "ExternalMeetingId")]
    pub external_meeting_id: String,
    #[serde(default, rename = "MediaRegion")]
    pub media_region: String,
    #[serde(default, rename = "MeetingArn")]
    pub meeting_arn: String,
    #[serde(default, rename = "MediaPlacement")]
    pub media_placement: ChimeMediaPlacement,
}

/// `call.free_willy`: Amazon Chime SDK session credentials handed out by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeJoinData {
    #[serde(default)]
    pub attendee: ChimeAttendee,
    #[serde(default)]
    pub meeting: ChimeMeeting,
}

/// Optional huddle notes canvas linkage returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct HuddleCanvasRef {
    #[serde(default)]
    pub root_thread_ts: Option<Ts>,
    #[serde(default)]
    pub thread_channel_id: Option<ChannelId>,
    #[serde(default)]
    pub file_id: Option<String>,
}

/// Parsed response from `rooms.join` (`call` envelope).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomJoinResponse {
    pub call_id: String,
    pub chime: ChimeJoinData,
    pub huddle: crate::types::HuddleRoom,
    pub canvas: Option<HuddleCanvasRef>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct RoomsJoinWire {
    #[serde(default)]
    pub(in crate::methods) call: Option<RoomCallWire>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct RoomCallWire {
    #[serde(default)]
    pub(in crate::methods) call_id: String,
    #[serde(default)]
    pub(in crate::methods) free_willy: Option<ChimeJoinData>,
    #[serde(default)]
    pub(in crate::methods) huddle: crate::types::HuddleRoom,
    #[serde(default)]
    pub(in crate::methods) canvas: Option<HuddleCanvasRef>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct RoomsGetLinkWire {
    #[serde(default)]
    pub(in crate::methods) link: Option<String>,
    #[serde(default)]
    pub(in crate::methods) huddle_link: Option<String>,
    #[serde(default)]
    pub(in crate::methods) url: Option<String>,
}
