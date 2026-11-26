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
        let ctx = self.ctx();
        self.spawn("conversations.history", async move {
            let fetched_at = now_secs();
            let items = wait_out_rate_limits(|| {
                ctx.backend
                    .recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
            })
            .await?;
            ctx.unfurls.resolve(items.iter());
            let target = channel.clone();
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                s.upsert_history_page(&ws, &target, &items, fetched_at)
            })
            .await?;
            ctx.events.emit(Event::StoreChanged { channel });
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
            self.spawn_fetch_bot_profile(bot, None);
        }
    }

    /// Unified bot-profile fetch, memory cache, SQLite persistence, and optional message-backfill
    /// helper shared between [`Self::resolve_bots`] and RTM message handling (`rtm.rs`).
    pub(in crate::client) fn spawn_fetch_bot_profile(
        &self,
        bot: BotId,
        message_ref: Option<(ChannelId, Ts)>,
    ) {
        let ctx = self.ctx();
        let bots = Arc::clone(&self.bots);
        self.spawn("bots.info", async move {
            let profile = ctx.backend.bot_info(bot.clone()).await?;
            bots.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(bot.clone(), profile.clone());
            let ws = ctx.workspace.clone();
            let bot_store = bot.clone();
            let profile_store = profile.clone();
            let msg_ref = message_ref.clone();
            let updated_channel = with_store(&ctx.store, move |s| {
                s.upsert_bot_profile(&ws, &bot_store, &profile_store)?;
                if let Some((ch, ts)) = msg_ref
                    && let Some(mut stored) = s.message(&ws, &ch, &ts)?
                    && stored.message.bot_profile.is_none()
                {
                    stored.message.bot_profile = Some(profile_store);
                    s.upsert_message(&ws, &ch, &stored.message)?;
                    return Ok(Some(ch));
                }
                Ok(None)
            })
            .await;
            match updated_channel {
                Ok(Some(channel)) => {
                    ctx.events.emit(Event::StoreChanged { channel });
                }
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(%bot, %err, "failed to persist bot profile");
                }
            }
            ctx.events.emit(Event::BotProfileLoaded {
                bot_id: bot,
                profile,
            });
            Ok(())
        });
    }

    pub(super) fn set_notify_level(&self, channel: ChannelId, level: NotifyLevel) {
        let ctx = self.ctx();
        self.spawn("users.prefs.setNotifications", async move {
            for &(name, value) in level.writes() {
                ctx.backend
                    .set_notification_pref(channel.clone(), name, value)
                    .await?;
            }
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                let mut prefs = s.prefs(&ws)?.unwrap_or_default();
                crate::notify::set_notify_level(&mut prefs, &channel, level);
                s.replace_prefs(&ws, &prefs)
            })
            .await?;
            ctx.events.emit(Event::SidebarChanged);
            Ok(())
        });
    }

    pub(super) fn set_global_notification_pref(&self, name: String, value: String, sync: bool) {
        let ctx = self.ctx();
        self.spawn("users.prefs.setNotifications(global)", async move {
            ctx.backend
                .set_global_notification_pref(name, value, sync)
                .await?;
            Ok(())
        });
    }

    pub(super) fn leave_conversation(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.leave", async move {
            ctx.backend.leave_conversation(channel.clone()).await?;
            update_stored_conversation(&ctx, channel, |conv| {
                conv.is_member = false;
            })
            .await
        });
    }

    pub(super) fn join_conversation(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.join", async move {
            let mut joined = ctx.backend.join_conversation(channel.clone()).await?;
            joined.is_member = true;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.upsert_conversation(&ws, &joined)).await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn set_channel_topic(&self, channel: ChannelId, topic: String) {
        let ctx = self.ctx();
        self.spawn("conversations.setTopic", async move {
            let updated = ctx.backend.set_topic(channel.clone(), topic).await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &updated)).await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn set_channel_purpose(&self, channel: ChannelId, purpose: String) {
        let ctx = self.ctx();
        self.spawn("conversations.setPurpose", async move {
            let updated = ctx.backend.set_purpose(channel.clone(), purpose).await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &updated)).await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn fetch_channel_managers(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("admin.roles.entity.listAssignments", async move {
            let users = ctx.backend.channel_managers(channel.clone()).await?;
            ctx.events.emit(Event::ChannelManagers { channel, users });
            Ok(())
        });
    }

    pub(super) fn open_dm(&self, users: Vec<UserId>) {
        let ctx = self.ctx();
        self.spawn("conversations.open", async move {
            let dm = ctx.backend.open_dm(users).await?;
            let channel = dm.id.clone();
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.upsert_conversation(&ws, &dm)).await?;
            ctx.events.emit(Event::Conversations(vec![channel.clone()]));
            ctx.events.emit(Event::ConversationOpened(channel));
            Ok(())
        });
    }

    pub(super) fn create_section(&self, name: String) {
        let ctx = self.ctx();
        self.spawn("users.channelSections.create", async move {
            ctx.backend.create_section(name).await?;
            refresh_sections(&ctx).await
        });
    }

    pub(super) fn create_section_with_channel(&self, name: String, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("users.channelSections.create", async move {
            let created = ctx.backend.create_section(name).await?;
            let section = created.channel_section_id;
            // The channel may sit in another section: move, not just add.
            let (probe, read_ws) = (channel.clone(), ctx.workspace.clone());
            let source = with_store(&ctx.store, move |s| {
                s.sections(&read_ws).map(|sections| {
                    sections
                        .iter()
                        .find(|known| known.channel_ids.contains(&probe))
                        .map(|known| known.id.clone())
                })
            })
            .await?;
            ctx.backend
                .move_channel(
                    Some((section.clone(), channel.clone())),
                    source.map(|from| (from, channel.clone())),
                )
                .await?;
            refresh_sections(&ctx).await
        });
    }

    pub(super) fn rename_section(&self, section: String, name: String) {
        let ctx = self.ctx();
        self.spawn("users.channelSections.set", async move {
            // `.set` clears every omitted key, so the emoji is re-read and resent.
            let (read_ws, target) = (ctx.workspace.clone(), section.clone());
            let emoji = with_store(&ctx.store, move |s| {
                s.sections(&read_ws).map(|sections| {
                    sections
                        .iter()
                        .find(|known| known.id == target)
                        .map(|known| known.emoji.clone())
                        .unwrap_or_default()
                })
            })
            .await?;
            ctx.backend.rename_section(section, name, emoji).await?;
            refresh_sections(&ctx).await
        });
    }

    pub(super) fn delete_section(&self, section: String) {
        let ctx = self.ctx();
        self.spawn("users.channelSections.delete", async move {
            ctx.backend.delete_section(section).await?;
            refresh_sections(&ctx).await
        });
    }

    pub(super) fn move_channel(
        &self,
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
    ) {
        let ctx = self.ctx();
        self.spawn("users.channelSections.channels.bulkUpdate", async move {
            if from != to {
                ctx.backend
                    .move_channel(
                        to.map(|to| (to, channel.clone())),
                        from.map(|from| (from, channel.clone())),
                    )
                    .await?;
            }
            refresh_sections(&ctx).await
        });
    }

    pub(super) fn create_channel(&self, name: String, is_private: bool, purpose: Option<String>) {
        let ctx = self.ctx();
        self.spawn("conversations.create", async move {
            let mut created = ctx.backend.create_channel(name, is_private).await?;
            let id = created.id.clone();
            if let Some(purpose) = purpose.filter(|p| !p.trim().is_empty())
                && let Ok(updated) = ctx.backend.set_purpose(id.clone(), purpose).await
            {
                created = updated;
            }
            created.is_member = true;
            let for_store = created.clone();
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.upsert_conversation(&ws, &for_store)).await?;
            ctx.events.emit(Event::Conversations(vec![id.clone()]));
            ctx.events.emit(Event::ConversationOpened(id));
            Ok(())
        });
    }

    pub(super) fn rename_channel(&self, channel: ChannelId, name: String) {
        let ctx = self.ctx();
        self.spawn("conversations.rename", async move {
            let updated = ctx.backend.rename_channel(channel.clone(), name).await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &updated)).await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn archive_channel(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.archive", async move {
            ctx.backend.archive_channel(channel.clone()).await?;
            update_stored_conversation(&ctx, channel, |conv| {
                conv.is_archived = true;
            })
            .await
        });
    }

    pub(super) fn unarchive_channel(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.unarchive", async move {
            ctx.backend.unarchive_channel(channel.clone()).await?;
            update_stored_conversation(&ctx, channel, |conv| {
                conv.is_archived = false;
            })
            .await
        });
    }

    pub(super) fn delete_channel(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("admin.conversations.delete", async move {
            ctx.backend.delete_channel(channel.clone()).await?;
            update_stored_conversation(&ctx, channel, |conv| {
                conv.is_archived = true;
                conv.is_member = false;
            })
            .await
        });
    }

    pub(super) fn convert_channel_to_private(&self, channel: ChannelId) {
        let ctx = self.ctx();
        self.spawn("conversations.convertToPrivate", async move {
            let updated = ctx
                .backend
                .convert_channel_to_private(channel.clone())
                .await?;
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &updated)).await?;
            ctx.events.emit(Event::Conversations(vec![channel]));
            ctx.events.emit(Event::SidebarChanged);
            Ok(())
        });
    }

    pub(super) fn invite_to_channel(&self, channel: ChannelId, users: Vec<UserId>) {
        if users.is_empty() {
            return;
        }
        let ctx = self.ctx();
        self.spawn("conversations.invite", async move {
            let updated = ctx
                .backend
                .invite_to_channel(channel.clone(), users)
                .await?;
            let ws_merge = ctx.workspace.clone();
            with_store(&ctx.store, move |s| {
                s.merge_conversation(&ws_merge, &updated)
            })
            .await?;
            if let Ok(members) = ctx.backend.conversation_members(channel.clone()).await {
                let for_counts = members.clone();
                let ws_counts = ctx.workspace.clone();
                let counts = with_store(&ctx.store, move |s| {
                    s.member_counts(&ws_counts, &for_counts)
                })
                .await
                .ok();
                ctx.events.emit(Event::ConversationInfo {
                    channel: channel.clone(),
                    num_members: counts.map(|c| c.humans),
                    num_apps: counts.map(|c| c.apps),
                    members,
                });
            }
            ctx.events.emit(Event::Conversations(vec![channel]));
            Ok(())
        });
    }

    pub(super) fn add_conversation_tab(
        &self,
        channel: ChannelId,
        tab_type: String,
        label: Option<String>,
    ) {
        let ctx = self.ctx();
        self.spawn("conversations.addTab", async move {
            ctx.backend
                .add_conversation_tab(channel.clone(), tab_type, label)
                .await?;
            refresh_conversation_if_ok(&ctx, channel).await
        });
    }

    pub(super) fn remove_conversation_tab(&self, channel: ChannelId, tab_id: String) {
        let ctx = self.ctx();
        self.spawn("conversations.removeTab", async move {
            ctx.backend
                .remove_conversation_tab(channel.clone(), tab_id)
                .await?;
            refresh_conversation_if_ok(&ctx, channel).await
        });
    }

    pub(super) fn move_conversation_tab(
        &self,
        channel: ChannelId,
        tab_id: String,
        after: Option<String>,
    ) {
        let ctx = self.ctx();
        self.spawn("conversations.moveTab", async move {
            ctx.backend
                .move_conversation_tab(channel.clone(), tab_id, after)
                .await?;
            refresh_conversation_if_ok(&ctx, channel).await
        });
    }

    pub(super) fn set_dnd_snooze(&self, minutes: u32) {
        let ctx = self.ctx();
        self.spawn("dnd.setSnooze", async move {
            let status = ctx.backend.set_dnd_snooze(minutes).await?;
            ctx.events.emit(Event::SelfDndUpdated(status));
            Ok(())
        });
    }

    pub(super) fn end_dnd_snooze(&self) {
        let ctx = self.ctx();
        self.spawn("dnd.endSnooze", async move {
            let status = ctx.backend.end_dnd_snooze().await?;
            ctx.events.emit(Event::SelfDndUpdated(status));
            Ok(())
        });
    }

    pub(super) fn update_profile(&self, update: ProfileUpdate) {
        let ctx = self.ctx();
        let self_user_id = Arc::clone(&self.self_user_id);
        self.spawn("users.profile.set", async move {
            let profile = ctx.backend.update_profile(update).await?;
            let me = self_user_id
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(me) = me {
                let me_for_store = me.clone();
                let ws = ctx.workspace.clone();
                if let Err(err) = with_store(&ctx.store, move |s| {
                    if let Some(mut user) = s.user(&ws, &me_for_store)? {
                        user.profile = profile;
                        s.upsert_user(&ws, &user)?;
                    }
                    Ok(())
                })
                .await
                {
                    tracing::warn!(%err, "failed to persist updated self profile");
                }
                ctx.events.emit(Event::UsersChanged(vec![me]));
            }
            Ok(())
        });
    }

    pub(super) fn set_presence(&self, away: bool) {
        let ctx = self.ctx();
        let self_user_id = Arc::clone(&self.self_user_id);
        self.spawn("presence.set", async move {
            ctx.backend.set_presence(away).await?;
            let me = self_user_id
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            if let Some(me) = me {
                ctx.events.emit(Event::Presence {
                    users: vec![me],
                    presence: if away {
                        crate::types::Presence::Away
                    } else {
                        crate::types::Presence::Active
                    },
                });
            }
            Ok(())
        });
    }

    pub(super) fn set_user_pref(&self, name: String, value: String) {
        let ctx = self.ctx();
        self.spawn("users.prefs.set", async move {
            ctx.backend.set_user_pref(name, value).await?;
            Ok(())
        });
    }

    pub(super) fn fetch_user_profile(&self, user: UserId) {
        let ctx = self.ctx();
        self.spawn("users.profile.get", async move {
            let (profile, field_defs) = tokio::join!(
                ctx.backend.user_profile(user.clone()),
                ctx.backend.team_profile_field_defs(),
            );
            let profile = profile?;
            let field_defs = match field_defs {
                Ok(defs) => defs,
                Err(e) if e.is_auth_expired() => return Err(e.into()),
                Err(e) => {
                    tracing::debug!(%e, "team.profile.get failed");
                    Vec::new()
                }
            };
            let labels: HashMap<String, String> = field_defs
                .iter()
                .filter(|f| !f.label.is_empty())
                .map(|f| (f.id.clone(), f.label.clone()))
                .collect();
            let mut locked_fields: Vec<(String, String)> = Vec::new();
            for def in &field_defs {
                if def.is_locked {
                    let reason = def
                        .lock_reason
                        .clone()
                        .unwrap_or_else(|| "Locked by workspace admin".to_owned());
                    if let Some(name) = def.field_name.as_deref() {
                        locked_fields.push((name.to_ascii_lowercase(), reason.clone()));
                    }
                    if !def.label.is_empty() {
                        locked_fields.push((def.label.clone(), reason.clone()));
                    }
                    locked_fields.push((def.id.clone(), reason));
                }
            }
            let pronouns = profile.pronouns.clone().filter(|p| !p.trim().is_empty());
            let mut fields = Vec::new();
            if let Some(map) = profile.fields.as_ref().and_then(|v| v.as_object()) {
                for (id, entry) in map {
                    let label = entry
                        .get("label")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| labels.get(id).cloned())
                        .unwrap_or_else(|| id.clone());
                    let entry_locked = entry
                        .get("is_locked")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false)
                        || entry
                            .get("is_protected")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                    if entry_locked {
                        let reason = "Locked by workspace admin".to_owned();
                        if !locked_fields.iter().any(|(k, _)| k == &label || k == id) {
                            locked_fields.push((label.clone(), reason.clone()));
                            locked_fields.push((id.clone(), reason));
                        }
                    }
                    let val = entry
                        .get("value")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or_default();
                    if val.is_empty() {
                        continue;
                    }
                    fields.push((label, val.to_owned()));
                }
            }
            ctx.events.emit(Event::UserProfileLoaded {
                user,
                pronouns,
                fields,
                locked_fields,
            });
            Ok(())
        });
    }

    pub(super) fn set_user_photo(&self, filename: String, bytes: Vec<u8>) {
        let ctx = self.ctx();
        self.spawn("users.setPhoto", async move {
            ctx.backend.users_set_photo(filename, bytes).await?;
            Ok(())
        });
    }

    pub(super) fn create_usergroup(&self, name: String, handle: String, description: String) {
        let ctx = self.ctx();
        self.spawn("usergroups.create", async move {
            let _ = ctx
                .backend
                .create_usergroup(name, handle, description)
                .await?;
            refresh_usergroups(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn update_usergroup(
        &self,
        usergroup: String,
        name: String,
        handle: String,
        description: String,
    ) {
        let ctx = self.ctx();
        self.spawn("usergroups.update", async move {
            let _ = ctx
                .backend
                .update_usergroup(usergroup, name, handle, description)
                .await?;
            refresh_usergroups(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn disable_usergroup(&self, usergroup: crate::types::UsergroupId) {
        let ctx = self.ctx();
        self.spawn("usergroups.disable", async move {
            let _ = ctx.backend.disable_usergroup(usergroup).await?;
            refresh_usergroups(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn enable_usergroup(&self, usergroup: crate::types::UsergroupId) {
        let ctx = self.ctx();
        self.spawn("usergroups.enable", async move {
            let _ = ctx.backend.enable_usergroup(usergroup).await?;
            refresh_usergroups(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn load_usergroup_users(&self, usergroup: crate::types::UsergroupId) {
        let ctx = self.ctx();
        self.spawn("usergroups.users.list", async move {
            let users = ctx.backend.usergroup_users(usergroup.clone()).await?;
            ctx.events
                .emit(Event::UsergroupUsersLoaded { usergroup, users });
            Ok(())
        });
    }

    pub(super) fn update_usergroup_users(
        &self,
        usergroup: crate::types::UsergroupId,
        users: Vec<UserId>,
    ) {
        let ctx = self.ctx();
        self.spawn("usergroups.users.update", async move {
            let _ = ctx
                .backend
                .update_usergroup_users(usergroup.clone(), users.clone())
                .await?;
            ctx.events
                .emit(Event::UsergroupUsersLoaded { usergroup, users });
            refresh_usergroups(&ctx.backend, &ctx.events).await
        });
    }

    pub(super) fn sign_out(&self) {
        let ctx = self.ctx();
        self.spawn("auth.revoke", async move {
            if let Err(err) = ctx.backend.auth_revoke().await {
                tracing::debug!(%err, "auth.revoke failed during sign out");
            }
            Ok(())
        });
    }

    pub(super) fn backfill_channel(&mut self, channel: ChannelId, include_threads: bool) {
        if let Some(prev) = self.backfills.remove(&channel) {
            prev.abort();
        }
        // Clear any stale paused state when explicitly starting a single-channel backfill.
        if self.backfill.control.is_paused() {
            let _ = self.backfill.control.resume();
        }
        let backfill = self.backfill.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let ch_lookup = channel.clone();
        let ch_run = channel.clone();
        if let Some(abort) = self.tasks.spawn(async move {
            let name = with_store(&store, move |s| {
                Ok(s.conversation_summary(&ws, &ch_lookup)?
                    .and_then(|c| c.name))
            })
            .await
            .ok()
            .flatten();
            backfill
                .run_with_options(ch_run, name, include_threads)
                .await;
        }) {
            self.backfills.insert(channel, abort);
        }
    }

    pub(super) fn backfill_workspace(&mut self, include_threads: bool) {
        if self
            .workspace_backfill
            .as_ref()
            .is_some_and(|h| !h.is_finished())
        {
            if self.backfill.control.is_paused() {
                let snap = self.backfill.control.resume();
                self.events.emit(Event::BackfillProgress(snap));
            }
            return;
        }
        // Clear any stale paused state when explicitly starting a new workspace backfill.
        if self.backfill.control.is_paused() {
            let _ = self.backfill.control.resume();
        }
        let backfill = self.backfill.clone();
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        self.workspace_backfill = self.tasks.spawn(async move {
            let channels = with_store(&store, move |s| {
                let mut convs = s.conversations(&ws)?;
                convs.retain(|c| c.is_member || c.is_im || c.is_mpim);
                convs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                Ok(convs
                    .into_iter()
                    .map(|c| (c.id, c.name))
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
            backfill.run_workspace(channels, include_threads).await;
        });
    }

    pub(super) fn pause_backfill(&mut self) {
        let snap = self.backfill.control.pause();
        self.events.emit(Event::BackfillProgress(snap));
    }

    pub(super) fn resume_backfill(&mut self) {
        let snap = self.backfill.control.resume();
        self.events.emit(Event::BackfillProgress(snap));
    }

    pub(super) fn query_backfill_progress(&self) {
        let store = Arc::clone(&self.store);
        let ws = self.workspace.clone();
        let control = self.backfill.control.clone();
        let synced = Arc::clone(&self.backfill.synced);
        let events = self.events.clone();
        self.tasks.spawn(async move {
            let (channels, msg_counts) = with_store(&store, move |s| {
                let mut convs = s.conversations(&ws)?;
                convs.retain(|c| c.is_member || c.is_im || c.is_mpim);
                convs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
                let list: Vec<(ChannelId, Option<String>)> =
                    convs.into_iter().map(|c| (c.id, c.name)).collect();
                let counts = s.channel_message_counts(&ws)?;
                Ok((list, counts))
            })
            .await
            .unwrap_or_default();
            let spans = synced
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let snap = control.sync_inventory(&channels, &spans, &msg_counts, false);
            events.emit(Event::BackfillProgress(snap));
        });
    }
}

async fn refresh_usergroups<B: Backend>(backend: &B, events: &Emitter) -> Result<(), CoreError> {
    match backend.usergroups().await {
        Ok(groups) => {
            events.emit(Event::Usergroups(groups));
            Ok(())
        }
        Err(e) if e.is_auth_expired() => Err(e.into()),
        Err(e) => {
            tracing::warn!(%e, "usergroups.list refresh failed");
            Ok(())
        }
    }
}

async fn update_stored_conversation<B: Backend>(
    ctx: &TaskCtx<B>,
    channel: ChannelId,
    mutate: impl FnOnce(&mut slackless_types::Conversation) + Send + 'static,
) -> Result<(), CoreError> {
    let id = channel.clone();
    let ws = ctx.workspace.clone();
    with_store(&ctx.store, move |s| {
        if let Some(mut conv) = s.conversation(&ws, &id)? {
            mutate(&mut conv);
            s.upsert_conversation(&ws, &conv)?;
        }
        Ok(())
    })
    .await?;
    ctx.events.emit(Event::Conversations(vec![channel]));
    Ok(())
}

async fn refresh_sections<B: Backend>(ctx: &TaskCtx<B>) -> Result<(), CoreError> {
    let sections = ctx.backend.channel_sections().await?;
    let ws = ctx.workspace.clone();
    with_store(&ctx.store, move |s| s.replace_sections(&ws, &sections)).await?;
    ctx.events.emit(Event::SidebarChanged);
    Ok(())
}

async fn refresh_conversation_if_ok<B: Backend>(
    ctx: &TaskCtx<B>,
    channel: ChannelId,
) -> Result<(), CoreError> {
    match ctx.backend.conversation_info(channel.clone()).await {
        Ok(fresh) => {
            let ws = ctx.workspace.clone();
            with_store(&ctx.store, move |s| s.merge_conversation(&ws, &fresh)).await?;
        }
        Err(e) if e.is_auth_expired() => return Err(e.into()),
        Err(e) => tracing::debug!(%channel, %e, "post-tab-edit conversations.info failed"),
    }
    ctx.events.emit(Event::Conversations(vec![channel]));
    Ok(())
}
