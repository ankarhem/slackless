//! Realtime websocket event handling, channel/user resolution, and presence tracking.

use super::*;

/// Slack's recommended ceiling for one `presence_sub` frame.
const PRESENCE_SUB_MAX: usize = 500;

impl<B: Backend> Core<B> {
    /// Starts (or restarts) the backfill of `channel`, newest page first.
    pub(super) fn open(&mut self, channel: ChannelId) {
        if self
            .backfills
            .get(&channel)
            .is_some_and(|task| !task.is_finished())
        {
            return;
        }
        if let Some(abort) = self.tasks.spawn(self.backfill.clone().run(channel.clone())) {
            self.backfills.insert(channel, abort);
        }
    }

    pub(super) async fn on_rtm(&mut self, event: RtmEvent) {
        match event {
            RtmEvent::State(state) => {
                // Subscriptions last one websocket session: (re)subscribe on every `hello`.
                match &state {
                    ConnectionState::Connected { .. } => {
                        let reconnected = self.has_connected_once;
                        self.has_connected_once = true;
                        self.is_offline = false;
                        self.subscribe_dm_presence();
                        if reconnected {
                            let ctx = self.ctx();
                            self.spawn("reconnect counts", async move {
                                let counts = ctx.backend.counts().await?;
                                let targets = select_prefetch_targets(&counts);
                                ctx.events.emit(Event::Counts(Box::new(counts)));
                                prefetch_targets(ctx, targets).await
                            });
                        }
                        if !self.offline_queue.is_empty() {
                            let queued: Vec<(i64, QueuedMutation)> =
                                self.offline_queue.drain(..).collect();
                            self.events.emit(Event::OfflineQueueChanged(0));
                            self.flush_queued_mutations_sequentially(queued);
                        }
                        self.outbox_retry.reset();
                        self.flush_outbox();
                    }
                    ConnectionState::Reconnecting { .. } | ConnectionState::Disconnected { .. } => {
                        self.is_offline = true;
                    }
                    ConnectionState::Connecting => {}
                }
                self.events.emit(Event::Connection(state));
            }
            RtmEvent::Event(mut event) => {
                match &event {
                    SlackEvent::DndUpdated(ev) => {
                        self.events
                            .emit(Event::SelfDndUpdated(ev.dnd_status.clone()));
                    }
                    SlackEvent::DndUpdatedUser(ev) => {
                        if let Some(user) = &ev.user {
                            self.events.emit(Event::DndPaused {
                                user: user.clone(),
                                paused: ev.dnd_status.paused(now_secs()),
                            });
                        }
                    }
                    SlackEvent::ShRoomJoin(ev)
                    | SlackEvent::ShRoomUpdate(ev)
                    | SlackEvent::ShRoomLeave(ev) => {
                        let ended = !ev.room.is_active();
                        let participants = ev.room.participants.clone();
                        let topic = ev.room.name.clone();
                        let background_id = ev.room.background_id.clone();
                        let thread_root_ts = ev
                            .room
                            .thread_root_ts
                            .clone()
                            .filter(|ts| !ts.as_str().trim().is_empty());
                        for channel in ev.target_channels() {
                            self.events.emit(Event::Huddle(HuddleEvent::Updated {
                                channel,
                                participants: participants.clone(),
                                ended,
                                topic: topic.clone(),
                                background_id: background_id.clone(),
                                thread_root_ts: thread_root_ts.clone(),
                            }));
                        }
                    }
                    SlackEvent::HuddleInvite(inv) => {
                        if !inv.channel_id.as_str().is_empty()
                            && !inv.sender_user_id.as_str().is_empty()
                        {
                            self.events.emit(Event::Huddle(HuddleEvent::Invite {
                                channel: inv.channel_id.clone(),
                                inviter: inv.sender_user_id.clone(),
                                call_id: inv.call_id.clone(),
                            }));
                        }
                    }
                    SlackEvent::HuddleInviteCancel(cancel) => {
                        self.events
                            .emit(Event::Huddle(HuddleEvent::InviteCancelled {
                                channel: (!cancel.channel_id.as_str().is_empty())
                                    .then(|| cancel.channel_id.clone()),
                                call_id: None,
                            }));
                    }
                    _ => {}
                }
                if matches!(&event, SlackEvent::EmojiChanged(_)) {
                    let ctx = self.ctx();
                    self.spawn("emoji.list", async move {
                        let emoji = ctx.backend.custom_emoji().await?;
                        let pairs: Vec<(String, String)> = emoji.into_iter().collect();
                        let ws = ctx.workspace.clone();
                        with_store(&ctx.store, move |s| s.replace_custom_emoji(&ws, &pairs))
                            .await?;
                        ctx.events.emit(Event::SidebarChanged);
                        Ok(())
                    });
                }
                if let SlackEvent::Unknown { r#type, raw } = &event {
                    if matches!(
                        r#type.as_str(),
                        "saved_added"
                            | "saved_updated"
                            | "saved_deleted"
                            | "saved_clear"
                            | "saved_due"
                    ) {
                        self.fetch_saved();
                    } else if r#type == "commands_changed" {
                        let ctx = self.ctx();
                        self.spawn("client.appCommands", async move {
                            let commands = ctx.backend.app_commands().await?;
                            ctx.events.emit(Event::AppCommands(commands));
                            Ok(())
                        });
                    } else if r#type == "user_status_changed" {
                        if let Some(user_val) = raw.get("user")
                            && let Ok(user) =
                                serde_json::from_value::<crate::types::User>(user_val.clone())
                        {
                            let id = user.id.clone();
                            let ws = self.workspace.clone();
                            if let Err(err) =
                                with_store(&self.store, move |s| s.upsert_user(&ws, &user)).await
                            {
                                tracing::warn!(%err, "failed to store user_status_changed");
                            }
                            self.events.emit(Event::UsersChanged(vec![id]));
                        }
                    } else if matches!(
                        r#type.as_str(),
                        "view_opened" | "view_open" | "view_updated" | "view_update"
                    ) {
                        let view = merge_view_envelope(raw);
                        if view.get("type").and_then(serde_json::Value::as_str) == Some("home") {
                            self.events.emit(Event::AppHomeUpdated(view));
                        } else if matches!(r#type.as_str(), "view_updated" | "view_update") {
                            self.events.emit(Event::ModalUpdated(view));
                        } else {
                            self.events.emit(Event::ModalOpened(view));
                        }
                    } else if matches!(r#type.as_str(), "view_closed" | "view_close") {
                        let id = raw
                            .get("view")
                            .and_then(|v| v.get("id"))
                            .or_else(|| raw.get("view_id"))
                            .or_else(|| raw.get("root_view_id"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.events.emit(Event::ModalClosed(id));
                    }
                }
                if let Some(channel) = refresh_target(&event) {
                    self.refresh_conversation(channel);
                }
                let deferred_bot = self.attach_cached_bot_profile(&mut event);
                self.unfurls.resolve(event_messages(&event));
                let posted = posted_message(&event);
                let ws = self.workspace.clone();
                let changed = with_store_opt(&self.store, move |s| apply_event(s, &ws, event))
                    .await
                    .flatten();
                if let Some(changed) = changed {
                    self.events.emit(changed);
                }
                if let Some((channel, message)) = posted {
                    self.events.emit(Event::MessagePosted { channel, message });
                }
                // Resolve unknown bot profiles asynchronously AFTER storing the message so live
                // HTTP `bots.info` never blocks the RTM `select!` loop.
                if let Some((bot, msg_ref)) = deferred_bot {
                    self.spawn_fetch_bot_profile(bot, msg_ref);
                }
            }
        }
    }

    /// Attach a cached bot profile synchronously when available; otherwise claim `bot` in
    /// `bots_asked` and return `(bot, optional_message_ref)` so the caller spawns `bots.info`
    /// asynchronously off the RTM event loop.
    fn attach_cached_bot_profile(
        &mut self,
        event: &mut SlackEvent,
    ) -> Option<(BotId, Option<(ChannelId, Ts)>)> {
        let (channel, message) = match event {
            SlackEvent::Message(MessageEvent::Message(message)) => {
                (message.channel.clone(), message.as_mut())
            }
            SlackEvent::Message(MessageEvent::Changed(changed)) => {
                (changed.channel.clone(), changed.message.as_mut())
            }
            _ => return None,
        };
        let bot = message
            .bot_id
            .clone()
            .filter(|_| message.bot_profile.is_none())?;
        if let Some(profile) = self
            .bots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&bot)
            .cloned()
        {
            message.bot_profile = Some(profile);
            return None;
        }
        if !self.bots_asked.insert(bot.clone()) {
            return None;
        }
        let msg_ref = channel.map(|ch| (ch, message.ts.clone()));
        Some((bot, msg_ref))
    }

    /// Re-fetches a conversation whose event carries only its id, merging into the store so
    /// existing metadata is never clobbered by partial payloads.
    pub(super) fn refresh_conversation(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.info", async move {
            let conversation = ctx.backend.conversation_info(channel.clone()).await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                s.merge_conversation(&ws, &conversation)
            })
            .await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    /// Open-time `conversations.info` + `conversations.members` refresh.
    pub(super) fn merge_channel_info(&self, channel: ChannelId) {
        let ctx = self.ctx();
        let teams_asked = Arc::clone(&self.teams_asked);
        self.spawn("conversations.info", async move {
            let mut fresh = match ctx.backend.conversation_info(channel.clone()).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    ctx.events.emit(Event::ConversationInfo {
                        channel,
                        num_members: None,
                        num_apps: None,
                        members: Vec::new(),
                    });
                    return Err(e.into());
                }
            };
            let mut members = Vec::new();
            let mut num_apps = None;
            let mut external_teams: Vec<TeamId> = fresh.external_team_ids(Some(&ctx.workspace));
            if let Some(host) = fresh.host_team_id()
                && host != ctx.workspace
                && !external_teams.contains(&host)
            {
                external_teams.push(host);
            }
            for pending in &fresh.pending_connected_team_ids {
                if *pending != ctx.workspace && !external_teams.contains(pending) {
                    external_teams.push(pending.clone());
                }
            }
            let is_shared_channel = fresh.is_external_connect() || fresh.is_org_shared;
            if let Ok(listed) = ctx.backend.conversation_members(channel.clone()).await {
                let ws2 = ctx.workspace.clone();
                let (counts, teams, listed) = with_store(&ctx.store, move |s| {
                    let counts = s.member_counts(&ws2, &listed)?;
                    let teams = if is_shared_channel {
                        s.member_team_ids(&ws2, &listed)?
                    } else {
                        Vec::new()
                    };
                    Ok((counts, teams, listed))
                })
                .await?;
                fresh.num_members = Some(counts.humans);
                num_apps = Some(counts.apps);
                for team in teams {
                    if team != ctx.workspace && !external_teams.contains(&team) {
                        external_teams.push(team);
                    }
                }
                members = listed;
            }
            // Enrich connected organization records via `conversations.teamConnections` when
            // available; non-fatal if Slack restricts or omits it on the current workspace.
            let mut resolved_conn_teams = Vec::new();
            if fresh.is_external_connect()
                && let Ok(conns) = ctx.backend.team_connections(channel.clone()).await
            {
                for team in conns
                    .connections
                    .into_iter()
                    .chain(conns.pending_connections)
                {
                    if !team.id.as_str().is_empty() {
                        external_teams.retain(|id| id != &team.id);
                        resolved_conn_teams.push(team);
                    }
                }
            }
            let num_members = fresh.num_members;
            let ws2 = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                s.merge_conversation(&ws2, &fresh)?;
                for team in &resolved_conn_teams {
                    s.upsert_team(&ws2, team)?;
                }
                Ok(())
            })
            .await?;
            ctx.events.emit(Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            });
            if !external_teams.is_empty() {
                resolve_teams(ctx, teams_asked, external_teams).await;
            }
            Ok(())
        });
    }

    /// Open-time `dnd.info` for a DM peer: whether their notifications are paused.
    pub(super) fn fetch_peer_dnd(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("dnd.info", async move {
            let Some(peer) = im_peer(&ctx.store, &ctx.workspace, &channel).await? else {
                return Ok(());
            };
            let status = ctx.backend.dnd_info(peer.clone()).await?;
            ctx.events.emit(Event::DndPaused {
                user: peer,
                paused: status.paused(now_secs()),
            });
            Ok(())
        });
    }

    /// `users.info` for people `users.list` leaves out (members of other organizations in
    /// shared channels and DMs), each id asked once per session.
    pub(super) fn resolve_users(&mut self, ids: Vec<UserId>) {
        let ids: Vec<UserId> = ids
            .into_iter()
            .filter(|id| self.users_asked.insert(id.clone()))
            .collect();
        if ids.is_empty() {
            return;
        }
        let ctx = self.ctx();
        let teams_asked = Arc::clone(&self.teams_asked);
        self.spawn("users.info", async move {
            // Check only the requested ids via PK lookup instead of scanning every stored user.
            let ws_known = ctx.workspace.clone();
            let ids = with_store(&ctx.store, move |s| {
                let mut unknown = Vec::with_capacity(ids.len());
                for id in ids {
                    if s.user(&ws_known, &id)?.is_none() {
                        unknown.push(id);
                    }
                }
                Ok(unknown)
            })
            .await?;
            if ids.is_empty() {
                return Ok(());
            }
            let mut users = Vec::new();
            for id in ids {
                match ctx.backend.user_info(id).await {
                    Ok(user) => users.push(user),
                    Err(e) => ctx.events.report("users.info", e.into()),
                }
            }
            if users.is_empty() {
                return Ok(());
            }
            let ids = users.iter().map(|user| user.id.clone()).collect();
            let teams: Vec<TeamId> = users
                .iter()
                .filter_map(|user| user.team_id.clone())
                .filter(|team| *team != ctx.workspace)
                .collect();
            let ws2 = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                users.iter().try_for_each(|user| s.upsert_user(&ws2, user))
            })
            .await?;
            ctx.events.emit(Event::UsersChanged(ids));
            if !teams.is_empty() {
                resolve_teams(ctx, teams_asked, teams).await;
            }
            Ok(())
        });
    }

    /// Subscribes presence for every DM peer (capped at [`PRESENCE_SUB_MAX`]).
    pub(super) fn subscribe_dm_presence(&self) {
        let Some(rtm) = self.rtm.clone() else {
            return;
        };
        let ctx = self.ctx();
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("presence_sub", async move {
            let ws = ctx.workspace.clone();
            let dms = with_store(&ctx.store, move |s| {
                s.list_conversations_by_kinds(&ws, &["im"])
            })
            .await?;
            let peers = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                for peer in dms.into_iter().filter_map(|c| c.user) {
                    if !watched.contains(&peer) {
                        watched.push(peer);
                    }
                }
                watched.truncate(PRESENCE_SUB_MAX);
                watched.clone()
            };
            if let Err(e) = rtm.presence_sub(peers) {
                ctx.events.emit(Event::Error(format!("presence_sub: {e}")));
            }
            Ok(())
        });
    }

    /// Presence for a DM peer: one `users.getPresence` read the first time their DM opens,
    /// then the websocket subscription keeps it current (bounded to [`PRESENCE_SUB_MAX`]).
    pub(super) fn watch_peer_presence(&self, channel: ChannelId) {
        let ctx = self.ctx();
        let rtm = self.rtm.clone();
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("users.getPresence", async move {
            let Some(peer) = im_peer(&ctx.store, &ctx.workspace, &channel).await? else {
                return Ok(());
            };
            let newly_watched = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                if watched.contains(&peer) {
                    false
                } else {
                    while watched.len() >= PRESENCE_SUB_MAX {
                        watched.remove(0);
                    }
                    watched.push(peer.clone());
                    true
                }
            };
            if !newly_watched {
                return Ok(());
            }
            if let Some(rtm) = &rtm {
                let peers = watched
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let Err(e) = rtm.presence_sub(peers) {
                    ctx.events.emit(Event::Error(format!("presence_sub: {e}")));
                }
            }
            let presence = ctx.backend.user_presence(peer.clone()).await?;
            ctx.events.emit(Event::Presence {
                users: vec![peer],
                presence: presence.presence,
            });
            Ok(())
        });
    }
}

/// Thread broadcasts, not edits, deletions or channel events (joins, topics, ...).
pub(super) fn posted_message(event: &SlackEvent) -> Option<(ChannelId, Box<Message>)> {
    let SlackEvent::Message(MessageEvent::Message(message)) = event else {
        return None;
    };
    let posted = match message.subtype {
        None => true,
        Some(ref subtype) => matches!(
            subtype,
            MessageSubtype::BotMessage
                | MessageSubtype::MeMessage
                | MessageSubtype::FileShare
                | MessageSubtype::ThreadBroadcast
        ),
    };
    posted.then(|| Some((message.channel.clone()?, message.clone())))?
}

/// Conversation events without a conversation object; the store needs a fresh copy.
pub(super) fn refresh_target(event: &SlackEvent) -> Option<ChannelId> {
    match event {
        SlackEvent::ChannelArchive(e)
        | SlackEvent::ChannelUnarchive(e)
        | SlackEvent::ChannelLeft(e) => Some(e.channel.clone()),
        _ => None,
    }
}

/// Merge top-level Slack `view_opened`/`view_updated` envelope fields (`title`, `submit`, `close`,
/// `app_id`, `app_installed_team_id`, `root_view_id`, `previous_view_id`, `view_id`,
/// `client_token`) into `raw["view"]` when absent inside the nested view object
/// (`docs/research/api-apps-blocks.md`).
pub(super) fn merge_view_envelope(raw: &serde_json::Value) -> serde_json::Value {
    let mut view = raw.get("view").cloned().unwrap_or_else(|| raw.clone());
    let Some(obj) = view.as_object_mut() else {
        return view;
    };
    if !obj.contains_key("id")
        && let Some(id) = raw.get("view_id").filter(|v| !v.is_null())
    {
        obj.insert("id".to_owned(), id.clone());
    }
    if !obj.contains_key("type")
        && let Some(kind) = raw.get("view_type").filter(|v| !v.is_null())
    {
        obj.insert("type".to_owned(), kind.clone());
    }
    for key in [
        "title",
        "submit",
        "close",
        "app_id",
        "app_installed_team_id",
        "root_view_id",
        "previous_view_id",
        "client_token",
        "bot_id",
    ] {
        let missing = obj.get(key).is_none_or(serde_json::Value::is_null);
        if missing && let Some(val) = raw.get(key).filter(|v| !v.is_null()) {
            obj.insert(key.to_owned(), val.clone());
        }
    }
    view
}

/// The peer of an IM channel as stored; `None` unless the channel is an IM with a known peer.
pub(super) async fn im_peer(
    store: &SharedStore,
    workspace: &TeamId,
    channel: &ChannelId,
) -> Result<Option<UserId>, CoreError> {
    let (ws, channel) = (workspace.clone(), channel.clone());
    let stored = with_store(store, move |s| s.conversation(&ws, &channel)).await?;
    Ok(stored
        .filter(|c| c.kind() == ConversationKind::Im)
        .and_then(|c| c.user))
}
