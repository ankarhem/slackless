use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::connected::SessionRuntime;
use super::signaling_handlers::content_share_source_type;
use super::supervisor::LoopExit;
use crate::audio::{AmbientMusicPlayer, AudioEngine};
use crate::content_share::ContentShareSession;
use crate::peer_message::{
    SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message, build_content_share_details_message,
    build_draw_message, build_jukebox_message, build_mute_request_message, build_reacji_message,
    build_sticky_emoji_awareness,
};
use crate::rtc::ChimeRtcPeer;
use crate::signaling::{
    ChimeSignalClient, build_audio_control_frame, build_data_message_frame,
    build_remote_video_update_frame, build_subscribe_frame, now_ms,
};
use crate::video::{CameraCapture, VideoTileKey};
use crate::{
    HuddleAudioConfig, HuddleControl, HuddleDataEvent, HuddleSessionSnapshot, HuddleUpdate,
};

pub(super) async fn handle_control(
    ctrl: Option<HuddleControl>,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    peer: &ChimeRtcPeer,
    signal: &mut ChimeSignalClient,
    state: &mut SessionRuntime<'_>,
) -> Option<LoopExit> {
    match ctrl {
        Some(HuddleControl::SetMuted(muted)) => {
            config.muted = muted;
            state.shared.muted.store(muted, Ordering::Relaxed);
            snap.muted = muted;
            let _ = signal.send_frame(&build_audio_control_frame(muted)).await;
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetAudioProcessing { aec, ns, agc }) => {
            config.aec = aec;
            config.ns = ns;
            config.agc = agc;
            snap.aec = aec;
            snap.ns = ns;
            snap.agc = agc;
            state.shared.aec.store(aec, Ordering::Relaxed);
            state.shared.ns.store(ns, Ordering::Relaxed);
            state.shared.agc.store(agc, Ordering::Relaxed);
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetAudioDevices { input, output }) => {
            config.input_device = input.clone();
            config.output_device = output.clone();
            snap.input_device = input;
            snap.output_device = output;
            if let Some(rx) = state.audio_engine.stop()
                && let Ok(restarted) = AudioEngine::start(
                    config,
                    state.shared.clone(),
                    state.outbound_opus_tx.clone(),
                    rx,
                )
            {
                state.audio_engine = restarted;
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetCamera {
            enabled,
            device_index,
            blur,
        }) => {
            config.camera_device = device_index;
            config.blur_background = blur;
            snap.blur_background = blur;
            if enabled {
                if let Some(mut prev) = state.camera_capture.take() {
                    prev.stop();
                }
                match CameraCapture::start(
                    device_index,
                    blur,
                    snap.sharing,
                    state.outbound_h264_tx.clone(),
                    state.local_cam_tx.clone(),
                ) {
                    Ok(cap) => {
                        state.camera_capture = Some(cap);
                        snap.video = true;
                        snap.error = None;
                        if let Ok(offer) = peer.create_gathered_offer().await
                            && let Ok(f) = build_subscribe_frame(
                                state.creds,
                                &offer,
                                state.wants_compressed,
                                config.muted,
                                true,
                                &state.remote_sources,
                            )
                        {
                            let _ = signal.send_frame(&f).await;
                        }
                    }
                    Err(err) => {
                        snap.video = false;
                        snap.error = Some(err.to_string());
                    }
                }
            } else {
                if let Some(mut cap) = state.camera_capture.take() {
                    cap.stop();
                }
                snap.video = false;
                let _ = state
                    .updates
                    .send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
                if let Ok(offer) = peer.create_gathered_offer().await
                    && let Ok(f) = build_subscribe_frame(
                        state.creds,
                        &offer,
                        state.wants_compressed,
                        config.muted,
                        false,
                        &state.remote_sources,
                    )
                {
                    let _ = signal.send_frame(&f).await;
                }
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetBlurBackground(blur)) => {
            config.blur_background = blur;
            snap.blur_background = blur;
            if let Some(cap) = &state.camera_capture {
                cap.set_blur(blur);
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetScreenShare {
            source_id,
            share_audio,
        }) => {
            if let Some(mut prev) = state.content_share.take() {
                prev.stop_and_wait().await;
            }
            if let Some(src_id) = source_id {
                // Native `#content` second Chime session — runs simultaneously with camera!
                match ContentShareSession::start(
                    state.creds.clone(),
                    src_id.clone(),
                    share_audio,
                    state.local_screen_tx.clone(),
                    Arc::clone(&state.perf),
                )
                .await
                {
                    Ok(cs) => {
                        snap.screen_audio_device = cs.loopback_device().map(ToOwned::to_owned);
                        state.content_share = Some(cs);
                        snap.sharing = true;
                        snap.share_source_id = Some(src_id.clone());
                        snap.error = None;
                        if let Some(cap) = &state.camera_capture {
                            cap.set_compact(true);
                        }
                        // Announce desktop draw capability + content source details on
                        // official `"data-channel"` (`desktop:screen` / `desktop:window`
                        // is required by Official Slack to enable remote drawing!).
                        let cap_bytes = build_capabilities_message(&state.creds.attendee_id, true);
                        let _ = signal
                            .send_frame(&build_data_message_frame(
                                SLACK_DATA_CHANNEL_TOPIC,
                                cap_bytes,
                                300_000,
                            ))
                            .await;
                        let det_bytes = build_content_share_details_message(
                            &state.creds.attendee_id,
                            content_share_source_type(&src_id),
                        );
                        let _ = signal
                            .send_frame(&build_data_message_frame(
                                SLACK_DATA_CHANNEL_TOPIC,
                                det_bytes,
                                300_000,
                            ))
                            .await;
                    }
                    Err(err) => {
                        tracing::error!(
                            source_id = %src_id,
                            %err,
                            "failed to start screen share session"
                        );
                        snap.sharing = false;
                        snap.share_source_id = None;
                        snap.error = Some(err.to_string());
                    }
                }
            } else {
                snap.sharing = false;
                snap.share_source_id = None;
                if let Some(cap) = &state.camera_capture {
                    cap.set_compact(false);
                }
                let cap_bytes = build_capabilities_message(&state.creds.attendee_id, false);
                let _ = signal
                    .send_frame(&build_data_message_frame(
                        SLACK_DATA_CHANNEL_TOPIC,
                        cap_bytes,
                        30_000,
                    ))
                    .await;
                let _ = state
                    .updates
                    .send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen));
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SendReaction {
            emoji,
            kind,
            emoji_url,
        }) => {
            let bytes =
                build_reacji_message(&state.creds.attendee_id, &emoji, kind, emoji_url.clone());
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 4000);
            let _ = signal.send_frame(&frame).await;
            let _ = state
                .updates
                .send(HuddleUpdate::DataEvent(HuddleDataEvent::Reaction {
                    user: state.self_user.clone(),
                    emoji,
                    kind,
                    emoji_url,
                    at_ms: now_ms(),
                }));
        }
        Some(HuddleControl::SetHandRaised(raised)) => {
            snap.hand_raised = raised;
            let sticky = raised.then(|| "raised_hand".to_owned());
            snap.sticky_emoji = sticky.clone();
            let bytes = build_sticky_emoji_awareness(
                &state.creds.attendee_id,
                &state.creds.call_id,
                &state.self_user,
                sticky.as_deref(),
                now_ms() as i64,
            );
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 30_000);
            let _ = signal.send_frame(&frame).await;
            let _ = state
                .updates
                .send(HuddleUpdate::DataEvent(HuddleDataEvent::HandRaise {
                    user: state.self_user.clone(),
                    raised,
                }));
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetStickyEmoji(emoji)) => {
            snap.sticky_emoji = emoji.clone();
            snap.hand_raised = emoji.as_deref() == Some("raised_hand");
            let bytes = build_sticky_emoji_awareness(
                &state.creds.attendee_id,
                &state.creds.call_id,
                &state.self_user,
                emoji.as_deref(),
                now_ms() as i64,
            );
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 30_000);
            let _ = signal.send_frame(&frame).await;
            let _ = state
                .updates
                .send(HuddleUpdate::DataEvent(HuddleDataEvent::StickyEmoji {
                    user: state.self_user.clone(),
                    emoji,
                }));
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SendDraw(draw_msg)) => {
            let bytes = build_draw_message(&state.creds.attendee_id, &draw_msg);
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
            let _ = signal.send_frame(&frame).await;
            let _ = state
                .updates
                .send(HuddleUpdate::DataEvent(HuddleDataEvent::Draw {
                    user: state.self_user.clone(),
                    message: draw_msg,
                    at_ms: now_ms(),
                }));
        }
        Some(HuddleControl::SetJukebox(song_key)) => {
            if let Some(mut prev) = state.music_player.take() {
                prev.stop();
            }
            if song_key.is_some() {
                state.music_player =
                    AmbientMusicPlayer::start(config.output_device.clone(), song_key.as_deref())
                        .ok();
            }
            snap.jukebox_song = song_key.clone();
            let bytes = build_jukebox_message(&state.creds.attendee_id, song_key.as_deref());
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 15_000);
            let _ = signal.send_frame(&frame).await;
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::MutePeer(peer_id)) => {
            let bytes = build_mute_request_message(&state.creds.attendee_id, &peer_id);
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
            let _ = signal.send_frame(&frame).await;
        }
        Some(HuddleControl::SetTopic(topic)) => {
            snap.topic = (!topic.trim().is_empty()).then(|| topic.clone());
            let _ = state
                .updates
                .send(HuddleUpdate::DataEvent(HuddleDataEvent::Topic {
                    user: state.self_user.clone(),
                    topic,
                }));
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetBackground(bg_id)) => {
            snap.background_id = bg_id;
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetCaptionsEnabled(enabled)) => {
            snap.captions_enabled = enabled;
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetAudioOnly(audio_only)) => {
            snap.audio_only = audio_only;
            state.vdec_audio_only.store(audio_only, Ordering::Relaxed);
            if audio_only {
                if let Some(mut cap) = state.camera_capture.take() {
                    cap.stop();
                    snap.video = false;
                    let _ = state
                        .updates
                        .send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
                }
                let removed_mids: Vec<String> = state
                    .remote_sources
                    .iter()
                    .map(|(m, _)| m.clone())
                    .collect();
                if !removed_mids.is_empty() {
                    let upd = build_remote_video_update_frame(Vec::new(), removed_mids);
                    let _ = signal.send_frame(&upd).await;
                }
                state.remote_sources.clear();
                if let Ok(mut rt) = state.vdec_routing.lock() {
                    rt.remote_sources.clear();
                }
                state.vdec_clear.store(true, Ordering::Relaxed);
                for tile in state.active_remote_tiles.drain() {
                    let _ = state.updates.send(HuddleUpdate::VideoTileClosed(tile));
                }
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::Leave) | None => {
            return Some(LoopExit::LeftCleanly);
        }
    }
    None
}
