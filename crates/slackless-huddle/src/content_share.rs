//! Dedicated Amazon Chime `#content` attendee session for native screen sharing (`F-COLLAB-021`).
//!
//! Matches `amazon-chime-sdk-js`'s `DefaultContentShareController` bit-for-bit:
//! appends `#content` to both `attendee_id` and `join_token`, opens a second Chime Signaling
//! WebSocket + second `RTCPeerConnection` with silent Opus audio on mid 0 and H.264 video on
//! mid 1 (`track_label = "AmazonChimeExpressVideo"`, `stream_id = 2`, `group_id = 2`,
//! `receive_stream_ids = [0]`), so webcam video and screen sharing run simultaneously and appear
//! as a native screen share on official Slack desktop/web clients.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use tokio::sync::mpsc;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::metrics::HuddlePerfCounters;
use crate::proto::{SdkPingPongType, sdk_signal_frame};
use crate::rtc::ChimeRtcPeer;
use crate::screen::ScreenCapture;
use crate::signaling::{
    ChimeSignalClient, ClientMetricsSnapshot, build_client_metric_frame,
    build_content_subscribe_frame, build_join_frame, build_leave_frame, build_pong_frame,
    decode_sdp_answer,
};
use crate::video::VideoFrame;
use crate::{HuddleCredentials, HuddleError, Result};

/// Detect an installed virtual system-audio loopback input device (`BlackHole`, `Loopback`,
/// `Soundflower`, `VB-Cable`, PulseAudio/PipeWire `Monitor of ...`) for screen-share audio
/// capture (`F-COLLAB-022`).
pub fn detect_system_audio_loopback() -> Option<String> {
    let host = cpal::default_host();
    let inputs = host.input_devices().ok()?;
    for dev in inputs {
        let Ok(name) = dev.name() else { continue };
        let lower = name.to_ascii_lowercase();
        if lower.contains("blackhole")
            || lower.contains("loopback")
            || lower.contains("soundflower")
            || lower.contains("vb-cable")
            || lower.contains("cable output")
            || lower.contains("monitor of")
        {
            return Some(name);
        }
    }
    None
}

/// Human-readable status of system audio capture for screen sharing (`F-COLLAB-022`).
/// Prefers macOS 14.2+ native CoreAudio Process Tap (zero driver needed), then virtual loopback.
pub fn system_audio_source_label() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        if crate::macos_audio_tap::supports_native_system_audio_tap() {
            return Some("macOS Native System Audio".to_owned());
        }
    }
    detect_system_audio_loopback()
}

/// Running `#content` screen-share session handle.
pub struct ContentShareSession {
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
    done_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    loopback_device: Option<String>,
}

impl ContentShareSession {
    /// Name of the system audio loopback device feeding `#content` audio, if detected.
    pub fn loopback_device(&self) -> Option<&str> {
        self.loopback_device.as_deref()
    }
    /// Start a native `#content` attendee sharing `source_id`, forwarding local preview frames
    /// onto `preview_tx`. When `share_audio` is `true`, captures system audio via macOS CoreAudio
    /// Process Tap (14.2+) or a virtual loopback device.
    pub async fn start(
        base_creds: HuddleCredentials,
        source_id: String,
        share_audio: bool,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
        perf: Arc<HuddlePerfCounters>,
    ) -> Result<Self> {
        let content_creds = HuddleCredentials {
            attendee_id: format!("{}#content", base_creds.attendee_id),
            join_token: format!("{}#content", base_creds.join_token),
            ..base_creds
        };

        tracing::info!(
            source_id = %source_id,
            attendee = %content_creds.attendee_id,
            "starting #content screen share session"
        );
        let mut sig = ChimeSignalClient::connect(&content_creds).await?;
        let audio_session_id: u64 = rand::random();
        sig.send_frame(&build_join_frame(audio_session_id)).await?;

        let join_ack = loop {
            let Some(frame) = sig.next_frame().await? else {
                return Err(HuddleError::Signaling(
                    "content share WS closed before JOIN_ACK".into(),
                ));
            };
            if let Some(err) = frame.error {
                return Err(HuddleError::Protocol {
                    status: err.status.unwrap_or(500),
                    description: err.description.unwrap_or_default(),
                });
            }
            if frame.r#type == Some(sdk_signal_frame::Type::JoinAck as i32) {
                break frame.joinack.ok_or_else(|| {
                    HuddleError::Signaling("content JOIN_ACK missing payload".into())
                })?;
            }
        };
        let turn = join_ack
            .turn_credentials
            .as_ref()
            .ok_or_else(|| HuddleError::Signaling("content JOIN_ACK missing TURN creds".into()))?;

        let (dummy_opus_tx, _dummy_opus_rx) = mpsc::unbounded_channel();
        let (dummy_video_tx, _dummy_video_rx) = mpsc::unbounded_channel();
        let (state_tx, mut state_rx) = mpsc::unbounded_channel();
        let rtc = Arc::new(
            ChimeRtcPeer::new_content_sender(turn, dummy_opus_tx, dummy_video_tx, state_tx).await?,
        );

        let offer_sdp = rtc.create_gathered_offer().await?;
        let sub_frame = build_content_subscribe_frame(&content_creds, &offer_sdp, false)?;
        sig.send_frame(&sub_frame).await?;

        let sub_ack = loop {
            let Some(frame) = sig.next_frame().await? else {
                return Err(HuddleError::Signaling(
                    "content share WS closed before SUBSCRIBE_ACK".into(),
                ));
            };
            if let Some(err) = frame.error {
                return Err(HuddleError::Protocol {
                    status: err.status.unwrap_or(500),
                    description: err.description.unwrap_or_default(),
                });
            }
            if frame.r#type == Some(sdk_signal_frame::Type::SubscribeAck as i32) {
                break frame.suback.ok_or_else(|| {
                    HuddleError::Signaling("content SUBSCRIBE_ACK missing payload".into())
                })?;
            }
        };
        let answer = decode_sdp_answer(
            sub_ack.sdp_answer.as_deref(),
            sub_ack.compressed_sdp_answer.as_deref(),
        )?;
        rtc.set_remote_answer(answer).await?;
        tracing::info!(
            source_id = %source_id,
            "#content SUBSCRIBE_ACK applied; starting screen capture"
        );

        let (h264_tx, mut h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let capture = ScreenCapture::start(source_id, h264_tx, preview_tx, perf)?;
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();

        let running = Arc::new(AtomicBool::new(true));
        let running_audio = Arc::clone(&running);
        let rtc_audio = Arc::clone(&rtc);
        let loopback_pcm: Arc<Mutex<VecDeque<i16>>> =
            Arc::new(Mutex::new(VecDeque::with_capacity(48_000 * 2)));
        let loopback_capture_running = Arc::clone(&running);
        let loopback_pcm_tx = Arc::clone(&loopback_pcm);

        // Priority 1 (macOS 14.2+): Zero-driver CoreAudio Process Tap (`F-COLLAB-022`).
        #[cfg(target_os = "macos")]
        let native_tap = if share_audio
            && crate::macos_audio_tap::supports_native_system_audio_tap()
        {
            match crate::macos_audio_tap::SystemAudioTap::start(Arc::clone(&loopback_pcm)) {
                Ok(tap) => Some(tap),
                Err(err) => {
                    tracing::warn!(
                        %err,
                        "macOS native system audio tap unavailable; continuing without native tap"
                    );
                    None
                }
            }
        } else {
            None
        };
        #[cfg(not(target_os = "macos"))]
        let native_tap: Option<()> = None;

        let loopback_device = if !share_audio {
            None
        } else if native_tap.is_some() {
            Some("macOS Native System Audio".to_owned())
        } else {
            detect_system_audio_loopback()
        };

        if native_tap.is_none() {
            if let Some(dev_name) = loopback_device.clone() {
                let _ = std::thread::Builder::new()
                    .name("slackless-screen-audio".to_owned())
                    .spawn(move || {
                        let host = cpal::default_host();
                        let Some(dev) = host.input_devices().ok().and_then(|mut it| {
                            it.find(|d| d.name().ok().as_deref() == Some(dev_name.as_str()))
                        }) else {
                            return;
                        };
                        let Ok(supported) = dev.default_input_config() else {
                            return;
                        };
                        let fmt = supported.sample_format();
                        let ch = supported.channels() as usize;
                        let cfg: cpal::StreamConfig = supported.into();
                        let push = move |stereo: Vec<i16>| {
                            if let Ok(mut q) = loopback_pcm_tx.lock() {
                                if q.len() > 48_000 * 4 {
                                    q.drain(..48_000);
                                }
                                q.extend(stereo);
                            }
                        };
                        let stream = match fmt {
                            SampleFormat::F32 => dev
                                .build_input_stream(
                                    &cfg,
                                    move |data: &[f32], _| {
                                        let mut out =
                                            Vec::with_capacity((data.len() / ch.max(1)) * 2);
                                        for frame in data.chunks(ch.max(1)) {
                                            let l = (frame[0].clamp(-1.0, 1.0) * 32767.0) as i16;
                                            let r = (frame
                                                .get(1)
                                                .copied()
                                                .unwrap_or(frame[0])
                                                .clamp(-1.0, 1.0)
                                                * 32767.0)
                                                as i16;
                                            out.push(l);
                                            out.push(r);
                                        }
                                        push(out);
                                    },
                                    |_| {},
                                    None,
                                )
                                .ok(),
                            SampleFormat::I16 => dev
                                .build_input_stream(
                                    &cfg,
                                    move |data: &[i16], _| {
                                        let mut out =
                                            Vec::with_capacity((data.len() / ch.max(1)) * 2);
                                        for frame in data.chunks(ch.max(1)) {
                                            out.push(frame[0]);
                                            out.push(*frame.get(1).unwrap_or(&frame[0]));
                                        }
                                        push(out);
                                    },
                                    |_| {},
                                    None,
                                )
                                .ok(),
                            _ => None,
                        };
                        let Some(stream) = stream else { return };
                        let _ = stream.play();
                        while loopback_capture_running.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                    });
            }
        }
        // Feed live system audio (CoreAudio tap or loopback, `F-COLLAB-022`) or 20ms stereo Opus
        // silence (matching official Chime SDK) on the `#content` audio m-line.
        tokio::spawn(async move {
            let _tap_guard = native_tap;
            let Ok(mut enc) =
                opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)
            else {
                return;
            };
            let silence = vec![0i16; 960 * 2];
            let mut pcm = vec![0i16; 960 * 2];
            let mut out = vec![0u8; 4000];
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while running_audio.load(Ordering::Relaxed) {
                tick.tick().await;
                let has_audio = loopback_pcm.lock().ok().is_some_and(|mut q| {
                    if q.len() >= 960 * 2 {
                        for dst in &mut pcm {
                            *dst = q.pop_front().unwrap_or(0);
                        }
                        true
                    } else {
                        false
                    }
                });
                let frame = if has_audio { &pcm } else { &silence };
                if let Ok(n) = enc.encode(frame, &mut out) {
                    let _ = rtc_audio.write_opus_frame(out[..n].to_vec()).await;
                }
            }
        });

        tokio::spawn(async move {
            let capture = capture;
            let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
            metric_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut connected = false;
            let mut sent_first_frame = false;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    Some(st) = state_rx.recv() => {
                        tracing::info!(state = ?st, "#content peer connection state");
                        match st {
                            RTCPeerConnectionState::Connected => {
                                connected = true;
                                // Force an immediate IDR keyframe once WebRTC media is open.
                                capture.force_keyframe();
                            }
                            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => break,
                            _ => {}
                        }
                    }
                    Some(annex_b) = h264_rx.recv() => {
                        if connected {
                            match rtc.write_h264_frame(annex_b, Duration::from_millis(33)).await {
                                Ok(()) if !sent_first_frame => {
                                    sent_first_frame = true;
                                    tracing::info!("#content first H.264 screen frame sent");
                                }
                                Ok(()) => {}
                                Err(err) => {
                                    tracing::warn!(%err, "#content write_h264_frame error");
                                }
                            }
                        }
                    }
                    _ = metric_tick.tick() => {
                        let stats = ClientMetricsSnapshot {
                            stun_rtt_ms: 25.0,
                            ..Default::default()
                        };
                        let _ = sig.send_frame(&build_client_metric_frame(stats)).await;
                    }
                    res = sig.next_frame() => {
                        match res {
                            Ok(Some(frame)) => {
                                if frame.r#type == Some(sdk_signal_frame::Type::PingPong as i32) {
                                    if let Some(pp) = frame.ping_pong {
                                        if pp.r#type == Some(SdkPingPongType::Ping as i32) {
                                            let _ = sig.send_frame(&build_pong_frame(pp.ping_id.unwrap_or(0))).await;
                                        }
                                    }
                                } else if frame.r#type == Some(sdk_signal_frame::Type::Leave as i32) {
                                    break;
                                }
                            }
                            Ok(None) | Err(_) => break,
                        }
                    }
                }
            }

            running.store(false, Ordering::Relaxed);
            drop(capture);
            let _ = sig.send_frame(&build_leave_frame()).await;
            let _ = tokio::time::timeout(Duration::from_millis(300), sig.next_frame()).await;
            sig.close().await;
            rtc.close().await;
            let _ = done_tx.send(());
        });

        Ok(Self {
            stop_tx: Some(stop_tx),
            done_rx: Some(done_rx),
            loopback_device,
        })
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }

    /// Stop the `#content` share and wait briefly for its Chime `LEAVE` frame to flush.
    pub async fn stop_and_wait(&mut self) {
        self.stop();
        if let Some(rx) = self.done_rx.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), rx).await;
        }
    }
}

impl Drop for ContentShareSession {
    fn drop(&mut self) {
        self.stop();
    }
}
