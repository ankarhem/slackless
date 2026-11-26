//! Device operations the event loop runs off itself (audio device switches, camera and
//! `#content` screen-share starts) and applying their results once they report back.

use std::sync::Arc;

use super::connected::{SessionRuntime, announce_screen_share_capabilities};
use super::controls::send_camera_subscription;
use super::io::{RtcController, SignalSink};
use super::media::{AudioSwitch, BackgroundDone, MediaDevices};
use crate::backend::VideoFrameSink;
use crate::video::VideoTileKey;
use crate::{HuddleAudioConfig, HuddleSessionSnapshot, HuddleUpdate};

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

    /// Sink that publishes a local capture's preview frames as `tile` straight from the capture
    /// thread; the UI keeps only the latest frame per tile.
    fn preview_sink(&self, tile: VideoTileKey) -> VideoFrameSink {
        let updates = self.updates.clone();
        Arc::new(move |frame| {
            let _ = updates.send(HuddleUpdate::VideoFrame {
                tile: tile.clone(),
                frame,
            });
        })
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
            self.preview_sink(VideoTileKey::LocalCamera),
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
            self.preview_sink(VideoTileKey::LocalScreen),
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
