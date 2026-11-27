//! Background workspace event routing for counts, badges, huddles, and notifications.

use super::super::*;

impl App {
    /// Route an event from workspace `ws_index`: active workspace events update the live UI,
    /// while background workspace events keep their counts, activity badges, and team icons
    /// fresh on the workspace switcher rail.
    pub(in crate::app) fn on_workspace_event(
        &mut self,
        ws_index: usize,
        event: &Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Popped-out windows follow their conversation in whichever workspace it lives.
        if let Event::StoreChanged { channel } = event
            && let Some(team) = self
                .workspace_bridge(ws_index)
                .and_then(CoreBridge::team_id)
        {
            crate::popout::store_changed(team, channel, cx);
        }
        // A huddle belongs to the workspace running it, whichever one is shown.
        if let Event::Huddle(huddle) = event {
            self.on_huddle_core_event(ws_index, huddle, window, cx);
            return;
        }
        if ws_index == self.workspace {
            match event {
                Event::UsergroupUsersLoaded { usergroup, users } => {
                    self.on_usergroup_users_loaded(usergroup.clone(), users.clone(), cx);
                    return;
                }
                Event::Usergroups(groups) => {
                    let ws_id = self
                        .fixtures
                        .own_team()
                        .map(|t| t.id.clone())
                        .unwrap_or_default();
                    crate::directory::reconcile_usergroups_from_server(&ws_id, groups, cx);
                }
                Event::Conversations(_) | Event::SidebarChanged => self.ws.notify_levels = None,
                _ => {}
            }
            self.on_core_event(event, window, cx);
            return;
        }
        let Some(slot) = self.slots.get_mut(ws_index) else {
            return;
        };
        match event {
            Event::Counts(counts) => {
                let newly_unread = slot.state.fold_counts(counts);
                let threads = &counts.threads;
                let (has_unreads, mentions) = (threads.has_unreads, threads.mention_count);
                if let Some(parked) = &slot.parked {
                    parked.views.sidebar.update(cx, |sidebar, cx| {
                        sidebar.set_thread_counts(has_unreads, mentions, cx);
                        for id in &newly_unread {
                            sidebar.mark_unread(id, cx);
                        }
                    });
                }
                self.sync_badge(cx);
                cx.notify();
            }
            Event::ActivityView { feed } => {
                if let Ok(feed) = feed.as_deref() {
                    let unread = feed.items.iter().filter(|item| item.unread).count();
                    slot.activity_unread = u32::try_from(unread).unwrap_or(u32::MAX);
                    self.sync_badge(cx);
                    cx.notify();
                }
            }
            Event::SavedLoaded(Ok(items)) => {
                self.apply_saved_loaded(ws_index, items, cx);
            }
            Event::MessageScheduled {
                channel,
                scheduled_message_id,
                post_at,
            } => {
                self.drafts.update(cx, |drafts, cx| {
                    drafts.confirm_scheduled(
                        ws_index,
                        channel,
                        *post_at,
                        scheduled_message_id.clone(),
                        cx,
                    );
                });
                cx.notify();
            }
            Event::Error(error) => {
                tracing::warn!(workspace = %slot.team.id, error = %error, "background workspace core error");
                if error.starts_with("chat.scheduleMessage:")
                    || error.starts_with("chat.deleteScheduledMessage:")
                {
                    self.drafts
                        .update(cx, |drafts, cx| drafts.drop_pending_scheduled(ws_index, cx));
                }
            }
            Event::AuthExpired(error) => {
                tracing::warn!(
                    workspace = %slot.team.id,
                    error = %error,
                    "Slack rejected the saved session for background workspace"
                );
            }
            Event::UsersChanged(users) => {
                if slot.parked.is_some() && !users.is_empty() {
                    let next = Reload::Users(users.iter().cloned().collect());
                    slot.state.reload = Some(match slot.state.reload.take() {
                        Some(prev) => prev.merge(next),
                        None => next,
                    });
                }
            }
            Event::FileTranscriptReady {
                file_id,
                text,
                cues,
            } => {
                if let Some(parked) = &mut slot.parked {
                    parked.fixtures = Arc::new(parked.fixtures.with_file_transcript_cues(
                        file_id.clone(),
                        text.clone(),
                        cues.clone(),
                    ));
                }
            }
            Event::Conversations(_) | Event::SidebarChanged => {
                // Notification levels may have changed: read them again when next needed.
                slot.state.notify_levels = None;
                // A visited background workspace's parked snapshot is now stale: reload it on return.
                if slot.parked.is_some() {
                    slot.state.reload = Some(Reload::Full);
                }
                // Refresh this background workspace's team name/icon if hydrate just stored a
                // richer `team.info` record (O(1) single-row team lookup).
                let fresh_team = slot.bridge.as_ref().and_then(CoreBridge::team_summary);
                if let Some(team) = fresh_team
                    && slot.team != team
                {
                    slot.team = team;
                    self.sync_rail(cx);
                    self.sync_badge(cx);
                    self.update_menus(cx);
                }
            }
            Event::Usergroups(groups) => slot.state.usergroups = groups.clone(),
            Event::AppCommands(commands) => slot.state.app_commands = commands.clone(),
            Event::Connection(state) => slot.state.connection = Some(state.clone()),
            Event::OfflineQueueChanged(count) => slot.state.offline_queued = *count,
            Event::OutboxUpdated(items) => slot.state.outbox = items.clone(),
            Event::BackfillProgress(progress) => {
                slot.state.backfill_progress = Some(progress.clone());
            }
            Event::OfflineMutationFailed(failed) => {
                if !slot
                    .state
                    .failed_mutations
                    .iter()
                    .any(|m| m.id == failed.id)
                {
                    slot.state.failed_mutations.push(failed.clone());
                }
            }
            Event::BotProfileLoaded { bot_id, profile } => {
                slot.state
                    .bots
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(bot_id.clone(), profile.clone());
            }
            Event::Presence { users, presence } => {
                slot.state
                    .presence
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(users.iter().map(|u| (u.clone(), presence.clone())));
            }
            Event::ReadMarker { channel, ts } => {
                // The workspace's snapshot knows the conversation once visited, else its store.
                let kind = (slot.parked.as_ref())
                    .and_then(|parked| {
                        parked
                            .fixtures
                            .conversation(channel)
                            .map(Conversation::kind)
                    })
                    .or_else(|| {
                        let ctx = slot.bridge.as_ref()?.bg_message_context(channel, None)?;
                        ctx.conversation.map(|c| c.kind())
                    });
                let dm = matches!(kind, Some(ConversationKind::Im | ConversationKind::Mpim));
                let read = slot.state.fold_read_marker(channel, ts, dm);
                if let Some(parked) = &slot.parked {
                    if read {
                        parked
                            .views
                            .sidebar
                            .update(cx, |s, cx| s.mark_read(channel.clone(), cx));
                    } else {
                        parked
                            .views
                            .sidebar
                            .update(cx, |s, cx| s.mark_unread(channel, cx));
                    }
                }
                self.sync_badge(cx);
                cx.notify();
            }
            Event::MessagePosted { channel, message } => {
                let bg_ctx = (slot.bridge.as_ref())
                    .and_then(|b| b.bg_message_context(channel, message.user.as_ref()));
                let (mentioned, notifications_on) = {
                    let settings = self.preferences.read(cx).settings();
                    let mentioned = (bg_ctx.as_ref()).is_some_and(|ctx| {
                        mentions_me(&message.text, &ctx.self_user, &settings.keywords)
                    });
                    (mentioned, settings.desktop_notifications)
                };
                let reply = message
                    .thread_ts
                    .as_ref()
                    .is_some_and(|ts| *ts != message.ts);
                let broadcast =
                    message.subtype == Some(slackless_core::types::MessageSubtype::ThreadBroadcast);
                let counts_in_channel =
                    !message.hidden && !message.is_ephemeral && (!reply || broadcast);
                if counts_in_channel {
                    let me = bg_ctx.as_ref().map(|ctx| &ctx.self_user);
                    let arrival = if me.is_some_and(|me| message.user.as_ref() == Some(me)) {
                        Arrival::Own
                    } else {
                        let kind = (bg_ctx.as_ref())
                            .and_then(|ctx| ctx.conversation.as_ref())
                            .map(Conversation::kind)
                            .or_else(|| super::event_kind(channel, message));
                        super::unread_arrival(kind, mentioned)
                    };
                    let unread = matches!(arrival, Arrival::Unread { .. });
                    slot.state.fold_message(channel, &message.ts, arrival);
                    if unread {
                        if let Some(parked) = &slot.parked {
                            parked
                                .views
                                .sidebar
                                .update(cx, |s, cx| s.mark_unread(channel, cx));
                        }
                        self.sync_badge(cx);
                        cx.notify();
                    }
                }
                if !notifications_on {
                    return;
                }
                let Some(bg_ctx) = bg_ctx else {
                    return;
                };
                if message.user.as_ref() == Some(&bg_ctx.self_user) {
                    return;
                }
                let conv = bg_ctx.conversation.as_ref();
                let Some(kind) =
                    (conv.map(Conversation::kind)).or_else(|| super::event_kind(channel, message))
                else {
                    return;
                };
                let Some(slot) = self.slots.get_mut(ws_index) else {
                    return;
                };
                let levels = slot.state.notify_levels.get_or_insert_with(|| {
                    (slot.bridge.as_ref())
                        .and_then(|bridge| bridge.placeholder(&HashMap::new()))
                        .map(|mut prefs| mem::take(&mut prefs.notify_levels))
                        .unwrap_or_default()
                });
                let level = levels.get(channel).copied();
                let owned_ctx = super::notifications::OwnedRuleContext {
                    sender_id: message
                        .user
                        .as_ref()
                        .map(|u| u.as_str().to_owned())
                        .or_else(|| message.bot_id.as_ref().map(|b| b.as_str().to_owned())),
                    sender_handle: bg_ctx
                        .sender
                        .as_ref()
                        .map(|x| x.name.clone())
                        .or_else(|| message.username.clone())
                        .or_else(|| message.bot_profile.as_ref().map(|b| b.name.clone())),
                    sender_display_name: bg_ctx
                        .sender
                        .as_ref()
                        .map(|u| u.display_name().to_owned())
                        .or_else(|| message.bot_profile.as_ref().map(|b| b.name.clone())),
                    channel_id: channel.as_str().to_owned(),
                    channel_name: conv.and_then(|conv| conv.name.clone()),
                    text: message.text.clone(),
                    is_thread_reply: reply,
                    mentions_me: mentioned,
                };
                let rules = &self
                    .preferences
                    .read(cx)
                    .settings()
                    .smart_notification_rules;
                let has_semantic_rule = rules.iter().any(|r| {
                    r.enabled
                        && r.semantic_topic
                            .as_deref()
                            .is_some_and(|t| !t.trim().is_empty())
                });
                if !has_semantic_rule {
                    let rule_eval = if rules.is_empty() {
                        slackless_core::rules::RuleEvaluation::default()
                    } else {
                        let rule_ctx = owned_ctx.as_rule_context();
                        slackless_core::rules::evaluate_rules(rules, &rule_ctx)
                    };
                    self.dispatch_bg_message_notification(
                        ws_index,
                        &bg_ctx.self_user,
                        channel,
                        message,
                        conv,
                        bg_ctx.sender.as_ref(),
                        level,
                        kind,
                        &rule_eval,
                        cx,
                    );
                    return;
                }
                let rules = rules.clone();
                let self_user = bg_ctx.self_user.clone();
                let channel_owned = channel.clone();
                let message_owned = message.clone();
                let conv_owned = conv.cloned();
                let sender_owned = bg_ctx.sender.clone();
                cx.spawn(async move |this, cx| {
                    let rule_eval = cx
                        .background_executor()
                        .spawn(async move {
                            let ctx = owned_ctx.as_rule_context();
                            let scorer = crate::preferences::compute_local_semantic_similarity;
                            slackless_core::rules::evaluate_rules_with_semantic(
                                &rules,
                                &ctx,
                                Some(&scorer),
                            )
                        })
                        .await;
                    let _ = this.update(cx, |app, cx| {
                        app.dispatch_bg_message_notification(
                            ws_index,
                            &self_user,
                            &channel_owned,
                            &message_owned,
                            conv_owned.as_ref(),
                            sender_owned.as_ref(),
                            level,
                            kind,
                            &rule_eval,
                            cx,
                        );
                    });
                })
                .detach();
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_bg_message_notification(
        &mut self,
        ws_index: usize,
        self_user: &UserId,
        channel: &ChannelId,
        message: &Message,
        conv: Option<&Conversation>,
        sender_user: Option<&slackless_core::types::User>,
        level: Option<NotifyLevel>,
        kind: ConversationKind,
        rule_eval: &slackless_core::rules::RuleEvaluation,
        cx: &mut Context<Self>,
    ) {
        let decision = {
            let settings = self.preferences.read(cx).settings();
            super::decide_message_notification(&super::NotificationInput {
                self_user,
                channel,
                message,
                sender: sender_user,
                kind: Some(kind),
                level,
                on_screen_active: false,
                paused: self.self_state.paused(),
                focused: self.self_state.focused(),
                settings,
                rule_eval,
            })
        };
        if !decision.notify {
            return;
        }
        let show_preview = self.preferences.read(cx).settings().show_preview;
        let ws_name = self
            .slots
            .get(ws_index)
            .map(|slot| slot.team.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Slack".to_owned());
        let sender = sender_user
            .map(|u| u.display_name().to_owned())
            .or_else(|| message.user.as_ref().map(|u| u.as_str().to_owned()))
            .unwrap_or_default();
        let text = if show_preview {
            if !message.text.trim().is_empty() {
                message.text.trim().to_owned()
            } else if let Some(file) = message.files.first() {
                file.title
                    .clone()
                    .or_else(|| file.name.clone())
                    .unwrap_or_else(|| "Shared a file".to_owned())
            } else {
                "New message".to_owned()
            }
        } else {
            "New message".to_owned()
        };
        let (raw_title, body) = match kind {
            ConversationKind::Im => (format!("{ws_name} — {sender}"), text),
            ConversationKind::Mpim => (
                format!(
                    "{ws_name} — {}",
                    (conv.and_then(|conv| conv.name.as_deref())).unwrap_or("Group DM")
                ),
                format!("{sender}: {text}"),
            ),
            ConversationKind::PublicChannel | ConversationKind::PrivateChannel => {
                // A channel the store does not hold yet has no name to show.
                let Some(conv) = conv else {
                    return;
                };
                (
                    format!("{ws_name} — #{}", conv.name.as_deref().unwrap_or_default()),
                    format!("{sender}: {text}"),
                )
            }
        };
        let title = if rule_eval.is_priority_highlight() {
            format!("\u{2605} {raw_title}")
        } else {
            raw_title
        };
        let tag = format!("bg:{ws_index}:{channel}:{}", message.ts);
        let vip_sound = decision
            .sender_is_vip
            .then(|| self.preferences.read(cx).settings().vip_sound);
        self.system_notification_with_sound(
            &tag,
            &title,
            &body,
            decision.is_direct_or_mention,
            decision.breakthrough,
            vip_sound,
            cx,
        );
    }
}
