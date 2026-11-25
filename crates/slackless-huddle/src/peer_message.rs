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

    #[test]
    fn peer_message_draw_variants_and_defaults() {
        let sender = UserId::from("U01BOB");
        let seg = HuddleDrawSegment {
            a: [0.0, 0.1],
            b: [0.2, 0.3],
            c: [0.4, 0.5],
            d: [0.6, 0.7],
        };
        let cases = vec![
            HuddleDrawMessage::Edit {
                item_id: "stroke-1".to_owned(),
                segment: seg,
            },
            HuddleDrawMessage::End {
                item_id: "stroke-1".to_owned(),
            },
            HuddleDrawMessage::ChangeHue {
                item_id: "stroke-1".to_owned(),
                hue: 320,
            },
            HuddleDrawMessage::Position {
                item_id: "stroke-1".to_owned(),
                x: 0.25,
                y: 0.75,
            },
        ];
        for draw in cases {
            let raw = build_draw_message("att-bob", &draw);
            let evs = decode_slack_peer_message(&sender, "att-bob", "att-me", &raw, 55);
            assert_eq!(
                evs,
                vec![HuddleDataEvent::Draw {
                    user: sender.clone(),
                    message: draw,
                    at_ms: 55,
                }]
            );
        }

        // Missing item_id falls back to sender_attendee_id; missing hue defaults to 210.
        let raw_defaults = encode_peer_message(&PeerMessage {
            draw: Some(peer_message::Draw {
                r#type: Some(peer_message::draw::Type::ChangeHue as i32),
                item_id: Some(String::new()),
                hue: None,
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(
            decode_slack_peer_message(&sender, "att-fallback", "att-me", &raw_defaults, 1),
            vec![HuddleDataEvent::Draw {
                user: sender.clone(),
                message: HuddleDrawMessage::ChangeHue {
                    item_id: "att-fallback".to_owned(),
                    hue: 210,
                },
                at_ms: 1,
            }]
        );

        // Edit without segment emits no Draw event.
        let raw_no_seg = encode_peer_message(&PeerMessage {
            draw: Some(peer_message::Draw {
                r#type: Some(peer_message::draw::Type::Edit as i32),
                item_id: Some("x".to_owned()),
                segment: None,
                ..Default::default()
            }),
            ..Default::default()
        });
        assert!(decode_slack_peer_message(&sender, "att", "att-me", &raw_no_seg, 1).is_empty());
    }

    #[test]
    fn peer_message_reacji_jukebox_mute_and_awareness_edge_cases() {
        let sender = UserId::from("U02CAROL");

        // Reacji Standard & Gif with URL vs empty URL vs empty emoji.
        let std_bytes = build_reacji_message(
            "att-c",
            "tada",
            HuddleReactionKind::Standard,
            Some(String::new()),
        );
        assert_eq!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &std_bytes, 10),
            vec![HuddleDataEvent::Reaction {
                user: sender.clone(),
                emoji: "tada".to_owned(),
                kind: HuddleReactionKind::Standard,
                emoji_url: None,
                at_ms: 10,
            }]
        );

        let gif_bytes = build_reacji_message(
            "att-c",
            "party_parrot",
            HuddleReactionKind::Gif,
            Some("https://cdn.example/parrot.gif".to_owned()),
        );
        assert_eq!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &gif_bytes, 20),
            vec![HuddleDataEvent::Reaction {
                user: sender.clone(),
                emoji: "party_parrot".to_owned(),
                kind: HuddleReactionKind::Gif,
                emoji_url: Some("https://cdn.example/parrot.gif".to_owned()),
                at_ms: 20,
            }]
        );

        let empty_emoji = build_reacji_message("att-c", "", HuddleReactionKind::Standard, None);
        assert!(decode_slack_peer_message(&sender, "att-c", "att-me", &empty_emoji, 30).is_empty());

        // Jukebox start & stop.
        let play_juke = build_jukebox_message("att-c", Some("lofi"));
        assert_eq!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &play_juke, 40),
            vec![HuddleDataEvent::Jukebox {
                user: sender.clone(),
                song_key: Some("lofi".to_owned()),
            }]
        );
        let stop_juke = build_jukebox_message("att-c", None);
        assert_eq!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &stop_juke, 41),
            vec![HuddleDataEvent::Jukebox {
                user: sender.clone(),
                song_key: None,
            }]
        );

        // Mute request: targeting self emits MutedByPeer, targeting another peer emits nothing.
        let mute_self = build_mute_request_message("att-c", "att-me");
        assert_eq!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &mute_self, 50),
            vec![HuddleDataEvent::MutedByPeer {
                actor: sender.clone(),
            }]
        );
        let mute_other = build_mute_request_message("att-c", "att-other");
        assert!(decode_slack_peer_message(&sender, "att-c", "att-me", &mute_other, 51).is_empty());

        // AwarenessBroadcast: all 4 raised-hand aliases.
        for alias in ["raised_hand", "hand", "raising_hand", "✋"] {
            let bytes = build_sticky_emoji_awareness("att-c", "R1", &sender, Some(alias), 60);
            let evs = decode_slack_peer_message(&sender, "att-c", "att-me", &bytes, 60);
            assert!(evs.contains(&HuddleDataEvent::HandRaise {
                user: sender.clone(),
                raised: true,
            }));
            assert!(evs.contains(&HuddleDataEvent::StickyEmoji {
                user: sender.clone(),
                emoji: Some(alias.to_owned()),
            }));
        }

        // AwarenessBroadcast: sticky clear sets raised=false and emoji=None.
        let clear_bytes = build_sticky_emoji_awareness("att-c", "R1", &sender, None, 61);
        let clear_evs = decode_slack_peer_message(&sender, "att-c", "att-me", &clear_bytes, 61);
        assert_eq!(
            clear_evs,
            vec![
                HuddleDataEvent::HandRaise {
                    user: sender.clone(),
                    raised: false,
                },
                HuddleDataEvent::StickyEmoji {
                    user: sender.clone(),
                    emoji: None,
                },
            ]
        );

        // AwarenessBroadcast with huddleActiveSong + non-hand sticky emoji.
        let aw_song = encode_peer_message(&PeerMessage {
            awareness_broadcast: Some(peer_message::AwarenessBroadcast {
                channel: Some("R1".to_owned()),
                user: None, // falls back to sender
                awareness_state: vec![
                    peer_message::AwarenessPayload {
                        field_name: Some(AWARENESS_STICKY_EMOJI.to_owned()),
                        field_value: Some("coffee".to_owned()),
                        field_timestamp_ms: Some(100),
                    },
                    peer_message::AwarenessPayload {
                        field_name: Some(AWARENESS_ACTIVE_SONG.to_owned()),
                        field_value: Some("jazz".to_owned()),
                        field_timestamp_ms: Some(100),
                    },
                ],
            }),
            ..Default::default()
        });
        let song_evs = decode_slack_peer_message(&sender, "att-c", "att-me", &aw_song, 62);
        assert_eq!(
            song_evs,
            vec![
                HuddleDataEvent::HandRaise {
                    user: sender.clone(),
                    raised: false,
                },
                HuddleDataEvent::StickyEmoji {
                    user: sender.clone(),
                    emoji: Some("coffee".to_owned()),
                },
                HuddleDataEvent::Jukebox {
                    user: sender.clone(),
                    song_key: Some("jazz".to_owned()),
                },
            ]
        );

        // Corrupt bytes -> empty Vec.
        assert!(
            decode_slack_peer_message(&sender, "att-c", "att-me", &[0xFF, 0xFF, 0xFF, 0xFF], 70)
                .is_empty()
        );
    }

    #[test]
    fn chime_transcript_frame_partial_blank_missing_speaker_non_transcript_and_corrupt() {
        use crate::proto::SdkTranscriptionStatus;

        let frame = SdkTranscriptFrame {
            events: vec![
                // Non-transcript event (Status) -> skipped.
                SdkTranscriptEvent {
                    event: Some(TranscriptEventKind::Status(
                        SdkTranscriptionStatus::default(),
                    )),
                },
                SdkTranscriptEvent {
                    event: Some(TranscriptEventKind::Transcript(SdkTranscript {
                        results: vec![
                            // Blank / whitespace-only -> skipped.
                            SdkTranscriptResult {
                                is_partial: Some(false),
                                alternatives: vec![SdkTranscriptAlternative {
                                    transcript: Some("   \t\n ".to_owned()),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            },
                            // Partial caption with missing speaker -> USLACKBOT fallback.
                            SdkTranscriptResult {
                                is_partial: Some(true),
                                alternatives: vec![SdkTranscriptAlternative {
                                    transcript: Some("  hello world  ".to_owned()),
                                    items: Vec::new(),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            },
                        ],
                    })),
                },
            ],
        };
        let evs = decode_chime_transcript_frame(&frame.encode_to_vec(), 1234);
        assert_eq!(
            evs,
            vec![HuddleDataEvent::Caption {
                user: UserId::from("USLACKBOT"),
                text: "hello world".to_owned(),
                is_partial: true,
                at_ms: 1234,
            }]
        );

        // Corrupt protobuf -> empty.
        assert!(decode_chime_transcript_frame(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF], 1).is_empty());
    }
}
