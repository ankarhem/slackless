//! Official Slack Huddle `PeerMessage` (`calls/client/peer_message.proto`) encoder/decoder on
//! Amazon Chime `DATA_MESSAGE` topic `"data-channel"`, plus official AWS Chime live captions
//! (`SdkTranscriptFrame`) decoder on topic `"aws:chime:transcription"`.

use prost::Message as _;
use slackless_types::UserId;

use crate::peer_proto::PeerMessage;
use crate::peer_proto::peer_message;
use crate::proto::SdkTranscriptFrame;
use crate::proto::sdk_transcript_event::Event as TranscriptEventKind;
use crate::signaling::parse_external_user_id;
use crate::{HuddleDataEvent, HuddleDrawMessage, HuddleDrawSegment, HuddleReactionKind};

/// Exact Chime `DATA_MESSAGE` topic used by official Slack clients for `PeerMessage` frames.
pub const SLACK_DATA_CHANNEL_TOPIC: &str = "data-channel";

/// Exact Chime `DATA_MESSAGE` topic used by Amazon Chime / AWS Transcribe for live captions.
pub const CHIME_TRANSCRIPTION_TOPIC: &str = "aws:chime:transcription";

/// Awareness field name used by official Slack for sticky reactions (including `"raised_hand"`).
pub const AWARENESS_STICKY_EMOJI: &str = "stickyEmoji";

/// Awareness field name used by official Slack for active huddle jukebox song.
pub const AWARENESS_ACTIVE_SONG: &str = "huddleActiveSong";

/// Encode a Slack `PeerMessage` into protobuf bytes ready for `SdkDataMessagePayload.data`.
pub fn encode_peer_message(msg: &PeerMessage) -> Vec<u8> {
    msg.encode_to_vec()
}

pub fn build_reacji_message(
    from_attendee_id: &str,
    emoji: &str,
    kind: HuddleReactionKind,
    emoji_url: Option<String>,
) -> Vec<u8> {
    let rtype = match kind {
        HuddleReactionKind::Effect => peer_message::reacji::Type::Effect as i32,
        HuddleReactionKind::Standard => peer_message::reacji::Type::Standard as i32,
        HuddleReactionKind::Gif => peer_message::reacji::Type::Gif as i32,
    };
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        reacji: Some(peer_message::Reacji {
            emoji: Some(emoji.to_owned()),
            r#type: Some(rtype),
            emoji_url,
        }),
        ..Default::default()
    })
}

pub fn build_draw_message(from_attendee_id: &str, draw: &HuddleDrawMessage) -> Vec<u8> {
    let wire = match draw {
        HuddleDrawMessage::New { item_id, segment } => peer_message::Draw {
            r#type: Some(peer_message::draw::Type::New as i32),
            item_id: Some(item_id.clone()),
            segment: Some(segment_to_wire(segment)),
            ..Default::default()
        },
        HuddleDrawMessage::Edit { item_id, segment } => peer_message::Draw {
            r#type: Some(peer_message::draw::Type::Edit as i32),
            item_id: Some(item_id.clone()),
            segment: Some(segment_to_wire(segment)),
            ..Default::default()
        },
        HuddleDrawMessage::End { item_id } => peer_message::Draw {
            r#type: Some(peer_message::draw::Type::End as i32),
            item_id: Some(item_id.clone()),
            ..Default::default()
        },
        HuddleDrawMessage::ChangeHue { item_id, hue } => peer_message::Draw {
            r#type: Some(peer_message::draw::Type::ChangeHue as i32),
            item_id: Some(item_id.clone()),
            hue: Some(*hue),
            ..Default::default()
        },
        HuddleDrawMessage::Position { item_id, x, y } => peer_message::Draw {
            r#type: Some(peer_message::draw::Type::Position as i32),
            item_id: Some(item_id.clone()),
            x: Some(*x),
            y: Some(*y),
            ..Default::default()
        },
    };
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        draw: Some(wire),
        ..Default::default()
    })
}

pub fn build_capabilities_message(from_attendee_id: &str, is_draw_enabled: bool) -> Vec<u8> {
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        capabilities: Some(peer_message::Capabilities {
            is_draw_enabled: Some(is_draw_enabled),
        }),
        ..Default::default()
    })
}

pub fn build_sticky_emoji_awareness(
    from_attendee_id: &str,
    call_id: &str,
    user_id: &UserId,
    emoji: Option<&str>,
    now_ms: i64,
) -> Vec<u8> {
    let awareness_state = match emoji {
        Some(val) if !val.is_empty() => vec![peer_message::AwarenessPayload {
            field_name: Some(AWARENESS_STICKY_EMOJI.to_owned()),
            field_value: Some(val.to_owned()),
            field_timestamp_ms: Some(now_ms),
        }],
        _ => Vec::new(),
    };
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        awareness_broadcast: Some(peer_message::AwarenessBroadcast {
            channel: Some(call_id.to_owned()),
            user: Some(user_id.as_str().to_owned()),
            awareness_state,
        }),
        ..Default::default()
    })
}

pub fn build_jukebox_message(from_attendee_id: &str, song_key: Option<&str>) -> Vec<u8> {
    let req = match song_key {
        Some(key) if !key.is_empty() => peer_message::JukeboxRequest {
            song_key: Some(key.to_owned()),
            stop: None,
        },
        _ => peer_message::JukeboxRequest {
            song_key: None,
            stop: Some(true),
        },
    };
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        jukebox_request: Some(req),
        ..Default::default()
    })
}

pub fn build_mute_request_message(from_attendee_id: &str, target_peer_id: &str) -> Vec<u8> {
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        mute_request: Some(peer_message::MuteRequest {
            peer_id: Some(target_peer_id.to_owned()),
        }),
        ..Default::default()
    })
}

pub fn build_content_share_details_message(from_attendee_id: &str, source_type: &str) -> Vec<u8> {
    encode_peer_message(&PeerMessage {
        from_peer_id_string: Some(from_attendee_id.to_owned()),
        content_share_details: Some(peer_message::ContentShareDetails {
            source_type: Some(source_type.to_owned()),
            item_id: Some(from_attendee_id.to_owned()),
        }),
        ..Default::default()
    })
}

fn segment_to_wire(seg: &HuddleDrawSegment) -> peer_message::Segment {
    let pt = |p: [f64; 2]| peer_message::Point {
        x: Some(p[0]),
        y: Some(p[1]),
    };
    peer_message::Segment {
        a: Some(pt(seg.a)),
        b: Some(pt(seg.b)),
        c: Some(pt(seg.c)),
        d: Some(pt(seg.d)),
    }
}

fn segment_from_wire(seg: &peer_message::Segment) -> HuddleDrawSegment {
    let pt = |p: Option<&peer_message::Point>| {
        p.map(|p| [p.x.unwrap_or(0.0), p.y.unwrap_or(0.0)])
            .unwrap_or([0.0, 0.0])
    };
    HuddleDrawSegment {
        a: pt(seg.a.as_ref()),
        b: pt(seg.b.as_ref()),
        c: pt(seg.c.as_ref()),
        d: pt(seg.d.as_ref()),
    }
}

/// Decode an incoming Chime `DATA_MESSAGE` on `"data-channel"` into zero or more [`HuddleDataEvent`]s.
pub fn decode_slack_peer_message(
    sender: &UserId,
    sender_attendee_id: &str,
    self_attendee_id: &str,
    raw: &[u8],
    now_ms: u64,
) -> Vec<HuddleDataEvent> {
    let Ok(msg) = PeerMessage::decode(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    if let Some(reacji) = msg.reacji {
        if let Some(emoji) = reacji.emoji.filter(|e| !e.is_empty()) {
            let kind = match reacji.r#type {
                Some(x) if x == peer_message::reacji::Type::Effect as i32 => {
                    HuddleReactionKind::Effect
                }
                Some(x) if x == peer_message::reacji::Type::Gif as i32 => HuddleReactionKind::Gif,
                _ => HuddleReactionKind::Standard,
            };
            out.push(HuddleDataEvent::Reaction {
                user: sender.clone(),
                emoji,
                kind,
                emoji_url: reacji.emoji_url.filter(|u| !u.is_empty()),
                at_ms: now_ms,
            });
        }
    }

    if let Some(draw) = msg.draw {
        let item_id = draw
            .item_id
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| sender_attendee_id.to_owned());
        let parsed = match draw.r#type {
            Some(x) if x == peer_message::draw::Type::New as i32 => {
                draw.segment.as_ref().map(|seg| HuddleDrawMessage::New {
                    item_id,
                    segment: segment_from_wire(seg),
                })
            }
            Some(x) if x == peer_message::draw::Type::Edit as i32 => {
                draw.segment.as_ref().map(|seg| HuddleDrawMessage::Edit {
                    item_id,
                    segment: segment_from_wire(seg),
                })
            }
            Some(x) if x == peer_message::draw::Type::End as i32 => {
                Some(HuddleDrawMessage::End { item_id })
            }
            Some(x) if x == peer_message::draw::Type::ChangeHue as i32 => {
                Some(HuddleDrawMessage::ChangeHue {
                    item_id,
                    hue: draw.hue.unwrap_or(210),
                })
            }
            Some(x) if x == peer_message::draw::Type::Position as i32 => {
                Some(HuddleDrawMessage::Position {
                    item_id,
                    x: draw.x.unwrap_or(0.0),
                    y: draw.y.unwrap_or(0.0),
                })
            }
            _ => None,
        };
        if let Some(message) = parsed {
            out.push(HuddleDataEvent::Draw {
                user: sender.clone(),
                message,
                at_ms: now_ms,
            });
        }
    }

    if let Some(aw) = msg.awareness_broadcast {
        let user = aw
            .user
            .filter(|u| !u.is_empty())
            .map(UserId::from)
            .unwrap_or_else(|| sender.clone());
        let sticky = aw
            .awareness_state
            .iter()
            .find(|p| p.field_name.as_deref() == Some(AWARENESS_STICKY_EMOJI));
        let raised = sticky
            .and_then(|p| p.field_value.as_deref())
            .is_some_and(|v| matches!(v, "raised_hand" | "hand" | "raising_hand" | "✋"));
        let sticky_val = sticky
            .and_then(|p| p.field_value.clone())
            .filter(|v| !v.is_empty());
        out.push(HuddleDataEvent::HandRaise {
            user: user.clone(),
            raised,
        });
        out.push(HuddleDataEvent::StickyEmoji {
            user: user.clone(),
            emoji: sticky_val,
        });
        if let Some(song) = aw
            .awareness_state
            .iter()
            .find(|p| p.field_name.as_deref() == Some(AWARENESS_ACTIVE_SONG))
        {
            out.push(HuddleDataEvent::Jukebox {
                user,
                song_key: song.field_value.clone().filter(|s| !s.is_empty()),
            });
        }
    }

    if let Some(jukebox) = msg.jukebox_request {
        let song_key = if jukebox.stop.unwrap_or(false) {
            None
        } else {
            jukebox.song_key.filter(|s| !s.is_empty())
        };
        out.push(HuddleDataEvent::Jukebox {
            user: sender.clone(),
            song_key,
        });
    }

    if let Some(mute) = msg.mute_request {
        if mute.peer_id.as_deref() == Some(self_attendee_id) {
            out.push(HuddleDataEvent::MutedByPeer {
                actor: sender.clone(),
            });
        }
    }

    out
}

/// Decode an official AWS Chime `SdkTranscriptFrame` (`"aws:chime:transcription"`).
pub fn decode_chime_transcript_frame(raw: &[u8], now_ms: u64) -> Vec<HuddleDataEvent> {
    let Ok(frame) = SdkTranscriptFrame::decode(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in frame.events {
        if let Some(TranscriptEventKind::Transcript(tr)) = ev.event {
            for res in tr.results {
                let is_partial = res.is_partial.unwrap_or(false);
                if let Some(alt) = res.alternatives.first() {
                    let text = alt.transcript.as_deref().unwrap_or("").trim();
                    if text.is_empty() {
                        continue;
                    }
                    let speaker = alt
                        .items
                        .iter()
                        .find_map(|item| item.speaker_external_user_id.as_deref())
                        .and_then(parse_external_user_id)
                        .unwrap_or_else(|| UserId::from("USLACKBOT".to_owned()));
                    out.push(HuddleDataEvent::Caption {
                        user: speaker,
                        text: text.to_owned(),
                        is_partial,
                        at_ms: now_ms,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::{
        SdkTranscript, SdkTranscriptAlternative, SdkTranscriptEvent, SdkTranscriptItem,
        SdkTranscriptResult,
    };

    #[test]
    fn peer_message_reacji_draw_and_awareness_round_trip() {
        let sender = UserId::from("U01ALICE");
        let bytes = build_reacji_message("att-1", "confetti", HuddleReactionKind::Effect, None);
        let evs = decode_slack_peer_message(&sender, "att-1", "att-2", &bytes, 100);
        assert_eq!(
            evs,
            vec![HuddleDataEvent::Reaction {
                user: sender.clone(),
                emoji: "confetti".to_owned(),
                kind: HuddleReactionKind::Effect,
                emoji_url: None,
                at_ms: 100,
            }]
        );

        let draw_msg = HuddleDrawMessage::New {
            item_id: "att-2".to_owned(),
            segment: HuddleDrawSegment {
                a: [0.1, 0.2],
                b: [0.2, 0.3],
                c: [0.3, 0.4],
                d: [0.4, 0.5],
            },
        };
        let dbytes = build_draw_message("att-1", &draw_msg);
        let devs = decode_slack_peer_message(&sender, "att-1", "att-2", &dbytes, 200);
        assert_eq!(
            devs,
            vec![HuddleDataEvent::Draw {
                user: sender.clone(),
                message: draw_msg,
                at_ms: 200,
            }]
        );

        let abytes =
            build_sticky_emoji_awareness("att-1", "R01CALL", &sender, Some("raised_hand"), 300);
        let aevs = decode_slack_peer_message(&sender, "att-1", "att-2", &abytes, 300);
        assert!(aevs.contains(&HuddleDataEvent::HandRaise {
            user: sender.clone(),
            raised: true,
        }));
    }

    #[test]
    fn chime_transcript_frame_decodes_speaker_and_caption() {
        let frame = SdkTranscriptFrame {
            events: vec![SdkTranscriptEvent {
                event: Some(TranscriptEventKind::Transcript(SdkTranscript {
                    results: vec![SdkTranscriptResult {
                        is_partial: Some(false),
                        alternatives: vec![SdkTranscriptAlternative {
                            transcript: Some("Hello from AWS Transcribe".to_owned()),
                            items: vec![SdkTranscriptItem {
                                speaker_external_user_id: Some(
                                    "T02HD8VA1-R01CALL-U01ALICE".to_owned(),
                                ),
                                ..Default::default()
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                })),
            }],
        };
        let raw = frame.encode_to_vec();
        let evs = decode_chime_transcript_frame(&raw, 999);
        assert_eq!(
            evs,
            vec![HuddleDataEvent::Caption {
                user: UserId::from("U01ALICE"),
                text: "Hello from AWS Transcribe".to_owned(),
                is_partial: false,
                at_ms: 999,
            }]
        );
    }
}
