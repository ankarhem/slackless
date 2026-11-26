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
    // Unmatched mid and unmatched SSRC must NOT hijack tile 0.
    assert_eq!(
        resolve_remote_video_tile(999, "99", &ssrc_map, &sources),
        None
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
    let mut stap_a = vec![24u8];
    for nal in crate::video::split_annex_b_nals(&annex_b) {
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

// ---------------------------------------------------------------------------
// Full session state-machine tests via `SignalSink` / `RtcController` / `MediaDevices`
// ---------------------------------------------------------------------------

mod state_machine {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;

    use crate::audio::AudioSharedState;
    use crate::peer_message::{
        build_jukebox_message, build_mute_request_message, build_reacji_message,
    };
    use crate::proto::{
        SdkAudioAttendeeState, SdkAudioMetadataFrame, SdkAudioStreamIdInfo,
        SdkAudioStreamIdInfoFrame, SdkDataMessageFrame, SdkDataMessagePayload, SdkErrorFrame,
        SdkIndexFrame, SdkPingPongFrame, SdkPingPongType, SdkSignalFrame, SdkStreamDescriptor,
        SdkStreamMediaType, SdkSubscribeAckFrame, SdkTrackMapping, sdk_signal_frame,
    };
    use crate::session::connected::SessionRuntime;
    use crate::session::controls::{apply_background_done, handle_control};
    use crate::session::io::{
        AudioSwitch, BackgroundDone, MediaDevices, RtcController, SignalSink,
    };
    use crate::session::signaling_handlers::handle_signal_frame;
    use crate::session::video_rx::VideoRoutingTable;
    use crate::video::VideoFrame;
    use crate::{
        HuddleAudioConfig, HuddleCredentials, HuddleDrawMessage, HuddleDrawSegment,
        HuddleReactionKind,
    };
    use slackless_types::ChannelId;

    #[derive(Default)]
    struct FakeSignal {
        sent: Vec<SdkSignalFrame>,
        pongs_replied: u32,
        fail_send: bool,
    }

    impl SignalSink for FakeSignal {
        fn send_frame<'a>(
            &'a mut self,
            frame: &'a SdkSignalFrame,
        ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 'a>> {
            Box::pin(async move {
                if self.fail_send {
                    return Err(HuddleError::Signaling("injected send error".into()));
                }
                self.sent.push(frame.clone());
                Ok(())
            })
        }

        fn reply_ping_if_needed<'a>(
            &'a mut self,
            pp: &'a SdkPingPongFrame,
        ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
            Box::pin(async move {
                if pp.r#type == Some(SdkPingPongType::Ping as i32) {
                    self.pongs_replied += 1;
                    true
                } else {
                    false
                }
            })
        }
    }

    #[derive(Default)]
    struct FakeRtc {
        remote_answers: Mutex<Vec<String>>,
        offer_calls: Mutex<u32>,
        video_receive_requests: Mutex<Vec<usize>>,
        fail_prepare_video: bool,
    }

    impl RtcController for FakeRtc {
        fn set_remote_answer<'a>(
            &'a self,
            answer: String,
        ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send + 'a>> {
            Box::pin(async move {
                self.remote_answers.lock().unwrap().push(answer);
                Ok(())
            })
        }

        fn create_gathered_offer<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = crate::Result<String>> + Send + 'a>> {
            Box::pin(async move {
                let mut calls = self.offer_calls.lock().unwrap();
                *calls += 1;
                Ok(format!("v=0\r\ns=offer-{}\r\n", *calls))
            })
        }

        fn prepare_video_receive_offer<'a>(
            &'a self,
            count: usize,
        ) -> Pin<Box<dyn Future<Output = crate::Result<(String, Vec<String>)>> + Send + 'a>>
        {
            Box::pin(async move {
                self.video_receive_requests.lock().unwrap().push(count);
                if self.fail_prepare_video {
                    return Err(HuddleError::WebRtc("injected prepare_video error".into()));
                }
                let mids: Vec<String> = (0..count).map(|i| (i + 2).to_string()).collect();
                Ok(("v=0\r\ns=video-rx-offer\r\n".to_owned(), mids))
            })
        }
    }

    /// Scripted audio restarts, shared with the device switches [`FakeMedia`] hands out (a failed
    /// switch drops the engine, so the script cannot live in it).
    #[derive(Default)]
    struct FakeAudioScript {
        /// Outcomes for successive restart attempts (`true` = ok, `false` = err).
        results: VecDeque<bool>,
        /// Input device of every restart attempt, in order.
        starts: Vec<Option<String>>,
    }

    struct FakeMedia {
        /// Token representing the active inbound audio stream (`Some(id)` while running).
        running_audio: Option<u64>,
        audio_script: Arc<Mutex<FakeAudioScript>>,
        camera_active: bool,
        camera_blur: bool,
        camera_compact: bool,
        fail_camera: bool,
        screen_share_active: Option<String>,
        fail_screen_share: bool,
        jukebox_history: Vec<(Option<String>, Option<String>)>,
    }

    impl Default for FakeMedia {
        fn default() -> Self {
            Self {
                running_audio: Some(1),
                audio_script: Arc::default(),
                camera_active: false,
                camera_blur: false,
                camera_compact: false,
                fail_camera: false,
                screen_share_active: None,
                fail_screen_share: false,
                jukebox_history: Vec::new(),
            }
        }
    }

    impl FakeMedia {
        fn audio_starts(&self) -> Vec<Option<String>> {
            self.audio_script.lock().unwrap().starts.clone()
        }
    }

    impl MediaDevices for FakeMedia {
        type InboundAudio = u64;
        /// The running inbound stream token.
        type Audio = u64;
        type Camera = ();
        /// Loopback device feeding the share's audio.
        type ScreenShare = Option<String>;

        fn stop_audio(&mut self) {
            self.running_audio = None;
        }

        fn switch_audio(
            &mut self,
            new: HuddleAudioConfig,
            prev: HuddleAudioConfig,
            parked: Option<u64>,
        ) -> Pin<Box<dyn Future<Output = AudioSwitch<u64, u64>> + Send>> {
            // Stopping the engine hands back its inbound stream.
            let inbound = self.running_audio.take().or(parked);
            let script = Arc::clone(&self.audio_script);
            Box::pin(async move {
                let Some(inbound) = inbound else {
                    return AudioSwitch::Failed {
                        err: HuddleError::Audio("inbound audio stream lost".into()),
                        revert_err: None,
                        inbound: None,
                    };
                };
                let mut script = script.lock().unwrap();
                script.starts.push(new.input_device);
                if script.results.pop_front().unwrap_or(true) {
                    return AudioSwitch::Switched(inbound);
                }
                script.starts.push(prev.input_device);
                if script.results.pop_front().unwrap_or(true) {
                    return AudioSwitch::Reverted {
                        audio: inbound,
                        err: HuddleError::Audio("device unavailable".to_owned()),
                    };
                }
                AudioSwitch::Failed {
                    err: HuddleError::Audio("device unavailable".to_owned()),
                    revert_err: Some(HuddleError::Audio("previous device unavailable".to_owned())),
                    inbound: Some(inbound),
                }
            })
        }

        fn install_audio(&mut self, audio: u64) {
            self.running_audio = Some(audio);
        }

        fn start_camera(
            &mut self,
            _device_index: Option<usize>,
            blur: bool,
            compact: bool,
            _outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
            _local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
        ) -> Pin<Box<dyn Future<Output = crate::Result<()>> + Send>> {
            if self.fail_camera {
                return Box::pin(async {
                    Err(HuddleError::Video("camera permission denied".into()))
                });
            }
            self.camera_active = true;
            self.camera_blur = blur;
            self.camera_compact = compact;
            Box::pin(async { Ok(()) })
        }

        fn install_camera(&mut self, _camera: ()) {}

        fn stop_camera(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            self.camera_active = false;
            Box::pin(async {})
        }

        fn set_camera_blur(&mut self, blur: bool) {
            if self.camera_active {
                self.camera_blur = blur;
            }
        }

        fn set_camera_compact(&mut self, compact: bool) {
            if self.camera_active {
                self.camera_compact = compact;
            }
        }

        fn force_camera_keyframe(&mut self) {}

        fn set_camera_target_bitrate(&mut self, _bps: u32) {}

        fn start_screen_share(
            &mut self,
            _creds: HuddleCredentials,
            source_id: String,
            share_audio: bool,
            _local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
            _perf: Arc<HuddlePerfCounters>,
        ) -> Pin<Box<dyn Future<Output = crate::Result<Self::ScreenShare>> + Send>> {
            if self.fail_screen_share {
                return Box::pin(async { Err(HuddleError::Video("screen capture denied".into())) });
            }
            self.screen_share_active = Some(source_id);
            Box::pin(async move { Ok(share_audio.then(|| "System Audio Tap".to_owned())) })
        }

        fn screen_share_loopback(share: &Self::ScreenShare) -> Option<String> {
            share.clone()
        }

        fn install_screen_share(&mut self, _share: Option<String>) {}

        fn stop_screen_share(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            self.screen_share_active = None;
            Box::pin(async {})
        }

        fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>) {
            self.jukebox_history
                .push((output_device, song_key.map(ToOwned::to_owned)));
        }
    }

    /// Let device operations spawned by `handle_control` report back and apply their results
    /// (an audio switch may chain straight into another one).
    async fn settle_background(
        rt: &mut SessionRuntime<'_, FakeMedia>,
        bg_rx: &mut mpsc::UnboundedReceiver<BackgroundDone<FakeMedia>>,
        config: &mut HuddleAudioConfig,
        snap: &mut HuddleSessionSnapshot,
        rtc: &FakeRtc,
        sig: &mut FakeSignal,
    ) {
        loop {
            tokio::task::yield_now().await;
            match bg_rx.try_recv() {
                Ok(done) => apply_background_done(done, config, snap, rtc, sig, rt).await,
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    unreachable!("runtime holds a sender")
                }
            }
        }
    }

    fn test_creds() -> HuddleCredentials {
        HuddleCredentials {
            channel: ChannelId::from("C0HUDDLE"),
            call_id: "R0CALL".into(),
            meeting_id: "M0MEET".into(),
            attendee_id: "att-self".into(),
            external_user_id: "T0TEAM-R0CALL-U0SELF".into(),
            join_token: "tok-secret".into(),
            signaling_url: "wss://example.invalid/control".into(),
            audio_host_url: "audio.example.invalid:3478".into(),
            turn_control_url: "https://example.invalid/turn".into(),
            thread_root_ts: None,
            canvas_file_id: None,
            huddle_link: None,
        }
    }

    fn make_runtime<'a>(
        creds: &'a HuddleCredentials,
        updates: &'a mpsc::UnboundedSender<HuddleUpdate>,
        media: FakeMedia,
    ) -> (
        SessionRuntime<'a, FakeMedia>,
        mpsc::UnboundedReceiver<BackgroundDone<FakeMedia>>,
    ) {
        let (outbound_h264_tx, _h264_rx) = mpsc::unbounded_channel();
        let (local_cam_tx, _cam_rx) = mpsc::unbounded_channel();
        let (local_screen_tx, _scr_rx) = mpsc::unbounded_channel();
        let (bg_done_tx, bg_done_rx) = mpsc::unbounded_channel();
        let rt = SessionRuntime {
            creds,
            updates,
            wants_compressed: false,
            self_user: UserId::from("U0SELF"),
            self_content_attendee: format!("{}#content", creds.attendee_id),
            media,
            parked_inbound_audio: None,
            audio_switch_in_flight: false,
            camera_gen: 0,
            shared: AudioSharedState::from_config(&HuddleAudioConfig::default()),
            remote_sources: Vec::new(),
            active_remote_tiles: HashSet::new(),
            stream_users: HashMap::new(),
            attendee_to_user: HashMap::new(),
            muted_peers: HashSet::new(),
            remote_speakers: Vec::new(),
            ssrc_to_stream_id: HashMap::new(),
            last_sdp_answer: None,
            vdec_routing: Arc::new(Mutex::new(VideoRoutingTable::default())),
            vdec_audio_only: Arc::new(AtomicBool::new(false)),
            vdec_clear: Arc::new(AtomicBool::new(false)),
            vdec_shutdown: Arc::new(AtomicBool::new(false)),
            perf: Arc::new(HuddlePerfCounters::default()),
            outbound_h264_tx,
            local_cam_tx,
            local_screen_tx,
            bg_done_tx,
        };
        (rt, bg_done_rx)
    }

    fn drain_updates(rx: &mut mpsc::UnboundedReceiver<HuddleUpdate>) -> Vec<HuddleUpdate> {
        let mut out = Vec::new();
        while let Ok(u) = rx.try_recv() {
            out.push(u);
        }
        out
    }

    #[tokio::test]
    async fn audio_device_hotswap_reverts_on_failure_and_recovers_parked_rx_on_double_failure() {
        let creds = test_creds();
        let (tx, _rx) = mpsc::unbounded_channel();
        let media = FakeMedia::default();
        // 1) Switch to BrokenMic fails, revert to Default succeeds.
        // 2) Switch to BadMic2 fails AND revert to Default also fails -> inbound rx parked!
        // 3) Subsequent switch to UsbMic succeeds using the parked inbound rx!
        media
            .audio_script
            .lock()
            .unwrap()
            .results
            .extend([false, true, false, false, true]);
        let (mut rt, mut bg_rx) = make_runtime(&creds, &tx, media);
        let mut config = HuddleAudioConfig {
            input_device: Some("Built-in Mic".into()),
            output_device: Some("Built-in Speakers".into()),
            ..Default::default()
        };
        let mut snap = HuddleSessionSnapshot {
            input_device: config.input_device.clone(),
            output_device: config.output_device.clone(),
            ..Default::default()
        };
        let rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        // Step 1: Fail -> clean revert.
        let exit = handle_control(
            Some(HuddleControl::SetAudioDevices {
                input: Some("BrokenMic".into()),
                output: Some("Built-in Speakers".into()),
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(exit.is_none());
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert_eq!(config.input_device.as_deref(), Some("Built-in Mic"));
        assert_eq!(snap.input_device.as_deref(), Some("Built-in Mic"));
        assert!(
            snap.error
                .as_deref()
                .unwrap()
                .contains("device unavailable")
        );
        assert_eq!(rt.media.running_audio, Some(1));
        assert!(rt.parked_inbound_audio.is_none());

        // Step 2: Fail + revert also fails -> inbound rx is parked (NOT dropped!).
        handle_control(
            Some(HuddleControl::SetAudioDevices {
                input: Some("BadMic2".into()),
                output: Some("BadOut2".into()),
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert_eq!(config.input_device.as_deref(), Some("Built-in Mic"));
        assert_eq!(rt.media.running_audio, None);
        assert_eq!(
            rt.parked_inbound_audio,
            Some(1),
            "inbound audio stream must be preserved for a later retry"
        );

        // Step 3: Next device switch picks up the parked stream and clears the error.
        handle_control(
            Some(HuddleControl::SetAudioDevices {
                input: Some("USB Mic".into()),
                output: Some("Headphones".into()),
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert_eq!(config.input_device.as_deref(), Some("USB Mic"));
        assert_eq!(snap.output_device.as_deref(), Some("Headphones"));
        assert!(snap.error.is_none(), "error must clear after recovery");
        assert_eq!(rt.media.running_audio, Some(1));
        assert!(rt.parked_inbound_audio.is_none());
        assert_eq!(
            rt.media
                .audio_starts()
                .iter()
                .map(Option::as_deref)
                .collect::<Vec<_>>(),
            [
                Some("BrokenMic"),
                Some("Built-in Mic"),
                Some("BadMic2"),
                Some("Built-in Mic"),
                Some("USB Mic"),
            ]
        );
    }

    /// A device change requested while an earlier switch still owns the hardware is applied as
    /// soon as that switch reports back, ending on the requested devices.
    #[tokio::test]
    async fn audio_device_switch_requested_during_a_switch_is_chained() {
        let creds = test_creds();
        let (tx, _rx) = mpsc::unbounded_channel();
        let (mut rt, mut bg_rx) = make_runtime(&creds, &tx, FakeMedia::default());
        let mut config = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        handle_control(
            Some(HuddleControl::SetAudioDevices {
                input: Some("First".into()),
                output: None,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        // The first switch owns the audio hardware until it reports back.
        handle_control(
            Some(HuddleControl::SetAudioDevices {
                input: Some("Second".into()),
                output: None,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;

        assert_eq!(config.input_device.as_deref(), Some("Second"));
        assert_eq!(snap.input_device.as_deref(), Some("Second"));
        assert_eq!(rt.media.running_audio, Some(1));
        assert_eq!(
            rt.media
                .audio_starts()
                .iter()
                .map(Option::as_deref)
                .collect::<Vec<_>>(),
            [Some("First"), Some("Second")]
        );
    }

    #[tokio::test]
    async fn index_suback_audio_only_and_late_joiner_screenshare_protocol_flow() {
        let creds = test_creds();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (mut rt, mut bg_rx) = make_runtime(&creds, &tx, FakeMedia::default());
        let mut config = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        // 1. Start camera, then start screen share -> camera becomes compact + draw capabilities sent.
        handle_control(
            Some(HuddleControl::SetCamera {
                enabled: true,
                device_index: Some(0),
                blur: true,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(snap.video);
        assert!(rt.media.camera_active);
        assert!(rt.media.camera_blur);
        assert!(!rt.media.camera_compact);
        assert_eq!(
            sig.sent.last().and_then(|f| f.r#type),
            Some(sdk_signal_frame::Type::Subscribe as i32)
        );

        handle_control(
            Some(HuddleControl::SetScreenShare {
                source_id: Some("monitor:1".into()),
                share_audio: true,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(snap.sharing);
        assert_eq!(snap.share_source_id.as_deref(), Some("monitor:1"));
        assert_eq!(
            snap.screen_audio_device.as_deref(),
            Some("System Audio Tap")
        );
        assert!(
            rt.media.camera_compact,
            "camera must switch to compact during screen share"
        );
        let sent_before_join = sig.sent.len();

        // 2. Late peer U0ALICE joins via AUDIO_STREAM_ID_INFO -> must re-announce draw capabilities!
        let join_frame = SdkSignalFrame {
            audio_stream_id_info: Some(SdkAudioStreamIdInfoFrame {
                streams: vec![SdkAudioStreamIdInfo {
                    audio_stream_id: Some(7),
                    attendee_id: Some("att-alice".into()),
                    external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                    muted: Some(false),
                    dropped: Some(false),
                }],
            }),
            ..Default::default()
        };
        assert!(
            handle_signal_frame(join_frame, &mut config, &mut snap, &rtc, &mut sig, &mut rt)
                .await
                .is_none()
        );
        assert_eq!(
            sig.sent.len(),
            sent_before_join + 2,
            "late joiner while sharing must trigger capabilities + content-share-details frames"
        );
        assert_eq!(rt.stream_users.get(&7), Some(&UserId::from("U0ALICE")));

        // 3. INDEX advertises self camera (filtered), self #content (filtered), Alice cam + Alice screen.
        let index_frame = SdkSignalFrame {
            index: Some(SdkIndexFrame {
                sources: vec![
                    SdkStreamDescriptor {
                        stream_id: Some(1),
                        group_id: Some(1),
                        attendee_id: Some("att-self".into()),
                        external_user_id: Some("T0TEAM-R0CALL-U0SELF".into()),
                        media_type: Some(SdkStreamMediaType::Video as i32),
                        max_bitrate_kbps: Some(1200),
                        ..Default::default()
                    },
                    SdkStreamDescriptor {
                        stream_id: Some(2),
                        group_id: Some(2),
                        attendee_id: Some("att-self#content".into()),
                        external_user_id: Some("T0TEAM-R0CALL-U0SELF".into()),
                        media_type: Some(SdkStreamMediaType::Video as i32),
                        max_bitrate_kbps: Some(1500),
                        ..Default::default()
                    },
                    SdkStreamDescriptor {
                        stream_id: Some(10),
                        group_id: Some(10),
                        attendee_id: Some("att-alice".into()),
                        external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                        media_type: Some(SdkStreamMediaType::Video as i32),
                        max_bitrate_kbps: Some(1000),
                        ..Default::default()
                    },
                    SdkStreamDescriptor {
                        stream_id: Some(20),
                        group_id: Some(20),
                        attendee_id: Some("att-alice#content".into()),
                        external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                        media_type: Some(SdkStreamMediaType::Video as i32),
                        max_bitrate_kbps: Some(2000),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(index_frame, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert_eq!(rt.remote_sources.len(), 2);
        assert_eq!(
            rt.vdec_routing.lock().unwrap().remote_sources.len(),
            2,
            "decoder routing table must mirror subscribed remote sources"
        );
        assert!(
            rt.active_remote_tiles
                .contains(&VideoTileKey::RemoteCamera(UserId::from("U0ALICE")))
        );
        assert!(
            rt.active_remote_tiles
                .contains(&VideoTileKey::RemoteScreen(UserId::from("U0ALICE")))
        );

        // 4. SUBACK updates SSRC->stream_id map & applies remote SDP answer.
        let suback_frame = SdkSignalFrame {
            suback: Some(SdkSubscribeAckFrame {
                sdp_answer: Some("v=0\r\ns=answer-1\r\n".into()),
                tracks: vec![
                    SdkTrackMapping {
                        stream_id: Some(10),
                        ssrc: Some(1001),
                        ..Default::default()
                    },
                    SdkTrackMapping {
                        stream_id: Some(20),
                        ssrc: Some(2002),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(
            suback_frame,
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(rt.ssrc_to_stream_id.get(&1001), Some(&10));
        assert_eq!(
            rt.vdec_routing.lock().unwrap().ssrc_to_stream_id.get(&2002),
            Some(&20)
        );
        assert_eq!(
            rtc.remote_answers.lock().unwrap().as_slice(),
            ["v=0\r\ns=answer-1\r\n"]
        );

        // 5. Enabling AudioOnly stops local camera, unsubscribes all remote mids, clears decoders,
        // and emits VideoTileClosed for local camera + both remote tiles.
        let _ = drain_updates(&mut rx);
        handle_control(
            Some(HuddleControl::SetAudioOnly(true)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.audio_only);
        assert!(!snap.video);
        assert!(!rt.media.camera_active);
        assert!(rt.remote_sources.is_empty());
        assert!(rt.active_remote_tiles.is_empty());
        assert!(rt.vdec_audio_only.load(Ordering::Relaxed));
        assert!(rt.vdec_clear.load(Ordering::Relaxed));
        assert_eq!(
            sig.sent.last().and_then(|f| f.r#type),
            Some(sdk_signal_frame::Type::RemoteVideoUpdate as i32)
        );
        let closed_tiles: HashSet<VideoTileKey> = drain_updates(&mut rx)
            .into_iter()
            .filter_map(|u| match u {
                HuddleUpdate::VideoTileClosed(tile) => Some(tile),
                _ => None,
            })
            .collect();
        assert!(closed_tiles.contains(&VideoTileKey::LocalCamera));
        assert!(closed_tiles.contains(&VideoTileKey::RemoteCamera(UserId::from("U0ALICE"))));
        assert!(closed_tiles.contains(&VideoTileKey::RemoteScreen(UserId::from("U0ALICE"))));
    }

    #[tokio::test]
    async fn signal_ping_audio_metadata_data_messages_and_protocol_error_behave_deterministically()
    {
        let creds = test_creds();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (mut rt, _bg_rx) = make_runtime(&creds, &tx, FakeMedia::default());
        let mut config = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        // Register two peers in stream_users.
        rt.stream_users.insert(1, UserId::from("U0ALICE"));
        rt.stream_users.insert(2, UserId::from("U0BOB"));
        rt.stream_users.insert(3, UserId::from("U0CAROL"));

        // PING -> replies PONG & increments pongs_sent; AUDIO_METADATA -> sorts active speakers & tracks muted.
        let frame = SdkSignalFrame {
            ping_pong: Some(SdkPingPongFrame {
                r#type: Some(SdkPingPongType::Ping as i32),
                ping_id: Some(99),
            }),
            audio_metadata: Some(SdkAudioMetadataFrame {
                attendee_states: vec![
                    SdkAudioAttendeeState {
                        audio_stream_id: Some(1),
                        volume: Some(30),
                        muted: Some(false),
                        ..Default::default()
                    },
                    SdkAudioAttendeeState {
                        audio_stream_id: Some(2),
                        volume: Some(80),
                        muted: Some(false),
                        ..Default::default()
                    },
                    SdkAudioAttendeeState {
                        audio_stream_id: Some(3),
                        volume: Some(95),
                        muted: Some(true),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };
        assert!(
            handle_signal_frame(frame, &mut config, &mut snap, &rtc, &mut sig, &mut rt)
                .await
                .is_none()
        );
        assert_eq!(snap.pongs_sent, 1);
        assert_eq!(sig.pongs_replied, 1);
        assert!(rt.muted_peers.contains(&UserId::from("U0CAROL")));
        assert_eq!(rt.remote_speakers.len(), 2);
        assert_eq!(rt.remote_speakers[0].0, UserId::from("U0BOB"));
        assert_eq!(rt.remote_speakers[1].0, UserId::from("U0ALICE"));

        // Remote MutedByPeer + Jukebox data messages force local mute & start jukebox.
        let mute_req = build_mute_request_message("att-alice", "att-self");
        let jukebox = build_jukebox_message("att-alice", Some("lofi"));
        let reacji = build_reacji_message("att-alice", "tada", HuddleReactionKind::Standard, None);
        let dm_frame = SdkSignalFrame {
            data_message: Some(SdkDataMessageFrame {
                messages: vec![
                    SdkDataMessagePayload {
                        topic: Some(SLACK_DATA_CHANNEL_TOPIC.into()),
                        data: Some(mute_req),
                        sender_attendee_id: Some("att-alice".into()),
                        sender_external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                        ingest_time_ns: Some(10_000_000),
                        ..Default::default()
                    },
                    SdkDataMessagePayload {
                        topic: Some(SLACK_DATA_CHANNEL_TOPIC.into()),
                        data: Some(jukebox),
                        sender_attendee_id: Some("att-alice".into()),
                        sender_external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                        ingest_time_ns: Some(20_000_000),
                        ..Default::default()
                    },
                    SdkDataMessagePayload {
                        topic: Some(SLACK_DATA_CHANNEL_TOPIC.into()),
                        data: Some(reacji),
                        sender_attendee_id: Some("att-alice".into()),
                        sender_external_user_id: Some("T0TEAM-R0CALL-U0ALICE".into()),
                        ingest_time_ns: Some(30_000_000),
                        ..Default::default()
                    },
                ],
            }),
            ..Default::default()
        };
        handle_signal_frame(dm_frame, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(config.muted && snap.muted && rt.shared.muted.load(Ordering::Relaxed));
        assert_eq!(snap.jukebox_song.as_deref(), Some("lofi"));
        assert_eq!(rt.media.jukebox_history.len(), 1);
        assert!(
            sig.sent
                .iter()
                .any(|f| f.r#type == Some(sdk_signal_frame::Type::AudioControl as i32)),
            "MutedByPeer must send an AudioControl mute frame back to Chime"
        );
        let updates = drain_updates(&mut rx);
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::DataEvent(HuddleDataEvent::Reaction { emoji, .. }) if emoji == "tada"
        )));

        // Interactive controls (HandRaise, Draw, Leave) emit frames & clean exit.
        handle_control(
            Some(HuddleControl::SetHandRaised(true)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.hand_raised);
        assert_eq!(snap.sticky_emoji.as_deref(), Some("raised_hand"));

        handle_control(
            Some(HuddleControl::SendDraw(HuddleDrawMessage::New {
                item_id: "stroke-1".into(),
                segment: HuddleDrawSegment {
                    a: [0.1, 0.1],
                    b: [0.2, 0.2],
                    c: [0.3, 0.3],
                    d: [0.4, 0.4],
                },
            })),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;

        let leave_exit = handle_control(
            Some(HuddleControl::Leave),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(matches!(leave_exit, Some(LoopExit::LeftCleanly)));

        // Protocol ERROR frame returns Fatal with exact status & description.
        let err_frame = SdkSignalFrame {
            error: Some(SdkErrorFrame {
                status: Some(403),
                description: Some("meeting ended".into()),
            }),
            ..Default::default()
        };
        let err_exit =
            handle_signal_frame(err_frame, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(matches!(
            err_exit,
            Some(LoopExit::Fatal(HuddleError::Protocol { status: 403, .. }))
        ));
    }

    #[tokio::test]
    async fn every_remaining_control_and_signaling_branch_behaves_consistently() {
        use crate::session::signaling_handlers::apply_audio_stream_info;

        let creds = test_creds();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let media = FakeMedia {
            fail_camera: true,
            fail_screen_share: true,
            ..Default::default()
        };
        let (mut rt, mut bg_rx) = make_runtime(&creds, &tx, media);
        let mut config = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        // 1. SetMuted & SetAudioProcessing update config, atomics, snap, and signal.
        handle_control(
            Some(HuddleControl::SetMuted(true)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(config.muted && snap.muted && rt.shared.muted.load(Ordering::Relaxed));

        handle_control(
            Some(HuddleControl::SetAudioProcessing {
                aec: false,
                ns: false,
                agc: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(!config.aec && !snap.ns && !rt.shared.agc.load(Ordering::Relaxed));

        // 2. Camera start failure & screen-share start failure record errors and keep flags false.
        handle_control(
            Some(HuddleControl::SetCamera {
                enabled: true,
                device_index: Some(1),
                blur: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(!snap.video);
        assert!(snap.error.as_deref().unwrap().contains("camera"));

        handle_control(
            Some(HuddleControl::SetScreenShare {
                source_id: Some("window:42".into()),
                share_audio: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(!snap.sharing);
        assert!(snap.share_source_id.is_none());
        assert!(snap.error.as_deref().unwrap().contains("screen capture"));

        // 3. Now allow camera & screen share: test blur live update, stop share, stop camera.
        rt.media.fail_camera = false;
        rt.media.fail_screen_share = false;
        handle_control(
            Some(HuddleControl::SetCamera {
                enabled: true,
                device_index: Some(0),
                blur: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(snap.video && !rt.media.camera_blur);
        handle_control(
            Some(HuddleControl::SetBlurBackground(true)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.blur_background && rt.media.camera_blur);

        handle_control(
            Some(HuddleControl::SetScreenShare {
                source_id: Some("window:42".into()),
                share_audio: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        settle_background(&mut rt, &mut bg_rx, &mut config, &mut snap, &rtc, &mut sig).await;
        assert!(snap.sharing && rt.media.camera_compact);

        let _ = drain_updates(&mut rx);
        handle_control(
            Some(HuddleControl::SetScreenShare {
                source_id: None,
                share_audio: false,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(!snap.sharing && !rt.media.camera_compact);
        assert!(
            drain_updates(&mut rx)
                .iter()
                .any(|u| matches!(u, HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen)))
        );

        handle_control(
            Some(HuddleControl::SetCamera {
                enabled: false,
                device_index: Some(0),
                blur: true,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(!snap.video && !rt.media.camera_active);
        assert!(
            drain_updates(&mut rx)
                .iter()
                .any(|u| matches!(u, HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera)))
        );

        // 4. SendReaction, SetStickyEmoji, SetJukebox, MutePeer, SetTopic, SetBackground,
        // SetCaptionsEnabled, SetAudioOnly(false), None (channel closed -> LeftCleanly).
        handle_control(
            Some(HuddleControl::SendReaction {
                emoji: "rocket".into(),
                kind: HuddleReactionKind::Effect,
                emoji_url: None,
            }),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        handle_control(
            Some(HuddleControl::SetStickyEmoji(Some("coffee".into()))),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(snap.sticky_emoji.as_deref(), Some("coffee"));
        assert!(!snap.hand_raised);
        handle_control(
            Some(HuddleControl::SetStickyEmoji(Some("raised_hand".into()))),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.hand_raised);

        handle_control(
            Some(HuddleControl::SetJukebox(Some("chill".into()))),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(snap.jukebox_song.as_deref(), Some("chill"));
        handle_control(
            Some(HuddleControl::MutePeer("att-bob".into())),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        handle_control(
            Some(HuddleControl::SetTopic("Sprint triage".into())),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(snap.topic.as_deref(), Some("Sprint triage"));
        handle_control(
            Some(HuddleControl::SetTopic("   ".into())),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.topic.is_none());
        handle_control(
            Some(HuddleControl::SetBackground(Some("bg-sunset".into()))),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(snap.background_id.as_deref(), Some("bg-sunset"));
        handle_control(
            Some(HuddleControl::SetCaptionsEnabled(true)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(snap.captions_enabled);
        handle_control(
            Some(HuddleControl::SetAudioOnly(false)),
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(!snap.audio_only);

        // 5. Shared `apply_audio_stream_info`: join muted -> unmute -> drop cleans up both maps.
        let join_muted = SdkAudioStreamIdInfoFrame {
            streams: vec![SdkAudioStreamIdInfo {
                audio_stream_id: Some(11),
                attendee_id: Some("att-dave#content".into()),
                external_user_id: Some("T0TEAM-R0CALL-U0DAVE".into()),
                muted: Some(true),
                dropped: Some(false),
            }],
        };
        assert!(apply_audio_stream_info(join_muted, &mut rt));
        assert_eq!(
            rt.attendee_to_user.get("att-dave"),
            Some(&UserId::from("U0DAVE"))
        );
        assert!(rt.muted_peers.contains(&UserId::from("U0DAVE")));

        let unmute = SdkAudioStreamIdInfoFrame {
            streams: vec![SdkAudioStreamIdInfo {
                audio_stream_id: Some(11),
                attendee_id: Some("att-dave".into()),
                external_user_id: Some("T0TEAM-R0CALL-U0DAVE".into()),
                muted: Some(false),
                dropped: Some(false),
            }],
        };
        assert!(!apply_audio_stream_info(unmute, &mut rt));
        assert!(!rt.muted_peers.contains(&UserId::from("U0DAVE")));

        let dropped = SdkAudioStreamIdInfoFrame {
            streams: vec![SdkAudioStreamIdInfo {
                audio_stream_id: Some(11),
                attendee_id: Some("att-dave".into()),
                external_user_id: Some("T0TEAM-R0CALL-U0DAVE".into()),
                muted: Some(true),
                dropped: Some(true),
            }],
        };
        assert!(!apply_audio_stream_info(dropped, &mut rt));
        assert!(!rt.stream_users.contains_key(&11));
        assert!(!rt.muted_peers.contains(&UserId::from("U0DAVE")));

        // 6. INDEX bitrate-only change skips renegotiation, then empty INDEX unsubscribes.
        let one_peer_idx = SdkSignalFrame {
            index: Some(SdkIndexFrame {
                sources: vec![SdkStreamDescriptor {
                    stream_id: Some(50),
                    group_id: Some(50),
                    attendee_id: Some("att-dave".into()),
                    external_user_id: None, // resolved via attendee_to_user!
                    media_type: Some(SdkStreamMediaType::Video as i32),
                    max_bitrate_kbps: Some(1000),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(
            one_peer_idx,
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        let sent_after_first = sig.sent.len();
        let bitrate_jitter_idx = SdkSignalFrame {
            index: Some(SdkIndexFrame {
                sources: vec![SdkStreamDescriptor {
                    stream_id: Some(50),
                    group_id: Some(50),
                    attendee_id: Some("att-dave".into()),
                    external_user_id: None,
                    media_type: Some(SdkStreamMediaType::Video as i32),
                    max_bitrate_kbps: Some(600),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(
            bitrate_jitter_idx,
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert_eq!(
            sig.sent.len(),
            sent_after_first,
            "bitrate jitter alone must not trigger SUBSCRIBE renegotiation"
        );

        let _ = drain_updates(&mut rx);
        let empty_idx = SdkSignalFrame {
            index: Some(SdkIndexFrame {
                sources: vec![],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(empty_idx, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(rt.remote_sources.is_empty());
        assert_eq!(
            sig.sent.last().and_then(|f| f.r#type),
            Some(sdk_signal_frame::Type::RemoteVideoUpdate as i32)
        );
        assert!(drain_updates(&mut rx).iter().any(|u| matches!(
            u,
            HuddleUpdate::VideoTileClosed(VideoTileKey::RemoteCamera(uid)) if uid.as_str() == "U0DAVE"
        )));

        // Control channel closed (`None`) exits cleanly.
        assert!(matches!(
            handle_control(None, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await,
            Some(LoopExit::LeftCleanly)
        ));
    }

    #[tokio::test]
    async fn signaling_error_branches_and_video_decoder_thread_pipeline_reach_full_coverage() {
        use crate::session::video_rx::{VideoDecoderThreadArgs, spawn_video_decoder_thread};

        let creds = test_creds();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (mut rt, _bg_rx) = make_runtime(&creds, &tx, FakeMedia::default());
        let mut config = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut rtc = FakeRtc::default();
        let mut sig = FakeSignal::default();

        // 1. Signaling handler error paths: prepare_video_receive_offer fails, then send fails,
        // then SUBACK has corrupt compressed SDP, then DATA_MESSAGE Topic updates snap.topic.
        rtc.fail_prepare_video = true;
        let idx = SdkSignalFrame {
            index: Some(SdkIndexFrame {
                sources: vec![SdkStreamDescriptor {
                    stream_id: Some(77),
                    group_id: Some(77),
                    attendee_id: Some("att-err".into()),
                    external_user_id: Some("T0TEAM-R0CALL-U0ERR".into()),
                    media_type: Some(SdkStreamMediaType::Video as i32),
                    max_bitrate_kbps: Some(900),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(idx.clone(), &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(rt.remote_sources.is_empty());

        rtc.fail_prepare_video = false;
        sig.fail_send = true;
        handle_signal_frame(idx, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(
            rt.remote_sources.is_empty(),
            "failed SUBSCRIBE send must not record remote_sources"
        );
        sig.fail_send = false;

        let bad_suback = SdkSignalFrame {
            suback: Some(SdkSubscribeAckFrame {
                compressed_sdp_answer: Some(vec![0xFF, 0x00, 0xAA]),
                ..Default::default()
            }),
            ..Default::default()
        };
        handle_signal_frame(bad_suback, &mut config, &mut snap, &rtc, &mut sig, &mut rt).await;
        assert!(rtc.remote_answers.lock().unwrap().is_empty());

        // Audio-only INDEX suppression keeps remote_sources empty.
        snap.audio_only = true;
        handle_signal_frame(
            SdkSignalFrame {
                index: Some(SdkIndexFrame {
                    sources: vec![SdkStreamDescriptor {
                        stream_id: Some(78),
                        group_id: Some(78),
                        attendee_id: Some("att-err".into()),
                        external_user_id: Some("T0TEAM-R0CALL-U0ERR".into()),
                        media_type: Some(SdkStreamMediaType::Video as i32),
                        max_bitrate_kbps: Some(900),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
            &mut config,
            &mut snap,
            &rtc,
            &mut sig,
            &mut rt,
        )
        .await;
        assert!(rt.remote_sources.is_empty());
        snap.audio_only = false;

        // 2. Full `spawn_video_decoder_thread` pipeline: encode real H.264 keyframe, route via
        // `VideoRoutingTable`, verify decoded `HuddleUpdate::VideoFrame`, `codec_rx`, `keyframe_req_rx`,
        // AV1 decoder init, unsupported codec MIME, unmapped tile warning, `audio_only` drop,
        // `clear_decoders` reset, and clean thread join on `None`.
        let mut enc = crate::video::H264VideoEncoder::new_camera().unwrap();
        let (w, h) = (96usize, 64usize);
        let annex_b = enc.encode_rgb(&vec![90u8; w * h * 3], w, h).unwrap();
        let mut stap_a = vec![24u8];
        for nal in crate::video::split_annex_b_nals(&annex_b) {
            if !nal.is_empty() {
                stap_a.extend_from_slice(&(nal.len() as u16).to_be_bytes());
                stap_a.extend_from_slice(nal);
            }
        }

        // Direct AV1 + unknown MIME checks on `RemoteVideoDecoder`.
        assert!(RemoteVideoDecoder::for_mime("video/VP9").is_none());
        if let Some(mut av1_dec) = RemoteVideoDecoder::for_mime("video/AV1") {
            assert!(!av1_dec.backend_name().is_empty());
            let _ = av1_dec.push(&InboundVideoPacket {
                ssrc: 700,
                mid: "3".into(),
                track_id: "v1".into(),
                mime_type: "video/AV1".into(),
                seq: 1,
                payload: vec![0x00, 0x01, 0x02],
                marker: true,
            });
        }

        let routing = Arc::new(Mutex::new(VideoRoutingTable {
            ssrc_to_stream_id: HashMap::from([(888, 77)]),
            remote_sources: vec![(
                "2".into(),
                RemoteVideoSource {
                    stream_id: 77,
                    group_id: 77,
                    attendee_id: "att-err".into(),
                    user_id: UserId::from("U0ERR"),
                    is_content: false,
                    max_bitrate_kbps: 900,
                },
            )],
        }));
        let audio_only = Arc::new(AtomicBool::new(false));
        let clear_decoders = Arc::new(AtomicBool::new(false));
        let perf = Arc::new(HuddlePerfCounters::default());
        let (vdec_tx, vdec_rx) = std::sync::mpsc::channel();
        let (kf_tx, mut kf_rx) = mpsc::unbounded_channel();
        let (codec_tx, mut codec_rx) = mpsc::unbounded_channel();

        let _ = drain_updates(&mut rx);
        let idle_timeout = Duration::from_millis(200);
        let join = spawn_video_decoder_thread(VideoDecoderThreadArgs {
            inbound_video_rx: vdec_rx,
            audio_only: Arc::clone(&audio_only),
            clear_decoders: Arc::clone(&clear_decoders),
            shutdown: Arc::new(AtomicBool::new(false)),
            idle_timeout,
            routing: Arc::clone(&routing),
            perf: Arc::clone(&perf),
            updates: tx.clone(),
            keyframe_req_tx: kf_tx,
            codec_tx,
        })
        .expect("decoder thread spawned");

        let make_pkt = |seq: u16| InboundVideoPacket {
            ssrc: 888,
            mid: "2".into(),
            track_id: "v0".into(),
            mime_type: "video/H264".into(),
            seq,
            payload: stap_a.clone(),
            marker: true,
        };
        vdec_tx.send(Some(make_pkt(1))).unwrap();
        // Wait deterministically for the worker thread to decode packet 1 before toggling flags.
        let decoded = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(u) = rx.recv().await {
                if matches!(
                    u,
                    HuddleUpdate::VideoFrame { tile: VideoTileKey::RemoteCamera(ref uid), .. }
                        if uid.as_str() == "U0ERR"
                ) {
                    return true;
                }
            }
            false
        })
        .await
        .expect("timed out waiting for decoded VideoFrame");
        assert!(decoded, "decoder thread must emit VideoFrame for U0ERR");

        // Send packet with seq gap (1 -> 6) and wait for its decoded frame so seq gap is recorded.
        vdec_tx.send(Some(make_pkt(6))).unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("second decoded frame");

        // Unsupported codec MIME + unmapped SSRC/mid tile warning paths.
        vdec_tx
            .send(Some(InboundVideoPacket {
                ssrc: 901,
                mid: "99".into(),
                track_id: "v9".into(),
                mime_type: "video/VP9".into(),
                seq: 1,
                payload: vec![1, 2, 3],
                marker: true,
            }))
            .unwrap();
        vdec_tx
            .send(Some(InboundVideoPacket {
                ssrc: 902,
                mid: "99".into(),
                track_id: "v10".into(),
                mime_type: "video/H264".into(),
                seq: 1,
                payload: stap_a.clone(),
                marker: true,
            }))
            .unwrap();

        // Both H.264 tracks (888 and 902) get a decoder worker; 901 (VP9) is unsupported.
        let mut codecs = Vec::new();
        for _ in 0..2 {
            codecs.push(
                tokio::time::timeout(Duration::from_secs(3), codec_rx.recv())
                    .await
                    .expect("codec notification for a new decoder worker")
                    .expect("codec channel open"),
            );
        }
        assert!(
            codecs
                .iter()
                .all(|c| c.contains("H.264") || c.contains("H264")),
            "got {codecs:?}"
        );

        // Idle sweep: with no packet for `idle_timeout`, both decoders are dropped so departed
        // participants and camera toggles never accumulate decoders for the whole call.
        tokio::time::timeout(idle_timeout * 10, async {
            while perf.rx_decoders() != 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("idle decoder workers must be swept");

        // Now trigger clear_decoders + audio_only drop path, then shut down cleanly.
        clear_decoders.store(true, Ordering::Relaxed);
        audio_only.store(true, Ordering::Relaxed);
        vdec_tx.send(Some(make_pkt(7))).unwrap();
        vdec_tx.send(None).unwrap();
        join.join().expect("decoder thread joined cleanly");

        let _ = kf_rx.try_recv();
        assert_eq!(perf.sample(1.0, 0, 0).rx_seq_gaps, 4);
    }
}

mod handshake_and_connected_loop_tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use slackless_types::ChannelId;
    use tokio::sync::mpsc;
    use webrtc::peer_connection::RTCPeerConnectionState;

    use super::super::connected::run_connected_loop_with;
    use super::super::handshake::perform_handshake_with;
    use super::super::io::{
        AudioSwitch, BoxFuture, MediaDevices, RtcController, RtcSession, SessionFactory,
        SignalSession, SignalSink, VideoTrackOut,
    };
    use super::super::supervisor::LoopExit;
    use crate::audio::AudioSharedState;
    use crate::metrics::HuddlePerfCounters;
    use crate::proto::{
        SdkAudioStreamIdInfo, SdkAudioStreamIdInfoFrame, SdkIndexFrame, SdkJoinAckFrame,
        SdkPingPongFrame, SdkPingPongType, SdkSignalFrame, SdkStreamDescriptor, SdkStreamMediaType,
        SdkSubscribeAckFrame, SdkTrackMapping, SdkTurnCredentials, sdk_signal_frame,
    };
    use crate::rtc::{InboundAudioPacket, InboundVideoPacket, VideoSenderFeedback};
    use crate::signaling::ClientMetricsSnapshot;
    use crate::video::{VideoFrame, VideoFrameData, VideoTileKey};
    use crate::{
        HuddleAudioConfig, HuddleControl, HuddleCredentials, HuddleError, HuddlePhase,
        HuddleSessionSnapshot, HuddleUpdate, Result,
    };

    #[derive(Default)]
    struct MockLog {
        sent_frames: Mutex<Vec<SdkSignalFrame>>,
        leave_and_close_count: AtomicU32,
        peer_close_count: AtomicU32,
        audio_stop_count: AtomicU32,
        audio_restarts: AtomicU32,
        camera_stop_count: AtomicU32,
        camera_installs: AtomicU32,
        screen_stop_count: AtomicU32,
        screen_share_installs: AtomicU32,
        /// Fired when the client answers a server PING.
        pong_sent: tokio::sync::Notify,
        jukebox_calls: Mutex<Vec<(Option<String>, Option<String>)>>,
        opus_written: Mutex<Vec<bytes::Bytes>>,
        h264_written: Mutex<Vec<Vec<u8>>>,
        keyframe_requests: Mutex<Vec<u32>>,
        forced_keyframes: AtomicU32,
        target_bitrates: Mutex<Vec<u32>>,
    }

    struct MockSignal {
        log: Arc<MockLog>,
        fail_send_on_call: Option<usize>,
        send_calls: usize,
        join_ack_result: Option<Result<(Vec<SdkSignalFrame>, SdkJoinAckFrame)>>,
        sub_ack_result: Option<Result<(Vec<SdkSignalFrame>, SdkSubscribeAckFrame)>>,
        inbound_rx: mpsc::UnboundedReceiver<Result<Option<SdkSignalFrame>>>,
    }

    impl SignalSink for MockSignal {
        fn send_frame<'a>(&'a mut self, frame: &'a SdkSignalFrame) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.send_calls += 1;
                self.log.sent_frames.lock().unwrap().push(frame.clone());
                if self.fail_send_on_call == Some(self.send_calls) {
                    return Err(HuddleError::Signaling("injected send_frame error".into()));
                }
                Ok(())
            })
        }

        fn reply_ping_if_needed<'a>(&'a mut self, pp: &'a SdkPingPongFrame) -> BoxFuture<'a, bool> {
            Box::pin(async move {
                if pp.r#type == Some(SdkPingPongType::Ping as i32) {
                    self.log.pong_sent.notify_one();
                    true
                } else {
                    false
                }
            })
        }
    }

    impl SignalSession for MockSignal {
        fn next_frame<'a>(&'a mut self) -> BoxFuture<'a, Result<Option<SdkSignalFrame>>> {
            Box::pin(async move {
                match self.inbound_rx.recv().await {
                    Some(res) => res,
                    None => Ok(None),
                }
            })
        }

        fn wait_for_join_ack<'a>(
            &'a mut self,
            _timeout: Duration,
            on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
        ) -> BoxFuture<'a, Result<SdkJoinAckFrame>> {
            Box::pin(async move {
                match self.join_ack_result.take() {
                    Some(Ok((mut buffered, ack))) => {
                        for frame in &mut buffered {
                            on_other(frame);
                        }
                        Ok(ack)
                    }
                    Some(Err(e)) => Err(e),
                    None => Err(HuddleError::Signaling("no scripted join_ack".into())),
                }
            })
        }

        fn wait_for_subscribe_ack<'a>(
            &'a mut self,
            _timeout: Duration,
            on_other: &'a mut (dyn FnMut(&mut SdkSignalFrame) + Send),
        ) -> BoxFuture<'a, Result<SdkSubscribeAckFrame>> {
            Box::pin(async move {
                match self.sub_ack_result.take() {
                    Some(Ok((mut buffered, ack))) => {
                        for frame in &mut buffered {
                            on_other(frame);
                        }
                        Ok(ack)
                    }
                    Some(Err(e)) => Err(e),
                    None => Err(HuddleError::Signaling("no scripted sub_ack".into())),
                }
            })
        }

        fn send_leave_and_close<'a>(self, _drain_timeout: Duration) -> BoxFuture<'a, ()>
        where
            Self: 'a,
        {
            Box::pin(async move {
                self.log
                    .leave_and_close_count
                    .fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    #[derive(Clone)]
    struct MockVideoSender {
        log: Arc<MockLog>,
    }

    impl VideoTrackOut for MockVideoSender {
        fn write_h264_frame<'a>(
            &'a self,
            annex_b: Vec<u8>,
            _duration: Duration,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.log.h264_written.lock().unwrap().push(annex_b);
                Ok(())
            })
        }
    }

    struct MockRtc {
        log: Arc<MockLog>,
        initial_state: RTCPeerConnectionState,
        fail_offer: bool,
        fail_set_remote_answer: bool,
        cam_fb_tx_slot: Arc<Mutex<Option<mpsc::UnboundedSender<VideoSenderFeedback>>>>,
        packets_sent: Arc<AtomicU64>,
        packets_received: AtomicU64,
        bytes_sent: Arc<AtomicU64>,
        bytes_received: AtomicU64,
    }

    impl RtcController for MockRtc {
        fn set_remote_answer<'a>(&'a self, _answer: String) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                if self.fail_set_remote_answer {
                    return Err(HuddleError::WebRtc("injected set_remote_answer".into()));
                }
                Ok(())
            })
        }

        fn create_gathered_offer<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                if self.fail_offer {
                    return Err(HuddleError::WebRtc("injected offer failure".into()));
                }
                Ok("v=0\r\ns=mock-offer\r\n".to_owned())
            })
        }

        fn prepare_video_receive_offer<'a>(
            &'a self,
            count: usize,
        ) -> BoxFuture<'a, Result<(String, Vec<String>)>> {
            Box::pin(async move {
                let mids = (0..count).map(|i| (i + 2).to_string()).collect();
                Ok(("v=0\r\ns=mock-video-rx\r\n".to_owned(), mids))
            })
        }
    }

    impl RtcSession for MockRtc {
        type VideoSender = MockVideoSender;

        fn attach_video_feedback<'a>(
            &'a self,
            tx: mpsc::UnboundedSender<VideoSenderFeedback>,
        ) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                *self.cam_fb_tx_slot.lock().unwrap() = Some(tx);
            })
        }

        fn video_sender(&self) -> Self::VideoSender {
            MockVideoSender {
                log: Arc::clone(&self.log),
            }
        }

        fn write_opus_frame<'a>(&'a self, opus_bytes: bytes::Bytes) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                let len = opus_bytes.len() as u64;
                self.packets_sent.fetch_add(1, Ordering::Relaxed);
                self.bytes_sent.fetch_add(len, Ordering::Relaxed);
                self.log.opus_written.lock().unwrap().push(opus_bytes);
                Ok(())
            })
        }

        fn request_keyframe<'a>(&'a self, ssrc: u32) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.log.keyframe_requests.lock().unwrap().push(ssrc);
            })
        }

        fn connection_state(&self) -> RTCPeerConnectionState {
            self.initial_state
        }

        fn total_packets_sent(&self) -> u64 {
            self.packets_sent.load(Ordering::Relaxed)
        }

        fn total_packets_received(&self) -> u64 {
            self.packets_received.load(Ordering::Relaxed)
        }

        fn total_bytes_sent(&self) -> u64 {
            self.bytes_sent.load(Ordering::Relaxed)
        }

        fn total_bytes_received(&self) -> u64 {
            self.bytes_received.load(Ordering::Relaxed)
        }

        fn take_metrics_snapshot(&self, local_rms: f32) -> ClientMetricsSnapshot {
            ClientMetricsSnapshot {
                stun_rtt_ms: 15.0,
                mic_pps: self.total_packets_sent() as f64,
                mic_bitrate_bps: (self.total_bytes_sent() * 8) as f64,
                mic_audio_level: (local_rms as f64 * 32767.0).clamp(0.0, 32767.0),
                spk_pps: self.total_packets_received() as f64,
                spk_bitrate_bps: (self.total_bytes_received() * 8) as f64,
            }
        }

        fn close<'a>(&'a self) -> BoxFuture<'a, ()> {
            Box::pin(async move {
                self.log.peer_close_count.fetch_add(1, Ordering::SeqCst);
            })
        }
    }

    /// Blocks a mocked device restart until the test releases it, standing in for an OS device
    /// (or a Chime handshake) that takes seconds.
    struct RestartGate {
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl RestartGate {
        fn block(self) {
            let _ = self.entered.map(|tx| tx.send(()));
            let _ = self.release.recv();
        }
    }

    struct MockMedia {
        log: Arc<MockLog>,
        running_audio: bool,
        camera_active: bool,
        restart_gate: Option<RestartGate>,
        outbound_h264_tx_slot: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
        local_cam_tx_slot: Arc<Mutex<Option<mpsc::UnboundedSender<VideoFrame>>>>,
        local_screen_tx_slot: Arc<Mutex<Option<mpsc::UnboundedSender<VideoFrame>>>>,
    }

    impl MediaDevices for MockMedia {
        type InboundAudio = ();
        type Audio = ();
        type Camera = ();
        type ScreenShare = ();

        fn stop_audio(&mut self) {
            if self.running_audio {
                self.running_audio = false;
                self.log.audio_stop_count.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn switch_audio(
            &mut self,
            _new: HuddleAudioConfig,
            _prev: HuddleAudioConfig,
            _parked: Option<()>,
        ) -> BoxFuture<'static, AudioSwitch<(), ()>> {
            self.stop_audio();
            let gate = self.restart_gate.take();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(gate) = gate {
                        gate.block();
                    }
                    AudioSwitch::Switched(())
                })
                .await
                .unwrap_or_else(|e| AudioSwitch::Failed {
                    err: HuddleError::Audio(format!("mock audio switch panicked: {e}")),
                    revert_err: None,
                    inbound: None,
                })
            })
        }

        fn install_audio(&mut self, _audio: ()) {
            self.running_audio = true;
            self.log.audio_restarts.fetch_add(1, Ordering::SeqCst);
        }

        fn start_camera(
            &mut self,
            _device_index: Option<usize>,
            _blur: bool,
            _compact: bool,
            outbound_h264_tx: mpsc::UnboundedSender<Vec<u8>>,
            local_cam_tx: mpsc::UnboundedSender<VideoFrame>,
        ) -> BoxFuture<'static, Result<()>> {
            let gate = self.restart_gate.take();
            let h264_slot = Arc::clone(&self.outbound_h264_tx_slot);
            let cam_slot = Arc::clone(&self.local_cam_tx_slot);
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(gate) = gate {
                        gate.block();
                    }
                    *h264_slot.lock().unwrap() = Some(outbound_h264_tx);
                    *cam_slot.lock().unwrap() = Some(local_cam_tx);
                })
                .await
                .map_err(|e| HuddleError::Video(format!("mock camera start panicked: {e}")))
            })
        }

        fn install_camera(&mut self, _camera: ()) {
            self.camera_active = true;
            self.log.camera_installs.fetch_add(1, Ordering::SeqCst);
        }

        fn stop_camera(&mut self) -> BoxFuture<'static, ()> {
            let was_active = self.camera_active;
            self.camera_active = false;
            let log = Arc::clone(&self.log);
            Box::pin(async move {
                if was_active {
                    log.camera_stop_count.fetch_add(1, Ordering::SeqCst);
                }
            })
        }

        fn set_camera_blur(&mut self, _blur: bool) {}

        fn set_camera_compact(&mut self, _compact: bool) {}

        fn force_camera_keyframe(&mut self) {
            self.log.forced_keyframes.fetch_add(1, Ordering::SeqCst);
        }

        fn set_camera_target_bitrate(&mut self, bps: u32) {
            self.log.target_bitrates.lock().unwrap().push(bps);
        }

        fn start_screen_share(
            &mut self,
            _creds: HuddleCredentials,
            _source_id: String,
            _share_audio: bool,
            local_screen_tx: mpsc::UnboundedSender<VideoFrame>,
            _perf: Arc<HuddlePerfCounters>,
        ) -> BoxFuture<'static, Result<()>> {
            let gate = self.restart_gate.take();
            let slot = Arc::clone(&self.local_screen_tx_slot);
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    if let Some(gate) = gate {
                        gate.block();
                    }
                    *slot.lock().unwrap() = Some(local_screen_tx);
                })
                .await
                .map_err(|e| HuddleError::Video(format!("mock screen share panicked: {e}")))
            })
        }

        fn screen_share_loopback(_: &()) -> Option<String> {
            None
        }

        fn install_screen_share(&mut self, _share: ()) {
            self.log
                .screen_share_installs
                .fetch_add(1, Ordering::SeqCst);
        }

        fn stop_screen_share(&mut self) -> BoxFuture<'static, ()> {
            let log = Arc::clone(&self.log);
            Box::pin(async move {
                log.screen_stop_count.fetch_add(1, Ordering::SeqCst);
            })
        }

        fn set_jukebox(&mut self, output_device: Option<String>, song_key: Option<&str>) {
            self.log
                .jukebox_calls
                .lock()
                .unwrap()
                .push((output_device, song_key.map(ToOwned::to_owned)));
        }
    }

    enum AudioStartScript {
        Ok,
        Err(String),
        Panic,
        DelayedOk(Duration),
    }

    struct MockFactory {
        log: Arc<MockLog>,
        connect_err: Option<HuddleError>,
        fail_send_on_call: Option<usize>,
        join_ack: Option<Result<(Vec<SdkSignalFrame>, SdkJoinAckFrame)>>,
        sub_ack: Option<Result<(Vec<SdkSignalFrame>, SdkSubscribeAckFrame)>>,
        create_rtc_err: Option<HuddleError>,
        rtc_initial_state: RTCPeerConnectionState,
        fail_offer: bool,
        fail_set_remote_answer: bool,
        audio_script: AudioStartScript,
        /// When set, the first audio / camera / screen-share restart blocks on it.
        restart_gate: Option<RestartGate>,
        inbound_signal_rx: Option<mpsc::UnboundedReceiver<Result<Option<SdkSignalFrame>>>>,
        // Captured channel handles for driving the connected loop from tests:
        pc_state_tx: Arc<Mutex<Option<mpsc::UnboundedSender<RTCPeerConnectionState>>>>,
        outbound_opus_tx: Arc<Mutex<Option<mpsc::UnboundedSender<bytes::Bytes>>>>,
        inbound_video_tx: Arc<Mutex<Option<mpsc::UnboundedSender<InboundVideoPacket>>>>,
        cam_fb_tx: Arc<Mutex<Option<mpsc::UnboundedSender<VideoSenderFeedback>>>>,
        outbound_h264_tx: Arc<Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>>,
        local_cam_tx: Arc<Mutex<Option<mpsc::UnboundedSender<VideoFrame>>>>,
        local_screen_tx: Arc<Mutex<Option<mpsc::UnboundedSender<VideoFrame>>>>,
        shared_audio: Arc<Mutex<Option<AudioSharedState>>>,
    }

    impl MockFactory {
        fn ok_default() -> (
            Self,
            mpsc::UnboundedSender<Result<Option<SdkSignalFrame>>>,
            Arc<MockLog>,
        ) {
            let log = Arc::new(MockLog::default());
            let (sig_tx, sig_rx) = mpsc::unbounded_channel();
            let join_ack = SdkJoinAckFrame {
                turn_credentials: Some(SdkTurnCredentials {
                    username: Some("u".into()),
                    password: Some("p".into()),
                    ttl: Some(3600),
                    uris: vec!["turn:relay.example:3478?transport=udp".into()],
                }),
                video_subscription_limit: Some(16),
                wants_compressed_sdp: Some(false),
                ..Default::default()
            };
            let sub_ack = SdkSubscribeAckFrame {
                sdp_answer: Some("v=0\r\ns=mock-answer\r\n".into()),
                tracks: vec![SdkTrackMapping {
                    stream_id: Some(77),
                    ssrc: Some(888),
                    track_label: Some("v0".into()),
                }],
                ..Default::default()
            };
            let f = Self {
                log: Arc::clone(&log),
                connect_err: None,
                fail_send_on_call: None,
                join_ack: Some(Ok((Vec::new(), join_ack))),
                sub_ack: Some(Ok((Vec::new(), sub_ack))),
                create_rtc_err: None,
                rtc_initial_state: RTCPeerConnectionState::Connecting,
                fail_offer: false,
                fail_set_remote_answer: false,
                audio_script: AudioStartScript::Ok,
                restart_gate: None,
                inbound_signal_rx: Some(sig_rx),
                pc_state_tx: Arc::new(Mutex::new(None)),
                outbound_opus_tx: Arc::new(Mutex::new(None)),
                inbound_video_tx: Arc::new(Mutex::new(None)),
                cam_fb_tx: Arc::new(Mutex::new(None)),
                outbound_h264_tx: Arc::new(Mutex::new(None)),
                local_cam_tx: Arc::new(Mutex::new(None)),
                local_screen_tx: Arc::new(Mutex::new(None)),
                shared_audio: Arc::new(Mutex::new(None)),
            };
            (f, sig_tx, log)
        }
    }

    impl SessionFactory for MockFactory {
        type Signal = MockSignal;
        type Rtc = MockRtc;
        type Media = MockMedia;

        fn connect_signal<'a>(
            &'a mut self,
            _creds: &'a HuddleCredentials,
        ) -> BoxFuture<'a, Result<Self::Signal>> {
            Box::pin(async move {
                if let Some(err) = self.connect_err.take() {
                    return Err(err);
                }
                let (_tx, fallback_rx) = mpsc::unbounded_channel();
                Ok(MockSignal {
                    log: Arc::clone(&self.log),
                    fail_send_on_call: self.fail_send_on_call,
                    send_calls: 0,
                    join_ack_result: self.join_ack.take(),
                    sub_ack_result: self.sub_ack.take(),
                    inbound_rx: self.inbound_signal_rx.take().unwrap_or(fallback_rx),
                })
            })
        }

        fn spawn_audio_start(
            &mut self,
            _config: HuddleAudioConfig,
            shared: AudioSharedState,
            outbound_opus_tx: mpsc::UnboundedSender<bytes::Bytes>,
            _inbound_opus_rx: mpsc::UnboundedReceiver<InboundAudioPacket>,
        ) -> tokio::task::JoinHandle<Result<Self::Media>> {
            *self.shared_audio.lock().unwrap() = Some(shared);
            *self.outbound_opus_tx.lock().unwrap() = Some(outbound_opus_tx);
            let log = Arc::clone(&self.log);
            let h264_slot = Arc::clone(&self.outbound_h264_tx);
            let cam_slot = Arc::clone(&self.local_cam_tx);
            let scr_slot = Arc::clone(&self.local_screen_tx);
            let script = std::mem::replace(&mut self.audio_script, AudioStartScript::Ok);
            let gate = self.restart_gate.take();
            tokio::spawn(async move {
                let media = |log| MockMedia {
                    log,
                    running_audio: true,
                    camera_active: false,
                    restart_gate: gate,
                    outbound_h264_tx_slot: h264_slot,
                    local_cam_tx_slot: cam_slot,
                    local_screen_tx_slot: scr_slot,
                };
                match script {
                    AudioStartScript::Ok => Ok(media(log)),
                    AudioStartScript::DelayedOk(d) => {
                        tokio::time::sleep(d).await;
                        Ok(media(log))
                    }
                    AudioStartScript::Err(msg) => Err(HuddleError::Audio(msg)),
                    AudioStartScript::Panic => panic!("simulated audio startup panic"),
                }
            })
        }

        fn create_rtc<'a>(
            &'a mut self,
            _turn: &'a SdkTurnCredentials,
            _inbound_opus_tx: mpsc::UnboundedSender<InboundAudioPacket>,
            inbound_video_tx: mpsc::UnboundedSender<InboundVideoPacket>,
            pc_state_tx: mpsc::UnboundedSender<RTCPeerConnectionState>,
        ) -> BoxFuture<'a, Result<Self::Rtc>> {
            Box::pin(async move {
                if let Some(err) = self.create_rtc_err.take() {
                    return Err(err);
                }
                *self.inbound_video_tx.lock().unwrap() = Some(inbound_video_tx);
                *self.pc_state_tx.lock().unwrap() = Some(pc_state_tx);
                Ok(MockRtc {
                    log: Arc::clone(&self.log),
                    initial_state: self.rtc_initial_state,
                    fail_offer: self.fail_offer,
                    fail_set_remote_answer: self.fail_set_remote_answer,
                    cam_fb_tx_slot: Arc::clone(&self.cam_fb_tx),
                    packets_sent: Arc::new(AtomicU64::new(0)),
                    packets_received: AtomicU64::new(0),
                    bytes_sent: Arc::new(AtomicU64::new(0)),
                    bytes_received: AtomicU64::new(0),
                })
            })
        }
    }

    fn creds() -> HuddleCredentials {
        HuddleCredentials {
            channel: ChannelId::from("C0HUDDLE"),
            call_id: "R0CALL".into(),
            meeting_id: "M0MEET".into(),
            attendee_id: "att-self".into(),
            external_user_id: "T0TEAM-R0CALL-U0SELF".into(),
            join_token: "tok".into(),
            signaling_url: "wss://example.invalid/control".into(),
            audio_host_url: "audio.example.invalid:3478".into(),
            turn_control_url: "https://example.invalid/turn".into(),
            thread_root_ts: None,
            canvas_file_id: None,
            huddle_link: None,
        }
    }

    fn buffered_frames() -> (SdkSignalFrame, SdkSignalFrame) {
        let audio_info_frame = SdkSignalFrame {
            r#type: Some(sdk_signal_frame::Type::AudioStreamIdInfo as i32),
            audio_stream_id_info: Some(SdkAudioStreamIdInfoFrame {
                streams: vec![SdkAudioStreamIdInfo {
                    audio_stream_id: Some(42),
                    attendee_id: Some("att-peer".into()),
                    external_user_id: Some("T0-R0-U0PEER".into()),
                    muted: Some(true),
                    ..Default::default()
                }],
            }),
            ..Default::default()
        };
        let index_frame = SdkSignalFrame {
            r#type: Some(sdk_signal_frame::Type::Index as i32),
            index: Some(SdkIndexFrame {
                sources: vec![SdkStreamDescriptor {
                    stream_id: Some(77),
                    group_id: Some(7),
                    attendee_id: Some("att-peer".into()),
                    external_user_id: Some("T0-R0-U0PEER".into()),
                    media_type: Some(SdkStreamMediaType::Video as i32),
                    max_bitrate_kbps: Some(1200),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        (audio_info_frame, index_frame)
    }

    #[tokio::test]
    async fn handshake_happy_path_buffers_interleaved_frames_and_maps_ssrcs() {
        let (mut factory, _sig_tx, log) = MockFactory::ok_default();
        let (audio_frame, index_frame) = buffered_frames();
        let (_, join_ack) = factory.join_ack.take().unwrap().unwrap();
        let (_, sub_ack) = factory.sub_ack.take().unwrap().unwrap();
        factory.join_ack = Some(Ok((
            vec![audio_frame.clone(), index_frame.clone()],
            join_ack,
        )));
        factory.sub_ack = Some(Ok((vec![audio_frame, index_frame], sub_ack)));

        let Ok(hs) = perform_handshake_with(
            &mut factory,
            &creds(),
            &HuddleAudioConfig::default(),
            &HuddleSessionSnapshot::default(),
            false,
        )
        .await
        else {
            panic!("handshake succeeds");
        };

        assert_eq!(hs.ssrc_to_stream_id.get(&888), Some(&77));
        assert_eq!(
            hs.last_sdp_answer.as_deref(),
            Some("v=0\r\ns=mock-answer\r\n")
        );
        assert!(hs.pending_index.is_some());
        assert_eq!(hs.pending_audio_info.len(), 2);
        assert_eq!(log.sent_frames.lock().unwrap().len(), 2); // JOIN + SUBSCRIBE
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 0);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 0);
        assert_eq!(log.audio_stop_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn handshake_failure_paths_tear_down_audio_peer_and_signal_cleanly() {
        let cfg = HuddleAudioConfig::default();
        let snap = HuddleSessionSnapshot::default();
        let c = creds();

        // 1. connect_signal fails: initial => Fatal, reconnect => Reconnect.
        let (mut f, _, _) = MockFactory::ok_default();
        f.connect_err = Some(HuddleError::Signaling("dns".into()));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(_))
        ));
        let (mut f, _, _) = MockFactory::ok_default();
        f.connect_err = Some(HuddleError::Signaling("dns".into()));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, true).await,
            Err(LoopExit::Reconnect {
                was_connected: false,
                ..
            })
        ));

        // 2. send JOIN fails.
        let (mut f, _, _) = MockFactory::ok_default();
        f.fail_send_on_call = Some(1);
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(_))
        ));

        // 3. wait_for_join_ack fails.
        let (mut f, _, _) = MockFactory::ok_default();
        f.join_ack = Some(Err(HuddleError::Signaling("join timeout".into())));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(_))
        ));

        // 4. JOIN_ACK missing TURN credentials -> sends LEAVE + Fatal.
        let (mut f, _, log) = MockFactory::ok_default();
        f.join_ack = Some(Ok((Vec::new(), SdkJoinAckFrame::default())));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(HuddleError::Signaling(m))) if m.contains("TURN")
        ));
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);

        // 5. create_rtc fails -> stops audio_guard AND sends LEAVE.
        let (mut f, _, log) = MockFactory::ok_default();
        f.create_rtc_err = Some(HuddleError::WebRtc("no relay".into()));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, true).await,
            Err(LoopExit::Reconnect { .. })
        ));
        assert_eq!(log.audio_stop_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 0);

        // 6. Post-peer failure (SUBSCRIBE_ACK error & set_remote_answer error) ->
        //    MUST stop audio_guard, close peer, AND send LEAVE (regression check!).
        let (mut f, _, log) = MockFactory::ok_default();
        f.sub_ack = Some(Err(HuddleError::Protocol {
            status: 403,
            description: "forbidden".into(),
        }));
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, true).await,
            Err(LoopExit::Fatal(HuddleError::Protocol { status: 403, .. }))
        ));
        assert_eq!(log.audio_stop_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);

        let (mut f, _, log) = MockFactory::ok_default();
        f.fail_set_remote_answer = true;
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(HuddleError::WebRtc(_)))
        ));
        assert_eq!(log.audio_stop_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);

        // 7. Audio startup returns Err or panics -> MUST close peer AND send LEAVE.
        let (mut f, _, log) = MockFactory::ok_default();
        f.audio_script = AudioStartScript::Err("no mic".into());
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(HuddleError::Audio(_)))
        ));
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);

        let (mut f, _, log) = MockFactory::ok_default();
        f.audio_script = AudioStartScript::Panic;
        assert!(matches!(
            perform_handshake_with(&mut f, &c, &cfg, &snap, false).await,
            Err(LoopExit::Fatal(HuddleError::Audio(_)))
        ));
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);

        // 8. Dropping the handshake future mid-flight triggers AudioStartGuard::drop cleanup.
        let (mut f, _, log) = MockFactory::ok_default();
        f.audio_script = AudioStartScript::DelayedOk(Duration::from_millis(20));
        // Stall inside create_rtc? Simpler: fail_offer after delayed audio start? No, cancel the
        // future while awaiting audio_guard.finish() by timing out at 5ms!
        let timed_out = tokio::time::timeout(
            Duration::from_millis(5),
            perform_handshake_with(&mut f, &c, &cfg, &snap, false),
        )
        .await;
        assert!(timed_out.is_err());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            log.audio_stop_count.load(Ordering::SeqCst),
            1,
            "AudioStartGuard::drop must still stop the delayed audio engine"
        );
    }

    #[tokio::test]
    async fn connected_loop_exercises_all_select_arms_ticks_and_clean_teardown() {
        tokio::time::pause();

        let (mut factory, sig_tx, log) = MockFactory::ok_default();
        let (audio_frame, index_frame) = buffered_frames();
        let (_, join_ack) = factory.join_ack.take().unwrap().unwrap();
        let (_, sub_ack) = factory.sub_ack.take().unwrap().unwrap();
        factory.join_ack = Some(Ok((vec![audio_frame], join_ack)));
        factory.sub_ack = Some(Ok((vec![index_frame], sub_ack)));
        factory.rtc_initial_state = RTCPeerConnectionState::Connected;

        let pc_state_slot = Arc::clone(&factory.pc_state_tx);
        let opus_slot = Arc::clone(&factory.outbound_opus_tx);
        let inbound_video_slot = Arc::clone(&factory.inbound_video_tx);
        let cam_fb_slot = Arc::clone(&factory.cam_fb_tx);
        let h264_slot = Arc::clone(&factory.outbound_h264_tx);
        let local_cam_slot = Arc::clone(&factory.local_cam_tx);
        let local_screen_slot = Arc::clone(&factory.local_screen_tx);
        let shared_slot = Arc::clone(&factory.shared_audio);

        let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, mut upd_rx) = mpsc::unbounded_channel();
        let mut pending = VecDeque::from([HuddleControl::SetMuted(true)]);

        let c = creds();
        let loop_task = tokio::spawn(async move {
            let mut config = HuddleAudioConfig::default();
            let mut snap = HuddleSessionSnapshot::default();
            let exit = run_connected_loop_with(
                &mut factory,
                &c,
                &mut config,
                &mut snap,
                &mut ctrl_rx,
                &mut pending,
                &upd_tx,
                false,
            )
            .await;
            (exit, snap)
        });

        // Wait until handshake completes and the loop enters its select!
        for _ in 0..20 {
            if pc_state_slot.lock().unwrap().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let pc_tx = pc_state_slot.lock().unwrap().clone().expect("pc_state_tx");
        let opus_tx = opus_slot.lock().unwrap().clone().expect("outbound_opus_tx");
        let cam_fb_tx = cam_fb_slot.lock().unwrap().clone().expect("cam_fb_tx");
        let shared = shared_slot.lock().unwrap().clone().expect("shared_audio");

        // 1. Outbound Opus packet + camera feedback (ForceKeyframe + clamped bitrates).
        opus_tx
            .send(bytes::Bytes::from_static(b"opus-20ms"))
            .unwrap();
        cam_fb_tx.send(VideoSenderFeedback::ForceKeyframe).unwrap();
        cam_fb_tx
            .send(VideoSenderFeedback::TargetBitrateBps(100_000))
            .unwrap();
        cam_fb_tx
            .send(VideoSenderFeedback::TargetBitrateBps(9_000_000))
            .unwrap();

        // 2. Enable camera + screen share via ctrl_tx to populate local_cam_tx / local_screen_tx / h264_tx.
        ctrl_tx
            .send(HuddleControl::SetCamera {
                enabled: true,
                device_index: Some(0),
                blur: false,
            })
            .unwrap();
        ctrl_tx
            .send(HuddleControl::SetScreenShare {
                source_id: Some("monitor:0".into()),
                share_audio: false,
            })
            .unwrap();
        wait_until("camera and screen-share slots", || {
            local_cam_slot.lock().unwrap().is_some() && local_screen_slot.lock().unwrap().is_some()
        })
        .await;
        let dummy_frame = || VideoFrame {
            width: 2,
            height: 2,
            data: VideoFrameData::Bgra(Arc::new(vec![0u8; 16])),
        };
        local_cam_slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(dummy_frame())
            .unwrap();
        local_screen_slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(dummy_frame())
            .unwrap();
        h264_slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(vec![0, 0, 0, 1, 0x65])
            .unwrap();

        // 3. Peer connection transitions: New (ignored) -> Disconnected -> Connected.
        pc_tx.send(RTCPeerConnectionState::New).unwrap();
        pc_tx.send(RTCPeerConnectionState::Disconnected).unwrap();
        pc_tx.send(RTCPeerConnectionState::Connected).unwrap();

        // 3b. Inbound video packet through `vdec_fwd` + decoder thread to trigger `codec_rx`
        //     and `keyframe_req_rx` arms in `connected.rs`.
        {
            let mut enc = openh264::encoder::Encoder::with_api_config(
                openh264::OpenH264API::from_source(),
                openh264::encoder::EncoderConfig::new(),
            )
            .unwrap();
            let yuv = openh264::formats::YUVBuffer::from_rgb_source(
                openh264::formats::RgbSliceU8::new(&vec![128u8; 64 * 64 * 3], (64, 64)),
            );
            let bs = enc.encode(&yuv).unwrap();
            let annex_b = bs.to_vec();
            let nals = crate::video::split_annex_b_nals(&annex_b);
            let mut stap_a = vec![24u8];
            for nal in nals {
                let len = u16::try_from(nal.len()).unwrap();
                stap_a.extend_from_slice(&len.to_be_bytes());
                stap_a.extend_from_slice(nal);
            }
            let vtx = inbound_video_slot.lock().unwrap().clone().unwrap();
            vtx.send(InboundVideoPacket {
                ssrc: 888,
                mid: "2".into(),
                track_id: "v0".into(),
                mime_type: "video/H264".into(),
                seq: 1,
                payload: stap_a.clone(),
                marker: true,
            })
            .unwrap();
            // Sequence gap forces a PLI keyframe request back on `keyframe_req_rx`.
            vtx.send(InboundVideoPacket {
                ssrc: 888,
                mid: "2".into(),
                track_id: "v0".into(),
                mime_type: "video/H264".into(),
                seq: 8,
                payload: stap_a,
                marker: true,
            })
            .unwrap();
        }

        // 4. Inbound PING + AUDIO_METADATA (active remote speaker) signaling frames.
        sig_tx
            .send(Ok(Some(SdkSignalFrame {
                r#type: Some(sdk_signal_frame::Type::PingPong as i32),
                ping_pong: Some(SdkPingPongFrame {
                    r#type: Some(SdkPingPongType::Ping as i32),
                    ping_id: Some(7),
                }),
                ..Default::default()
            })))
            .unwrap();
        sig_tx
            .send(Ok(Some(SdkSignalFrame {
                r#type: Some(sdk_signal_frame::Type::AudioMetadata as i32),
                audio_metadata: Some(crate::proto::SdkAudioMetadataFrame {
                    attendee_states: vec![crate::proto::SdkAudioAttendeeState {
                        audio_stream_id: Some(42),
                        volume: Some(64),
                        muted: Some(false),
                        ..Default::default()
                    }],
                }),
                ..Default::default()
            })))
            .unwrap();

        // 5. Unmute (after pending_controls muted us), raise local RMS so level_tick emits
        //    AudioLevels, then advance virtual time past level_tick (120ms), metric_tick (1s),
        //    and perf_tick (5s).
        ctrl_tx.send(HuddleControl::SetMuted(false)).unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        shared
            .local_rms_bits
            .store(0.25f32.to_bits(), Ordering::Relaxed);
        tokio::time::advance(Duration::from_millis(150)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }

        // 6. Clean leave -> teardown runs completely.
        ctrl_tx.send(HuddleControl::Leave).unwrap();
        let (exit, snap) = loop_task.await.expect("loop task joins");
        assert!(matches!(exit, LoopExit::LeftCleanly));
        assert!(!snap.muted);
        assert_eq!(snap.phase, HuddlePhase::Leaving);
        assert_eq!(snap.pongs_sent, 1);
        assert!(snap.packets_sent >= 1);

        assert_eq!(log.opus_written.lock().unwrap().len(), 1);
        assert_eq!(log.forced_keyframes.load(Ordering::SeqCst), 1);
        assert_eq!(
            *log.target_bitrates.lock().unwrap(),
            vec![350_000, 1_500_000],
            "REMB bitrate must clamp to [350kbps, 1.5Mbps]"
        );
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.audio_stop_count.load(Ordering::SeqCst), 1);
        assert!(log.camera_stop_count.load(Ordering::SeqCst) >= 1);
        assert!(log.screen_stop_count.load(Ordering::SeqCst) >= 1);
        assert!(
            log.jukebox_calls
                .lock()
                .unwrap()
                .last()
                .is_some_and(|(_, song)| song.is_none())
        );

        // Verify updates included LocalCamera, LocalScreen, AudioLevels, Reconnecting, Connected, Leaving.
        let mut updates = Vec::new();
        while let Ok(u) = upd_rx.try_recv() {
            updates.push(u);
        }
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::VideoFrame {
                tile: VideoTileKey::LocalCamera,
                ..
            }
        )));
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::VideoFrame {
                tile: VideoTileKey::LocalScreen,
                ..
            }
        )));
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::AudioLevels(l)
                if l.local_rms > 0.1
                    && l.active_speakers.iter().any(|(u, _)| u.as_str() == "U0PEER")
        )));
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::State(s) if s.phase == HuddlePhase::Reconnecting
        )));
        assert!(updates.iter().any(|u| matches!(
            u,
            HuddleUpdate::State(s) if s.phase == HuddlePhase::Leaving
        )));
    }

    /// Await a condition driven by another task or thread, bounded by real time (the test clock
    /// may be paused, so tokio timeouts must not be used for it).
    async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what}"
            );
            tokio::task::yield_now().await;
        }
    }

    /// Device restarts and the `#content` handshake block on the OS/network for seconds; the
    /// event loop must keep answering server PINGs and applying controls meanwhile, and apply the
    /// result once it lands.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connected_loop_keeps_answering_pings_and_controls_during_blocking_device_work() {
        type SlowControl = (HuddleControl, &'static str, fn(&MockLog) -> u32);
        let slow_controls: [SlowControl; 3] = [
            (
                HuddleControl::SetAudioDevices {
                    input: Some("Slow Mic".into()),
                    output: Some("Slow Speakers".into()),
                },
                "audio device switch",
                |log: &MockLog| log.audio_restarts.load(Ordering::SeqCst),
            ),
            (
                HuddleControl::SetCamera {
                    enabled: true,
                    device_index: Some(1),
                    blur: false,
                },
                "camera start",
                |log: &MockLog| log.camera_installs.load(Ordering::SeqCst),
            ),
            (
                HuddleControl::SetScreenShare {
                    source_id: Some("monitor:1".into()),
                    share_audio: false,
                },
                "screen share start",
                |log: &MockLog| log.screen_share_installs.load(Ordering::SeqCst),
            ),
        ];
        for (ctrl, what, installs) in slow_controls {
            let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let (mut factory, sig_tx, log) = MockFactory::ok_default();
            factory.rtc_initial_state = RTCPeerConnectionState::Connected;
            factory.restart_gate = Some(RestartGate {
                entered: Some(entered_tx),
                release: release_rx,
            });
            let pc_state_slot = Arc::clone(&factory.pc_state_tx);
            let (ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
            let (upd_tx, mut upd_rx) = mpsc::unbounded_channel();
            let c = creds();
            let loop_task = tokio::spawn(async move {
                let mut config = HuddleAudioConfig::default();
                let mut snap = HuddleSessionSnapshot::default();
                let mut pending = VecDeque::new();
                run_connected_loop_with(
                    &mut factory,
                    &c,
                    &mut config,
                    &mut snap,
                    &mut ctrl_rx,
                    &mut pending,
                    &upd_tx,
                    false,
                )
                .await
            });
            wait_until("handshake", || pc_state_slot.lock().unwrap().is_some()).await;

            ctrl_tx.send(ctrl.clone()).unwrap();
            tokio::time::timeout(Duration::from_secs(5), entered_rx)
                .await
                .expect("device work started")
                .unwrap();
            sig_tx
                .send(Ok(Some(SdkSignalFrame {
                    r#type: Some(sdk_signal_frame::Type::PingPong as i32),
                    ping_pong: Some(SdkPingPongFrame {
                        r#type: Some(SdkPingPongType::Ping as i32),
                        ping_id: Some(7),
                    }),
                    ..Default::default()
                })))
                .unwrap();
            ctrl_tx.send(HuddleControl::SetMuted(true)).unwrap();
            let pong = tokio::time::timeout(Duration::from_secs(5), log.pong_sent.notified()).await;
            let muted = tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(u) = upd_rx.recv().await {
                    if matches!(u, HuddleUpdate::State(ref s) if s.muted) {
                        return true;
                    }
                }
                false
            })
            .await;
            release_tx.send(()).unwrap();
            assert!(
                pong.is_ok(),
                "server PING must be answered while the {what} is in flight"
            );
            assert!(
                muted.unwrap_or_else(|_| panic!("event loop stalled during the {what}")),
                "SetMuted must apply while the {what} is in flight"
            );
            // Released: the device work completes and its result is installed.
            tokio::time::timeout(Duration::from_secs(5), async {
                while installs(&log) == 0 {
                    let Some(_u) = upd_rx.recv().await else {
                        panic!("updates channel closed before the {what} landed");
                    };
                }
            })
            .await
            .expect("device work result installed");

            ctrl_tx.send(HuddleControl::Leave).unwrap();
            let exit = loop_task.await.unwrap();
            assert!(matches!(exit, LoopExit::LeftCleanly), "{what}");
        }
    }

    #[tokio::test]
    async fn connected_loop_handles_connect_timeout_pc_failed_and_signal_close_and_error() {
        tokio::time::pause();

        // Case A: Connect timeout (12s while !ever_connected) -> Fatal(UdpBlocked) on attempt 0.
        let (mut factory, _sig_tx, log) = MockFactory::ok_default();
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut pending = VecDeque::new();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(exit, LoopExit::Fatal(HuddleError::UdpBlocked)));
        assert_eq!(log.leave_and_close_count.load(Ordering::SeqCst), 1);
        assert_eq!(log.peer_close_count.load(Ordering::SeqCst), 1);

        // Case B: RTCPeerConnectionState::Failed after Connected -> Reconnect { was_connected: true }.
        let (mut factory, _sig_tx, _) = MockFactory::ok_default();
        let pc_slot = Arc::clone(&factory.pc_state_tx);
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let c = creds();
        let task = tokio::spawn(async move {
            let mut cfg = HuddleAudioConfig::default();
            let mut snap = HuddleSessionSnapshot::default();
            let mut pending = VecDeque::new();
            run_connected_loop_with(
                &mut factory,
                &c,
                &mut cfg,
                &mut snap,
                &mut ctrl_rx,
                &mut pending,
                &upd_tx,
                false,
            )
            .await
        });
        for _ in 0..20 {
            if pc_slot.lock().unwrap().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let pc_tx = pc_slot.lock().unwrap().clone().unwrap();
        pc_tx.send(RTCPeerConnectionState::Connected).unwrap();
        pc_tx.send(RTCPeerConnectionState::Failed).unwrap();
        let exit = task.await.unwrap();
        assert!(matches!(
            exit,
            LoopExit::Reconnect {
                err: HuddleError::UdpBlocked,
                was_connected: true
            }
        ));

        // Case C: Signal socket closes cleanly (`Ok(None)`) before connect -> Fatal on attempt 0.
        let (mut factory, sig_tx, _) = MockFactory::ok_default();
        sig_tx.send(Ok(None)).unwrap();
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut pending = VecDeque::new();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(exit, LoopExit::Fatal(HuddleError::Signaling(_))));

        // Case D: Signal socket errors after connect -> Reconnect { was_connected: true }.
        let (mut factory, sig_tx, _) = MockFactory::ok_default();
        factory.rtc_initial_state = RTCPeerConnectionState::Connected;
        sig_tx
            .send(Err(HuddleError::Signaling("ws reset".into())))
            .unwrap();
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut pending = VecDeque::new();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(
            exit,
            LoopExit::Reconnect {
                was_connected: true,
                ..
            }
        ));

        // Case E: Signal socket errors before connect on attempt 0 -> Fatal.
        let (mut factory, sig_tx, _) = MockFactory::ok_default();
        sig_tx
            .send(Err(HuddleError::Signaling("ws reset".into())))
            .unwrap();
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut pending = VecDeque::new();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(exit, LoopExit::Fatal(HuddleError::Signaling(_))));

        // Case F: RTCPeerConnectionState::Failed before connect on attempt 0 -> Fatal(UdpBlocked).
        let (mut factory, _sig_tx, _) = MockFactory::ok_default();
        let pc_slot = Arc::clone(&factory.pc_state_tx);
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let c = creds();
        let task = tokio::spawn(async move {
            let mut cfg = HuddleAudioConfig::default();
            let mut snap = HuddleSessionSnapshot::default();
            let mut pending = VecDeque::new();
            run_connected_loop_with(
                &mut factory,
                &c,
                &mut cfg,
                &mut snap,
                &mut ctrl_rx,
                &mut pending,
                &upd_tx,
                false,
            )
            .await
        });
        for _ in 0..20 {
            if pc_slot.lock().unwrap().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
        pc_slot
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(RTCPeerConnectionState::Failed)
            .unwrap();
        let exit = task.await.unwrap();
        assert!(matches!(exit, LoopExit::Fatal(HuddleError::UdpBlocked)));

        // Case G: Handshake error inside `run_connected_loop_with` propagates immediately.
        let (mut factory, _sig_tx, _) = MockFactory::ok_default();
        factory.connect_err = Some(HuddleError::Signaling("handshake fail".into()));
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut pending = VecDeque::new();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(exit, LoopExit::Fatal(HuddleError::Signaling(_))));

        // Case H: Inbound server error frame exits via `handle_signal_frame` returning `Some(exit)`.
        let (mut factory, sig_tx, _) = MockFactory::ok_default();
        factory.rtc_initial_state = RTCPeerConnectionState::Connected;
        sig_tx
            .send(Ok(Some(SdkSignalFrame {
                r#type: Some(sdk_signal_frame::Type::Leave as i32),
                error: Some(crate::proto::SdkErrorFrame {
                    status: Some(410),
                    description: Some("meeting ended".into()),
                }),
                ..Default::default()
            })))
            .unwrap();
        let (_ctrl_tx, mut ctrl_rx) = mpsc::unbounded_channel();
        let (upd_tx, _upd_rx) = mpsc::unbounded_channel();
        let mut cfg = HuddleAudioConfig::default();
        let mut snap = HuddleSessionSnapshot::default();
        let mut pending = VecDeque::new();
        let exit = run_connected_loop_with(
            &mut factory,
            &creds(),
            &mut cfg,
            &mut snap,
            &mut ctrl_rx,
            &mut pending,
            &upd_tx,
            false,
        )
        .await;
        assert!(matches!(
            exit,
            LoopExit::Fatal(HuddleError::Protocol { status: 410, .. })
        ));
    }
}
