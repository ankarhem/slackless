//! Conversation, channel, sidebar section, tab, DM, bot resolution, profile, presence, DND,
//! usergroup, and sign-out command handlers.

use super::super::*;
use crate::NotifyLevel;
use crate::api::ProfileUpdate;

impl<B: Backend> Core<B> {
    pub(super) fn open_conversation(&mut self, channel: ChannelId) {
        self.open(channel.clone());
        self.merge_channel_info(channel.clone());
        self.fetch_peer_dnd(channel.clone());
        self.watch_peer_presence(channel);
    }

    pub(super) fn fetch_latest_history(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let unfurls = self.unfurls.clone();
        self.spawn("conversations.history", async move {
            let items = wait_out_rate_limits(|| {
                backend.recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
            })
            .await?;
            unfurls.resolve(items.iter());
            let target = channel.clone();
            with_store(&store, move |s| s.upsert_messages(&ws, &target, &items)).await?;
            events.emit(Event::StoreChanged { channel });
            Ok(())
        });
    }

    pub(super) fn close_conversation(&mut self, channel: &ChannelId) {
        if let Some(task) = self.backfills.remove(channel) {
            task.abort();
        }
    }

    pub(super) fn resolve_bots(&mut self, ids: Vec<BotId>) {
        for bot in ids {
            if let Some(profile) = self
                .bots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&bot)
                .cloned()
            {
                self.events.emit(Event::BotProfileLoaded {
                    bot_id: bot,
                    profile,
                });
                continue;
            }
            if !self.bots_asked.insert(bot.clone()) {
                continue;
            }
            let backend = self.backend.clone();
            let store = Arc::clone(&self.store);
            let bots = Arc::clone(&self.bots);
            let ws = self.workspace.clone();
            let events = self.events.clone();
            self.spawn("bots.info", async move {
                let profile = backend.bot_info(bot.clone()).await?;
                bots.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(bot.clone(), profile.clone());
                let bot_store = bot.clone();
                let profile_store = profile.clone();
                let _ = with_store(&store, move |s| {
                    s.upsert_bot_profile(&ws, &bot_store, &profile_store)
                })
                .await;
                events.emit(Event::BotProfileLoaded {
                    bot_id: bot,
                    profile,
                });
                Ok(())
            });
        }
    }

    pub(super) fn set_notify_level(&self, channel: ChannelId, level: NotifyLevel) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.prefs.setNotifications", async move {
            for &(name, value) in level.writes() {
                backend
                    .set_notification_pref(channel.clone(), name, value)
                    .await?;
            }
            with_store(&store, move |s| {
                let mut prefs = s.prefs(&ws)?.unwrap_or_default();
                crate::notify::set_notify_level(&mut prefs, &channel, level);
                s.replace_prefs(&ws, &prefs)
            })
            .await?;
            events.emit(Event::SidebarChanged);
            Ok(())
        });
    }

    pub(super) fn set_global_notification_pref(&self, name: String, value: String, sync: bool) {
        let backend = self.backend.clone();
        self.spawn("users.prefs.setNotifications(global)", async move {
            backend
                .set_global_notification_pref(name, value, sync)
                .await?;
            Ok(())
        });
    }

    pub(super) fn leave_conversation(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.leave", async move {
            backend.leave_conversation(channel.clone()).await?;
            let target = channel.clone();
            with_store(&store, move |s| match s.conversation(&ws, &target)? {
                Some(mut left) => {
                    left.is_member = false;
                    s.upsert_conversation(&ws, &left)
                }
                None => Ok(()),
            })
            .await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn join_conversation(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.join", async move {
            let mut joined = backend.join_conversation(channel.clone()).await?;
            joined.is_member = true;
            with_store(&store, move |s| s.upsert_conversation(&ws, &joined)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn set_channel_topic(&self, channel: ChannelId, topic: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.setTopic", async move {
            let updated = backend.set_topic(channel.clone(), topic).await?;
            with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn set_channel_purpose(&self, channel: ChannelId, purpose: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.setPurpose", async move {
            let updated = backend.set_purpose(channel.clone(), purpose).await?;
            with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn fetch_channel_managers(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("admin.roles.entity.listAssignments", async move {
            let users = backend.channel_managers(channel.clone()).await?;
            events.emit(Event::ChannelManagers { channel, users });
            Ok(())
        });
    }

    pub(super) fn open_dm(&self, users: Vec<UserId>) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.open", async move {
            let dm = backend.open_dm(users).await?;
            let channel = dm.id.clone();
            with_store(&store, move |s| s.upsert_conversation(&ws, &dm)).await?;
            events.emit(Event::Conversations(vec![channel.clone()]));
            events.emit(Event::ConversationOpened(channel));
            Ok(())
        });
    }

    pub(super) fn create_section(&self, name: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.channelSections.create", async move {
            backend.create_section(name).await?;
            refresh_sections(&backend, &store, ws, &events).await
        });
    }

    pub(super) fn create_section_with_channel(&self, name: String, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.channelSections.create", async move {
            let created = backend.create_section(name).await?;
            let section = created.channel_section_id;
            // The channel may sit in another section: move, not just add.
            let (probe, read_ws) = (channel.clone(), ws.clone());
            let source = with_store(&store, move |s| {
                s.sections(&read_ws).map(|sections| {
                    sections
                        .iter()
                        .find(|known| known.channel_ids.contains(&probe))
                        .map(|known| known.id.clone())
                })
            })
            .await?;
            backend
                .move_channel(
                    Some((section.clone(), channel.clone())),
                    source.map(|from| (from, channel.clone())),
                )
                .await?;
            refresh_sections(&backend, &store, ws, &events).await
        });
    }

    pub(super) fn rename_section(&self, section: String, name: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.channelSections.set", async move {
            // `.set` clears every omitted key, so the emoji is re-read and resent.
            let (read_ws, target) = (ws.clone(), section.clone());
            let emoji = with_store(&store, move |s| {
                s.sections(&read_ws).map(|sections| {
                    sections
                        .iter()
                        .find(|known| known.id == target)
                        .map(|known| known.emoji.clone())
                        .unwrap_or_default()
                })
            })
            .await?;
            backend.rename_section(section, name, emoji).await?;
            refresh_sections(&backend, &store, ws, &events).await
        });
    }

    pub(super) fn delete_section(&self, section: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.channelSections.delete", async move {
            backend.delete_section(section).await?;
            refresh_sections(&backend, &store, ws, &events).await
        });
    }

    pub(super) fn move_channel(
        &self,
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
    ) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("users.channelSections.channels.bulkUpdate", async move {
            if from != to {
                backend
                    .move_channel(
                        to.map(|to| (to, channel.clone())),
                        from.map(|from| (from, channel.clone())),
                    )
                    .await?;
            }
            refresh_sections(&backend, &store, ws, &events).await
        });
    }

    pub(super) fn create_channel(&self, name: String, is_private: bool, purpose: Option<String>) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.create", async move {
            let mut created = backend.create_channel(name, is_private).await?;
            let id = created.id.clone();
            if let Some(purpose) = purpose.filter(|p| !p.trim().is_empty())
                && let Ok(updated) = backend.set_purpose(id.clone(), purpose).await
            {
                created = updated;
            }
            created.is_member = true;
            let for_store = created.clone();
            with_store(&store, move |s| s.upsert_conversation(&ws, &for_store)).await?;
            events.emit(Event::Conversations(vec![id.clone()]));
            events.emit(Event::ConversationOpened(id));
            Ok(())
        });
    }

    pub(super) fn rename_channel(&self, channel: ChannelId, name: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.rename", async move {
            let updated = backend.rename_channel(channel.clone(), name).await?;
            with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn archive_channel(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.archive", async move {
            backend.archive_channel(channel.clone()).await?;
            update_stored_conversation(&store, ws, &events, channel, |conv| {
                conv.is_archived = true;
            })
            .await
        });
    }

    pub(super) fn unarchive_channel(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.unarchive", async move {
            backend.unarchive_channel(channel.clone()).await?;
            update_stored_conversation(&store, ws, &events, channel, |conv| {
                conv.is_archived = false;
            })
            .await
        });
    }

    pub(super) fn delete_channel(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("admin.conversations.delete", async move {
            backend.delete_channel(channel.clone()).await?;
            update_stored_conversation(&store, ws, &events, channel, |conv| {
                conv.is_archived = true;
                conv.is_member = false;
            })
            .await
        });
    }

    pub(super) fn convert_channel_to_private(&self, channel: ChannelId) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.convertToPrivate", async move {
            let updated = backend.convert_channel_to_private(channel.clone()).await?;
            with_store(&store, move |s| s.merge_conversation(&ws, &updated)).await?;
            events.emit(Event::Conversations(vec![channel]));
            events.emit(Event::SidebarChanged);
            Ok(())
        });
    }

    pub(super) fn invite_to_channel(&self, channel: ChannelId, users: Vec<UserId>) {
        if users.is_empty() {
            return;
        }
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.invite", async move {
            let updated = backend.invite_to_channel(channel.clone(), users).await?;
            let ws_merge = ws.clone();
            with_store(&store, move |s| s.merge_conversation(&ws_merge, &updated)).await?;
            if let Ok(members) = backend.conversation_members(channel.clone()).await {
                let for_counts = members.clone();
                let counts = with_store(&store, move |s| s.member_counts(&ws, &for_counts))
                    .await
                    .ok();
                events.emit(Event::ConversationInfo {
                    channel: channel.clone(),
                    num_members: counts.map(|c| c.humans),
                    num_apps: counts.map(|c| c.apps),
                    members,
                });
            }
            events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn add_conversation_tab(
        &self,
        channel: ChannelId,
        tab_type: String,
        label: Option<String>,
    ) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.addTab", async move {
            backend
                .add_conversation_tab(channel.clone(), tab_type, label)
                .await?;
            refresh_conversation_if_ok(&backend, &store, ws, &events, channel).await
        });
    }

    pub(super) fn remove_conversation_tab(&self, channel: ChannelId, tab_id: String) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.removeTab", async move {
            backend
                .remove_conversation_tab(channel.clone(), tab_id)
                .await?;
            refresh_conversation_if_ok(&backend, &store, ws, &events, channel).await
        });
    }

    pub(super) fn move_conversation_tab(
        &self,
        channel: ChannelId,
        tab_id: String,
        after: Option<String>,
    ) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        self.spawn("conversations.moveTab", async move {
            backend
                .move_conversation_tab(channel.clone(), tab_id, after)
                .await?;
            refresh_conversation_if_ok(&backend, &store, ws, &events, channel).await
        });
    }

    pub(super) fn set_dnd_snooze(&self, minutes: u32) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("dnd.setSnooze", async move {
            let status = backend.set_dnd_snooze(minutes).await?;
            events.emit(Event::SelfDndUpdated(status));
            Ok(())
        });
    }

    pub(super) fn end_dnd_snooze(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("dnd.endSnooze", async move {
            let status = backend.end_dnd_snooze().await?;
            events.emit(Event::SelfDndUpdated(status));
            Ok(())
        });
    }

    pub(super) fn end_dnd(&self) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("dnd.endDnd", async move {
            backend.dnd_end_dnd().await?;
            events.emit(Event::SelfDndUpdated(slackless_types::DndStatus::default()));
            Ok(())
        });
    }

    pub(super) fn load_team_dnd(&self, users: Vec<UserId>) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("dnd.teamInfo", async move {
            let map = backend.dnd_team_info(users).await?;
            let now = now_secs();
            let paused = map
                .into_iter()
                .map(|(uid, status)| (uid, status.paused(now)))
                .collect();
            events.emit(Event::TeamDndLoaded(paused));
            Ok(())
        });
    }

    pub(super) fn update_profile(&self, update: ProfileUpdate) {
        let backend = self.backend.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let events = self.events.clone();
        let self_user_id = Arc::clone(&self.self_user_id);
        self.spawn("users.profile.set", async move {
            let profile = backend.update_profile(update).await?;
            let me = self_user_id.lock().ok().and_then(|g| g.clone());
            if let Some(me) = me {
                let me_for_store = me.clone();
                let _ = with_store(&store, move |s| {
                    if let Some(mut user) = s.users(&ws)?.into_iter().find(|u| u.id == me_for_store)
                    {
                        user.profile = profile;
                        s.upsert_user(&ws, &user)?;
                    }
                    Ok::<_, StoreError>(())
                })
                .await;
                events.emit(Event::UsersChanged(vec![me]));
            }
            Ok(())
        });
    }

    pub(super) fn set_presence(&self, away: bool) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        let self_user_id = Arc::clone(&self.self_user_id);
        self.spawn("presence.set", async move {
            backend.set_presence(away).await?;
            let me = self_user_id.lock().ok().and_then(|g| g.clone());
            if let Some(me) = me {
                events.emit(Event::Presence {
                    users: vec![me],
                    presence: if away {
                        crate::Presence::Away
                    } else {
                        crate::Presence::Active
                    },
                });
            }
            Ok(())
        });
    }

    pub(super) fn set_user_pref(&self, name: String, value: String) {
        let backend = self.backend.clone();
        self.spawn("users.prefs.set", async move {
            backend.set_user_pref(name, value).await?;
            Ok(())
        });
    }

    pub(super) fn fetch_user_profile(&self, user: UserId) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("users.profile.get", async move {
            let (profile, labels) = tokio::join!(
                backend.user_profile(user.clone()),
                backend.team_profile_fields(),
            );
            let profile = profile?;
            let labels = labels.unwrap_or_default();
            let pronouns = profile.pronouns.clone().filter(|p| !p.trim().is_empty());
            let mut fields = Vec::new();
            if let Some(map) = profile.fields.as_ref().and_then(|v| v.as_object()) {
                for (id, entry) in map {
                    let val = entry
                        .get("value")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or_default();
                    if val.is_empty() {
                        continue;
                    }
                    let label = entry
                        .get("label")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| labels.get(id).cloned())
                        .unwrap_or_else(|| id.clone());
                    fields.push((label, val.to_owned()));
                }
            }
            events.emit(Event::UserProfileLoaded {
                user,
                pronouns,
                fields,
            });
            Ok(())
        });
    }

    pub(super) fn set_user_photo(&self, filename: String, bytes: Vec<u8>) {
        let backend = self.backend.clone();
        self.spawn("users.setPhoto", async move {
            backend.users_set_photo(filename, bytes).await?;
            Ok(())
        });
    }

    pub(super) fn create_usergroup(&self, name: String, handle: String, description: String) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("usergroups.create", async move {
            let _ = backend.create_usergroup(name, handle, description).await?;
            if let Ok(groups) = backend.usergroups().await {
                events.emit(Event::Usergroups(groups));
            }
            Ok(())
        });
    }

    pub(super) fn update_usergroup(
        &self,
        usergroup: String,
        name: String,
        handle: String,
        description: String,
    ) {
        let backend = self.backend.clone();
        let events = self.events.clone();
        self.spawn("usergroups.update", async move {
            let _ = backend
                .update_usergroup(usergroup, name, handle, description)
                .await?;
            if let Ok(groups) = backend.usergroups().await {
                events.emit(Event::Usergroups(groups));
            }
            Ok(())
        });
    }

    pub(super) fn sign_out(&self) {
        let backend = self.backend.clone();
        self.spawn("auth.revoke", async move {
            let _ = backend.auth_revoke().await;
            Ok(())
        });
    }
}

async fn update_stored_conversation(
    store: &SharedStore,
    ws: TeamId,
    events: &Emitter,
    channel: ChannelId,
    mutate: impl FnOnce(&mut slackless_types::Conversation) + Send + 'static,
) -> Result<(), CoreError> {
    let id = channel.clone();
    with_store(store, move |s| {
        if let Some(mut conv) = s.conversation(&ws, &id)? {
            mutate(&mut conv);
            s.upsert_conversation(&ws, &conv)?;
        }
        Ok::<_, StoreError>(())
    })
    .await?;
    events.emit(Event::Conversations(vec![channel]));
    Ok(())
}

async fn refresh_sections<B: Backend>(
    backend: &B,
    store: &SharedStore,
    ws: TeamId,
    events: &Emitter,
) -> Result<(), CoreError> {
    let sections = backend.channel_sections().await?;
    with_store(store, move |s| s.replace_sections(&ws, &sections)).await?;
    events.emit(Event::SidebarChanged);
    Ok(())
}

async fn refresh_conversation_if_ok<B: Backend>(
    backend: &B,
    store: &SharedStore,
    ws: TeamId,
    events: &Emitter,
    channel: ChannelId,
) -> Result<(), CoreError> {
    if let Ok(fresh) = backend.conversation_info(channel.clone()).await {
        with_store(store, move |s| s.merge_conversation(&ws, &fresh)).await?;
    }
    events.emit(Event::Conversations(vec![channel]));
    Ok(())
}
