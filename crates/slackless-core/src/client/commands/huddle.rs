//! Native Chime huddle command handlers and session-update watcher.

use super::super::*;
use crate::domain::huddles::{CaptionDeduper, resolve_segment_at_ms};
use crate::huddle::{HuddleAudioConfig, HuddleControl, HuddlePhase, HuddleUpdate};

impl<B: Backend> Core<B> {
    /// Dispatch any huddle media control command (legacy per-action variants or
    /// [`Command::HuddleControl`]) to the active huddle session, returning `None` if handled or
    /// `Some(command)` if the command should continue through the main dispatcher.
    pub(super) fn try_dispatch_huddle_control(&mut self, command: Command) -> Option<Command> {
        let ctrl = match command {
            Command::HuddleControl(ctrl) => ctrl.into(),
            Command::SetHuddleMuted(muted) => HuddleControl::SetMuted(muted),
            Command::SetHuddleAudioDevices { input, output } => {
                HuddleControl::SetAudioDevices { input, output }
            }
            Command::SetHuddleAudioProcessing { aec, ns, agc } => {
                HuddleControl::SetAudioProcessing { aec, ns, agc }
            }
            Command::SetHuddleCamera {
                enabled,
                device_index,
                blur,
            } => HuddleControl::SetCamera {
                enabled,
                device_index,
                blur,
            },
            Command::SetHuddleBlurBackground(blur) => HuddleControl::SetBlurBackground(blur),
            Command::SetHuddleScreenShare {
                source_id,
                share_audio,
            } => HuddleControl::SetScreenShare {
                source_id,
                share_audio,
            },
            Command::SendHuddleReaction {
                emoji,
                kind,
                emoji_url,
            } => HuddleControl::SendReaction {
                emoji,
                kind,
                emoji_url,
            },
            Command::SetHuddleHandRaised(raised) => HuddleControl::SetHandRaised(raised),
            Command::SetHuddleStickyEmoji(emoji) => HuddleControl::SetStickyEmoji(emoji),
            Command::SendHuddleDraw(draw) => HuddleControl::SendDraw(draw),
            Command::SetHuddleJukebox(song) => HuddleControl::SetJukebox(song),
            Command::MuteHuddlePeer(peer_id) => HuddleControl::MutePeer(peer_id),
            Command::SetHuddleAudioOnly(enabled) => HuddleControl::SetAudioOnly(enabled),
            other => return Some(other),
        };
        self.send_huddle_control(ctrl);
        None
    }

    pub(super) fn join_huddle(&mut self, channel: ChannelId, audio: HuddleAudioConfig) {
        // Reject duplicate concurrent join while `rooms.join` is already in-flight;
        // otherwise leave any active huddle and claim a fresh atomic generation id.
        let (my_gen, gen_counter) = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.joining {
                return;
            }
            let next_gen = slot
                .generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            slot.joining = true;
            slot.call_id = None;
            if let Some(prev) = slot.handle.take() {
                prev.leave();
            }
            (next_gen, Arc::clone(&slot.generation))
        };
        let ctx = self.ctx();
        let huddle = Arc::clone(&self.huddle);
        let save_transcripts = Arc::clone(&self.save_huddle_transcripts);
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
                    }
                }
            }
            let _join_guard = JoinGuard {
                huddle: Arc::clone(&huddle),
                generation: my_gen,
            };
            let joined = ctx.backend.rooms_join(channel.clone()).await?;
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
                Some(persist),
            );
            Ok(())
        });
    }

    pub(super) fn leave_huddle(&mut self) {
        let prev = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            slot.generation.fetch_add(1, Ordering::AcqRel);
            slot.joining = false;
            slot.call_id = None;
            slot.handle.take()
        };
        if let Some(handle) = prev {
            handle.leave();
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
            for user in users {
                ctx.backend
                    .rooms_notify_member(channel.clone(), user)
                    .await?;
            }
            Ok(())
        });
    }

    pub(super) fn copy_huddle_link(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("rooms.getLink", async move {
            let url = ctx
                .backend
                .rooms_get_link(ctx.workspace, channel.clone())
                .await?;
            ctx.events.emit(Event::HuddleLinkLoaded { channel, url });
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
    persist: Option<TranscriptPersistCtx>,
) {
    let tasks = TaskTracker::default();
    spawn_huddle_watcher_tracked(&tasks, huddle, my_gen, upd_rx, events, persist);
}

/// Forward huddle session updates from `upd_rx` to `events`, tagged with `my_gen` so an older
/// session transitioning to `Idle`/`Failed` can never clobber a newer active session's state.
///
/// High-frequency media updates (60Hz video frames, audio meters, data events) check the atomic
/// `generation` counter lock-free instead of taking the `HuddleSlot` mutex.
fn spawn_huddle_watcher_tracked(
    tasks: &TaskTracker,
    huddle: Arc<Mutex<HuddleSlot>>,
    my_gen: u64,
    mut upd_rx: tokio::sync::mpsc::UnboundedReceiver<HuddleUpdate>,
    events: Emitter,
    persist: Option<TranscriptPersistCtx>,
) {
    let gen_counter = Arc::clone(
        &huddle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .generation,
    );
    tasks.spawn(async move {
        let mut deduper = CaptionDeduper::default();
        while let Some(upd) = upd_rx.recv().await {
            match upd {
                HuddleUpdate::State(snap) => {
                    let done = matches!(snap.phase, HuddlePhase::Idle | HuddlePhase::Failed);
                    let should_emit = {
                        let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        let is_current = slot.generation.load(Ordering::Acquire) == my_gen;
                        if is_current && done {
                            slot.handle = None;
                            slot.call_id = None;
                        }
                        is_current || (done && slot.handle.is_none() && !slot.joining)
                    };
                    if should_emit {
                        events.emit(Event::HuddleSessionChanged(snap));
                    }
                }
                HuddleUpdate::AudioLevels(levels) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        events.emit(Event::HuddleAudioLevels(levels));
                    }
                }
                HuddleUpdate::VideoFrame { tile, frame } => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        events.emit(Event::HuddleVideoFrame { tile, frame });
                    }
                }
                HuddleUpdate::VideoTileClosed(tile) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        events.emit(Event::HuddleVideoTileClosed(tile));
                    }
                }
                HuddleUpdate::DataEvent(ev) => {
                    if gen_counter.load(Ordering::Acquire) == my_gen {
                        if let crate::huddle::HuddleDataEvent::Caption {
                            ref user,
                            ref text,
                            is_partial,
                            at_ms,
                            ref lang,
                        } = ev
                            && let Some(ctx) = persist.as_ref()
                            && ctx.enabled.load(Ordering::Relaxed)
                            && let Some(trimmed) = deduper.accept(user, text, is_partial, at_ms)
                        {
                            let seg_at_ms = resolve_segment_at_ms(at_ms);
                            let ws = ctx.workspace.clone();
                            let channel = ctx.channel.clone();
                            let call_id = ctx.call_id.clone();
                            let thread_root_ts = ctx.thread_root_ts.clone();
                            let speaker_id = user.clone();
                            let lang_owned = lang.clone();
                            let source = ctx.source;
                            if let Err(err) = with_store(&ctx.store, move |store| {
                                let seg = slackless_store::NewHuddleTranscriptSegment {
                                    channel: &channel,
                                    call_id: &call_id,
                                    thread_root_ts: thread_root_ts.as_ref(),
                                    speaker_id: Some(&speaker_id),
                                    lang: lang_owned.as_deref(),
                                    source,
                                    text: &trimmed,
                                    at_ms: seg_at_ms,
                                };
                                store.insert_huddle_transcript_segment(&ws, &seg)
                            })
                            .await
                            {
                                tracing::warn!(%err, "failed to persist huddle transcript segment");
                            }
                        }
                        events.emit(Event::HuddleDataEvent(ev));
                    }
                }
            }
        }
    });
}
