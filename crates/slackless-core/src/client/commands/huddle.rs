//! Native Chime huddle command handlers and session-update watcher.

use super::super::*;
use crate::domain::huddles::{CaptionDeduper, resolve_segment_at_ms};
use crate::huddle::{HuddleAudioConfig, HuddleControl, HuddlePhase, HuddleUpdate};

impl<B: Backend> Core<B> {
    pub(super) fn join_huddle(&mut self, channel: ChannelId, audio: HuddleAudioConfig) {
        // Reject duplicate concurrent join for the SAME channel while `rooms.join` is already
        // in-flight; joining a DIFFERENT channel supersedes the in-flight join via generation.
        let (my_gen, gen_counter, prev_handle) = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.joining && slot.joining_channel.as_ref() == Some(&channel) {
                return;
            }
            let next_gen = slot
                .generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            slot.joining = true;
            slot.joining_channel = Some(channel.clone());
            slot.call_id = None;
            let prev = slot.handle.take();
            (next_gen, Arc::clone(&slot.generation), prev)
        };
        let ctx = self.ctx();
        let huddle = Arc::clone(&self.huddle);
        let save_transcripts = Arc::clone(&self.save_huddle_transcripts);
        let huddle_media = self.huddle_media.clone();
        let tasks = self.tasks.clone();
        self.spawn("rooms.join", async move {
            struct JoinGuard {
                huddle: Arc<Mutex<HuddleSlot>>,
                generation: u64,
            }
            impl Drop for JoinGuard {
                fn drop(&mut self) {
                    let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if slot.generation.load(Ordering::Acquire) == self.generation {
                        slot.joining = false;
                        slot.joining_channel = None;
                    }
                }
            }
            let _join_guard = JoinGuard {
                huddle: Arc::clone(&huddle),
                generation: my_gen,
            };
            // Tear down any prior huddle session concurrently with `rooms.join` so CoreAudio /
            // cpal hardware is released before the new session opens audio.
            let prev_leave = async {
                if let Some(prev) = prev_handle {
                    prev.leave_and_wait(std::time::Duration::from_millis(1500))
                        .await;
                }
            };
            let (joined_res, ()) =
                tokio::join!(ctx.backend.rooms_join(channel.clone()), prev_leave);
            let joined = match joined_res {
                Ok(joined) => joined,
                Err(err) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        ctx.events
                            .emit(Event::Huddle(HuddleEvent::SessionChanged(Box::new(
                                crate::huddle::HuddleSessionSnapshot {
                                    phase: HuddlePhase::Failed,
                                    channel,
                                    error: Some(err.to_string()),
                                    ..Default::default()
                                },
                            ))));
                    }
                    return Err(err.into());
                }
            };
            if gen_counter.load(Ordering::Acquire) != my_gen {
                return Ok(());
            }
            let call_id = joined.call_id.clone();
            let thread_root_ts = joined
                .huddle
                .thread_root_ts
                .filter(|ts| !ts.as_str().trim().is_empty())
                .or_else(|| {
                    joined
                        .canvas
                        .as_ref()
                        .and_then(|c| c.root_thread_ts.clone())
                        .filter(|ts| !ts.as_str().trim().is_empty())
                });
            let creds = crate::huddle::HuddleCredentials {
                channel: channel.clone(),
                call_id: joined.call_id,
                meeting_id: joined.chime.meeting.meeting_id,
                attendee_id: joined.chime.attendee.attendee_id,
                external_user_id: joined.chime.attendee.external_user_id,
                join_token: joined.chime.attendee.join_token,
                signaling_url: joined.chime.meeting.media_placement.signaling_url,
                turn_control_url: joined.chime.meeting.media_placement.turn_control_url,
                audio_host_url: joined.chime.meeting.media_placement.audio_host_url,
                thread_root_ts: thread_root_ts.clone(),
                canvas_file_id: joined.canvas.as_ref().and_then(|c| c.file_id.clone()),
                huddle_link: joined.huddle.huddle_link.or_else(|| {
                    Some(crate::types::net::huddle_fallback_url(
                        ctx.workspace.as_str(),
                        channel.as_str(),
                    ))
                }),
            };
            let transcript_source = audio.transcription_provider.source_tag();
            let (upd_tx, upd_rx) = tokio::sync::mpsc::unbounded_channel();
            let handle = crate::huddle::spawn_huddle_session(creds, audio, upd_tx);
            let rejected = {
                let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                if slot.generation.load(Ordering::Acquire) == my_gen {
                    slot.joining = false;
                    slot.joining_channel = None;
                    slot.call_id = Some(call_id.clone());
                    slot.handle = Some(handle);
                    None
                } else {
                    Some(handle)
                }
            };
            if let Some(handle) = rejected {
                handle.leave();
                return Ok(());
            }
            let persist = TranscriptPersistCtx {
                store: ctx.store,
                workspace: ctx.workspace,
                channel,
                call_id,
                thread_root_ts,
                source: transcript_source,
                enabled: save_transcripts,
            };
            spawn_huddle_watcher_tracked(
                &tasks,
                Arc::clone(&huddle),
                my_gen,
                upd_rx,
                ctx.events,
                huddle_media,
                Some(persist),
            );
            Ok(())
        });
    }

    pub(super) fn leave_huddle(&mut self) {
        let (prev, cancelled_joining_channel) = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            slot.generation.fetch_add(1, Ordering::AcqRel);
            let cancelled = slot.joining.then(|| slot.joining_channel.take()).flatten();
            slot.joining = false;
            slot.joining_channel = None;
            slot.call_id = None;
            (slot.handle.take(), cancelled)
        };
        if let Some(handle) = prev {
            handle.leave();
        } else if let Some(channel) = cancelled_joining_channel {
            // Leaving while `rooms.join` was still in flight: emit `Idle` so callers waiting on
            // lifecycle events don't stay stuck in `Joining`.
            self.events
                .emit(Event::Huddle(HuddleEvent::SessionChanged(Box::new(
                    crate::huddle::HuddleSessionSnapshot {
                        phase: HuddlePhase::Idle,
                        channel,
                        ..Default::default()
                    },
                ))));
        }
    }

    pub(super) fn send_huddle_control(&self, control: HuddleControl) {
        if let Some(handle) = self
            .huddle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .handle
            .as_ref()
        {
            handle.send(control);
        }
    }

    /// Forward `control` to the active huddle session and return its `call_id` (if active) so
    /// the caller can issue the matching Slack REST call.
    fn send_huddle_control_and_call_id(&self, control: HuddleControl) -> Option<String> {
        let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(handle) = slot.handle.as_ref() {
            handle.send(control);
        }
        slot.call_id.clone()
    }

    pub(super) fn invite_to_huddle(&self, channel: ChannelId, users: Vec<UserId>) {
        let ctx = self.ctx();
        self.spawn("rooms.notifyMember", async move {
            let mut first_err: Option<CoreError> = None;
            for user in users {
                if let Err(err) = ctx.backend.rooms_notify_member(channel.clone(), user).await
                    && first_err.is_none()
                {
                    first_err = Some(err.into());
                }
            }
            match first_err {
                Some(err) => Err(err),
                None => Ok(()),
            }
        });
    }

    pub(super) fn copy_huddle_link(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("rooms.getLink", async move {
            let url = ctx
                .backend
                .rooms_get_link(ctx.workspace, channel.clone())
                .await?;
            ctx.events
                .emit(Event::Huddle(HuddleEvent::LinkLoaded { channel, url }));
            Ok(())
        });
    }

    pub(super) fn test_huddle_speaker(&self, output_device: Option<String>) {
        #[cfg(test)]
        let _ = output_device;
        #[cfg(not(test))]
        {
            let events = self.events.clone();
            tokio::task::spawn_blocking(move || {
                if let Err(err) = crate::huddle::play_speaker_test_tone(output_device.as_deref()) {
                    events.emit(Event::Error(format!("speaker test: {err}")));
                }
            });
        }
    }

    pub(super) fn set_huddle_topic(&self, topic: String) {
        if let Some(call_id) =
            self.send_huddle_control_and_call_id(HuddleControl::SetTopic(topic.clone()))
        {
            let ctx = self.ctx();
            self.spawn("screenhero.rooms.rename", async move {
                ctx.backend.rooms_rename(call_id, topic).await?;
                Ok(())
            });
        }
    }

    pub(super) fn set_huddle_background(&self, background_id: String) {
        let peer_bg = (!background_id.is_empty() && background_id != "default")
            .then(|| background_id.clone());
        if let Some(call_id) =
            self.send_huddle_control_and_call_id(HuddleControl::SetBackground(peer_bg))
        {
            let ctx = self.ctx();
            self.spawn("huddles.update", async move {
                ctx.backend
                    .huddles_update_background(call_id, background_id)
                    .await?;
                Ok(())
            });
        }
    }

    pub(super) fn set_huddle_captions(&self, enabled: bool) {
        if let Some(call_id) =
            self.send_huddle_control_and_call_id(HuddleControl::SetCaptionsEnabled(enabled))
        {
            let ctx = self.ctx();
            self.spawn(
                if enabled {
                    "rooms.startTranscription"
                } else {
                    "rooms.stopTranscription"
                },
                async move {
                    if enabled {
                        ctx.backend.rooms_start_transcription(call_id).await?;
                    } else {
                        ctx.backend.rooms_stop_transcription(call_id).await?;
                    }
                    Ok(())
                },
            );
        }
    }

    pub(super) fn decline_huddle_invite(&self, channel: ChannelId, call_id: String) {
        let ctx = self.ctx();
        self.spawn("rooms.inviteResponse", async move {
            ctx.backend
                .rooms_invite_response(channel, call_id, "decline".to_owned())
                .await?;
            Ok(())
        });
    }
}

/// Context used by [`spawn_huddle_watcher`] to archive finalized captions into SQLite FTS5.
pub(in crate::client) struct TranscriptPersistCtx {
    pub store: SharedStore,
    pub workspace: TeamId,
    pub channel: ChannelId,
    pub call_id: String,
    pub thread_root_ts: Option<Ts>,
    pub source: &'static str,
    pub enabled: Arc<AtomicBool>,
}

/// Test-facing wrapper for [`spawn_huddle_watcher_tracked`].
#[cfg(test)]
pub(in crate::client) fn spawn_huddle_watcher(
    huddle: Arc<Mutex<HuddleSlot>>,
    my_gen: u64,
    upd_rx: tokio::sync::mpsc::UnboundedReceiver<HuddleUpdate>,
    events: Emitter,
    huddle_media: HuddleMediaSender,
    persist: Option<TranscriptPersistCtx>,
) {
    let tasks = TaskTracker::default();
    spawn_huddle_watcher_tracked(
        &tasks,
        huddle,
        my_gen,
        upd_rx,
        events,
        huddle_media,
        persist,
    );
}

/// Forward huddle session updates from `upd_rx` to `events` (control/lifecycle/data and tile
/// closures) and `huddle_media` (high-frequency video frames, audio meters and best-effort tile
/// closures via non-blocking `try_send`), tagged with `my_gen` so an older session transitioning
/// to `Idle`/`Failed` can never clobber a newer active session's state.
///
/// High-frequency media updates check the atomic `generation` counter lock-free instead of taking
/// the `HuddleSlot` mutex.
fn spawn_huddle_watcher_tracked(
    tasks: &TaskTracker,
    huddle: Arc<Mutex<HuddleSlot>>,
    my_gen: u64,
    mut upd_rx: tokio::sync::mpsc::UnboundedReceiver<HuddleUpdate>,
    events: Emitter,
    mut huddle_media: HuddleMediaSender,
    persist: Option<TranscriptPersistCtx>,
) {
    let gen_counter = Arc::clone(
        &huddle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation,
    );
    // Offload SQLite transcript writes to a dedicated sequential writer task so Store mutex
    // contention (backfill, VACUUM, FTS queries) can never stall 60 fps huddle video forwarding.
    struct PendingSegment {
        speaker_id: UserId,
        lang: Option<String>,
        text: String,
        at_ms: i64,
    }
    let transcript_tx = persist.map(|ctx| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<PendingSegment>();
        tasks.spawn(async move {
            while let Some(item) = rx.recv().await {
                let ws = ctx.workspace.clone();
                let channel = ctx.channel.clone();
                let call_id = ctx.call_id.clone();
                let thread_root_ts = ctx.thread_root_ts.clone();
                let source = ctx.source;
                if let Err(err) = with_store(&ctx.store, move |store| {
                    let seg = slackless_store::NewHuddleTranscriptSegment {
                        channel: &channel,
                        call_id: &call_id,
                        thread_root_ts: thread_root_ts.as_ref(),
                        speaker_id: Some(&item.speaker_id),
                        lang: item.lang.as_deref(),
                        source,
                        text: &item.text,
                        at_ms: item.at_ms,
                    };
                    store.insert_huddle_transcript_segment(&ws, &seg)
                })
                .await
                {
                    tracing::warn!(%err, "failed to persist huddle transcript segment");
                }
            }
        });
        (tx, ctx.enabled)
    });

    tasks.spawn(async move {
        let mut deduper = CaptionDeduper::default();
        while let Some(upd) = upd_rx.recv().await {
            match upd {
                HuddleUpdate::State(snap) => {
                    let done = matches!(snap.phase, HuddlePhase::Idle | HuddlePhase::Failed);
                    let is_idle = snap.phase == HuddlePhase::Idle;
                    let should_emit = {
                        let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        let is_current = slot.generation.load(Ordering::Acquire) == my_gen;
                        if is_current && done {
                            slot.handle = None;
                            slot.call_id = None;
                        }
                        // Only emit `Idle` (not a stale `Failed` error) on fallback after Leave.
                        is_current || (is_idle && slot.handle.is_none() && !slot.joining)
                    };
                    if should_emit {
                        events.emit(Event::Huddle(HuddleEvent::SessionChanged(snap)));
                    }
                }
                HuddleUpdate::AudioLevels(levels) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        let _ = huddle_media.try_send(HuddleMediaEvent::AudioLevels(levels));
                    }
                }
                HuddleUpdate::VideoFrame { tile, frame } => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        let _ = huddle_media.try_send(HuddleMediaEvent::VideoFrame { tile, frame });
                    }
                }
                HuddleUpdate::VideoTileClosed(tile) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        // On the media lane the closure stays behind the tile's queued frames,
                        // but a full lane drops it; the ordered stream never does.
                        let _ =
                            huddle_media.try_send(HuddleMediaEvent::VideoTileClosed(tile.clone()));
                        events.emit(Event::Huddle(HuddleEvent::VideoTileClosed(tile)));
                    }
                }
                HuddleUpdate::DataEvent(ev) => {
                    // Always persist finalized captions flushed during teardown even after
                    // `leave_huddle` bumped `generation`; only gate UI event forwarding on
                    // `is_current`.
                    if let crate::huddle::HuddleDataEvent::Caption {
                        ref user,
                        ref text,
                        is_partial,
                        at_ms,
                        ref lang,
                    } = ev
                        && let Some((tx, enabled)) = transcript_tx.as_ref()
                        && enabled.load(Ordering::Relaxed)
                        && let Some(trimmed) = deduper.accept(user, text, is_partial, at_ms)
                    {
                        let _ = tx.send(PendingSegment {
                            speaker_id: user.clone(),
                            lang: lang.clone(),
                            text: trimmed,
                            at_ms: resolve_segment_at_ms(at_ms),
                        });
                    }
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        events.emit(Event::Huddle(HuddleEvent::Data(ev)));
                    }
                }
            }
        }
    });
}
