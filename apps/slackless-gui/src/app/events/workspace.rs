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
        if ws_index == self.workspace {
            match event {
                Event::UsergroupUsersLoaded { usergroup, users } => {
                    self.on_usergroup_users_loaded(usergroup.clone(), users.clone(), cx);
                    return;
                }
                Event::Usergroups(groups) => {
                    crate::directory::reconcile_usergroups_from_server(groups, cx);
                }
                _ => {}
            }
            self.on_core_event(event, window, cx);
            return;
        }
        match event {
            Event::Counts(counts) => {
                let map: HashMap<ChannelId, ConversationCounts> =
                    [&counts.channels, &counts.ims, &counts.mpims]
                        .into_iter()
                        .flatten()
                        .map(|entry| (entry.id.clone(), entry.clone()))
                        .collect();
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.counts = map.clone();
                }
                if let Some(slot) = self.bg_counts.get_mut(ws_index) {
                    *slot = map;
                }
                self.sync_badge(cx);
                cx.notify();
            }
            Event::ActivityView { feed } => {
                if let Ok(feed) = feed.as_deref() {
                    let unread = feed.items.iter().filter(|item| item.unread).count() as u32;
                    if let Some(slot) = self.bg_activity_unread.get_mut(ws_index) {
                        *slot = unread;
                    }
                    self.sync_badge(cx);
                    cx.notify();
                }
            }
            Event::SavedLoaded(Ok(items)) => {
                self.apply_saved_loaded(ws_index, items, cx);
            }
            Event::Conversations(_) | Event::SidebarChanged => {
                // Refresh this background workspace's team name/icon in `live_teams` if hydrate
                // just stored a richer `team.info` record (O(1) single-row team lookup).
                let fresh_team = self
                    .parked_bridges
                    .get(ws_index)
                    .and_then(Option::as_ref)
                    .and_then(CoreBridge::team_summary);
                if let (Some(team), Some(slot)) = (fresh_team, self.live_teams.get_mut(ws_index))
                    && *slot != team
                {
                    *slot = team;
                    self.reload_live(window, cx);
                }
            }
            Event::Usergroups(groups) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.usergroups = groups.clone();
                }
            }
            Event::AppCommands(commands) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.app_commands = commands.clone();
                }
            }
            Event::Connection(state) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.connection = Some(state.clone());
                }
            }
            Event::OfflineQueueChanged(count) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.offline_queued = *count;
                }
            }
            Event::OutboxUpdated(items) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.outbox = items.clone();
                }
            }
            Event::BackfillProgress(progress) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked.state.backfill_progress = Some(progress.clone());
                }
            }
            Event::OfflineMutationFailed(failed) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut)
                    && !parked
                        .state
                        .failed_mutations
                        .iter()
                        .any(|m| m.id == failed.id)
                {
                    parked.state.failed_mutations.push(failed.clone());
                }
            }
            Event::BotProfileLoaded { bot_id, profile } => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked
                        .state
                        .bots
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(bot_id.clone(), profile.clone());
                }
            }
            Event::Presence { users, presence } => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    parked
                        .state
                        .presence
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .extend(users.iter().map(|u| (u.clone(), presence.clone())));
                }
            }
            Event::ReadMarker { channel, ts } => {
                let apply = |counts: &mut HashMap<ChannelId, ConversationCounts>| {
                    let entry =
                        counts
                            .entry(channel.clone())
                            .or_insert_with(|| ConversationCounts {
                                id: channel.clone(),
                                ..Default::default()
                            });
                    entry.last_read = Some(ts.clone());
                    let caught_up = entry.latest.as_ref().is_none_or(|latest| *ts >= *latest);
                    if caught_up {
                        entry.clear_unreads();
                    } else {
                        entry.has_unreads = true;
                    }
                };
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    apply(&mut parked.state.counts);
                    if parked
                        .state
                        .counts
                        .get(channel)
                        .is_some_and(|c| !c.is_unread())
                    {
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
                if let Some(slot) = self.bg_counts.get_mut(ws_index) {
                    apply(slot);
                }
                self.sync_badge(cx);
                cx.notify();
            }
            Event::MessagePosted { channel, message } => {
                let settings = self.preferences.read(cx).settings().clone();
                let bg_ctx = self
                    .parked_bridges
                    .get(ws_index)
                    .and_then(Option::as_ref)
                    .and_then(|b| b.bg_message_context(channel, message.user.as_ref()));
                let reply = message
                    .thread_ts
                    .as_ref()
                    .is_some_and(|ts| *ts != message.ts);
                let broadcast =
                    message.subtype == Some(slackless_core::types::MessageSubtype::ThreadBroadcast);
                let counts_in_channel =
                    !message.hidden && !message.is_ephemeral && (!reply || broadcast);
                if counts_in_channel {
                    let from_self = bg_ctx
                        .as_ref()
                        .is_some_and(|s| message.user.as_ref() == Some(&s.self_user));
                    if !from_self {
                        let is_dm = bg_ctx
                            .as_ref()
                            .and_then(|s| s.conversation.as_ref())
                            .is_some_and(|c| {
                                matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim)
                            });
                        let is_im = bg_ctx
                            .as_ref()
                            .and_then(|s| s.conversation.as_ref())
                            .is_some_and(|c| c.kind() == ConversationKind::Im);
                        let mentioned = bg_ctx.as_ref().is_some_and(|s| {
                            mentions_me(&message.text, &s.self_user, &settings.keywords)
                        });
                        let bump = |counts: &mut HashMap<ChannelId, ConversationCounts>| {
                            let entry = counts.entry(channel.clone()).or_insert_with(|| {
                                ConversationCounts {
                                    id: channel.clone(),
                                    ..Default::default()
                                }
                            });
                            if entry.latest.as_ref().is_none_or(|l| message.ts > *l) {
                                entry.latest = Some(message.ts.clone());
                            }
                            entry.has_unreads = true;
                            if is_dm || mentioned {
                                entry.mention_count = entry.mention_count.saturating_add(1);
                            }
                            if is_im {
                                entry.dm_count = entry.dm_count.saturating_add(1);
                            }
                        };
                        if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut)
                        {
                            bump(&mut parked.state.counts);
                            parked
                                .views
                                .sidebar
                                .update(cx, |s, cx| s.mark_unread(channel, cx));
                        }
                        if let Some(slot) = self.bg_counts.get_mut(ws_index) {
                            bump(slot);
                        }
                        self.sync_badge(cx);
                        cx.notify();
                    }
                }
                if !settings.desktop_notifications {
                    return;
                }
                let Some(bg_ctx) = bg_ctx else {
                    return;
                };
                if message.user.as_ref() == Some(&bg_ctx.self_user) {
                    return;
                }
                let level = bg_ctx.notify_level;
                if level == Some(NotifyLevel::Muted) {
                    return;
                }
                let Some(conv) = bg_ctx.conversation.as_ref() else {
                    return;
                };
                let dm = matches!(conv.kind(), ConversationKind::Im | ConversationKind::Mpim);
                let sender_is_vip = message.user.as_ref().is_some_and(|u| {
                    let handle = bg_ctx
                        .sender
                        .as_ref()
                        .map(|x| x.name.as_str())
                        .unwrap_or("");
                    let display = bg_ctx
                        .sender
                        .as_ref()
                        .map(slackless_core::types::User::display_name)
                        .unwrap_or("");
                    settings.is_vip(u.as_str(), handle, display)
                });
                let breakthrough =
                    sender_is_vip || settings.is_urgent_dm_breakthrough(dm, &message.text);
                if !breakthrough
                    && (self.self_state.paused()
                        || self.self_state.focused()
                        || !settings.notifications_allowed_now())
                {
                    return;
                }
                let mentioned = mentions_me(&message.text, &bg_ctx.self_user, &settings.keywords);
                let wanted = match (level, settings.notify_level) {
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
                    .live_teams
                    .get(ws_index)
                    .map(|t| t.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| "Slack".to_owned());
                let sender = bg_ctx
                    .sender
                    .as_ref()
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
                let (title, body) = match conv.kind() {
                    ConversationKind::Im => (format!("{ws_name} — {sender}"), text),
                    ConversationKind::Mpim => (
                        format!("{ws_name} — {}", conv.name.as_deref().unwrap_or("Group DM")),
                        format!("{sender}: {text}"),
                    ),
                    _ => (
                        format!("{ws_name} — #{}", conv.name.as_deref().unwrap_or_default()),
                        format!("{sender}: {text}"),
                    ),
                };
                let tag = format!("bg:{ws_index}:{channel}:{}", message.ts);
                self.system_notification_kind(&tag, &title, &body, dm || mentioned, cx);
                if sender_is_vip {
                    let vip_sound = self.preferences.read(cx).settings().vip_sound;
                    self.play_sound(vip_sound, cx);
                }
            }
            Event::Huddle(HuddleEvent::Updated {
                channel,
                participants,
                ended,
                ..
            }) => {
                if let Some(parked) = self.parked.get_mut(ws_index).and_then(Option::as_mut) {
                    if *ended {
                        parked.state.active_huddles.remove(channel);
                    } else {
                        parked
                            .state
                            .active_huddles
                            .insert(channel.clone(), participants.clone());
                    }
                    let huddles = parked.state.active_huddles.clone();
                    parked
                        .views
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.set_huddles(huddles, cx));
                }
            }
            _ => {}
        }
    }
}
