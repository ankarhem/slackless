//! Live `CoreBridge` event folding, store reads, notifications, typing, and badges.

mod core;
mod huddles;
mod notifications;
mod store;
mod workspace;

use super::*;

/// What a conversation is that a live `message` opens, which neither snapshot nor store holds
/// yet: what the event names (`channel_type`), else what its id does (`D…`, a DM).
fn event_kind(channel: &ChannelId, message: &Message) -> Option<ConversationKind> {
    let named = match (message.extra.get("channel_type")).and_then(serde_json::Value::as_str) {
        Some("im") => Some(ConversationKind::Im),
        Some("mpim") => Some(ConversationKind::Mpim),
        Some("channel") => Some(ConversationKind::PublicChannel),
        Some("group") => Some(ConversationKind::PrivateChannel),
        _ => None,
    };
    named.or_else(|| {
        channel
            .as_str()
            .starts_with('D')
            .then_some(ConversationKind::Im)
    })
}

/// How someone else's unread message moves its conversation's counts, `kind` being what the
/// conversation is: a DM counts as a mention.
fn unread_arrival(kind: Option<ConversationKind>, mentioned: bool) -> Arrival {
    Arrival::Unread {
        mention: mentioned || matches!(kind, Some(ConversationKind::Im | ConversationKind::Mpim)),
        im: kind == Some(ConversationKind::Im),
    }
}

impl App {
    /// Rebuild the menu bar when its moving parts change: the workspaces (File > Workspace,
    /// Sign Out) and the History menu's places, labelled as Slack's.
    pub(super) fn update_menus(&mut self, cx: &mut Context<Self>) {
        /// Slack's History menu lists this many places.
        const PLACES: usize = 15;
        let update_label: SharedString = match &self.update_state {
            crate::updater::UpdateState::Checking => "Checking for Updates\u{2026}".into(),
            crate::updater::UpdateState::Downloading(ver) => {
                format!("Downloading Update ({ver})\u{2026}").into()
            }
            crate::updater::UpdateState::Available(rel) => {
                format!("Download Update ({})\u{2026}", rel.version).into()
            }
            _ => match &self.available_update {
                Some(rel) => format!("Download Update ({})\u{2026}", rel.version).into(),
                None => "Check for Updates\u{2026}".into(),
            },
        };
        let bar = MenuBar {
            workspaces: (self.fixtures.workspaces.iter())
                .map(|team| team.name.clone().into())
                .collect(),
            recent: (self.recent_entries().into_iter().take(PLACES))
                .map(|(id, label, icon)| {
                    let label = match icon {
                        IconName::Hash => format!("#{label}").into(),
                        IconName::Lock => format!("#{label} (Private)").into(),
                        _ => label,
                    };
                    (label, id)
                })
                .collect(),
            update_label,
        };
        if self.menu_bar.as_ref() != Some(&bar) {
            menu_bar::set(&bar, cx);
            self.menu_bar = Some(bar);
        }
    }

    /// Put the shown workspace's Later items on its message views' saved markers.
    pub(super) fn sync_saved(&mut self, cx: &mut Context<Self>) {
        let saved: HashSet<Ts> = self
            .later
            .read(cx)
            .saved_in(self.workspace)
            .into_iter()
            .map(|(_, ts)| ts)
            .collect();
        self.messages
            .update(cx, |list, cx| list.replace_saved(saved.clone(), cx));
        self.thread
            .update(cx, |thread, cx| thread.replace_saved(saved.clone(), cx));
        self.threads
            .update(cx, |threads, cx| threads.replace_saved(saved.clone(), cx));
        self.unreads
            .update(cx, |unreads, cx| unreads.replace_saved(saved, cx));
    }

    pub(super) fn sync_later_count(&mut self, cx: &mut Context<Self>) {
        let count = self.later.read(cx).in_progress_count();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_later_count(count, cx));
    }

    /// A message view saved, unsaved or set a reminder on a message of the shown workspace.
    pub(super) fn save_for_later(&mut self, request: &SaveRequest, cx: &mut Context<Self>) {
        let label = self.label(&request.channel);
        let workspace = self.workspace;
        self.later.update(cx, |later, cx| {
            later.save(request.clone(), workspace, label, cx)
        });
        if request.saved {
            self.bridge.send(MessagingCommand::SaveItem {
                channel: request.channel.clone(),
                ts: request.ts.clone(),
                date_due: request.due,
            });
        } else {
            self.bridge.send(MessagingCommand::DeleteSavedItem {
                channel: request.channel.clone(),
                ts: request.ts.clone(),
            });
        }
    }

    /// Map live `saved.list` rows into [`LaterItem`]s using `fixtures` (falling back to the
    /// workspace's channel/user names when a message body was not returned by `messages.list`).
    pub(super) fn apply_saved_loaded(
        &mut self,
        workspace: usize,
        items: &[slackless_core::api::SavedItem],
        cx: &mut Context<Self>,
    ) {
        let Some(fixtures) = self.workspace_fixtures(workspace) else {
            return;
        };
        let mapped: Vec<crate::later::LaterItem> = items
            .iter()
            .map(|item| {
                let label = fixtures
                    .conversation(&item.channel)
                    .map(|c| fixtures.conversation_label(c))
                    .unwrap_or_else(|| item.channel.to_string());
                let (author, text) = match &item.message {
                    Some(msg) => {
                        let author = msg
                            .user
                            .as_ref()
                            .map(|u| fixtures.user_name(u).to_owned())
                            .or_else(|| msg.username.clone())
                            .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()))
                            .unwrap_or_else(|| "Message".to_owned());
                        (author, fixtures.preview(msg))
                    }
                    None => ("Saved message".to_owned(), String::new()),
                };
                let state = match item.state {
                    slackless_core::api::SavedItemState::InProgress => {
                        crate::later::LaterState::InProgress
                    }
                    slackless_core::api::SavedItemState::Archived => {
                        crate::later::LaterState::Archived
                    }
                    slackless_core::api::SavedItemState::Completed => {
                        crate::later::LaterState::Completed
                    }
                };
                crate::later::LaterItem {
                    workspace,
                    channel: item.channel.clone(),
                    ts: item.ts.clone(),
                    label,
                    author,
                    text,
                    state,
                    due: item.date_due,
                    fired: false,
                    is_todo: false,
                }
            })
            .collect();
        self.later
            .update(cx, |later, cx| later.replace_live(workspace, mapped, cx));
        if workspace == self.workspace {
            self.sync_saved(cx);
        }
    }

    /// Post every Later reminder that came due: system notification plus in-app toast.
    pub(super) fn fire_reminders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let due = self
            .later
            .update(cx, |later, cx| later.take_due(now_secs(), cx));
        for item in due {
            let title = format!("Reminder: {} in {}", item.author, item.label);
            let tag = format!("later:{}:{}", item.channel, item.ts);
            self.post_notification(&tag, &title, &item.text, window, cx);
        }
    }

    /// Remember the window's frame once it has settled after a move or resize, so the next
    /// launch reopens it there, like Slack (live mode; a no-op offline or in full screen).
    pub(super) fn save_window_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const SETTLE: Duration = Duration::from_millis(500);
        self.window_state_save = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update_in(cx, |app, window, cx| {
                let write = window
                    .native_window_state()
                    .and_then(|state| app.bridge.remember(&WINDOW_STATE, &state));
                if let Some(write) = write {
                    cx.background_executor().spawn(write).detach();
                }
            });
        });
    }

    /// Compute Slack's window title (Window menu, Mission Control):
    /// "<conversation> (Channel|DM) - <workspace> - N new items - <app>".
    fn compute_window_title(&self) -> String {
        let mut parts = Vec::with_capacity(4);
        if let Some(conversation) = self.fixtures.conversation(&self.channel) {
            let kind = match conversation.kind() {
                ConversationKind::Im | ConversationKind::Mpim => "DM",
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel => "Channel",
            };
            let label = self.fixtures.conversation_label(conversation);
            let name = label.strip_prefix("# ").unwrap_or(&label);
            parts.push(format!("{name} ({kind})"));
        }
        let team = self.team();
        if !team.name.is_empty() {
            parts.push(team.name.clone());
        }
        match self.badge {
            Some(1) => parts.push("1 new item".to_owned()),
            Some(n) if n > 1 => parts.push(format!("{n} new items")),
            _ => {}
        }
        parts.push("slackless".to_owned());
        parts.join(" - ")
    }

    /// Update the OS window title immediately when a `&mut Window` is already in scope.
    pub(super) fn sync_window_title_in(&mut self, window: &mut Window) {
        let title = self.compute_window_title();
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }

    /// Update the OS window title from state-change paths without a `&mut Window` (defers the
    /// window handle update until after the current entity update finishes, avoiding re-entrancy).
    pub(super) fn sync_window_title(&mut self, cx: &mut Context<Self>) {
        let title = self.compute_window_title();
        if title != self.window_title {
            self.window_title = title.clone();
            let handle = self.window_handle;
            cx.defer(move |cx| {
                let _ = handle.update(cx, |_, window, _| {
                    window.set_window_title(&title);
                });
            });
        }
    }

    /// The rail badge of the workspace in slot `ix`: the shown sidebar's; else its live counts;
    /// else (sample) its parked sidebar's.
    pub(super) fn workspace_badge(&self, ix: usize, cx: &gpui::App) -> WorkspaceBadge {
        let sidebar_badge = |sidebar: &Entity<Sidebar>| {
            let sidebar = sidebar.read(cx);
            WorkspaceBadge {
                mentions: sidebar.badge_count(),
                has_unreads: sidebar.has_any_unreads(),
            }
        };
        if ix == self.workspace {
            return sidebar_badge(&self.sidebar);
        }
        let Some(slot) = self.slots.get(ix) else {
            return WorkspaceBadge::default();
        };
        if !slot.state.counts.is_empty() {
            return workspace_badge_from_counts(&slot.state.counts, slot.activity_unread);
        }
        (slot.parked.as_ref())
            .map_or_else(WorkspaceBadge::default, |p| sidebar_badge(&p.views.sidebar))
    }

    /// Hand the total unread count (across every workspace) to the dock badge, and push
    /// per-workspace badges to the workspace switcher rail (`F-CORE-006`).
    pub(super) fn sync_badge(&mut self, cx: &mut Context<Self>) {
        let mut badges = Vec::with_capacity(self.rail.len());
        let mut status_workspaces = Vec::with_capacity(self.rail.len());
        let mut total_dock: u32 = 0;
        let mut any_unreads = false;
        for &ix in &self.rail {
            let badge = self.workspace_badge(ix, cx);
            total_dock = total_dock.saturating_add(badge.mentions);
            any_unreads |= badge.has_unreads;
            let name = self.slots[ix].team.name.clone();
            status_workspaces.push((name, badge.mentions, badge.has_unreads));
            badges.push(badge);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_workspace_badges(badges, cx));
        let (show_dock_badge, badge_on_unreads, show_menu_bar_icon) = {
            let settings = self.preferences.read(cx).settings();
            (
                settings.show_dock_badge,
                settings.badge_on_unreads,
                settings.show_menu_bar_icon,
            )
        };
        let prev_badge = self.badge;
        self.badge = Some(if show_dock_badge { total_dock } else { 0 });
        if self.badge != prev_badge {
            self.sync_window_title(cx);
        }
        let next_label = if !show_dock_badge {
            None
        } else if total_dock > 0 {
            Some(total_dock.to_string())
        } else if badge_on_unreads && any_unreads {
            Some("\u{2022}".to_owned())
        } else {
            None
        };
        if self.badge_label.as_ref() != Some(&next_label) {
            self.badge_label = Some(next_label.clone());
            self.os.set_badge(next_label.as_deref());
        }
        let status_bar_state = StatusBarState {
            visible: show_menu_bar_icon,
            unread_mentions: total_dock,
            has_unreads: any_unreads,
            dnd_active: self.self_state.paused(),
            workspaces: status_workspaces,
        };
        if self.last_status_bar.as_ref() != Some(&status_bar_state) {
            self.last_status_bar = Some(status_bar_state.clone());
            self.os
                .update_status_bar(Some(include_bytes!("../../../icon.png")), &status_bar_state);
        }
    }
}
