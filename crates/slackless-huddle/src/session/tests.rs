use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use slackless_types::UserId;
use tokio::sync::mpsc;

use super::signaling_handlers::{content_share_source_type, parse_incoming_data_message};
use super::supervisor::{
    LoopExit, ReconnectRunner, SessionDoneGuard, classify_handshake_err, classify_signal_close,
    run_reconnect_supervisor,
};
use super::video_rx::{RemoteVideoDecoder, resolve_remote_video_tile, same_remote_video_sources};
use crate::metrics::HuddlePerfCounters;
use crate::peer_message::{
    CHIME_TRANSCRIPTION_TOPIC, SLACK_DATA_CHANNEL_TOPIC, build_reacji_message,
};
use crate::rtc::InboundVideoPacket;
use crate::signaling::RemoteVideoSource;
use crate::video::VideoTileKey;
use crate::{
    HuddleControl, HuddleDataEvent, HuddleError, HuddleHandle, HuddlePhase, HuddleSessionSnapshot,
    HuddleUpdate,
};

#[test]
fn content_share_source_type_matches_official_slack_desktop_values() {
    assert_eq!(content_share_source_type("monitor:2"), "desktop:screen");
    assert_eq!(content_share_source_type("window:12345"), "desktop:window");
}

#[test]
fn same_remote_video_sources_ignores_bitrate_fluctuations() {
    let a = RemoteVideoSource {
        stream_id: 21,
        group_id: 2,
        attendee_id: "att-1".to_owned(),
        user_id: UserId::from("U1"),
        is_content: false,
        max_bitrate_kbps: 1400,
    };
    let b_same_diff_bps = RemoteVideoSource {
        max_bitrate_kbps: 888,
        ..a.clone()
    };
    let b_diff_stream = RemoteVideoSource {
        stream_id: 22,
        ..a.clone()
    };
    assert!(same_remote_video_sources(&[&a], &[b_same_diff_bps]));
    assert!(!same_remote_video_sources(&[&a], &[b_diff_stream]));
    assert!(!same_remote_video_sources(&[&a], &[]));
    let b_diff_content = RemoteVideoSource {
        is_content: true,
        ..a.clone()
    };
    assert!(!same_remote_video_sources(&[&a], &[b_diff_content]));
}

#[test]
fn resolve_remote_video_tile_matches_mid_ssrc_and_fallback() {
    let cam = RemoteVideoSource {
        stream_id: 10,
        group_id: 1,
        attendee_id: "att-a".into(),
        user_id: UserId::from("U1"),
        is_content: false,
        max_bitrate_kbps: 1000,
    };
    let screen = RemoteVideoSource {
        stream_id: 20,
        group_id: 2,
        attendee_id: "att-b#content".into(),
        user_id: UserId::from("U2"),
        is_content: true,
        max_bitrate_kbps: 2000,
    };
    let sources = vec![("2".to_owned(), cam), ("3".to_owned(), screen)];
    let mut ssrc_map = HashMap::new();
    ssrc_map.insert(555, 20);
    ssrc_map.insert(444, 10);

    // Exact mid match takes precedence over ssrc.
    assert_eq!(
        resolve_remote_video_tile(555, "2", &ssrc_map, &sources),
        Some(VideoTileKey::RemoteCamera(UserId::from("U1")))
    );
    assert_eq!(
        resolve_remote_video_tile(444, "3", &ssrc_map, &sources),
        Some(VideoTileKey::RemoteScreen(UserId::from("U2")))
    );
    // Empty or unmatched mid falls back to SSRC -> stream_id map.
    assert_eq!(
        resolve_remote_video_tile(555, "", &ssrc_map, &sources),
        Some(VideoTileKey::RemoteScreen(UserId::from("U2")))
    );
    assert_eq!(
        resolve_remote_video_tile(555, "99", &ssrc_map, &sources),
        Some(VideoTileKey::RemoteScreen(UserId::from("U2")))
    );
    // Unmatched mid and unmatched SSRC falls back to first source.
    assert_eq!(
        resolve_remote_video_tile(999, "99", &ssrc_map, &sources),
        Some(VideoTileKey::RemoteCamera(UserId::from("U1")))
    );
    // Empty sources -> None.
    assert_eq!(resolve_remote_video_tile(555, "2", &ssrc_map, &[]), None);
}

#[test]
fn parse_incoming_data_message_routes_topics_and_timestamps() {
    use crate::HuddleReactionKind;
    use crate::proto::{
        SdkDataMessagePayload, SdkTranscript, SdkTranscriptAlternative, SdkTranscriptEvent,
        SdkTranscriptFrame, SdkTranscriptItem, SdkTranscriptResult,
        sdk_transcript_event::Event as TrEvent,
    };
    use prost::Message as _;

    // Slack data-channel topic.
    let reacji = build_reacji_message("att-1", "fire", HuddleReactionKind::Standard, None);
    let msg = SdkDataMessagePayload {
        topic: Some(SLACK_DATA_CHANNEL_TOPIC.into()),
        data: Some(reacji),
        ingest_time_ns: Some(42_000_000),
        sender_attendee_id: Some("att-1".into()),
        sender_external_user_id: Some("T1-R1-U0ALICE".into()),
        ..Default::default()
    };
    assert_eq!(
        parse_incoming_data_message(&msg, "att-me"),
        vec![HuddleDataEvent::Reaction {
            user: UserId::from("U0ALICE"),
            emoji: "fire".into(),
            kind: HuddleReactionKind::Standard,
            emoji_url: None,
            at_ms: 42,
        }]
    );

    // Chime transcription topic with negative ingest_time_ns clamped to 0.
    let tr = SdkTranscriptFrame {
        events: vec![SdkTranscriptEvent {
            event: Some(TrEvent::Transcript(SdkTranscript {
                results: vec![SdkTranscriptResult {
                    is_partial: Some(false),
                    alternatives: vec![SdkTranscriptAlternative {
                        transcript: Some("Hi".into()),
                        items: vec![SdkTranscriptItem {
                            speaker_external_user_id: Some("T1-R1-U0BOB".into()),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            })),
        }],
    };
    let tr_msg = SdkDataMessagePayload {
        topic: Some(CHIME_TRANSCRIPTION_TOPIC.into()),
        data: Some(tr.encode_to_vec()),
        ingest_time_ns: Some(-500),
        ..Default::default()
    };
    assert_eq!(
        parse_incoming_data_message(&tr_msg, "att-me"),
        vec![HuddleDataEvent::Caption {
            user: UserId::from("U0BOB"),
            text: "Hi".into(),
            is_partial: false,
            at_ms: 0,
            lang: None,
        }]
    );

    // Unknown topic -> empty.
    let unknown = SdkDataMessagePayload {
        topic: Some("custom:other".into()),
        data: Some(b"hello".to_vec()),
        ..Default::default()
    };
    assert!(parse_incoming_data_message(&unknown, "att-me").is_empty());
}

#[test]
fn handshake_and_signal_close_classification() {
    // Initial connect failure is Fatal; reconnect failure is Reconnect (except 4xx protocol error).
    assert!(matches!(
        classify_handshake_err(HuddleError::Signaling("dns".into()), false),
        LoopExit::Fatal(_)
    ));
    assert!(matches!(
        classify_handshake_err(HuddleError::Signaling("dns".into()), true),
        LoopExit::Reconnect {
            was_connected: false,
            ..
        }
    ));
    assert!(matches!(
        classify_handshake_err(
            HuddleError::Protocol {
                status: 403,
                description: "expired".into()
            },
            true
        ),
        LoopExit::Fatal(_)
    ));
    assert!(matches!(
        classify_handshake_err(
            HuddleError::Protocol {
                status: 503,
                description: "unavailable".into()
            },
            true
        ),
        LoopExit::Reconnect {
            was_connected: false,
            ..
        }
    ));

    // Ok(None) while !ever_connected is NEVER LeftCleanly: Fatal on attempt 0, Reconnect on retry.
    assert!(matches!(
        classify_signal_close(false, false),
        LoopExit::Fatal(_)
    ));
    assert!(matches!(
        classify_signal_close(false, true),
        LoopExit::Reconnect {
            was_connected: false,
            ..
        }
    ));
    assert!(matches!(
        classify_signal_close(true, false),
        LoopExit::Reconnect {
            was_connected: true,
            ..
        }
    ));
}

#[tokio::test]
async fn huddle_handle_leave_and_wait_drop_and_session_done_guard() {
    // 1. leave_and_wait sends Leave and returns as soon as SessionDoneGuard drops.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let finished = Arc::new(AtomicBool::new(false));
    let done = Arc::new(tokio::sync::Notify::new());
    let handle = HuddleHandle::new(tx, Arc::clone(&finished), Arc::clone(&done));
    let guard = SessionDoneGuard {
        finished: Arc::clone(&finished),
        done: Arc::clone(&done),
    };
    let worker = tokio::spawn(async move {
        let _g = guard;
        let ctrl = rx.recv().await;
        assert!(matches!(ctrl, Some(HuddleControl::Leave)));
    });
    handle.leave_and_wait(Duration::from_secs(2)).await;
    assert!(finished.load(Ordering::Acquire));
    worker.await.unwrap();
    // Calling leave_and_wait again after already finished returns immediately.
    handle.leave_and_wait(Duration::from_millis(10)).await;

    // 2. Dropping HuddleHandle sends Leave automatically.
    let (tx2, mut rx2) = mpsc::unbounded_channel();
    let handle2 = HuddleHandle::new(
        tx2,
        Arc::new(AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    );
    drop(handle2);
    assert!(matches!(rx2.recv().await, Some(HuddleControl::Leave)));
}

struct ScriptedRunner {
    exits: VecDeque<LoopExit>,
    observed: Vec<(u32, Duration, usize)>,
    buffered_seen: Vec<Vec<String>>,
    start: tokio::time::Instant,
}

impl ScriptedRunner {
    fn new(exits: Vec<LoopExit>) -> Self {
        Self {
            exits: exits.into(),
            observed: Vec::new(),
            buffered_seen: Vec::new(),
            start: tokio::time::Instant::now(),
        }
    }
}

impl ReconnectRunner for ScriptedRunner {
    fn run_attempt<'a>(
        &'a mut self,
        attempt: u32,
        _snap: &'a mut HuddleSessionSnapshot,
        _ctrl_rx: &'a mut mpsc::UnboundedReceiver<HuddleControl>,
        pending_controls: &'a mut VecDeque<HuddleControl>,
    ) -> Pin<Box<dyn Future<Output = LoopExit> + Send + 'a>> {
        let elapsed = self.start.elapsed();
        let names: Vec<String> = pending_controls
            .drain(..)
            .map(|c| format!("{c:?}"))
            .collect();
        self.observed.push((attempt, elapsed, names.len()));
        self.buffered_seen.push(names);
        let next = self.exits.pop_front().unwrap_or(LoopExit::LeftCleanly);
        Box::pin(async move { next })
    }
}

#[tokio::test(start_paused = true)]
async fn reconnect_supervisor_backoff_doubling_and_max_retries_to_failed() {
    let mut runner = ScriptedRunner::new(vec![
        LoopExit::Reconnect {
            err: HuddleError::Signaling("drop 0".into()),
            was_connected: true,
        },
        LoopExit::Reconnect {
            err: HuddleError::Signaling("retry 1 failed".into()),
            was_connected: false,
        },
        LoopExit::Reconnect {
            err: HuddleError::Signaling("retry 2 failed".into()),
            was_connected: false,
        },
        LoopExit::Reconnect {
            err: HuddleError::UdpBlocked,
            was_connected: false,
        },
    ]);
    let mut snap = HuddleSessionSnapshot::default();
    let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
    let (upd_tx, mut upd_rx) = mpsc::unbounded_channel();

    run_reconnect_supervisor(&mut runner, &mut snap, &mut ctrl_rx, &upd_tx).await;

    // Initial attempt at t=0, retries at +1s (t=1s), +2s (t=3s), +4s (t=7s).
    assert_eq!(runner.observed.len(), 4);
    assert_eq!(runner.observed[0].0, 0);
    assert_eq!(runner.observed[0].1, Duration::from_secs(0));
    assert_eq!(runner.observed[1].0, 1);
    assert_eq!(runner.observed[1].1, Duration::from_secs(1));
    assert_eq!(runner.observed[2].0, 2);
    assert_eq!(runner.observed[2].1, Duration::from_secs(3));
    assert_eq!(runner.observed[3].0, 3);
    assert_eq!(runner.observed[3].1, Duration::from_secs(7));

    assert_eq!(snap.phase, HuddlePhase::Failed);
    assert!(snap.udp_blocked);
    assert!(snap.error.is_some());

    let mut phases = Vec::new();
    while let Ok(HuddleUpdate::State(s)) = upd_rx.try_recv() {
        phases.push(s.phase);
    }
    assert_eq!(
        phases,
        vec![
            HuddlePhase::Reconnecting,
            HuddlePhase::Reconnecting,
            HuddlePhase::Reconnecting,
            HuddlePhase::Failed,
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn reconnect_supervisor_resets_attempt_counter_after_healthy_session() {
    let mut runner = ScriptedRunner::new(vec![
        // First drop -> attempt 1 (1s)
        LoopExit::Reconnect {
            err: HuddleError::Signaling("drop1".into()),
            was_connected: true,
        },
        // Retry 1 fails -> attempt 2 (2s)
        LoopExit::Reconnect {
            err: HuddleError::Signaling("fail1".into()),
            was_connected: false,
        },
        // Retry 2 connects cleanly, then drops later -> resets attempt to 0 -> next is attempt 1 (1s)!
        LoopExit::Reconnect {
            err: HuddleError::Signaling("drop2".into()),
            was_connected: true,
        },
        // Retry 1 after reset fails -> attempt 2 (2s)
        LoopExit::Reconnect {
            err: HuddleError::Signaling("fail2".into()),
            was_connected: false,
        },
        // Retry 2 after reset fails -> attempt 3 (4s)
        LoopExit::Reconnect {
            err: HuddleError::Signaling("fail3".into()),
            was_connected: false,
        },
        // Retry 3 succeeds and user leaves cleanly.
        LoopExit::LeftCleanly,
    ]);
    let mut snap = HuddleSessionSnapshot::default();
    let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
    let (upd_tx, _upd_rx) = mpsc::unbounded_channel();

    run_reconnect_supervisor(&mut runner, &mut snap, &mut ctrl_rx, &upd_tx).await;

    let attempts: Vec<u32> = runner.observed.iter().map(|(a, _, _)| *a).collect();
    let times: Vec<u64> = runner
        .observed
        .iter()
        .map(|(_, d, _)| d.as_secs())
        .collect();
    assert_eq!(attempts, vec![0, 1, 2, 1, 2, 3]);
    // Cumulative backoffs: 0, +1=1, +2=3, +1=4, +2=6, +4=10.
    assert_eq!(times, vec![0, 1, 3, 4, 6, 10]);
    assert_eq!(snap.phase, HuddlePhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn reconnect_supervisor_buffers_non_leave_controls_and_preserves_full_backoff() {
    let mut runner = ScriptedRunner::new(vec![
        LoopExit::Reconnect {
            err: HuddleError::Signaling("drop".into()),
            was_connected: true,
        },
        LoopExit::LeftCleanly,
    ]);
    let mut snap = HuddleSessionSnapshot::default();
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
    let (upd_tx, _upd_rx) = mpsc::unbounded_channel();

    // Send non-Leave controls during the 1s backoff sleep.
    let sender = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = ctrl_tx.send(HuddleControl::SetMuted(true));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let _ = ctrl_tx.send(HuddleControl::SetTopic("Standup".into()));
        // Keep sender alive past the 1s backoff so channel doesn't close early.
        tokio::time::sleep(Duration::from_secs(5)).await;
    });

    run_reconnect_supervisor(&mut runner, &mut snap, &mut ctrl_rx, &upd_tx).await;
    sender.abort();

    // Backoff must NOT terminate early when non-Leave controls arrive!
    assert_eq!(runner.observed.len(), 2);
    assert_eq!(runner.observed[1].1, Duration::from_secs(1));
    assert_eq!(runner.buffered_seen[1].len(), 2);
    assert!(runner.buffered_seen[1][0].contains("SetMuted(true)"));
    assert!(runner.buffered_seen[1][1].contains("SetTopic(\"Standup\")"));
    assert_eq!(snap.phase, HuddlePhase::Idle);
}

#[tokio::test(start_paused = true)]
async fn reconnect_supervisor_leave_cancels_backoff_immediately() {
    let mut runner = ScriptedRunner::new(vec![
        LoopExit::Reconnect {
            err: HuddleError::Signaling("drop".into()),
            was_connected: true,
        },
        LoopExit::Reconnect {
            err: HuddleError::Signaling("fail1".into()),
            was_connected: false,
        },
    ]);
    let mut snap = HuddleSessionSnapshot::default();
    let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
    let (upd_tx, _upd_rx) = mpsc::unbounded_channel();

    let start = tokio::time::Instant::now();
    tokio::spawn(async move {
        // Let attempt 1 finish (t=1s), then cancel 300ms into the 2s backoff (t=1.3s).
        tokio::time::sleep(Duration::from_millis(1300)).await;
        let _ = ctrl_tx.send(HuddleControl::Leave);
    });

    run_reconnect_supervisor(&mut runner, &mut snap, &mut ctrl_rx, &upd_tx).await;

    assert_eq!(runner.observed.len(), 2);
    assert_eq!(start.elapsed(), Duration::from_millis(1300));
    assert_eq!(snap.phase, HuddlePhase::Idle);
    assert!(snap.error.is_none());
}

#[test]
fn remote_video_decoder_h264_propagates_seq_gaps_and_keyframe_requests() {
    let mut enc = crate::video::H264VideoEncoder::new_camera().unwrap();
    let (w, h) = (96usize, 64usize);
    let rgb = vec![120u8; w * h * 3];
    let annex_b = enc.encode_rgb(&rgb, w, h).unwrap();

    // Extract NALs and pack into a single STAP-A packet for deterministic test delivery.
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= annex_b.len() {
        if annex_b[i] == 0 && annex_b[i + 1] == 0 {
            if i + 4 <= annex_b.len() && annex_b[i + 2] == 0 && annex_b[i + 3] == 1 {
                starts.push((i, 4));
                i += 4;
                continue;
            }
            if annex_b[i + 2] == 1 {
                starts.push((i, 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    let mut stap_a = vec![24u8];
    for (idx, &(pos, sc)) in starts.iter().enumerate() {
        let end = starts.get(idx + 1).map_or(annex_b.len(), |&(n, _)| n);
        let nal = &annex_b[pos + sc..end];
        if !nal.is_empty() {
            stap_a.extend_from_slice(&(nal.len() as u16).to_be_bytes());
            stap_a.extend_from_slice(nal);
        }
    }

    let mut dec = RemoteVideoDecoder::for_mime("video/H264").expect("H.264 decoder");
    let perf = HuddlePerfCounters::default();
    let make_pkt = |seq: u16, payload: Vec<u8>, marker: bool| InboundVideoPacket {
        ssrc: 42,
        mid: "2".into(),
        track_id: "v0".into(),
        mime_type: "video/H264".into(),
        seq,
        payload,
        marker,
    };

    // Packet seq 10: clean STAP-A keyframe -> decodes, zero gaps, no keyframe request.
    let p10 = make_pkt(10, stap_a.clone(), true);
    perf.record_rx_packet(p10.payload.len() as u64);
    let r1 = dec.push(&p10);
    assert!(r1.frame.is_some());
    assert_eq!(r1.seq_gaps_delta, 0);
    assert!(!r1.needs_keyframe);

    // Packet seq 15 (4 missing packets: 11..=14) with STAP-A -> reports 4 missing packets,
    // flags needs_keyframe, and records accurately in perf.
    let p15 = make_pkt(15, stap_a, true);
    perf.record_rx_packet(p15.payload.len() as u64);
    let r2 = dec.push(&p15);
    assert!(r2.frame.is_some());
    assert_eq!(r2.seq_gaps_delta, 4);
    assert!(r2.needs_keyframe);
    perf.record_rx_seq_gap(r2.seq_gaps_delta);

    let snap = perf.sample(1.0, 10, 100);
    assert_eq!(snap.rx_seq_gaps, 4);
    // 2 rx packets + 4 missing packets -> 4/6 = 66.67% loss -> degraded!
    assert!((snap.rx_loss_pct - 66.6667).abs() < 0.1);
    assert!(snap.is_degraded());
}
