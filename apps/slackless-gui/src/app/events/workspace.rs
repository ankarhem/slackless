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
                    crate::directory::reconcile_usergroups_from_server(groups, cx);
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
                slot.state.counts = [&counts.channels, &counts.ims, &counts.mpims]
                    .into_iter()
                    .flatten()
                    .map(|entry| (entry.id.clone(), entry.clone()))
                    .collect();
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
            Event::Conversations(_) | Event::SidebarChanged => {
                // Notification levels may have changed: read them again when next needed.
                slot.state.notify_levels = None;
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
                        channel,
                        message,
                        conv,
                        bg_ctx.sender.as_ref(),
                        level,
                        kind,
                        mentioned,
                        &rule_eval,
                        cx,
                    );
                    return;
                }
                let rules = rules.clone();
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
                            &channel_owned,
                            &message_owned,
                            conv_owned.as_ref(),
                            sender_owned.as_ref(),
                            level,
                            kind,
                            mentioned,
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
        channel: &ChannelId,
        message: &Message,
        conv: Option<&Conversation>,
        sender_user: Option<&slackless_core::types::User>,
        level: Option<NotifyLevel>,
        kind: ConversationKind,
        mentioned: bool,
        rule_eval: &slackless_core::rules::RuleEvaluation,
        cx: &mut Context<Self>,
    ) {
        let dm = matches!(kind, ConversationKind::Im | ConversationKind::Mpim);
        let settings = self.preferences.read(cx).settings();
        if !settings.desktop_notifications || rule_eval.is_suppressed() {
            return;
        }
        let sender_is_vip = message.user.as_ref().is_some_and(|u| {
            let handle = sender_user.map(|x| x.name.as_str()).unwrap_or("");
            let display = sender_user
                .map(slackless_core::types::User::display_name)
                .unwrap_or("");
            settings.is_vip(u.as_str(), handle, display)
        });
        let breakthrough = sender_is_vip
            || settings.is_urgent_dm_breakthrough(dm, &message.text)
            || rule_eval.breaks_through_mute();
        if level == Some(NotifyLevel::Muted) && !breakthrough {
            return;
        }
        if !breakthrough
            && (self.self_state.paused()
                || self.self_state.focused()
                || !settings.notifications_allowed_now())
        {
            return;
        }
        let wanted = breakthrough
            || rule_eval.is_priority_highlight()
            || match (level, settings.notify_level) {
                (Some(NotifyLevel::Everything), _) => true,
                (Some(NotifyLevel::Mentions), _) => dm || mentioned,
                (Some(NotifyLevel::Muted), _) => false,
                (None, 0) => true,
                (None, 1) => dm || mentioned,
                (None, _) => false,
            };
        if !wanted {
            return;
        }
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
        let text = if settings.show_preview {
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
        self.system_notification_with_breakthrough(
            &tag,
            &title,
            &body,
            dm || mentioned || rule_eval.breaks_through_mute() || rule_eval.is_priority_highlight(),
            breakthrough,
            cx,
        );
        if sender_is_vip {
            let vip_sound = self.preferences.read(cx).settings().vip_sound;
            self.play_sound(vip_sound, cx);
        }
    }
}
