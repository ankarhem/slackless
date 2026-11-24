//! Startup hydration, unread prefetching, activity/saved loading, and team resolution.

use super::*;

impl<B: Backend> Core<B> {
    /// Startup hydrate: runs independent Slack API fetches concurrently (`tokio::join!`), then
    /// persists and emits their results in deterministic order (`client.userBoot`, `client.dms`,
    /// `users.conversations`, `users.list`, `client.counts`, `users.channelSections.list`,
    /// `emoji.list`, usergroups, app commands, and one [`Event::SidebarChanged`] at the end).
    /// Every step reports its own failure without aborting the rest. Ephemeral messages left from
    /// a past session are dropped first.
    pub async fn hydrate(&self) {
        let ws = self.workspace.clone();
        if let Err(e) = with_store(&self.store, move |s| s.drop_ephemeral(&ws)).await {
            self.events.report("store", e.into());
        }
        let (
            boot_res,
            dms_res,
            members_res,
            users_res,
            counts_res,
            sections_res,
            emoji_res,
            groups_res,
            commands_res,
        ) = tokio::join!(
            self.backend.user_boot(),
            self.backend.dm_list(),
            self.backend.channel_memberships(),
            self.backend.users(),
            self.backend.counts(),
            self.backend.channel_sections(),
            self.backend.custom_emoji(),
            self.backend.usergroups(),
            self.backend.app_commands(),
        );
        match boot_res {
            Ok(boot) => {
                let team = boot.team.clone();
                let prefs = boot.prefs.clone();
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    // `client.userBoot` objects omit `is_member`, so a row the membership
                    // sync already marked must keep its flag: the front-end rebuilds its
                    // snapshot on this step's event, before `users.conversations` runs.
                    let mut convs: Vec<_> = boot.channels.into_iter().chain(boot.ims).collect();
                    let ids: Vec<ChannelId> = convs.iter().map(|c| c.id.clone()).collect();
                    let member = store.member_flags(&ws, &ids)?;
                    for conversation in &mut convs {
                        conversation.is_member |= member.contains(&conversation.id);
                    }
                    store.upsert_conversations(&ws, &convs)?;
                    if let Some(me) = &boot.self_user {
                        store.upsert_user(&ws, me)?;
                    }
                    Ok::<_, StoreError>((ids, boot.self_user.map(|me| me.id)))
                })
                .await;
                match stored {
                    Ok((ids, me)) => {
                        self.events.emit(Event::Conversations(ids));
                        if let Some(me) = me {
                            if let Ok(mut guard) = self.self_user_id.lock() {
                                *guard = Some(me.clone());
                            }
                            self.events.emit(Event::UsersChanged(vec![me]));
                        }
                    }
                    Err(e) => self.events.report("hydrate", e.into()),
                }
                if let Some(team) = team {
                    let ws = self.workspace.clone();
                    let stored = with_store(&self.store, move |s| s.upsert_team(&ws, &team)).await;
                    if let Err(e) = stored {
                        self.events.report("hydrate team", e.into());
                    }
                }
                if let Some(prefs) = prefs {
                    let ws = self.workspace.clone();
                    let stored =
                        with_store(&self.store, move |s| s.replace_prefs(&ws, &prefs)).await;
                    if let Err(e) = stored {
                        self.events.report("hydrate prefs", e.into());
                    }
                }
            }
            Err(e) => self.events.report("client.userBoot", e.into()),
        }
        match dms_res {
            Ok(dms) => {
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    let mut ids = Vec::new();
                    for conversation in &dms {
                        // `client.userBoot` caps its channel list, so newer group DMs only
                        // ever arrive here; merging keeps boot's richer rows intact.
                        store.merge_conversation(&ws, conversation)?;
                        ids.push(conversation.id.clone());
                    }
                    Ok::<_, StoreError>(ids)
                })
                .await;
                match stored {
                    Ok(ids) if !ids.is_empty() => {
                        self.events.emit(Event::Conversations(ids));
                    }
                    Ok(_) => {}
                    Err(e) => self.events.report("client.dms", e.into()),
                }
            }
            Err(e) => self.events.report("client.dms", e.into()),
        }
        match members_res {
            Ok(members) => {
                let ws = self.workspace.clone();
                let ids = members.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
                let flipped =
                    with_store(&self.store, move |s| s.set_channel_memberships(&ws, &ids)).await;
                match flipped {
                    Ok(flipped) if !flipped.is_empty() => {
                        self.events.emit(Event::Conversations(flipped));
                    }
                    Ok(_) => {}
                    Err(e) => self.events.report("users.conversations", e.into()),
                }
            }
            // Slack Connect channels hosted by another org never reach `client.counts`, so
            // the GUI's membership gate needs this row state; failing it leaves the boot
            // flags (usually `false`) in place until the next hydrate.
            Err(e) => self.events.report("users.conversations", e.into()),
        }
        match users_res {
            Ok(users) => {
                let ws = self.workspace.clone();
                let stored = with_store(&self.store, move |store| {
                    store.upsert_users(&ws, &users)?;
                    Ok::<_, StoreError>(users.into_iter().map(|user| user.id).collect())
                })
                .await;
                match stored {
                    Ok(ids) => {
                        self.events.emit(Event::UsersChanged(ids));
                    }
                    Err(e) => self.events.report("hydrate users", e.into()),
                }
            }
            Err(e) => self.events.report("users.list", e.into()),
        }
        match counts_res {
            Ok(counts) => {
                self.prefetch_unreads(&counts);
                self.events.emit(Event::Counts(Box::new(counts)));
            }
            Err(e) => self.events.report("client.counts", e.into()),
        }
        match sections_res {
            Ok(sections) => {
                let ws = self.workspace.clone();
                let stored =
                    with_store(&self.store, move |s| s.replace_sections(&ws, &sections)).await;
                if let Err(e) = stored {
                    self.events.report("hydrate sections", e.into());
                }
            }
            Err(e) => self.events.report("users.channelSections.list", e.into()),
        }
        match emoji_res {
            Ok(emoji) => {
                let ws = self.workspace.clone();
                let emoji: Vec<(String, String)> = emoji.into_iter().collect();
                let stored =
                    with_store(&self.store, move |s| s.replace_custom_emoji(&ws, &emoji)).await;
                if let Err(e) = stored {
                    self.events.report("hydrate emoji", e.into());
                }
            }
            Err(e) => self.events.report("emoji.list", e.into()),
        }
        if let Ok(groups) = groups_res {
            let active: Vec<_> = groups
                .into_iter()
                .filter(|g| !g.is_deleted() && !g.handle.is_empty())
                .collect();
            if !active.is_empty() {
                self.events.emit(Event::Usergroups(active));
            }
        }
        if let Ok(commands) = commands_res {
            if !commands.is_empty() {
                self.events.emit(Event::AppCommands(commands));
            }
        }
        self.events.emit(Event::SidebarChanged);
        self.fetch_dm_previews();
        // The Activity badges need the feed's unread state; last, so the boot's own events
        // keep their order. The boot fetch skips the `messages.list` hydration (the view
        // asks for it with bodies when it opens).
        self.fetch_activity(false);
        self.fetch_saved();
        if let Some(me) = self.self_user_id.lock().ok().and_then(|g| g.clone()) {
            let (backend, events) = (self.backend.clone(), self.events.clone());
            self.spawn("dnd.info (self)", async move {
                if let Ok(status) = backend.dnd_info(me).await {
                    if status.snooze_enabled {
                        events.emit(Event::SelfDndUpdated(status));
                    }
                }
                Ok(())
            });
        }
    }

    /// Later / Saved items (`saved.list` across active, completed, and archived tabs), hydrated
    /// through `messages.list` so the Saved view and message markers stay synced across devices.
    pub(super) fn fetch_saved(&self) {
        let (backend, events) = (self.backend.clone(), self.events.clone());
        self.spawn("saved.list", async move {
            let loaded = load_saved_items(&backend, &events).await;
            answer_load(&events, loaded, Event::SavedLoaded)
        });
    }

    /// The Activity feed (`activity.feed`): announces [`Event::ActivityView`] when it lands,
    /// or the same event carrying the error text when it does not. `with_bodies` also
    /// hydrates the rows through `messages.list` (the view wants bodies; the boot fetch
    /// that feeds the badges does not).
    pub(super) fn fetch_activity(&self, with_bodies: bool) {
        let (backend, events) = (self.backend.clone(), self.events.clone());
        self.spawn("activity.feed", async move {
            let fetched = backend.activity_feed().await;
            match fetched {
                Ok(mut feed) => {
                    if with_bodies {
                        hydrate_activity(&backend, &events, &mut feed).await;
                    }
                    events.emit(Event::ActivityView {
                        feed: Ok(Box::new(feed)),
                    });
                    Ok(())
                }
                Err(e) => {
                    events.emit(Event::ActivityView {
                        feed: Err(e.to_string()),
                    });
                    Err(e.into())
                }
            }
        });
    }

    /// Background-prefetch the newest messages of unread conversations reported by
    /// `client.counts` whose local SQLite cache is behind `latest`, so clicking an unread channel
    /// or DM renders the new messages on frame 0 without waiting for an HTTP round-trip.
    pub(super) fn prefetch_unreads(&self, counts: &crate::api::ClientCounts) {
        /// Cap per hydrate/reconnect pass so workspaces with hundreds of muted unread channels
        /// stay well within Slack's Tier-3 rate limits.
        const MAX_PREFETCH: usize = 25;

        let mut targets: Vec<(ChannelId, Option<Ts>, u32, bool)> = Vec::new();
        for entry in counts.ims.iter().chain(&counts.mpims) {
            if entry.is_unread() {
                targets.push((
                    entry.id.clone(),
                    entry.latest.clone(),
                    entry.effective_mentions(),
                    true,
                ));
            }
        }
        for entry in &counts.channels {
            if entry.is_unread() {
                targets.push((
                    entry.id.clone(),
                    entry.latest.clone(),
                    entry.effective_mentions(),
                    false,
                ));
            }
        }
        if targets.is_empty() {
            return;
        }
        // Prioritize DMs and mentions first, then newest activity first.
        targets.sort_by(|a, b| {
            b.3.cmp(&a.3)
                .then_with(|| b.2.cmp(&a.2))
                .then_with(|| b.1.cmp(&a.1))
        });
        targets.truncate(MAX_PREFETCH);

        let (backend, store, ws, events, unfurls) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            self.unfurls.clone(),
        );
        self.spawn("unread prefetch", async move {
            let mut fetched = 0usize;
            for (channel, latest, _, _) in targets {
                // Skip channels whose local archive already contains `latest`.
                if let Some(ref latest_ts) = latest {
                    if !latest_ts.as_str().is_empty() {
                        let (ws_check, ch_check) = (ws.clone(), channel.clone());
                        let local_newest = with_store(&store, move |s| {
                            s.channel_history(&ws_check, &ch_check, None, 1)
                                .map(|rows| rows.into_iter().last().map(|r| r.message.ts))
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
                if fetched >= 3 {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                let items = match wait_out_rate_limits(|| {
                    backend.recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
                })
                .await
                {
                    Ok(items) => items,
                    Err(_) => continue,
                };
                fetched += 1;
                if items.is_empty() {
                    continue;
                }
                unfurls.resolve(items.iter());
                let (ws_put, target) = (ws.clone(), channel.clone());
                with_store(&store, move |s| s.upsert_messages(&ws_put, &target, &items)).await?;
                events.emit(Event::StoreChanged { channel });
            }
            Ok(())
        });
    }

    /// DM-list previews: every IM/MPIM that is open (listed in Slack's DMs tab) and has
    /// nothing stored at all gets its newest message (one `conversations.history` limit-1
    /// call each, sequential and paced at the Tier-3 rate) so the DM list previews and sorts
    /// conversations never opened in slackless, as Slack does. Closed conversations stay
    /// unfetched. A fetched preview only adds one message: it touches neither the backfill's
    /// synced spans nor any completeness state, so opening the conversation still walks its
    /// full history from the newest page.
    pub(super) fn fetch_dm_previews(&self) {
        let (backend, store, ws, events, unfurls) = (
            self.backend.clone(),
            Arc::clone(&self.store),
            self.workspace.clone(),
            self.events.clone(),
            self.unfurls.clone(),
        );
        self.spawn("dm previews", async move {
            let ws2 = ws.clone();
            let conversations = with_store(&store, move |s| s.conversations(&ws2)).await?;
            let dms: Vec<ChannelId> = conversations
                .iter()
                .filter(|c| {
                    matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim) && c.is_open
                })
                .map(|c| c.id.clone())
                .collect();
            let ws3 = ws.clone();
            let asked = dms.clone();
            let filled =
                with_store(&store, move |s| s.channels_with_messages(&ws3, &asked)).await?;
            let mut first = true;
            for channel in dms.iter().filter(|id| !filled.contains(*id)) {
                if !std::mem::take(&mut first) {
                    tokio::time::sleep(PAGE_DELAY).await;
                }
                let message =
                    wait_out_rate_limits(|| backend.latest_message(channel.clone())).await?;
                let Some(message) = message else {
                    continue;
                };
                unfurls.resolve(std::slice::from_ref(&message));
                let (ws4, target) = (ws.clone(), (*channel).clone());
                with_store(&store, move |s| s.upsert_message(&ws4, &target, &message)).await?;
                events.emit(Event::StoreChanged {
                    channel: (*channel).clone(),
                });
            }
            Ok(())
        });
    }
}

/// Fills each Activity row's body through `messages.list`, one batched call per
/// [`api`] channel cap. Rows whose channel stays silent (or the whole call, when it fails)
/// keep no body: the view renders them without text and the failure is reported.
pub(super) async fn hydrate_activity<B: Backend>(
    backend: &B,
    events: &Emitter,
    feed: &mut crate::api::ActivityFeed,
) {
    let mut by_channel: Vec<(ChannelId, Vec<Ts>)> = Vec::new();
    for item in &feed.items {
        if item.message.is_none() {
            match by_channel
                .iter_mut()
                .find(|(channel, _)| *channel == item.channel)
            {
                Some((_, stamps)) => stamps.push(item.ts.clone()),
                None => by_channel.push((item.channel.clone(), vec![item.ts.clone()])),
            }
        }
    }
    if by_channel.is_empty() {
        return;
    }
    match backend.messages_list(by_channel).await {
        Ok(bodies) => {
            for item in &mut feed.items {
                if item.message.is_none() {
                    item.message = bodies
                        .get(&item.channel)
                        .and_then(|messages| messages.iter().find(|message| message.ts == item.ts))
                        .cloned();
                }
            }
        }
        Err(e) => events.report("messages.list", e.into()),
    }
}

/// Fetch saved items across Slack's Later tabs (`saved`, `completed`, `archived`), deduplicate
/// by `(channel, ts)`, and hydrate their message bodies through `messages.list`.
pub(super) async fn load_saved_items<B: Backend>(
    backend: &B,
    events: &Emitter,
) -> Result<Vec<crate::api::SavedItem>, ApiError> {
    let mut items = backend.saved_list("saved").await?;
    for filter in ["completed", "archived"] {
        if let Ok(extra) = backend.saved_list(filter).await {
            for item in extra {
                if !items
                    .iter()
                    .any(|existing| existing.channel == item.channel && existing.ts == item.ts)
                {
                    items.push(item);
                }
            }
        }
    }
    let mut by_channel: Vec<(ChannelId, Vec<Ts>)> = Vec::new();
    for item in &items {
        if item.message.is_none() {
            match by_channel
                .iter_mut()
                .find(|(channel, _)| *channel == item.channel)
            {
                Some((_, stamps)) => stamps.push(item.ts.clone()),
                None => by_channel.push((item.channel.clone(), vec![item.ts.clone()])),
            }
        }
    }
    if !by_channel.is_empty() {
        match backend.messages_list(by_channel).await {
            Ok(bodies) => {
                for item in &mut items {
                    if item.message.is_none() {
                        item.message = bodies
                            .get(&item.channel)
                            .and_then(|messages| messages.iter().find(|m| m.ts == item.ts))
                            .cloned();
                    }
                }
            }
            Err(e) => events.report("messages.list", e.into()),
        }
    }
    Ok(items)
}

/// Answers a lazy tab load with its data, or with the error text the tab shows instead of
/// loading forever; the failure is also returned for the usual report.
pub(super) fn answer_load<T>(
    events: &Emitter,
    result: Result<T, ApiError>,
    answer: impl FnOnce(Result<T, String>) -> Event,
) -> Result<(), CoreError> {
    match result {
        Ok(data) => {
            events.emit(answer(Ok(data)));
            Ok(())
        }
        Err(e) => {
            events.emit(answer(Err(e.to_string())));
            Err(e.into())
        }
    }
}

/// `team.info` for the connected organizations of shared channels (`team_ids`), each id
/// asked once per session through `asked` (found or not). Records are stored beside the
/// workspace's own team and announced with [`Event::SidebarChanged`], whose reload re-reads
/// them; a failed lookup is reported and the rest still land, so one bad org never blocks
/// the others.
pub(super) async fn resolve_teams(
    backend: impl Backend,
    store: SharedStore,
    workspace: TeamId,
    events: Emitter,
    asked: Arc<Mutex<HashSet<TeamId>>>,
    team_ids: Vec<TeamId>,
) {
    let fresh: Vec<TeamId> = {
        let mut asked = asked.lock().unwrap_or_else(PoisonError::into_inner);
        team_ids
            .into_iter()
            .filter(|id| asked.insert(id.clone()))
            .collect()
    };
    if fresh.is_empty() {
        return;
    }
    let mut teams = Vec::new();
    for id in fresh {
        match backend.team_info(id).await {
            Ok(team) => teams.push(team),
            Err(e) => events.report("team.info", e.into()),
        }
    }
    if teams.is_empty() {
        return;
    }
    let outcome = with_store(&store, move |s| {
        teams
            .iter()
            .try_for_each(|team| s.upsert_team(&workspace, team))
    })
    .await;
    match outcome {
        Ok(()) => {
            events.emit(Event::SidebarChanged);
        }
        Err(e) => events.report("team.info store", e.into()),
    }
}
