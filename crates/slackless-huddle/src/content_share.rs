//! Dedicated Amazon Chime `#content` attendee session for native screen sharing (`F-COLLAB-021`).
//!
//! Matches `amazon-chime-sdk-js`'s `DefaultContentShareController` bit-for-bit:
//! appends `#content` to both `attendee_id` and `join_token`, opens a second Chime Signaling
//! WebSocket + second `RTCPeerConnection` with silent Opus audio on mid 0 and H.264 video on
//! mid 1 (`track_label = "AmazonChimeExpressVideo"`, `stream_id = 2`, `group_id = 2`,
//! `receive_stream_ids = [0]`), so webcam video and screen sharing run simultaneously and appear
//! as a native screen share on official Slack desktop/web clients.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

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

/// Running `#content` screen-share session handle.
pub struct ContentShareSession {
    stop_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ContentShareSession {
    /// Start a native `#content` attendee sharing `source_id`, forwarding local preview frames
    /// onto `preview_tx`.
    pub async fn start(
        base_creds: HuddleCredentials,
        source_id: String,
        preview_tx: mpsc::UnboundedSender<VideoFrame>,
    ) -> Result<Self> {
        let content_creds = HuddleCredentials {
            attendee_id: format!("{}#content", base_creds.attendee_id),
            join_token: format!("{}#content", base_creds.join_token),
            ..base_creds
        };

        let mut sig = ChimeSignalClient::connect(&content_creds).await?;
        let audio_session_id: u64 = rand::random();
        sig.send_frame(&build_join_frame(audio_session_id)).await?;

        let join_ack = loop {
            let Some(frame) = sig.next_frame().await? else {
                return Err(HuddleError::Signaling(
                    "content share WS closed before JOIN_ACK".into(),
                ));
            };
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
        let sub_frame = build_content_subscribe_frame(&content_creds, &offer_sdp, true)?;
        sig.send_frame(&sub_frame).await?;

        let sub_ack = loop {
            let Some(frame) = sig.next_frame().await? else {
                return Err(HuddleError::Signaling(
                    "content share WS closed before SUBSCRIBE_ACK".into(),
                ));
            };
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

        let (h264_tx, mut h264_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let capture = ScreenCapture::start(source_id, h264_tx, preview_tx)?;
        let (stop_tx, mut stop_rx) = tokio::sync::oneshot::channel::<()>();

        let running = Arc::new(AtomicBool::new(true));
        let running_audio = Arc::clone(&running);
        let rtc_audio = Arc::clone(&rtc);
        // Keep the audio m-line alive with 20ms stereo Opus silence (matching official Chime SDK).
        tokio::spawn(async move {
            let Ok(mut enc) =
                opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Voip)
            else {
                return;
            };
            let pcm = vec![0i16; 960 * 2];
            let mut out = vec![0u8; 4000];
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            while running_audio.load(Ordering::Relaxed) {
                tick.tick().await;
                if let Ok(n) = enc.encode(&pcm, &mut out) {
                    let _ = rtc_audio.write_opus_frame(out[..n].to_vec()).await;
                }
            }
        });

        tokio::spawn(async move {
            let _capture = capture;
            let mut metric_tick = tokio::time::interval(Duration::from_secs(1));
            metric_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut connected = false;

            loop {
                tokio::select! {
                    _ = &mut stop_rx => break,
                    Some(st) = state_rx.recv() => {
                        match st {
                            RTCPeerConnectionState::Connected => connected = true,
                            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => break,
                            _ => {}
                        }
                    }
                    Some(annex_b) = h264_rx.recv() => {
                        if connected {
                            let _ = rtc.write_h264_frame(annex_b, Duration::from_millis(125)).await;
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
            let _ = sig.send_frame(&build_leave_frame()).await;
            sig.close().await;
            rtc.close().await;
        });

        Ok(Self {
            stop_tx: Some(stop_tx),
        })
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for ContentShareSession {
    fn drop(&mut self) {
        self.stop();
    }
}
