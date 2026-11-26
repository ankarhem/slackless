use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::connected::{SessionRuntime, announce_screen_share_capabilities};
use super::io::{MediaDevices, RtcController, SignalSink};
use super::supervisor::LoopExit;
use crate::peer_message::{
    SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message, build_draw_message,
    build_jukebox_message, build_mute_request_message, build_reacji_message,
    build_sticky_emoji_awareness,
};
use crate::signaling::{
    build_audio_control_frame, build_data_message_frame, build_remote_video_update_frame,
    build_subscribe_frame, now_ms,
};
use crate::video::VideoTileKey;
use crate::{
    HuddleAudioConfig, HuddleControl, HuddleDataEvent, HuddleSessionSnapshot, HuddleUpdate,
};

pub(super) async fn handle_control<R: RtcController, S: SignalSink, M: MediaDevices>(
    ctrl: Option<HuddleControl>,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
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
            let prev_input = config.input_device.clone();
            let prev_output = config.output_device.clone();
            config.input_device = input.clone();
            config.output_device = output.clone();
            snap.input_device = input;
            snap.output_device = output;
            let inbound_rx = state
                .media
                .stop_audio()
                .or_else(|| state.parked_inbound_audio.take());
            if let Some(rx) = inbound_rx {
                match state.media.start_audio_recoverable(
                    config,
                    state.shared.clone(),
                    state.outbound_opus_tx.clone(),
                    rx,
                ) {
                    Ok(()) => {
                        snap.error = None;
                    }
                    Err((err, recovered_rx)) => {
                        tracing::error!(%err, "failed to switch huddle audio devices; reverting");
                        snap.error = Some(err.to_string());
                        config.input_device = prev_input.clone();
                        config.output_device = prev_output.clone();
                        snap.input_device = prev_input;
                        snap.output_device = prev_output;
                        if let Err((revert_err, parked_rx)) = state.media.start_audio_recoverable(
                            config,
                            state.shared.clone(),
                            state.outbound_opus_tx.clone(),
                            recovered_rx,
                        ) {
                            tracing::error!(
                                %revert_err,
                                "failed to revert huddle audio devices; preserving rx for retry"
                            );
                            state.parked_inbound_audio = Some(parked_rx);
                        }
                    }
                }
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
                match state.media.start_camera(
                    device_index,
                    blur,
                    snap.sharing,
                    state.outbound_h264_tx.clone(),
                    state.local_cam_tx.clone(),
                ) {
                    Ok(()) => {
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
                state.media.stop_camera();
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
            state.media.set_camera_blur(blur);
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::SetScreenShare {
            source_id,
            share_audio,
        }) => {
            state.media.stop_screen_share().await;
            if let Some(src_id) = source_id {
                // Native `#content` second Chime session — runs simultaneously with camera!
                match state
                    .media
                    .start_screen_share(
                        state.creds.clone(),
                        src_id.clone(),
                        share_audio,
                        state.local_screen_tx.clone(),
                        Arc::clone(&state.perf),
                    )
                    .await
                {
                    Ok(loopback_device) => {
                        snap.screen_audio_device = loopback_device;
                        snap.sharing = true;
                        snap.share_source_id = Some(src_id.clone());
                        snap.error = None;
                        state.media.set_camera_compact(true);
                        announce_screen_share_capabilities(
                            &state.creds.attendee_id,
                            signal,
                            &src_id,
                        )
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
                state.media.set_camera_compact(false);
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
            state.set_jukebox_player(config.output_device.clone(), song_key.as_deref());
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
                if state.media.stop_camera() {
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
