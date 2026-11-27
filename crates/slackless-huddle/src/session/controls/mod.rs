//! Applying [`HuddleControl`]s from the UI to a running session: one [`Control`] method per
//! control (the `PeerMessage` ones live in [`peer_messages`]).

mod peer_messages;

use std::sync::atomic::Ordering;
use std::time::Instant;

use super::connected::SessionRuntime;
use super::io::{RtcController, SignalSink};
use super::media::MediaDevices;
use super::signaling_handlers::{SUBSCRIBE_IN_FLIGHT_TIMEOUT, apply_index_frame};
use super::supervisor::LoopExit;
use crate::peer_message::{SLACK_DATA_CHANNEL_TOPIC, build_capabilities_message};
use crate::signaling::{
    build_audio_control_frame, build_data_message_frame, build_remote_video_update_frame,
    build_subscribe_frame,
};
use crate::video::VideoTileKey;
use crate::{
    HuddleAudioConfig, HuddleControl, HuddleDataEvent, HuddleSessionSnapshot, HuddleUpdate,
};

/// Renegotiate the `SUBSCRIBE` so Chime knows whether our camera video track is live.
pub(super) async fn send_camera_subscription<R: RtcController, S: SignalSink, M: MediaDevices>(
    config: &HuddleAudioConfig,
    peer: &R,
    signal: &mut S,
    state: &mut SessionRuntime<'_, M>,
    video: bool,
) {
    if state
        .subscribe_in_flight
        .is_some_and(|t| t.elapsed() < SUBSCRIBE_IN_FLIGHT_TIMEOUT)
    {
        state.pending_resubscribe = true;
        return;
    }
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
    } else {
        state.subscribe_in_flight = Some(Instant::now());
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
    let mut ctl = Control {
        config,
        snap,
        peer,
        signal,
        state,
    };
    match ctrl {
        Some(HuddleControl::SetMuted(muted)) => ctl.set_muted(muted).await,
        Some(HuddleControl::SetAudioDevices { input, output }) => {
            ctl.set_audio_devices(input, output);
        }
        Some(HuddleControl::SetAudioProcessing { aec, ns, agc }) => {
            ctl.set_audio_processing(aec, ns, agc);
        }
        Some(HuddleControl::SetCamera {
            enabled,
            device_index,
            blur,
        }) => ctl.set_camera(enabled, device_index, blur).await,
        Some(HuddleControl::SetBlurBackground(blur)) => ctl.set_blur_background(blur),
        Some(HuddleControl::SetScreenShare {
            source_id,
            share_audio,
        }) => ctl.set_screen_share(source_id, share_audio).await,
        Some(HuddleControl::SendReaction {
            emoji,
            kind,
            emoji_url,
        }) => ctl.send_reaction(emoji, kind, emoji_url).await,
        Some(HuddleControl::SetHandRaised(raised)) => ctl.set_hand_raised(raised).await,
        Some(HuddleControl::SetStickyEmoji(emoji)) => ctl.set_sticky_emoji(emoji).await,
        Some(HuddleControl::SendDraw(draw_msg)) => ctl.send_draw(draw_msg).await,
        Some(HuddleControl::SetJukebox(song_key)) => ctl.set_jukebox(song_key).await,
        Some(HuddleControl::MutePeer(peer_id)) => ctl.mute_peer(peer_id).await,
        Some(HuddleControl::SetTopic(topic)) => ctl.set_topic(topic),
        Some(HuddleControl::SetBackground(bg_id)) => ctl.set_background(bg_id),
        Some(HuddleControl::SetCaptionsEnabled(enabled)) => ctl.set_captions_enabled(enabled),
        Some(HuddleControl::SetAudioOnly(audio_only)) => ctl.set_audio_only(audio_only).await,
        Some(HuddleControl::Leave) | None => return Some(LoopExit::LeftCleanly),
    }
    None
}

/// What handling one [`HuddleControl`] may touch; one method per control.
struct Control<'c, 'a, R, S, M: MediaDevices> {
    config: &'c mut HuddleAudioConfig,
    snap: &'c mut HuddleSessionSnapshot,
    peer: &'c R,
    signal: &'c mut S,
    state: &'c mut SessionRuntime<'a, M>,
}

impl<R: RtcController, S: SignalSink, M: MediaDevices> Control<'_, '_, R, S, M> {
    fn publish(&self, update: HuddleUpdate) {
        let _ = self.state.updates.send(update);
    }

    fn publish_state(&self) {
        self.publish(HuddleUpdate::State(Box::new(self.snap.clone())));
    }

    fn close_local_camera_tile(&mut self) {
        if self.snap.video {
            self.snap.video = false;
            self.publish(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalCamera));
        }
    }

    async fn set_muted(&mut self, muted: bool) {
        self.config.muted = muted;
        self.state.shared.muted.store(muted, Ordering::Relaxed);
        self.snap.muted = muted;
        if let Err(err) = self
            .signal
            .send_frame(&build_audio_control_frame(muted))
            .await
        {
            tracing::warn!(%err, muted, "failed to send AUDIO_CONTROL mute state");
        }
        self.publish_state();
    }

    fn set_audio_processing(&mut self, aec: bool, ns: bool, agc: bool) {
        self.config.aec = aec;
        self.config.ns = ns;
        self.config.agc = agc;
        self.snap.aec = aec;
        self.snap.ns = ns;
        self.snap.agc = agc;
        self.state.shared.aec.store(aec, Ordering::Relaxed);
        self.state.shared.ns.store(ns, Ordering::Relaxed);
        self.state.shared.agc.store(agc, Ordering::Relaxed);
        self.publish_state();
    }

    fn set_audio_devices(&mut self, input: Option<String>, output: Option<String>) {
        let prev = self.config.clone();
        self.config.input_device = input.clone();
        self.config.output_device = output.clone();
        self.snap.input_device = input;
        self.snap.output_device = output;
        self.state.start_audio_switch(self.config.clone(), prev);
        self.publish_state();
    }

    async fn set_camera(&mut self, enabled: bool, device_index: Option<usize>, blur: bool) {
        self.config.camera_device = device_index;
        self.config.blur_background = blur;
        self.snap.blur_background = blur;
        self.state.camera_gen += 1;
        if enabled {
            let compact = self.snap.sharing;
            self.state
                .start_camera(self.state.camera_gen, device_index, blur, compact);
        } else {
            tokio::spawn(self.state.media.stop_camera());
            self.close_local_camera_tile();
            send_camera_subscription(self.config, self.peer, self.signal, self.state, false).await;
        }
        self.publish_state();
    }

    fn set_blur_background(&mut self, blur: bool) {
        self.config.blur_background = blur;
        self.snap.blur_background = blur;
        self.state.media.set_camera_blur(blur);
        self.publish_state();
    }

    async fn set_screen_share(&mut self, source_id: Option<String>, share_audio: bool) {
        self.state.screen_share_gen += 1;
        if let Some(src_id) = source_id {
            // Native `#content` second Chime session — runs simultaneously with camera!
            self.state
                .start_screen_share(self.state.screen_share_gen, src_id, share_audio);
        } else {
            tokio::spawn(self.state.media.stop_screen_share());
            self.snap.sharing = false;
            self.snap.share_source_id = None;
            self.snap.screen_audio_device = None;
            self.state.media.set_camera_compact(false);
            let cap_bytes = build_capabilities_message(&self.state.creds.attendee_id, false);
            let frame = build_data_message_frame(SLACK_DATA_CHANNEL_TOPIC, cap_bytes, 30_000);
            if let Err(err) = self.signal.send_frame(&frame).await {
                tracing::warn!(%err, "failed to withdraw screen share capabilities");
            }
            self.publish(HuddleUpdate::VideoTileClosed(VideoTileKey::LocalScreen));
        }
        self.publish_state();
    }

    fn set_topic(&mut self, topic: String) {
        self.snap.topic = (!topic.trim().is_empty()).then(|| topic.clone());
        self.publish(HuddleUpdate::DataEvent(HuddleDataEvent::Topic {
            user: self.state.self_user.clone(),
            topic,
        }));
        self.publish_state();
    }

    fn set_background(&mut self, bg_id: Option<String>) {
        self.snap.background_id = bg_id;
        self.publish_state();
    }

    fn set_captions_enabled(&mut self, enabled: bool) {
        self.snap.captions_enabled = enabled;
        self.publish_state();
    }

    async fn set_audio_only(&mut self, audio_only: bool) {
        self.snap.audio_only = audio_only;
        self.state
            .vdec_audio_only
            .store(audio_only, Ordering::Relaxed);
        if audio_only {
            let had_video = self.snap.video;
            self.state.camera_gen += 1;
            tokio::spawn(self.state.media.stop_camera());
            self.close_local_camera_tile();
            if had_video {
                send_camera_subscription(self.config, self.peer, self.signal, self.state, false)
                    .await;
            }
            let removed_mids: Vec<String> = self
                .state
                .remote_sources
                .iter()
                .map(|(m, _)| m.clone())
                .collect();
            if !removed_mids.is_empty() {
                let upd = build_remote_video_update_frame(Vec::new(), removed_mids);
                if let Err(err) = self.signal.send_frame(&upd).await {
                    tracing::warn!(%err, "failed to send audio-only REMOTE_VIDEO_UPDATE");
                }
            }
            self.state.remote_sources.clear();
            if let Ok(mut rt) = self.state.vdec_routing.lock() {
                rt.remote_sources.clear();
            }
            self.state.vdec_clear.store(true, Ordering::Relaxed);
            for tile in self.state.active_remote_tiles.drain() {
                let _ = self.state.updates.send(HuddleUpdate::VideoTileClosed(tile));
            }
        } else if let Some(index) = self.state.last_index.clone() {
            apply_index_frame(
                &index,
                self.config,
                self.snap,
                self.peer,
                self.signal,
                self.state,
            )
            .await;
        }
        self.publish_state();
    }
}
