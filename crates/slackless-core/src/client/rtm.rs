//! Realtime websocket event handling, channel/user resolution, and presence tracking.

use super::*;

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
        let task = tokio::spawn(self.backfill.clone().run(channel.clone()));
        self.backfills.insert(channel, task.abort_handle());
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
                            let (backend, store, ws, events, unfurls) = (
                                self.backend.clone(),
                                Arc::clone(&self.store),
                                self.workspace.clone(),
                                self.events.clone(),
                                self.unfurls.clone(),
                            );
                            self.spawn("reconnect counts", async move {
                                let counts = backend.counts().await?;
                                let unread_channels: Vec<(ChannelId, Option<Ts>)> = counts
                                    .ims
                                    .iter()
                                    .chain(&counts.mpims)
                                    .chain(&counts.channels)
                                    .filter(|c| c.is_unread())
                                    .take(25)
                                    .map(|c| (c.id.clone(), c.latest.clone()))
                                    .collect();
                                events.emit(Event::Counts(Box::new(counts)));
                                for (idx, (channel, latest)) in
                                    unread_channels.into_iter().enumerate()
                                {
                                    if let Some(ref latest_ts) = latest {
                                        if !latest_ts.as_str().is_empty() {
                                            let (ws_c, ch_c) = (ws.clone(), channel.clone());
                                            let local_newest = with_store(&store, move |s| {
                                                s.channel_history(&ws_c, &ch_c, None, 1).map(
                                                    |rows| {
                                                        rows.into_iter()
                                                            .last()
                                                            .map(|r| r.message.ts)
                                                    },
                                                )
                                            })
                                            .await?;
                                            if local_newest
                                                .as_ref()
                                                .is_some_and(|local| local >= latest_ts)
                                            {
                                                continue;
                                            }
                                        }
                                    }
                                    if idx >= 3 {
                                        tokio::time::sleep(std::time::Duration::from_millis(150))
                                            .await;
                                    }
                                    if let Ok(items) = wait_out_rate_limits(|| {
                                        backend.recent_history(
                                            channel.clone(),
                                            crate::backend::UNREADS_HISTORY_LIMIT,
                                        )
                                    })
                                    .await
                                    {
                                        if !items.is_empty() {
                                            unfurls.resolve(items.iter());
                                            let (ws_p, target) = (ws.clone(), channel.clone());
                                            let _ = with_store(&store, move |s| {
                                                s.upsert_messages(&ws_p, &target, &items)
                                            })
                                            .await;
                                            events.emit(Event::StoreChanged { channel });
                                        }
                                    }
                                }
                                Ok(())
                            });
                        }
                        if !self.offline_queue.is_empty() {
                            let queued: Vec<(i64, QueuedMutation)> =
                                self.offline_queue.drain(..).collect();
                            self.events.emit(Event::OfflineQueueChanged(0));
                            self.flush_queued_mutations_sequentially(queued);
                        }
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
                            self.events.emit(Event::HuddleUpdated {
                                channel,
                                participants: participants.clone(),
                                ended,
                                topic: topic.clone(),
                                background_id: background_id.clone(),
                                thread_root_ts: thread_root_ts.clone(),
                            });
                        }
                    }
                    SlackEvent::HuddleInvite(inv) => {
                        if !inv.channel_id.as_str().is_empty()
                            && !inv.sender_user_id.as_str().is_empty()
                        {
                            self.events.emit(Event::HuddleInvite {
                                channel: inv.channel_id.clone(),
                                inviter: inv.sender_user_id.clone(),
                                call_id: inv.call_id.clone(),
                            });
                        }
                    }
                    SlackEvent::HuddleInviteCancel(cancel) => {
                        self.events.emit(Event::HuddleInviteCancelled {
                            channel: (!cancel.channel_id.as_str().is_empty())
                                .then(|| cancel.channel_id.clone()),
                            call_id: None,
                        });
                    }
                    _ => {}
                }
                if matches!(&event, SlackEvent::EmojiChanged(_)) {
                    let (backend, store, ws, events) = (
                        self.backend.clone(),
                        Arc::clone(&self.store),
                        self.workspace.clone(),
                        self.events.clone(),
                    );
                    self.spawn("emoji.list", async move {
                        if let Ok(emoji) = backend.custom_emoji().await {
                            let pairs: Vec<(String, String)> = emoji.into_iter().collect();
                            let _ =
                                with_store(&store, move |s| s.replace_custom_emoji(&ws, &pairs))
                                    .await;
                            events.emit(Event::SidebarChanged);
                        }
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
                        let (backend, events) = (self.backend.clone(), self.events.clone());
                        self.spawn("client.appCommands", async move {
                            if let Ok(commands) = backend.app_commands().await {
                                events.emit(Event::AppCommands(commands));
                            }
                            Ok(())
                        });
                    } else if r#type == "user_status_changed" {
                        if let Some(user_val) = raw.get("user") {
                            if let Ok(user) =
                                serde_json::from_value::<crate::types::User>(user_val.clone())
                            {
                                let id = user.id.clone();
                                let ws = self.workspace.clone();
                                let _ = with_store(&self.store, move |s| s.upsert_user(&ws, &user))
                                    .await;
                                self.events.emit(Event::UsersChanged(vec![id]));
                            }
                        }
                    } else if matches!(
                        r#type.as_str(),
                        "view_opened" | "view_open" | "view_updated" | "view_update"
                    ) {
                        let view = raw.get("view").cloned().unwrap_or_else(|| raw.clone());
                        if view.get("type").and_then(serde_json::Value::as_str) == Some("home") {
                            self.events.emit(Event::AppHomeUpdated(view));
                        } else {
                            self.events.emit(Event::ModalOpened(view));
                        }
                    } else if matches!(r#type.as_str(), "view_closed" | "view_close") {
                        let id = raw
                            .get("view")
                            .and_then(|v| v.get("id"))
                            .or_else(|| raw.get("view_id"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        self.events.emit(Event::ModalClosed(id));
                    }
                }
                if let Some(channel) = refresh_target(&event) {
                    self.refresh_conversation(channel);
                }
                self.attach_bot_profile(&mut event).await;
                self.unfurls.resolve(event_messages(&event));
                let posted = posted_message(&event);
                let ws = self.workspace.clone();
                let changed = with_store(&self.store, move |s| apply_event(s, &ws, event)).await;
                if let Some(changed) = changed {
                    self.events.emit(changed);
                }
                if let Some((channel, message)) = posted {
                    self.events.emit(Event::MessagePosted { channel, message });
                }
            }
        }
    }

    /// An ephemeral app message (Giphy's preview) carries only its `bot_id`: give it the
    /// bot's profile (`bots.info`, once per bot per session) so it shows the app's name and
    /// icon and its buttons know the app to dispatch to.
    pub(super) async fn attach_bot_profile(&mut self, event: &mut SlackEvent) {
        let message = match event {
            SlackEvent::Message(MessageEvent::Message(message)) => message.as_mut(),
            SlackEvent::Message(MessageEvent::Changed(changed)) => changed.message.as_mut(),
            _ => return,
        };
        let Some(bot) = message
            .bot_id
            .clone()
            .filter(|_| message.bot_profile.is_none())
        else {
            return;
        };
        if !self.bots.contains_key(&bot) {
            match self.backend.bot_info(bot.clone()).await {
                Ok(profile) => {
                    self.bots.insert(bot.clone(), profile);
                }
                Err(e) => return self.events.report("bots.info", e.into()),
            }
        }
        message.bot_profile = self.bots.get(&bot).cloned();
    }

    /// Re-fetches a conversation whose event carries only its id.
    pub(super) fn refresh_conversation(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        self.spawn("conversations.info", async move {
            let conversation = backend.conversation_info(channel.clone()).await?;
            with_store(&store, move |s| s.upsert_conversation(&ws, &conversation)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    /// Open-time `conversations.info`: metadata `client.userBoot` omits (topic, member
    /// counts) merged into the stored copy, so richer stored fields survive the refresh. The
    /// header's counts come from `conversations.members` cross-checked against the stored
    /// profiles (`Event::ConversationInfo` carries the split, plus the member id list for
    /// member panels and Slack Connect org counts); when the member list cannot be read,
    /// `conversations.info`'s raw `num_members` stands. A Slack Connect channel also resolves
    /// the connected organizations' `team.info` records, each asked once per session.
    pub(super) fn merge_channel_info(&self, channel: ChannelId) {
        let (backend, store, ws, events, teams_asked) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            Arc::clone(&self.teams_asked),
        );
        self.spawn("conversations.info", async move {
            let mut fresh = match backend.conversation_info(channel.clone()).await {
                Ok(fresh) => fresh,
                Err(e) => {
                    // Nothing is known; the announcement still lands so front-ends waiting on
                    // the member list stop waiting (the failure itself is reported).
                    events.emit(Event::ConversationInfo {
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
            let mut external_teams = Vec::new();
            if let Ok(listed) = backend.conversation_members(channel.clone()).await {
                let (ws2, shared) = (ws.clone(), fresh.is_ext_shared);
                // One store read for both: the header's split and, for a Slack Connect
                // channel, the other organizations its stored people belong to. The list
                // rides the event only: stored on the row it would bloat every snapshot.
                let (counts, teams, listed) = with_store(&store, move |s| {
                    let counts = s.member_counts(&ws2, &listed)?;
                    let teams = if shared {
                        s.member_team_ids(&ws2, &listed)?
                    } else {
                        Vec::new()
                    };
                    Ok::<_, StoreError>((counts, teams, listed))
                })
                .await?;
                fresh.num_members = Some(counts.humans);
                num_apps = Some(counts.apps);
                external_teams = teams;
                members = listed;
            }
            let num_members = fresh.num_members;
            let ws2 = ws.clone();
            with_store(&store, move |s| s.merge_conversation(&ws2, &fresh)).await?;
            events.emit(Event::ConversationInfo {
                channel,
                num_members,
                num_apps,
                members,
            });
            if !external_teams.is_empty() {
                resolve_teams(backend, store, ws, events, teams_asked, external_teams).await;
            }
            Ok(())
        });
    }

    /// Open-time `dnd.info` for a DM peer: whether their notifications are paused. Transient
    /// by design, so it is re-fetched every time the DM opens and never stored.
    pub(super) fn fetch_peer_dnd(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        self.spawn("dnd.info", async move {
            let Some(peer) = im_peer(&store, &ws, &channel).await? else {
                return Ok(());
            };
            let status = backend.dnd_info(peer.clone()).await?;
            events.emit(Event::DndPaused {
                user: peer,
                paused: status.paused(now_secs()),
            });
            Ok(())
        });
    }

    /// `users.info` for people `users.list` leaves out (members of other organizations in
    /// shared channels and DMs), each id asked once per session. Found profiles are stored
    /// and announced together; a failed lookup is reported and the rest still land. The
    /// organizations the people found belong to are resolved next ([`resolve_teams`]), so a
    /// Slack Connect channel can name them on its first visit.
    pub(super) fn resolve_users(&mut self, ids: Vec<UserId>) {
        let ids: Vec<UserId> = ids
            .into_iter()
            .filter(|id| self.users_asked.insert(id.clone()))
            .collect();
        if ids.is_empty() {
            return;
        }
        let (backend, store, ws, events, teams_asked) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            Arc::clone(&self.teams_asked),
        );
        self.spawn("users.info", async move {
            // A full record already in the store (users.list or an earlier ask) needs no
            // call: only the store-unknown ids - another organization's people, guests -
            // are asked for.
            let ws_known = ws.clone();
            let stored = with_store(&store, move |s| {
                s.users(&ws_known).map(|users| {
                    users
                        .into_iter()
                        .map(|user| user.id)
                        .collect::<HashSet<UserId>>()
                })
            })
            .await
            .unwrap_or_default();
            let ids: Vec<UserId> = ids.into_iter().filter(|id| !stored.contains(id)).collect();
            if ids.is_empty() {
                return Ok(());
            }
            let mut users = Vec::new();
            for id in ids {
                match backend.user_info(id).await {
                    Ok(user) => users.push(user),
                    Err(e) => events.report("users.info", e.into()),
                }
            }
            if users.is_empty() {
                return Ok(());
            }
            let ids = users.iter().map(|user| user.id.clone()).collect();
            let teams: Vec<TeamId> = users
                .iter()
                .filter_map(|user| user.team_id.clone())
                .filter(|team| *team != ws)
                .collect();
            let ws2 = ws.clone();
            with_store(&store, move |s| {
                users.iter().try_for_each(|user| s.upsert_user(&ws2, user))
            })
            .await?;
            events.emit(Event::UsersChanged(ids));
            if !teams.is_empty() {
                resolve_teams(backend, store, ws, events, teams_asked, teams).await;
            }
            Ok(())
        });
    }

    /// Subscribes presence for every DM peer, so each DM row carries Slack's dot. Slack answers
    /// a `presence_sub` with the current presence of each newly subscribed id, so no per-user
    /// request is needed; each frame replaces the whole subscription, so the accumulated peers
    /// (including ones a DM open added) are resent.
    pub(super) fn subscribe_dm_presence(&self) {
        /// Slack's recommended ceiling for one presence subscription.
        const PRESENCE_SUB_MAX: usize = 500;
        let Some(rtm) = self.rtm.clone() else {
            return;
        };
        let (store, ws, events) = (
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("presence_sub", async move {
            let conversations = with_store(&store, move |s| s.conversations(&ws)).await?;
            let peers = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                for peer in conversations.into_iter().filter_map(|c| c.user) {
                    if !watched.contains(&peer) {
                        watched.push(peer);
                    }
                }
                watched.truncate(PRESENCE_SUB_MAX);
                watched.clone()
            };
            if let Err(e) = rtm.presence_sub(peers) {
                events.emit(Event::Error(format!("presence_sub: {e}")));
            }
            Ok(())
        });
    }

    /// Presence for a DM peer: one `users.getPresence` read the first time their DM opens,
    /// then the websocket subscription keeps it current through `presence_change` events
    /// (each `presence_sub` frame replaces the previous subscription, so the whole
    /// accumulated peer set is resent).
    pub(super) fn watch_peer_presence(&self, channel: ChannelId) {
        let (backend, store, ws, events) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
        );
        let rtm = self.rtm.clone();
        let watched = Arc::clone(&self.presence_peers);
        self.spawn("users.getPresence", async move {
            let Some(peer) = im_peer(&store, &ws, &channel).await? else {
                return Ok(());
            };
            let newly_watched = {
                let mut watched = watched.lock().unwrap_or_else(PoisonError::into_inner);
                if watched.contains(&peer) {
                    false
                } else {
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
                    events.emit(Event::Error(format!("presence_sub: {e}")));
                }
            }
            let presence = backend.user_presence(peer.clone()).await?;
            events.emit(Event::Presence {
                users: vec![peer],
                presence: presence.presence,
            });
            Ok(())
        });
    }
}

/// thread broadcasts, not edits, deletions or channel events (joins, topics, ...).
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

/// The peer of an IM channel as stored; `None` unless the channel is an IM with a known peer.
pub(super) async fn im_peer(
    store: &SharedStore,
    workspace: &TeamId,
    channel: &ChannelId,
) -> Result<Option<UserId>, StoreError> {
    let (ws, channel) = (workspace.clone(), channel.clone());
    let stored = with_store(store, move |s| s.conversation(&ws, &channel)).await?;
    Ok(stored
        .filter(|c| c.kind() == ConversationKind::Im)
        .and_then(|c| c.user))
}
