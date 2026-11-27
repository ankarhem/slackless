//! Amazon Chime huddle signaling and room lifecycle wire payloads.

use serde::Deserialize;

use crate::de::null_default;
use crate::types::{ChannelId, Ts};

/// Chime attendee capabilities returned inside `rooms.join`'s `call.free_willy.attendee`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeCapabilities {
    #[serde(default, deserialize_with = "null_default", rename = "Audio")]
    pub audio: String,
    #[serde(default, deserialize_with = "null_default", rename = "Video")]
    pub video: String,
    #[serde(default, deserialize_with = "null_default", rename = "Content")]
    pub content: String,
}

/// Chime attendee credentials returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeAttendee {
    #[serde(default, deserialize_with = "null_default", rename = "AttendeeId")]
    pub attendee_id: String,
    #[serde(default, deserialize_with = "null_default", rename = "ExternalUserId")]
    pub external_user_id: String,
    #[serde(default, deserialize_with = "null_default", rename = "JoinToken")]
    pub join_token: String,
    #[serde(default, deserialize_with = "null_default", rename = "Capabilities")]
    pub capabilities: ChimeCapabilities,
}

/// Chime media placement URLs returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeMediaPlacement {
    #[serde(default, deserialize_with = "null_default", rename = "SignalingUrl")]
    pub signaling_url: String,
    #[serde(default, deserialize_with = "null_default", rename = "AudioHostUrl")]
    pub audio_host_url: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        rename = "AudioFallbackUrl"
    )]
    pub audio_fallback_url: String,
    #[serde(default, deserialize_with = "null_default", rename = "TurnControlUrl")]
    pub turn_control_url: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        rename = "ScreenSharingUrl"
    )]
    pub screen_sharing_url: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        rename = "ScreenViewingUrl"
    )]
    pub screen_viewing_url: String,
    #[serde(default, deserialize_with = "null_default", rename = "ScreenDataUrl")]
    pub screen_data_url: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        rename = "EventIngestionUrl"
    )]
    pub event_ingestion_url: String,
}

/// Chime meeting descriptor returned by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeMeeting {
    #[serde(default, deserialize_with = "null_default", rename = "MeetingId")]
    pub meeting_id: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        rename = "ExternalMeetingId"
    )]
    pub external_meeting_id: String,
    #[serde(default, deserialize_with = "null_default", rename = "MediaRegion")]
    pub media_region: String,
    #[serde(default, deserialize_with = "null_default", rename = "MeetingArn")]
    pub meeting_arn: String,
    #[serde(default, deserialize_with = "null_default", rename = "MediaPlacement")]
    pub media_placement: ChimeMediaPlacement,
}

/// `call.free_willy`: Amazon Chime SDK session credentials handed out by `rooms.join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ChimeJoinData {
    #[serde(default, deserialize_with = "null_default")]
    pub attendee: ChimeAttendee,
    #[serde(default, deserialize_with = "null_default")]
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
    #[serde(default, deserialize_with = "null_default")]
    pub(in crate::methods) call_id: String,
    #[serde(default)]
    pub(in crate::methods) free_willy: Option<ChimeJoinData>,
    #[serde(default, deserialize_with = "null_default")]
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

/// Participant entry returned by `calls.info` (`F-COLLAB-086..088`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CallParticipant {
    #[serde(default)]
    pub slack_id: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// Third-party Call object returned by `calls.info` (`docs/research/api-huddles.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CallInfo {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub date_start: Option<i64>,
    #[serde(default)]
    pub date_end: Option<i64>,
    #[serde(default)]
    pub external_unique_id: Option<String>,
    #[serde(default)]
    pub external_display_id: Option<String>,
    #[serde(default)]
    pub join_url: Option<String>,
    #[serde(default)]
    pub desktop_app_join_url: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub users: Vec<CallParticipant>,
    #[serde(default, deserialize_with = "null_default")]
    pub channels: Vec<ChannelId>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct CallsInfoWire {
    #[serde(default, deserialize_with = "null_default")]
    pub(in crate::methods) call: CallInfo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_join_and_calls_info_tolerate_explicit_json_nulls() {
        let join_json = r#"{
            "call": {
                "call_id": null,
                "free_willy": {
                    "attendee": {
                        "AttendeeId": null,
                        "ExternalUserId": null,
                        "JoinToken": null,
                        "Capabilities": { "Audio": null, "Video": null, "Content": null }
                    },
                    "meeting": {
                        "MeetingId": null,
                        "ExternalMeetingId": null,
                        "MediaRegion": null,
                        "MeetingArn": null,
                        "MediaPlacement": {
                            "SignalingUrl": null,
                            "AudioHostUrl": null,
                            "AudioFallbackUrl": null,
                            "TurnControlUrl": null,
                            "ScreenSharingUrl": null,
                            "ScreenViewingUrl": null,
                            "ScreenDataUrl": null,
                            "EventIngestionUrl": null
                        }
                    }
                },
                "huddle": null,
                "canvas": null
            }
        }"#;
        let parsed: RoomsJoinWire =
            serde_json::from_str(join_json).expect("rooms.join must tolerate explicit nulls");
        let call = parsed.call.expect("call present");
        assert!(call.call_id.is_empty());
        assert!(call.free_willy.is_some());

        let call_info_json = r#"{
            "call": {
                "id": null,
                "title": null,
                "users": null,
                "channels": null
            }
        }"#;
        let info: CallsInfoWire =
            serde_json::from_str(call_info_json).expect("calls.info must tolerate explicit nulls");
        assert!(info.call.id.is_empty());
        assert!(info.call.users.is_empty());
        assert!(info.call.channels.is_empty());
    }
}
