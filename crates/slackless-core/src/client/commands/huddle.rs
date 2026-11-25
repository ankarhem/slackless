//! Native Chime huddle command handlers and session-update watcher.

use super::super::*;
use crate::huddle::{HuddleAudioConfig, HuddleControl, HuddlePhase, HuddleUpdate};

impl<B: Backend> Core<B> {
    pub(super) fn join_huddle(&mut self, channel: ChannelId, audio: HuddleAudioConfig) {
        // Reject duplicate concurrent join while `rooms.join` is already in-flight;
        // otherwise leave any active huddle and claim a fresh generation id.
        let my_gen = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if slot.joining {
                return;
            }
            slot.generation = slot.generation.wrapping_add(1);
            slot.joining = true;
            slot.call_id = None;
            if let Some(prev) = slot.handle.take() {
                prev.leave();
            }
            slot.generation
        };
        let backend = self.backend.clone();
        let events = self.events.clone();
        let huddle = Arc::clone(&self.huddle);
        let team_id = self.workspace.clone();
        let store = Arc::clone(&self.store);
        let save_transcripts = Arc::clone(&self.save_huddle_transcripts);
        self.spawn("rooms.join", async move {
            struct JoinGuard {
                huddle: Arc<Mutex<HuddleSlot>>,
                generation: u64,
            }
            impl Drop for JoinGuard {
                fn drop(&mut self) {
                    let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
                    if slot.generation == self.generation {
                        slot.joining = false;
                    }
                }
            }
            let _join_guard = JoinGuard {
                huddle: Arc::clone(&huddle),
                generation: my_gen,
            };
            let joined = backend.rooms_join(channel.clone()).await?;
            {
                let slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                if slot.generation != my_gen {
                    return Ok(());
                }
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
                huddle_link: joined
                    .huddle
                    .huddle_link
                    .or_else(|| Some(format!("https://app.slack.com/huddle/{team_id}/{channel}"))),
            };
            let transcript_source = audio.transcription_provider.source_tag();
            let (upd_tx, upd_rx) = tokio::sync::mpsc::unbounded_channel();
            let handle = crate::huddle::spawn_huddle_session(creds, audio, upd_tx);
            let rejected = {
                let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                if slot.generation == my_gen {
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
                store,
                workspace: team_id,
                channel,
                call_id,
                thread_root_ts,
                source: transcript_source,
                enabled: save_transcripts,
            };
            spawn_huddle_watcher(Arc::clone(&huddle), my_gen, upd_rx, events, Some(persist));
            Ok(())
        });
    }

    pub(super) fn leave_huddle(&mut self) {
        let prev = {
            let mut slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            slot.generation = slot.generation.wrapping_add(1);
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

    pub(super) fn invite_to_huddle(&self, channel: ChannelId, users: Vec<UserId>) {
        let backend = self.backend.clone();
        self.spawn("rooms.notifyMember", async move {
            for user in users {
                backend.rooms_notify_member(channel.clone(), user).await?;
            }
            Ok(())
        });
    }

    pub(super) fn copy_huddle_link(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("rooms.getLink", async move {
            let url = backend.rooms_get_link(channel.clone()).await?;
            events.emit(Event::HuddleLinkLoaded { channel, url });
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
        let call_id = {
            let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(handle) = slot.handle.as_ref() {
                handle.send(HuddleControl::SetTopic(topic.clone()));
            }
            slot.call_id.clone()
        };
        if let Some(call_id) = call_id {
            let backend = self.backend.clone();
            self.spawn("screenhero.rooms.rename", async move {
                backend.rooms_rename(call_id, topic).await?;
                Ok(())
            });
        }
    }

    pub(super) fn set_huddle_background(&self, background_id: String) {
        let call_id = {
            let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(handle) = slot.handle.as_ref() {
                handle.send(HuddleControl::SetBackground(
                    (!background_id.is_empty() && background_id != "default")
                        .then(|| background_id.clone()),
                ));
            }
            slot.call_id.clone()
        };
        if let Some(call_id) = call_id {
            let backend = self.backend.clone();
            self.spawn("huddles.update", async move {
                backend
                    .huddles_update_background(call_id, background_id)
                    .await?;
                Ok(())
            });
        }
    }

    pub(super) fn set_huddle_captions(&self, enabled: bool) {
        let call_id = {
            let slot = self.huddle.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(handle) = slot.handle.as_ref() {
                handle.send(HuddleControl::SetCaptionsEnabled(enabled));
            }
            slot.call_id.clone()
        };
        if let Some(call_id) = call_id {
            let backend = self.backend.clone();
            self.spawn(
                if enabled {
                    "rooms.startTranscription"
                } else {
                    "rooms.stopTranscription"
                },
                async move {
                    if enabled {
                        backend.rooms_start_transcription(call_id).await?;
                    } else {
                        backend.rooms_stop_transcription(call_id).await?;
                    }
                    Ok(())
                },
            );
        }
    }

    pub(super) fn decline_huddle_invite(&self, channel: ChannelId, call_id: String) {
        let backend = self.backend.clone();
        self.spawn("rooms.inviteResponse", async move {
            backend
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
    pub enabled: Arc<std::sync::atomic::AtomicBool>,
}

/// Forward huddle session updates from `upd_rx` to `events`, tagged with `my_gen` so an older
/// session transitioning to `Idle`/`Failed` can never clobber a newer active session's state.
pub(in crate::client) fn spawn_huddle_watcher(
    huddle: Arc<Mutex<HuddleSlot>>,
    my_gen: u64,
    mut upd_rx: tokio::sync::mpsc::UnboundedReceiver<HuddleUpdate>,
    events: Emitter,
    persist: Option<TranscriptPersistCtx>,
) {
    tokio::spawn(async move {
        let mut last_caption: Option<(UserId, String, u64)> = None;
        while let Some(upd) = upd_rx.recv().await {
            match upd {
                HuddleUpdate::State(snap) => {
                    let done = matches!(snap.phase, HuddlePhase::Idle | HuddlePhase::Failed);
                    let should_emit = {
                        let mut slot = huddle.lock().unwrap_or_else(PoisonError::into_inner);
                        let is_current = slot.generation == my_gen;
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
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleAudioLevels(levels));
                    }
                }
                HuddleUpdate::VideoFrame { tile, frame } => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleVideoFrame { tile, frame });
                    }
                }
                HuddleUpdate::VideoTileClosed(tile) => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        events.emit(Event::HuddleVideoTileClosed(tile));
                    }
                }
                HuddleUpdate::DataEvent(ev) => {
                    if huddle
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .generation
                        == my_gen
                    {
                        if let crate::huddle::HuddleDataEvent::Caption {
                            ref user,
                            ref text,
                            is_partial: false,
                            at_ms,
                            ref lang,
                        } = ev
                            && let Some(ctx) = persist.as_ref()
                            && ctx.enabled.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            let trimmed = text.trim();
                            let is_dup = last_caption.as_ref().is_some_and(|(u, t, ts)| {
                                u == user && t == trimmed && at_ms.saturating_sub(*ts) < 2_500
                            });
                            if !trimmed.is_empty() && !is_dup {
                                last_caption = Some((user.clone(), trimmed.to_owned(), at_ms));
                                let seg_at_ms = if at_ms > 0 {
                                    i64::try_from(at_ms).unwrap_or(i64::MAX)
                                } else {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
                                };
                                let seg = slackless_store::NewHuddleTranscriptSegment {
                                    channel: &ctx.channel,
                                    call_id: &ctx.call_id,
                                    thread_root_ts: ctx.thread_root_ts.as_ref(),
                                    speaker_id: Some(user),
                                    lang: lang.as_deref(),
                                    source: ctx.source,
                                    text: trimmed,
                                    at_ms: seg_at_ms,
                                };
                                if let Ok(store) = ctx.store.lock() {
                                    let _ = store
                                        .insert_huddle_transcript_segment(&ctx.workspace, &seg);
                                }
                            }
                        }
                        events.emit(Event::HuddleDataEvent(ev));
                    }
                }
            }
        }
    });
}
