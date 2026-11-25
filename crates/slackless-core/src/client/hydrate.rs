//! Startup hydration, unread prefetching, activity/saved loading, and team resolution.

use super::*;

impl<B: Backend> Core<B> {
    /// Startup hydrate: runs independent Slack API fetches concurrently (`tokio::join!`), then
    /// persists and emits their results in deterministic order (`client.userBoot`, `client.dms`,
    /// `users.conversations`, `users.list`, `client.counts`, `users.channelSections.list`,
    /// `emoji.list`, usergroups, app commands, and one [`Event::SidebarChanged`] at the end).
    /// Every step reports its own failure without aborting the rest. Ephemeral messages left from
    /// a past session are dropped first.
    #[cfg(test)]
    pub async fn hydrate(&self) {
        run_hydrate(
            self.ctx(),
            Arc::clone(&self.self_user_id),
            self.tasks.clone(),
        )
        .await;
    }

    /// Launch [`Self::hydrate`] concurrently on this core's tracked task set so RTM and command
    /// handling start immediately without waiting for full workspace hydration.
    pub(super) fn spawn_hydrate(&self) {
        let ctx = self.ctx();
        let self_user_id = Arc::clone(&self.self_user_id);
        let tasks = self.tasks.clone();
        self.tasks.spawn(async move {
            run_hydrate(ctx, self_user_id, tasks).await;
        });
    }

    /// Later / Saved items (`saved.list` across active, completed, and archived tabs), hydrated
    /// through `messages.list` so the Saved view and message markers stay synced across devices.
    pub(super) fn fetch_saved(&self) {
        spawn_fetch_saved(&self.tasks, self.backend.clone(), self.events.clone());
    }

    /// The Activity feed (`activity.feed`): announces [`Event::ActivityView`] when it lands,
    /// or the same event carrying the error text when it does not. `with_bodies` also
    /// hydrates the rows through `messages.list` (the view wants bodies; the boot fetch
    /// that feeds the badges does not).
    pub(super) fn fetch_activity(&self, with_bodies: bool) {
        spawn_fetch_activity(
            &self.tasks,
            self.backend.clone(),
            self.events.clone(),
            with_bodies,
        );
    }
}

async fn run_hydrate<B: Backend>(
    ctx: TaskCtx<B>,
    self_user_id: Arc<Mutex<Option<UserId>>>,
    tasks: TaskTracker,
) {
    let ws = ctx.workspace.clone();
    if let Err(e) = with_store(&ctx.store, move |s| s.drop_ephemeral(&ws)).await {
        ctx.events.report("store", e);
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
        ctx.backend.user_boot(),
        ctx.backend.dm_list(),
        ctx.backend.channel_memberships(),
        ctx.backend.users(),
        ctx.backend.counts(),
        ctx.backend.channel_sections(),
        ctx.backend.custom_emoji(),
        ctx.backend.usergroups(),
        ctx.backend.app_commands(),
    );
    match boot_res {
        Ok(boot) => {
            let team = boot.team.clone();
            let prefs = boot.prefs.clone();
            let ws = ctx.workspace.clone();
            let stored = with_store(&ctx.store, move |store| {
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
                Ok((ids, boot.self_user.map(|me| me.id)))
            })
            .await;
            match stored {
                Ok((ids, me)) => {
                    ctx.events.emit(Event::Conversations(ids));
                    if let Some(me) = me {
                        *self_user_id.lock().unwrap_or_else(PoisonError::into_inner) =
                            Some(me.clone());
                        ctx.events.emit(Event::UsersChanged(vec![me]));
                    }
                }
                Err(e) => ctx.events.report("hydrate", e),
            }
            if let Some(team) = team {
                let ws = ctx.workspace.clone();
                if let Err(e) = with_store(&ctx.store, move |s| s.upsert_team(&ws, &team)).await {
                    ctx.events.report("hydrate team", e);
                }
            }
            if let Some(prefs) = prefs {
                let ws = ctx.workspace.clone();
                if let Err(e) = with_store(&ctx.store, move |s| s.replace_prefs(&ws, &prefs)).await
                {
                    ctx.events.report("hydrate prefs", e);
                }
            }
        }
        Err(e) => ctx.events.report("client.userBoot", e.into()),
    }
    match dms_res {
        Ok(dms) => {
            let ws = ctx.workspace.clone();
            let stored = with_store(&ctx.store, move |store| {
                let mut ids = Vec::new();
                for conversation in &dms {
                    // `client.userBoot` caps its channel list, so newer group DMs only
                    // ever arrive here; merging keeps boot's richer rows intact.
                    store.merge_conversation(&ws, conversation)?;
                    ids.push(conversation.id.clone());
                }
                Ok(ids)
            })
            .await;
            match stored {
                Ok(ids) if !ids.is_empty() => {
                    ctx.events.emit(Event::Conversations(ids));
                }
                Ok(_) => {}
                Err(e) => ctx.events.report("client.dms", e),
            }
        }
        Err(e) => ctx.events.report("client.dms", e.into()),
    }
    match members_res {
        Ok(members) => {
            let ws = ctx.workspace.clone();
            let ids = members.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
            let flipped =
                with_store(&ctx.store, move |s| s.set_channel_memberships(&ws, &ids)).await;
            match flipped {
                Ok(flipped) if !flipped.is_empty() => {
                    ctx.events.emit(Event::Conversations(flipped));
                }
                Ok(_) => {}
                Err(e) => ctx.events.report("users.conversations", e),
            }
        }
        Err(e) => ctx.events.report("users.conversations", e.into()),
    }
    match users_res {
        Ok(users) => {
            let ws = ctx.workspace.clone();
            let stored = with_store(&ctx.store, move |store| {
                store.upsert_users(&ws, &users)?;
                Ok(users.into_iter().map(|user| user.id).collect())
            })
            .await;
            match stored {
                Ok(ids) => {
                    ctx.events.emit(Event::UsersChanged(ids));
                }
                Err(e) => ctx.events.report("hydrate users", e),
            }
        }
        Err(e) => ctx.events.report("users.list", e.into()),
    }
    match counts_res {
        Ok(counts) => {
            spawn_prefetch_unreads(&tasks, ctx.clone(), &counts);
            ctx.events.emit(Event::Counts(Box::new(counts)));
        }
        Err(e) => ctx.events.report("client.counts", e.into()),
    }
    match sections_res {
        Ok(sections) => {
            let ws = ctx.workspace.clone();
            if let Err(e) =
                with_store(&ctx.store, move |s| s.replace_sections(&ws, &sections)).await
            {
                ctx.events.report("hydrate sections", e);
            }
        }
        Err(e) => ctx.events.report("users.channelSections.list", e.into()),
    }
    match emoji_res {
        Ok(emoji) => {
            let ws = ctx.workspace.clone();
            let emoji: Vec<(String, String)> = emoji.into_iter().collect();
            if let Err(e) =
                with_store(&ctx.store, move |s| s.replace_custom_emoji(&ws, &emoji)).await
            {
                ctx.events.report("hydrate emoji", e);
            }
        }
        Err(e) => ctx.events.report("emoji.list", e.into()),
    }
    match groups_res {
        Ok(groups) => {
            let active: Vec<_> = groups
                .into_iter()
                .filter(|g| !g.is_deleted() && !g.handle.is_empty())
                .collect();
            if !active.is_empty() {
                ctx.events.emit(Event::Usergroups(active));
            }
        }
        Err(e) if e.is_auth_expired() => ctx.events.report("usergroups.list", e.into()),
        Err(e) => tracing::warn!(%e, "hydrate usergroups.list failed"),
    }
    match commands_res {
        Ok(commands) if !commands.is_empty() => {
            ctx.events.emit(Event::AppCommands(commands));
        }
        Ok(_) => {}
        Err(e) if e.is_auth_expired() => ctx.events.report("client.appCommands", e.into()),
        Err(e) => tracing::warn!(%e, "hydrate client.appCommands failed"),
    }
    ctx.events.emit(Event::SidebarChanged);
    spawn_fetch_dm_previews(&tasks, ctx.clone());
    // The Activity badges need the feed's unread state; last, so the boot's own events
    // keep their order. The boot fetch skips the `messages.list` hydration (the view
    // asks for it with bodies when it opens).
    spawn_fetch_activity(&tasks, ctx.backend.clone(), ctx.events.clone(), false);
    spawn_fetch_saved(&tasks, ctx.backend.clone(), ctx.events.clone());
    let me = self_user_id
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(me) = me {
        let (backend, events) = (ctx.backend, ctx.events);
        spawn_tracked(&tasks, &events.clone(), "dnd.info (self)", async move {
            match backend.dnd_info(me).await {
                Ok(status) if status.snooze_enabled => {
                    events.emit(Event::SelfDndUpdated(status));
                    Ok(())
                }
                Ok(_) => Ok(()),
                Err(e) if e.is_auth_expired() => Err(e.into()),
                Err(e) => {
                    tracing::debug!(%e, "hydrate dnd.info (self) failed");
                    Ok(())
                }
            }
        });
    }
}

fn spawn_fetch_saved<B: Backend>(tasks: &TaskTracker, backend: B, events: Emitter) {
    spawn_tracked(tasks, &events.clone(), "saved.list", async move {
        let loaded = load_saved_items(&backend, &events).await;
        answer_load(&events, loaded, Event::SavedLoaded)
    });
}

fn spawn_fetch_activity<B: Backend>(
    tasks: &TaskTracker,
    backend: B,
    events: Emitter,
    with_bodies: bool,
) {
    spawn_tracked(tasks, &events.clone(), "activity.feed", async move {
        match backend.activity_feed().await {
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

fn spawn_prefetch_unreads<B: Backend>(
    tasks: &TaskTracker,
    ctx: TaskCtx<B>,
    counts: &crate::api::ClientCounts,
) {
    let targets = select_prefetch_targets(counts);
    if targets.is_empty() {
        return;
    }
    let events = ctx.events.clone();
    spawn_tracked(tasks, &events, "unread prefetch", async move {
        prefetch_targets(ctx, targets).await
    });
}

fn spawn_fetch_dm_previews<B: Backend>(tasks: &TaskTracker, ctx: TaskCtx<B>) {
    let events = ctx.events.clone();
    spawn_tracked(tasks, &events, "dm previews", async move {
        let ws2 = ctx.workspace.clone();
        let conversations = with_store(&ctx.store, move |s| s.conversations(&ws2)).await?;
        let dms: Vec<ChannelId> = conversations
            .iter()
            .filter(|c| {
                matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim) && c.is_open
            })
            .map(|c| c.id.clone())
            .collect();
        let ws3 = ctx.workspace.clone();
        let asked = dms.clone();
        let filled =
            with_store(&ctx.store, move |s| s.channels_with_messages(&ws3, &asked)).await?;
        let mut first = true;
        for channel in dms.iter().filter(|id| !filled.contains(*id)) {
            if !std::mem::take(&mut first) {
                tokio::time::sleep(PAGE_DELAY).await;
            }
            let message =
                wait_out_rate_limits(|| ctx.backend.latest_message(channel.clone())).await?;
            let Some(message) = message else {
                continue;
            };
            ctx.unfurls.resolve(std::slice::from_ref(&message));
            let (ws4, target) = (ctx.workspace.clone(), (*channel).clone());
            with_store(&ctx.store, move |s| {
                s.upsert_message(&ws4, &target, &message)
            })
            .await?;
            ctx.events.emit(Event::StoreChanged {
                channel: (*channel).clone(),
            });
        }
        Ok(())
    });
}

/// Select up to 25 unread conversations to prefetch, prioritizing DMs, then mentions, then
/// newest activity.
pub(super) fn select_prefetch_targets(
    counts: &crate::api::ClientCounts,
) -> Vec<(ChannelId, Option<Ts>)> {
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
    // Prioritize DMs and mentions first, then newest activity first.
    targets.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| b.1.cmp(&a.1))
    });
    targets.truncate(MAX_PREFETCH);
    targets
        .into_iter()
        .map(|(id, latest, _, _)| (id, latest))
        .collect()
}

/// Prefetch recent history for unread `targets` whose local SQLite archive is behind `latest`.
pub(super) async fn prefetch_targets<B: Backend>(
    ctx: TaskCtx<B>,
    targets: Vec<(ChannelId, Option<Ts>)>,
) -> Result<(), CoreError> {
    let mut fetched = 0usize;
    for (channel, latest) in targets {
        // Skip channels whose local archive already contains `latest`.
        if let Some(ref latest_ts) = latest
            && !latest_ts.as_str().is_empty()
        {
            let (ws_check, ch_check) = (ctx.workspace.clone(), channel.clone());
            let local_newest = with_store(&ctx.store, move |s| {
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
        if fetched >= 3 {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        }
        let items = match wait_out_rate_limits(|| {
            ctx.backend
                .recent_history(channel.clone(), crate::backend::UNREADS_HISTORY_LIMIT)
        })
        .await
        {
            Ok(items) => items,
            Err(e) if e.is_auth_expired() => return Err(e.into()),
            Err(e) => {
                tracing::debug!(%channel, %e, "unread prefetch skipped channel");
                continue;
            }
        };
        fetched += 1;
        if items.is_empty() {
            continue;
        }
        ctx.unfurls.resolve(items.iter());
        let (ws_put, target) = (ctx.workspace.clone(), channel.clone());
        with_store(&ctx.store, move |s| {
            s.upsert_messages(&ws_put, &target, &items)
        })
        .await?;
        ctx.events.emit(Event::StoreChanged { channel });
    }
    Ok(())
}

/// Shared O(N) `messages.list` batch hydrator used by both Activity feed and Later/Saved items.
pub(super) async fn hydrate_messages_batch<B: Backend>(
    backend: &B,
    events: &Emitter,
    missing: impl IntoIterator<Item = (ChannelId, Ts)>,
) -> HashMap<(ChannelId, Ts), Message> {
    let mut grouped: HashMap<ChannelId, Vec<Ts>> = HashMap::new();
    for (channel, ts) in missing {
        grouped.entry(channel).or_default().push(ts);
    }
    if grouped.is_empty() {
        return HashMap::new();
    }
    let by_channel: Vec<(ChannelId, Vec<Ts>)> = grouped.into_iter().collect();
    match backend.messages_list(by_channel).await {
        Ok(bodies) => {
            let mut lookup = HashMap::new();
            for (channel, messages) in bodies {
                for msg in messages {
                    lookup.insert((channel.clone(), msg.ts.clone()), msg);
                }
            }
            lookup
        }
        Err(e) => {
            events.report("messages.list", e.into());
            HashMap::new()
        }
    }
}

/// Fills each Activity row's body through `messages.list`, one batched call per
/// [`api`] channel cap.
pub(super) async fn hydrate_activity<B: Backend>(
    backend: &B,
    events: &Emitter,
    feed: &mut crate::api::ActivityFeed,
) {
    let missing = feed
        .items
        .iter()
        .filter(|item| item.message.is_none())
        .map(|item| (item.channel.clone(), item.ts.clone()))
        .collect::<Vec<_>>();
    let lookup = hydrate_messages_batch(backend, events, missing).await;
    if lookup.is_empty() {
        return;
    }
    for item in &mut feed.items {
        if item.message.is_none() {
            item.message = lookup
                .get(&(item.channel.clone(), item.ts.clone()))
                .cloned();
        }
    }
}

/// Fetch saved items across Slack's Later tabs (`saved`, `completed`, `archived`), deduplicate
/// by `(channel, ts)`, and hydrate their message bodies through `messages.list`.
pub(super) async fn load_saved_items<B: Backend>(
    backend: &B,
    events: &Emitter,
) -> Result<Vec<crate::api::SavedItem>, ApiError> {
    let mut items = backend.saved_list("saved").await?;
    let mut seen: HashSet<(ChannelId, Ts)> = items
        .iter()
        .map(|item| (item.channel.clone(), item.ts.clone()))
        .collect();
    for filter in ["completed", "archived"] {
        match backend.saved_list(filter).await {
            Ok(extra) => {
                for item in extra {
                    if seen.insert((item.channel.clone(), item.ts.clone())) {
                        items.push(item);
                    }
                }
            }
            Err(e) if e.is_auth_expired() => return Err(e),
            Err(e) => tracing::debug!(filter, %e, "saved.list secondary filter failed"),
        }
    }
    let missing = items
        .iter()
        .filter(|item| item.message.is_none())
        .map(|item| (item.channel.clone(), item.ts.clone()))
        .collect::<Vec<_>>();
    let lookup = hydrate_messages_batch(backend, events, missing).await;
    if !lookup.is_empty() {
        for item in &mut items {
            if item.message.is_none() {
                item.message = lookup
                    .get(&(item.channel.clone(), item.ts.clone()))
                    .cloned();
            }
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
/// asked once per session through `asked` (found or not).
pub(super) async fn resolve_teams<B: Backend>(
    ctx: TaskCtx<B>,
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
        match ctx.backend.team_info(id).await {
            Ok(team) => teams.push(team),
            Err(e) => ctx.events.report("team.info", e.into()),
        }
    }
    if teams.is_empty() {
        return;
    }
    let ws = ctx.workspace.clone();
    let outcome = with_store(&ctx.store, move |s| {
        teams.iter().try_for_each(|team| s.upsert_team(&ws, team))
    })
    .await;
    match outcome {
        Ok(()) => {
            ctx.events.emit(Event::SidebarChanged);
        }
        Err(e) => ctx.events.report("team.info store", e),
    }
}
