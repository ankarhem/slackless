//! Typing indicators, incoming message notifications, notification rules, and sounds.

use super::super::*;

/// Owned snapshot of [`slackless_core::rules::RuleContext`] for off-thread semantic evaluation.
pub(super) struct OwnedRuleContext {
    pub(super) sender_id: Option<String>,
    pub(super) sender_handle: Option<String>,
    pub(super) sender_display_name: Option<String>,
    pub(super) channel_id: String,
    pub(super) channel_name: Option<String>,
    pub(super) text: String,
    pub(super) is_thread_reply: bool,
    pub(super) mentions_me: bool,
}

impl OwnedRuleContext {
    pub(super) fn as_rule_context(&self) -> slackless_core::rules::RuleContext<'_> {
        slackless_core::rules::RuleContext {
            sender_id: self.sender_id.as_deref(),
            sender_handle: self.sender_handle.as_deref(),
            sender_display_name: self.sender_display_name.as_deref(),
            channel_id: &self.channel_id,
            channel_name: self.channel_name.as_deref(),
            text: &self.text,
            is_thread_reply: self.is_thread_reply,
            mentions_me: self.mentions_me,
        }
    }
}

impl App {
    /// A system notification (a macOS banner) with the notification sound, as Slack's desktop
    /// notifications come.
    pub(in crate::app) fn system_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        cx: &mut Context<Self>,
    ) {
        self.system_notification_kind(tag, title, body, false, cx);
    }

    pub(in crate::app) fn system_notification_kind(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        is_direct_or_mention: bool,
        cx: &mut Context<Self>,
    ) {
        self.system_notification_with_breakthrough(
            tag,
            title,
            body,
            is_direct_or_mention,
            false,
            cx,
        );
    }

    pub(in crate::app) fn system_notification_with_breakthrough(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        is_direct_or_mention: bool,
        breakthrough: bool,
        cx: &mut Context<Self>,
    ) {
        // Duplicate-notification guard (`F-SHELL-181`): ignore identical notification tags
        // arriving within 60 seconds (e.g. reconnect replay or dual socket/poll delivery).
        let now = Instant::now();
        self.recent_notified_tags
            .retain(|(_, at)| now.duration_since(*at) < Duration::from_secs(60));
        if self.recent_notified_tags.iter().any(|(t, _)| t == tag) {
            return;
        }
        self.recent_notified_tags.push((tag.to_owned(), now));
        if self.recent_notified_tags.len() > 128 {
            let drop_n = self.recent_notified_tags.len() - 128;
            self.recent_notified_tags.drain(..drop_n);
        }
        cx.show_system_notification(SystemNotification {
            tag: tag.to_owned().into(),
            title: title.to_owned().into(),
            body: body.to_owned().into(),
            actions: Vec::new(),
        });
        self.os.notify(title, body);
        let bounce = self.preferences.read(cx).settings().bounce_dock;
        if bounce && is_direct_or_mention && !self.window_active {
            self.os.request_attention(false);
        }
        self.play_notification_sound_with_breakthrough(is_direct_or_mention, breakthrough, cx);
    }

    /// Check whether today's opt-in Daily Catch-Up reminder is due and fire it at most once per
    /// local calendar day. Clicking the resulting OS notification opens the Catch-Up view.
    pub(in crate::app) fn check_daily_catchup_reminder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now_utc = slackless_core::types::time::now_epoch_secs();
        let tz_offset = crate::messages::local_offset_now();
        self.check_daily_catchup_reminder_at(now_utc, tz_offset, window, cx);
    }

    pub(in crate::app) fn check_daily_catchup_reminder_at(
        &mut self,
        now_utc_secs: i64,
        tz_offset_secs: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (enabled, time_hhmm, last_fired) = {
            let settings = self.preferences.read(cx).settings();
            (
                settings.catchup_reminder_enabled,
                settings.catchup_reminder_time.clone(),
                settings.catchup_reminder_last_fired_day.clone(),
            )
        };
        let status = slackless_core::rules::check_catchup_reminder(
            enabled,
            &time_hhmm,
            last_fired.as_deref(),
            now_utc_secs,
            tz_offset_secs,
        );
        if !status.due {
            return;
        }
        let today = status.today;
        self.preferences.update(cx, |prefs, cx| {
            prefs.mark_catchup_reminder_fired(today.clone(), cx)
        });
        let tag = format!("catchup:{today}");
        let title = "Daily Catch-Up ready";
        let body = "Click to review unread channels, mentions, and threads across your workspaces.";
        self.post_notification(&tag, title, body, window, cx);
    }

    /// A notification mirrored as an in-app toast (Later reminders).
    pub(in crate::app) fn post_notification(
        &mut self,
        tag: &str,
        title: &str,
        body: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.system_notification(tag, title, body, cx);
        notify(title, body.to_owned(), window, cx);
    }

    /// Someone started or kept typing: show them until [`TYPING_TTL`] after their last
    /// announcement, as Slack does. The user's own typing (from another client) is not shown.
    pub(in crate::app) fn someone_typing(
        &mut self,
        channel: &ChannelId,
        user: &UserId,
        thread_ts: Option<&Ts>,
        cx: &mut Context<Self>,
    ) {
        if *user == self.fixtures.self_user {
            return;
        }
        let typists = self
            .typing
            .entry((channel.clone(), thread_ts.cloned()))
            .or_default();
        typists.retain(|(typist, _)| typist != user);
        typists.push((user.clone(), Instant::now() + TYPING_TTL));
        self.expire_typing(cx);
    }

    /// Drop typists whose announcement ran out, show who is left, and wake again when the
    /// next announcement runs out.
    pub(in crate::app) fn expire_typing(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        for typists in self.typing.values_mut() {
            typists.retain(|(_, until)| *until > now);
        }
        self.typing.retain(|_, typists| !typists.is_empty());
        self.show_typing(cx);
        let next = self
            .typing
            .values()
            .flatten()
            .map(|(_, until)| *until)
            .min();
        self.typing_expiry = match next {
            Some(next) => cx.spawn(async move |this, cx| {
                let wait = next.saturating_duration_since(Instant::now());
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |app, cx| app.expire_typing(cx));
            }),
            None => gpui::Task::ready(()),
        };
    }

    /// Put who is typing under the open conversation's box and the open thread's reply box.
    pub(in crate::app) fn show_typing(&self, cx: &mut Context<Self>) {
        let names = |key: (ChannelId, Option<Ts>)| {
            let names: Vec<SharedString> = (self.typing.get(&key).into_iter().flatten())
                .map(|(user, _)| self.fixtures.user_name(user).to_owned().into())
                .collect();
            TypingNames::of(&names)
        };
        let here = names((self.channel.clone(), None));
        self.composer
            .update(cx, |composer, cx| composer.set_typing(here, cx));
        let open = self.thread.read(cx).open_thread();
        let in_thread = open.and_then(|(channel, ts)| names((channel, Some(ts))));
        self.thread
            .update(cx, |thread, cx| thread.set_typing(in_thread, cx));
    }

    /// A message was just posted: its author stops typing there, and Slack's desktop
    /// notification rules decide whether it gets a banner and the notification sound.
    pub(in crate::app) fn message_posted(
        &mut self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(author) = &message.user {
            let thread = message.thread_ts.clone().filter(|ts| *ts != message.ts);
            if let Some(typists) = self.typing.get_mut(&(channel.clone(), thread)) {
                typists.retain(|(typist, _)| typist != author);
            }
            self.expire_typing(cx);
        }
        let reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let broadcast =
            message.subtype == Some(slackless_core::types::MessageSubtype::ThreadBroadcast);
        let counts_in_channel = !message.hidden && !message.is_ephemeral && (!reply || broadcast);
        if !self.fixtures.sample && counts_in_channel {
            let from_self = message.user.as_ref() == Some(&self.fixtures.self_user);
            let settings = self.preferences.read(cx).settings();
            let allow_mark =
                settings.mark_read != 2 && (!settings.no_bg_mark_read || self.window_active);
            let viewing = *channel == self.channel && self.transcript_visible(cx);
            let arrival = if from_self {
                Arrival::Own
            } else if viewing && allow_mark {
                Arrival::Seen
            } else {
                let me = &self.fixtures.self_user;
                super::unread_arrival(
                    self.conversation_kind(channel, message),
                    mentions_me(&message.text, me, &settings.keywords),
                )
            };
            let unread = matches!(arrival, Arrival::Unread { .. });
            self.ws.fold_message(channel, &message.ts, arrival);
            if unread {
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.mark_unread(channel, cx));
                self.sync_counts_to_views(cx);
            }
        }
        let has_semantic_rule = self
            .preferences
            .read(cx)
            .settings()
            .smart_notification_rules
            .iter()
            .any(|r| {
                r.enabled
                    && r.semantic_topic
                        .as_deref()
                        .is_some_and(|t| !t.trim().is_empty())
            });
        if !has_semantic_rule {
            let rule_eval = self.evaluate_smart_rules(channel, message, cx);
            self.dispatch_message_notification(channel, message, &rule_eval, window, cx);
            return;
        }
        // Off-thread semantic rule evaluation: keep the UI thread completely non-blocking when
        // an enabled rule uses `semantic_topic`.
        let (rules, owned_ctx) = self.build_owned_rule_context(channel, message, cx);
        let channel_owned = channel.clone();
        let message_owned = message.clone();
        let window_handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let rule_eval = cx
                .background_executor()
                .spawn(async move {
                    let ctx = owned_ctx.as_rule_context();
                    let scorer = crate::preferences::compute_local_semantic_similarity;
                    slackless_core::rules::evaluate_rules_with_semantic(&rules, &ctx, Some(&scorer))
                })
                .await;
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = this.update(cx, |app, cx| {
                    app.dispatch_message_notification(
                        &channel_owned,
                        &message_owned,
                        &rule_eval,
                        window,
                        cx,
                    );
                });
            });
        })
        .detach();
    }

    /// What `channel` is to the shown workspace, also when `message` opens it
    /// ([`super::event_kind`]).
    fn conversation_kind(
        &self,
        channel: &ChannelId,
        message: &Message,
    ) -> Option<ConversationKind> {
        (self.fixtures.conversation(channel).map(Conversation::kind))
            .or_else(|| super::event_kind(channel, message))
    }

    fn dispatch_message_notification(
        &mut self,
        channel: &ChannelId,
        message: &Message,
        rule_eval: &slackless_core::rules::RuleEvaluation,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.should_notify_with_eval(channel, message, rule_eval, window, cx) {
            return;
        }
        let Some(conversation) = self.fixtures.conversation(channel) else {
            return;
        };
        let sender = match &message.user {
            Some(user) => self.fixtures.user_name(user).to_owned(),
            None => (message.bot_profile.as_ref())
                .map(|bot| bot.name.clone())
                .unwrap_or_default(),
        };
        let text = if self.preferences.read(cx).settings().show_preview {
            crate::messages::plain_text(&message.text, &*self.fixtures)
        } else {
            "New message".to_owned()
        };
        let (raw_title, body) = match conversation.kind() {
            ConversationKind::Im => (sender, text),
            ConversationKind::Mpim => (
                self.fixtures.conversation_label(conversation),
                format!("{sender}: {text}"),
            ),
            _ => (
                format!("#{}", conversation.name.clone().unwrap_or_default()),
                format!("{sender}: {text}"),
            ),
        };
        let title = if rule_eval.is_priority_highlight() {
            format!("\u{2605} {raw_title}")
        } else {
            raw_title
        };
        let is_dm_or_mention = rule_eval.breaks_through_mute()
            || rule_eval.is_priority_highlight()
            || matches!(
                conversation.kind(),
                ConversationKind::Im | ConversationKind::Mpim
            )
            || mentions_me(
                &message.text,
                &self.fixtures.self_user,
                &self.preferences.read(cx).settings().keywords,
            );
        let tag = format!("msg:{channel}:{}", message.ts);
        self.system_notification_with_breakthrough(
            &tag,
            &title,
            &body,
            is_dm_or_mention,
            rule_eval.breaks_through_mute(),
            cx,
        );
        if self.preferences.read(cx).settings().announce_incoming {
            #[cfg(all(target_os = "macos", not(test)))]
            {
                let announcement: String = format!("{title}. {body}").chars().take(160).collect();
                if let Ok(mut child) = std::process::Command::new("say")
                    .args(["-r", "210", "--", &announcement])
                    .spawn()
                {
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                }
            }
        }
    }

    fn build_owned_rule_context(
        &self,
        channel: &ChannelId,
        message: &Message,
        cx: &gpui::App,
    ) -> (
        Vec<slackless_core::rules::NotificationRule>,
        OwnedRuleContext,
    ) {
        let settings = self.preferences.read(cx).settings();
        let user_obj = message.user.as_ref().and_then(|u| self.fixtures.user(u));
        let sender_id = message
            .user
            .as_ref()
            .map(|u| u.as_str().to_owned())
            .or_else(|| message.bot_id.as_ref().map(|b| b.as_str().to_owned()));
        let sender_handle = user_obj
            .map(|u| u.name.clone())
            .or_else(|| message.username.clone())
            .or_else(|| message.bot_profile.as_ref().map(|b| b.name.clone()));
        let sender_display = user_obj
            .map(|u| u.display_name().to_owned())
            .or_else(|| message.bot_profile.as_ref().map(|b| b.name.clone()));
        let channel_name = self
            .fixtures
            .conversation(channel)
            .and_then(|c| c.name.clone());
        let is_thread_reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let mentioned = mentions_me(&message.text, &self.fixtures.self_user, &settings.keywords);
        (
            settings.smart_notification_rules.clone(),
            OwnedRuleContext {
                sender_id,
                sender_handle,
                sender_display_name: sender_display,
                channel_id: channel.as_str().to_owned(),
                channel_name,
                text: message.text.clone(),
                is_thread_reply,
                mentions_me: mentioned,
            },
        )
    }

    /// Evaluate configured local smart notification rules for one incoming message.
    pub(in crate::app) fn evaluate_smart_rules(
        &self,
        channel: &ChannelId,
        message: &Message,
        cx: &gpui::App,
    ) -> slackless_core::rules::RuleEvaluation {
        let settings = self.preferences.read(cx).settings();
        if settings.smart_notification_rules.is_empty() {
            return slackless_core::rules::RuleEvaluation::default();
        }
        let user_obj = message.user.as_ref().and_then(|u| self.fixtures.user(u));
        let sender_id = message
            .user
            .as_ref()
            .map(UserId::as_str)
            .or_else(|| message.bot_id.as_ref().map(|b| b.as_str()));
        let sender_handle = user_obj
            .map(|u| u.name.as_str())
            .or(message.username.as_deref())
            .or_else(|| message.bot_profile.as_ref().map(|b| b.name.as_str()));
        let sender_display = user_obj
            .map(slackless_core::types::User::display_name)
            .or_else(|| message.bot_profile.as_ref().map(|b| b.name.as_str()));
        let channel_name = self
            .fixtures
            .conversation(channel)
            .and_then(|c| c.name.as_deref());
        let is_thread_reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let mentioned = mentions_me(&message.text, &self.fixtures.self_user, &settings.keywords);
        let ctx = slackless_core::rules::RuleContext {
            sender_id,
            sender_handle,
            sender_display_name: sender_display,
            channel_id: channel.as_str(),
            channel_name,
            text: &message.text,
            is_thread_reply,
            mentions_me: mentioned,
        };
        slackless_core::rules::evaluate_rules(&settings.smart_notification_rules, &ctx)
    }

    /// Slack's desktop notification rules for a message just posted, layered with local smart
    /// notification rules (`BreakThroughMute` > `Suppress` > `PriorityHighlight`).
    #[cfg(test)]
    pub(in crate::app) fn should_notify(
        &self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        let eval = self.evaluate_smart_rules(channel, message, cx);
        self.should_notify_with_eval(channel, message, &eval, window, cx)
    }

    fn should_notify_with_eval(
        &self,
        channel: &ChannelId,
        message: &Message,
        rule_eval: &slackless_core::rules::RuleEvaluation,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        let me = &self.fixtures.self_user;
        let settings = self.preferences.read(cx).settings();
        if !settings.desktop_notifications || message.user.as_ref() == Some(me) {
            return false;
        }
        if rule_eval.is_suppressed() {
            return false;
        }
        let dm = self
            .fixtures
            .conversation(channel)
            .is_some_and(|conversation| {
                matches!(
                    conversation.kind(),
                    ConversationKind::Im | ConversationKind::Mpim
                )
            });
        let sender_is_vip = message.user.as_ref().is_some_and(|u| {
            let user_obj = self.fixtures.user(u);
            let handle = user_obj.map(|x| x.name.as_str()).unwrap_or("");
            let display = user_obj.map(|x| x.display_name()).unwrap_or("");
            settings.is_vip(u.as_str(), handle, display)
        });
        let breakthrough = sender_is_vip
            || settings.is_urgent_dm_breakthrough(dm, &message.text)
            || rule_eval.breaks_through_mute();
        if !breakthrough
            && (self.self_state.paused()
                || self.self_state.focused()
                || !settings.notifications_allowed_now())
        {
            return false;
        }
        let reply = message
            .thread_ts
            .as_ref()
            .is_some_and(|ts| *ts != message.ts);
        let on_screen = if reply {
            self.right_pane.is_thread()
                && self
                    .thread
                    .read(cx)
                    .open_thread()
                    .is_some_and(|(open, ts)| {
                        open == *channel && message.thread_ts.as_ref() == Some(&ts)
                    })
        } else {
            self.channel == *channel
        };
        if on_screen && window.is_window_active() {
            return false;
        }
        let level = self.fixtures.notify_levels.get(channel).copied();
        if level == Some(NotifyLevel::Muted) && !breakthrough {
            return false;
        }
        if breakthrough || rule_eval.is_priority_highlight() {
            return true;
        }
        let mentioned = mentions_me(&message.text, me, &settings.keywords);
        let wanted = match (level, settings.notify_level) {
            (Some(NotifyLevel::Everything), _) => true,
            (Some(NotifyLevel::Mentions), _) => dm || mentioned,
            (Some(NotifyLevel::Muted), _) => false,
            (None, 0) => true,
            (None, 1) => dm || mentioned,
            (None, _) => false,
        };
        if reply && !dm && !mentioned {
            let channel_followed = settings
                .follow_all_threads_channels
                .iter()
                .any(|id| id == channel.as_str());
            return (wanted
                && settings.thread_replies
                && message.parent_user_id.as_ref() == Some(me))
                || channel_followed;
        }
        wanted
    }

    /// Play the notification sound chosen in Preferences, unless muted or in quiet hours / DND
    /// (`F-SHELL-180`); `breakthrough` skips the focus, pause and schedule checks.
    pub(in crate::app) fn play_notification_sound_with_breakthrough(
        &mut self,
        is_direct_or_mention: bool,
        breakthrough: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = self.preferences.read(cx).settings();
        if settings.mute_sounds {
            return;
        }
        if !breakthrough
            && (self.self_state.paused()
                || self.self_state.focused()
                || !settings.notifications_allowed_now())
        {
            return;
        }
        let choice = if is_direct_or_mention {
            settings.dm_sound
        } else {
            settings.sound
        };
        self.play_sound(choice, cx);
    }

    /// Play Preferences' sound `choice` ("None" plays nothing): fetched from Slack's CDN
    /// through the core the first time, then kept for the next notifications.
    pub(in crate::app) fn play_sound(&mut self, choice: usize, cx: &mut Context<Self>) {
        let Some(url) = crate::preferences::SOUND_FILES
            .get(choice)
            .copied()
            .flatten()
        else {
            return;
        };
        if let Some((cached, data)) = &self.sound
            && *cached == choice
        {
            self.os.play_sound(data);
            return;
        }
        let Some(fetch) = crate::media::fetch(url.to_owned(), cx) else {
            return;
        };
        self.sound_fetch = cx.spawn(async move |this, cx| match fetch.await {
            Ok(data) => {
                let _ = this.update(cx, |app, _| {
                    app.os.play_sound(&data);
                    app.sound = Some((choice, data));
                });
            }
            Err(e) => tracing::warn!(error = %e, "notification sound not loaded"),
        });
    }
}
