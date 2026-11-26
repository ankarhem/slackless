use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::connected::{SessionRuntime, announce_screen_share_capabilities};
use super::io::{AudioSwitch, BackgroundDone, MediaDevices, RtcController, SignalSink};
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

impl<M: MediaDevices> SessionRuntime<'_, M> {
    /// Stop whatever audio is running and restart it on `new` (falling back to `prev`) off the
    /// event loop, so pings and controls keep being handled while the OS opens the devices. A
    /// switch already in flight owns the hardware; newer devices are applied once it reports back.
    pub(super) fn start_audio_switch(&mut self, new: HuddleAudioConfig, prev: HuddleAudioConfig) {
        if self.audio_switch_in_flight {
            return;
        }
        self.audio_switch_in_flight = true;
        let parked = self.parked_inbound_audio.take();
        let switch = self.media.switch_audio(new.clone(), prev.clone(), parked);
        let done_tx = self.bg_done_tx.clone();
        tokio::spawn(async move {
            let outcome = switch.await;
            let _ = done_tx.send(BackgroundDone::AudioSwitch { new, prev, outcome });
        });
    }

    /// Start the camera off the event loop; the result reports back tagged with its request
    /// `generation` so a superseded start is dropped when it lands.
    pub(super) fn start_camera(
        &mut self,
        generation: u64,
        device_index: Option<usize>,
        blur: bool,
        compact: bool,
    ) {
        let start = self.media.start_camera(
            device_index,
            blur,
            compact,
            self.outbound_h264_tx.clone(),
            self.local_cam_tx.clone(),
        );
        let done_tx = self.bg_done_tx.clone();
        tokio::spawn(async move {
            let result = start.await;
            let _ = done_tx.send(BackgroundDone::CameraStart { generation, result });
        });
    }

    /// Start sharing `source_id` off the event loop (a whole second Chime handshake); the result
    /// reports back tagged with its request `generation`.
    pub(super) fn start_screen_share(
        &mut self,
        generation: u64,
        source_id: String,
        share_audio: bool,
    ) {
        let start = self.media.start_screen_share(
            self.creds.clone(),
            source_id.clone(),
            share_audio,
            self.local_screen_tx.clone(),
            Arc::clone(&self.perf),
        );
        let done_tx = self.bg_done_tx.clone();
        tokio::spawn(async move {
            let result = start.await;
            let _ = done_tx.send(BackgroundDone::ScreenShareStart {
                generation,
                source_id,
                result,
            });
        });
    }
}

/// Apply the outcome of a device operation that ran off the event loop.
pub(super) async fn apply_background_done<R: RtcController, S: SignalSink, M: MediaDevices>(
    done: BackgroundDone<M>,
    config: &mut HuddleAudioConfig,
    snap: &mut HuddleSessionSnapshot,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
) {
    match done {
        BackgroundDone::AudioSwitch { new, prev, outcome } => {
            state.audio_switch_in_flight = false;
            // A request that arrived meanwhile has already moved `config` on; it is chained below.
            let superseded = config.input_device != new.input_device
                || config.output_device != new.output_device;
            let running = match outcome {
                AudioSwitch::Switched(audio) => {
                    state.media.install_audio(audio);
                    snap.error = None;
                    new
                }
                AudioSwitch::Reverted { audio, err } => {
                    tracing::error!(%err, "failed to switch huddle audio devices; reverted");
                    state.media.install_audio(audio);
                    snap.error = Some(err.to_string());
                    prev
                }
                AudioSwitch::Failed {
                    err,
                    revert_err,
                    inbound,
                } => {
                    tracing::error!(%err, "failed to switch huddle audio devices");
                    if let Some(revert_err) = revert_err {
                        tracing::error!(
                            %revert_err,
                            "failed to revert huddle audio devices; preserving inbound stream for retry"
                        );
                    }
                    state.parked_inbound_audio = inbound;
                    snap.error = Some(err.to_string());
                    prev
                }
            };
            if !superseded {
                config.input_device = running.input_device.clone();
                config.output_device = running.output_device.clone();
            }
            snap.input_device = running.input_device.clone();
            snap.output_device = running.output_device.clone();
            if config.input_device != running.input_device
                || config.output_device != running.output_device
            {
                state.start_audio_switch(config.clone(), running);
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        BackgroundDone::CameraStart { generation, result } => {
            if generation != state.camera_gen {
                return; // superseded: dropping the result stops that capture
            }
            match result {
                Ok(camera) => {
                    state.media.install_camera(camera);
                    // Settings changed while the start was in flight win over its start arguments.
                    state.media.set_camera_blur(config.blur_background);
                    state.media.set_camera_compact(snap.sharing);
                    snap.video = true;
                    snap.error = None;
                    send_camera_subscription(config, peer, signal, state, true).await;
                }
                Err(err) => {
                    snap.video = false;
                    snap.error = Some(err.to_string());
                }
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        BackgroundDone::ScreenShareStart {
            generation,
            source_id,
            result,
        } => {
            if generation != state.screen_share_gen {
                return; // superseded: dropping the result stops that share
            }
            match result {
                Ok(share) => {
                    snap.screen_audio_device = M::screen_share_loopback(&share);
                    state.media.install_screen_share(share);
                    snap.sharing = true;
                    snap.share_source_id = Some(source_id.clone());
                    snap.error = None;
                    state.media.set_camera_compact(true);
                    announce_screen_share_capabilities(
                        &state.creds.attendee_id,
                        signal,
                        &source_id,
                    )
                    .await;
                }
                Err(err) => {
                    tracing::error!(
                        source_id = %source_id,
                        %err,
                        "failed to start screen share session"
                    );
                    snap.sharing = false;
                    snap.share_source_id = None;
                    snap.error = Some(err.to_string());
                }
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
    }
}

/// Renegotiate the `SUBSCRIBE` so Chime knows whether our camera video track is live.
async fn send_camera_subscription<R: RtcController, S: SignalSink, M: MediaDevices>(
    config: &HuddleAudioConfig,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
    video: bool,
) {
    let offer = match peer.create_gathered_offer().await {
        Ok(offer) => offer,
        Err(err) => {
            tracing::warn!(%err, video, "failed to create the camera SUBSCRIBE offer");
            return;
        }
    };
    let frame = match build_subscribe_frame(
        state.creds,
        &offer,
        state.wants_compressed,
        config.muted,
        video,
        &state.remote_sources,
    ) {
        Ok(frame) => frame,
        Err(err) => {
            tracing::warn!(%err, video, "failed to build the camera SUBSCRIBE frame");
            return;
        }
    };
    if let Err(err) = signal.send_frame(&frame).await {
        tracing::warn!(%err, video, "failed to send the camera SUBSCRIBE");
    }
}

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
            if let Err(err) = signal.send_frame(&build_audio_control_frame(muted)).await {
                tracing::warn!(%err, muted, "failed to send AUDIO_CONTROL mute state");
            }
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
            let prev = config.clone();
            config.input_device = input.clone();
            config.output_device = output.clone();
            snap.input_device = input;
            snap.output_device = output;
            state.start_audio_switch(config.clone(), prev);
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
            state.camera_gen += 1;
            if enabled {
                state.start_camera(state.camera_gen, device_index, blur, snap.sharing);
            } else {
                tokio::spawn(state.media.stop_camera());
                if snap.video {
                    snap.video = false;
                    let _ = state
                        .updates
                        .send(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
                }
                send_camera_subscription(config, peer, signal, state, false).await;
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
            state.screen_share_gen += 1;
            if let Some(src_id) = source_id {
                // Native `#content` second Chime session — runs simultaneously with camera!
                state.start_screen_share(state.screen_share_gen, src_id, share_audio);
            } else {
                tokio::spawn(state.media.stop_screen_share());
                snap.sharing = false;
                snap.share_source_id = None;
                state.media.set_camera_compact(false);
                let cap_bytes = build_capabilities_message(&state.creds.attendee_id, false);
                let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, cap_bytes, 30_000);
                if let Err(err) = signal.send_frame(&frame).await {
                    tracing::warn!(%err, "failed to withdraw screen share capabilities");
                }
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
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, %emoji, "failed to send reaction");
            }
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
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, raised, "failed to send hand raise");
            }
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
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, ?emoji, "failed to send sticky emoji");
            }
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
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, "failed to send draw message");
            }
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
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, ?song_key, "failed to send jukebox message");
            }
            let _ = state
                .updates
                .send(HuddleUpdate::State(Box::new(snap.clone())));
        }
        Some(HuddleControl::MutePeer(peer_id)) => {
            let bytes = build_mute_request_message(&state.creds.attendee_id, &peer_id);
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, bytes, 5000);
            if let Err(err) = signal.send_frame(&frame).await {
                tracing::warn!(%err, %peer_id, "failed to send mute request");
            }
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
                state.camera_gen += 1;
                tokio::spawn(state.media.stop_camera());
                if snap.video {
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
                    if let Err(err) = signal.send_frame(&upd).await {
                        tracing::warn!(%err, "failed to send audio-only REMOTE_VIDEO_UPDATE");
                    }
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
