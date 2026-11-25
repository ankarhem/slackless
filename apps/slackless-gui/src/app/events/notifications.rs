//! Typing indicators, incoming message notifications, notification rules, and sounds.

use super::super::*;

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
        self.play_notification_sound_kind(is_direct_or_mention, cx);
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
            let settings = self.preferences.read(cx).settings().clone();
            let allow_mark =
                settings.mark_read != 2 && (!settings.no_bg_mark_read || self.window_active);
            let viewing = *channel == self.channel && self.transcript_visible(cx);
            let entry =
                self.ws
                    .counts
                    .entry(channel.clone())
                    .or_insert_with(|| ConversationCounts {
                        id: channel.clone(),
                        ..Default::default()
                    });
            if entry.latest.as_ref().is_none_or(|l| message.ts > *l) {
                entry.latest = Some(message.ts.clone());
            }
            if from_self {
                if entry.last_read.as_ref().is_none_or(|r| message.ts > *r) {
                    entry.last_read = Some(message.ts.clone());
                }
            } else if !viewing || !allow_mark {
                let is_dm = self.fixtures.conversation(channel).is_some_and(|c| {
                    matches!(c.kind(), ConversationKind::Im | ConversationKind::Mpim)
                });
                let is_im = self
                    .fixtures
                    .conversation(channel)
                    .is_some_and(|c| c.kind() == ConversationKind::Im);
                let mentioned =
                    mentions_me(&message.text, &self.fixtures.self_user, &settings.keywords);
                entry.has_unreads = true;
                if is_dm || mentioned {
                    entry.mention_count = entry.mention_count.saturating_add(1);
                }
                if is_im {
                    entry.dm_count = entry.dm_count.saturating_add(1);
                }
                if self
                    .ws
                    .marked_read
                    .get(channel)
                    .is_some_and(|marked| message.ts > *marked)
                {
                    self.ws.marked_read.remove(channel);
                }
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.mark_unread(channel, cx));
                self.sync_counts_to_views(cx);
            }
        }
        if !self.should_notify(channel, message, window, cx) {
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
            self.fixtures.plain_text(message)
        } else {
            "New message".to_owned()
        };
        let (title, body) = match conversation.kind() {
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
        let is_dm_or_mention = matches!(
            conversation.kind(),
            ConversationKind::Im | ConversationKind::Mpim
        ) || mentions_me(
            &message.text,
            &self.fixtures.self_user,
            &self.preferences.read(cx).settings().keywords,
        );
        let tag = format!("msg:{channel}:{}", message.ts);
        self.system_notification_kind(&tag, &title, &body, is_dm_or_mention, cx);
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

    /// Slack's desktop notification rules for a message just posted: never your own, while
    /// notifications are paused or outside the notification schedule, or for the
    /// conversation (or thread) on screen in the focused window; never in a muted
    /// conversation; otherwise by the conversation's level (its bell menu choice, else
    /// Preferences > Notifications): every message, or direct messages, mentions and
    /// keywords. A thread reply that is neither a DM nor mentions you notifies only under
    /// "replies to threads I'm following", for a thread you started.
    pub(in crate::app) fn should_notify(
        &self,
        channel: &ChannelId,
        message: &Message,
        window: &Window,
        cx: &gpui::App,
    ) -> bool {
        let me = &self.fixtures.self_user;
        let settings = self.preferences.read(cx).settings();
        if !settings.desktop_notifications || message.user.as_ref() == Some(me) {
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
        let breakthrough = sender_is_vip || settings.is_urgent_dm_breakthrough(dm, &message.text);
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
            self.thread_open
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
        if breakthrough {
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

    /// Play the notification sound chosen in Preferences, unless muted or in quiet hours / DND (`F-SHELL-180`).
    pub(in crate::app) fn play_notification_sound_kind(
        &mut self,
        is_direct_or_mention: bool,
        cx: &mut Context<Self>,
    ) {
        let settings = self.preferences.read(cx).settings();
        if settings.mute_sounds
            || self.self_state.paused()
            || self.self_state.focused()
            || !settings.notifications_allowed_now()
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
            Err(e) => eprintln!("slackless: notification sound not loaded ({e})"),
        });
    }
}
