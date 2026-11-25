//! Custom channel sections, starring, notification levels, section filters/sorts, and dialogs.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::{
    Context, DismissEvent, Div, Focusable as _, ParentElement as _, SharedString, Styled as _,
    Window, div, px,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{
    ChannelId, ChannelSection, Conversation, ConversationKind, SectionKind,
};

use super::helpers::{SectionFormMode, SectionSearch};
use super::{Sidebar, SidebarEvent, Unread};
use crate::avatars::conversation_avatar;
use crate::sidebar_menu::{
    Hidden, HiddenConversations, MenuRow, SectionFilter, SectionMenuInput, SectionSort,
    more_conversations_menu,
};

impl Sidebar {
    /// The sidebar's sections: the fixture-mode overlay once anything changed locally,
    /// else the snapshot's.
    pub(super) fn section_set(&self) -> Vec<ChannelSection> {
        self.local_sections
            .clone()
            .unwrap_or_else(|| self.fixtures.sections.clone())
    }

    /// List a section created locally (fixture mode) after the stored ones.
    pub fn add_section(&mut self, name: String, cx: &mut Context<Self>) {
        let index = self.section_set().len() + 1;
        let mut sections = self.section_set();
        sections.push(ChannelSection {
            id: format!("LLOCAL{index}"),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: Vec::new(),
        });
        self.local_sections = Some(sections);
        cx.notify();
    }

    /// Fixture mode's row-menu "+ Create new section": create the section and move the
    /// channel into it in one step, as Slack does.
    pub fn add_section_with_channel(
        &mut self,
        name: String,
        channel: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let index = self.section_set().len() + 1;
        let id = format!("LLOCAL{index}");
        let mut sections = self.section_set();
        sections.push(ChannelSection {
            id: id.clone(),
            name,
            kind: SectionKind::Standard,
            emoji: String::new(),
            next_id: None,
            channel_ids: vec![channel.clone()],
        });
        for section in sections.iter_mut() {
            if section.kind == SectionKind::Standard && section.id != id {
                section.channel_ids.retain(|other| *other != channel);
            }
        }
        self.local_sections = Some(sections);
        cx.notify();
    }

    /// Fixture mode's rename of a section (live mode's goes through
    /// [`SidebarEvent::RenameSection`]).
    pub fn rename_section_local(&mut self, section: &str, name: &str, cx: &mut Context<Self>) {
        let mut sections = self.section_set();
        if let Some(target) = sections.iter_mut().find(|known| known.id == section) {
            target.name = name.to_owned();
            self.local_sections = Some(sections);
            cx.notify();
        }
    }

    /// Fixture mode's section delete; its conversations fall back to their default buckets.
    pub fn delete_section_local(&mut self, section: &str, cx: &mut Context<Self>) {
        let mut sections = self.section_set();
        let before = sections.len();
        sections.retain(|known| known.id != section);
        if sections.len() != before {
            self.local_sections = Some(sections);
            cx.notify();
        }
    }

    /// A conversation moved between sections: fixture mode edits the overlay; live mode
    /// asks the root for the `users.channelSections.channels.bulkUpdate` round.
    pub(crate) fn move_channel(
        &mut self,
        channel: ChannelId,
        from: Option<String>,
        to: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if from == to {
            return;
        }
        if self.fixtures.sample {
            let mut sections = self.section_set();
            for section in sections.iter_mut() {
                if section.kind == SectionKind::Standard {
                    section.channel_ids.retain(|id| *id != channel);
                }
            }
            if let Some(target) = to.as_ref().and_then(|to| {
                sections
                    .iter_mut()
                    .find(|section| section.id == *to && section.kind == SectionKind::Standard)
            }) {
                target.channel_ids.push(channel);
            }
            self.local_sections = Some(sections);
            cx.notify();
            return;
        }
        cx.emit(SidebarEvent::MoveChannel { channel, from, to });
    }

    /// Whether `channel` is currently starred (either in the server `Stars` section or in
    /// the local starred set, `F-CORE-019`).
    pub(crate) fn is_starred(&self, channel: &ChannelId) -> bool {
        if self.local_starred.contains(channel) {
            return true;
        }
        self.section_set().iter().any(|section| {
            section.kind == SectionKind::Stars && section.channel_ids.contains(channel)
        })
    }

    /// Star or unstar `channel` (`F-CORE-019`). Uses Slack's `Stars` channel section when
    /// present on the workspace, otherwise toggles the local Starred section at the top of Home.
    pub(crate) fn toggle_star(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        let sections = self.section_set();
        let stars_section = sections
            .iter()
            .find(|section| section.kind == SectionKind::Stars);
        if let Some(stars) = stars_section {
            let currently_in_stars = stars.channel_ids.contains(&channel);
            let stars_id = stars.id.clone();
            if self.fixtures.sample {
                let mut updated = sections;
                if let Some(target) = updated.iter_mut().find(|s| s.id == stars_id) {
                    if currently_in_stars {
                        target.channel_ids.retain(|id| *id != channel);
                    } else {
                        target.channel_ids.push(channel.clone());
                    }
                }
                self.local_sections = Some(updated);
                cx.notify();
            } else if currently_in_stars {
                self.local_starred.remove(&channel);
                cx.emit(SidebarEvent::MoveChannel {
                    channel,
                    from: Some(stars_id),
                    to: None,
                });
                cx.notify();
            } else {
                let from = sections
                    .iter()
                    .find(|s| s.kind == SectionKind::Standard && s.channel_ids.contains(&channel))
                    .map(|s| s.id.clone());
                self.local_starred.insert(channel.clone());
                cx.emit(SidebarEvent::MoveChannel {
                    channel,
                    from,
                    to: Some(stars_id),
                });
                cx.notify();
            }
            return;
        }
        if !self.local_starred.remove(&channel) {
            self.local_starred.insert(channel);
        }
        cx.notify();
    }

    /// One notification-level choice: fixture mode keeps it locally (the snapshot carries
    /// live ones); live mode asks the root for the `Command::SetNotifyLevel` path.
    pub(crate) fn set_notify(
        &mut self,
        channel: ChannelId,
        level: NotifyLevel,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            self.notify_override.insert(channel, level);
            cx.notify();
            return;
        }
        cx.emit(SidebarEvent::SetNotifyLevel { channel, level });
    }

    /// A conversation's effective notification level: this session's fixture override, or
    /// the snapshot's.
    pub(super) fn notify_level(&self, channel: &ChannelId) -> Option<NotifyLevel> {
        self.notify_override
            .get(channel)
            .or_else(|| self.fixtures.notify_levels.get(channel))
            .copied()
    }

    /// "Mute all conversations": one choice per unmuted conversation of the section.
    pub(crate) fn mute_section(&mut self, channels: &[ChannelId], cx: &mut Context<Self>) {
        for channel in channels {
            if self.notify_level(channel) != Some(NotifyLevel::Muted) {
                self.set_notify(channel.clone(), NotifyLevel::Muted, cx);
            }
        }
    }

    /// "Mark all as read": every conversation with unread messages in the section, through
    /// the root's existing read path (and, in fixture mode, the local read marks the
    /// snapshot cannot carry).
    pub(crate) fn mark_section_read(&mut self, channels: &[ChannelId], cx: &mut Context<Self>) {
        let watermark = self.read_watermark();
        for channel in channels {
            let Some(conversation) = self
                .fixtures
                .channels
                .iter()
                .chain(&self.fixtures.dms)
                .find(|conversation| conversation.id == *channel)
            else {
                continue;
            };
            if self.unread_in(conversation, watermark.as_ref()).count == 0 {
                continue;
            }
            let latest =
                self.fixtures
                    .counts
                    .get(channel)
                    .and_then(|counts| counts.latest.clone())
                    .or_else(|| {
                        self.fixtures.messages.get(channel).and_then(|history| {
                            history.iter().map(|message| message.ts.clone()).max()
                        })
                    })
                    .or_else(|| {
                        self.fixtures
                            .latest
                            .get(channel)
                            .map(|message| message.ts.clone())
                    });
            if let Some(latest) = latest {
                self.read.insert(channel.clone());
                cx.emit(SidebarEvent::MarkRead {
                    channel: channel.clone(),
                    ts: latest,
                });
            }
        }
        cx.notify();
    }

    /// The section's effective filter: its own choice, or the workspace's "Active only"
    /// default (`All` while the gear menu's toggle is off).
    pub(super) fn section_filter(&self, key: &SharedString) -> SectionFilter {
        self.section_filters.get(key).copied().unwrap_or({
            if self.active_only {
                SectionFilter::ActiveOnly
            } else {
                SectionFilter::All
            }
        })
    }

    pub(crate) fn set_section_filter(
        &mut self,
        key: SharedString,
        filter: SectionFilter,
        cx: &mut Context<Self>,
    ) {
        self.section_filters.insert(key, filter);
        cx.emit(SidebarEvent::SectionPrefsChanged);
        cx.notify();
    }

    /// The section's effective sort: its own choice, or the built-in default (the DMs
    /// bucket by activity, everything else by name).
    pub(super) fn section_sort(&self, key: &SharedString, dm: bool) -> SectionSort {
        self.section_sorts.get(key).copied().unwrap_or(if dm {
            SectionSort::Recency
        } else {
            SectionSort::Alpha
        })
    }

    pub(crate) fn set_section_sort(
        &mut self,
        key: SharedString,
        sort: SectionSort,
        cx: &mut Context<Self>,
    ) {
        self.section_sorts.insert(key, sort);
        cx.emit(SidebarEvent::SectionPrefsChanged);
        cx.notify();
    }

    /// Snapshot per-section filter & sort choices for SQLite `local_state` persistence.
    pub(crate) fn export_section_prefs(&self) -> crate::bridge::SectionPrefs {
        crate::bridge::SectionPrefs {
            filters: self
                .section_filters
                .iter()
                .map(|(k, &v)| (k.to_string(), v))
                .collect(),
            sorts: self
                .section_sorts
                .iter()
                .map(|(k, &v)| (k.to_string(), v))
                .collect(),
        }
    }

    /// Restore per-section filter & sort choices loaded from SQLite `local_state`.
    pub(crate) fn restore_section_prefs(
        &mut self,
        prefs: crate::bridge::SectionPrefs,
        cx: &mut Context<Self>,
    ) {
        self.section_filters = prefs
            .filters
            .into_iter()
            .map(|(k, v)| (SharedString::from(k), v))
            .collect();
        self.section_sorts = prefs
            .sorts
            .into_iter()
            .map(|(k, v)| (SharedString::from(k), v))
            .collect();
        cx.notify();
    }

    /// "Search in this section": the find field filters the list, so it takes the focus.
    pub(crate) fn focus_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find
            .update(cx, |state, cx| state.focus_handle(cx).focus(window, cx));
    }

    /// The row menu's "Channel details": the root owns the dialog.
    pub(crate) fn open_details(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::OpenChannelDetails(channel));
    }

    /// The row menu's "Leave channel": the root's existing leave path.
    pub(crate) fn leave(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::LeaveChannel(channel));
    }

    pub(crate) fn open_in_new_window(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        cx.emit(SidebarEvent::OpenInNewWindow(channel));
    }

    /// The rows a section's menus and popover list: this menu's labels and glyphs.
    pub(crate) fn menu_rows(&self, ids: &[ChannelId]) -> Vec<MenuRow> {
        let watermark = self.read_watermark();
        ids.iter()
            .filter_map(|id| {
                let conversation = self.fixtures.conversation(id)?;
                let label = self.fixtures.conversation_label(conversation).to_owned();
                Some(MenuRow {
                    id: id.clone(),
                    label,
                    private: matches!(conversation.kind(), ConversationKind::PrivateChannel),
                    is_dm: matches!(
                        conversation.kind(),
                        ConversationKind::Im | ConversationKind::Mpim
                    ),
                    unread: self.unread_in(conversation, watermark.as_ref()).count > 0,
                })
            })
            .collect()
    }

    /// A DM menu row's avatar, cut out of the menu's surface.
    pub(crate) fn menu_avatar(&self, id: &ChannelId, cx: &gpui_kit::gpui::App) -> Option<Div> {
        let conversation = self.fixtures.conversation(id)?;
        Some(conversation_avatar(
            &self.fixtures,
            conversation,
            20.,
            cx.theme().popover,
            cx,
        ))
    }

    /// One section menu's whole input, gathered as the menu opens (never per frame): the
    /// section's channels, the ones its rows leave out, and the mark-read / mute targets.
    pub(super) fn section_menu_input(
        &self,
        key: SharedString,
        section: Option<ChannelSection>,
        title: String,
        ids: &[ChannelId],
    ) -> SectionMenuInput {
        let watermark = self.read_watermark();
        let (mut unread, mut unmuted) = (Vec::new(), Vec::new());
        for id in ids {
            let Some(conversation) = self.fixtures.conversation(id) else {
                continue;
            };
            if self.unread_in(conversation, watermark.as_ref()).count > 0 {
                unread.push(id.clone());
            }
            if self.notify_level(id) != Some(NotifyLevel::Muted) {
                unmuted.push(id.clone());
            }
        }
        let dm = section
            .as_ref()
            .map(|section| section.kind == SectionKind::DirectMessages)
            .unwrap_or_else(|| key.as_ref() == "dms");
        let (filter, sort) = (self.section_filter(&key), self.section_sort(&key, dm));
        let hidden = self.hidden_conversations(&key, ids);
        SectionMenuInput {
            key,
            title,
            section,
            hidden,
            unread,
            unmuted,
            filter,
            sort,
        }
    }

    /// The conversations of `ids` (the section keyed `key`) its rows leave out, grouped by
    /// why ([`Self::hidden_reason`]).
    pub(super) fn hidden_conversations(
        &self,
        key: &SharedString,
        ids: &[ChannelId],
    ) -> HiddenConversations {
        let (open, filter) = (!self.collapsed.contains(key), self.section_filter(key));
        let watermark = self.read_watermark();
        let mut hidden = HiddenConversations::default();
        for conversation in ids.iter().filter_map(|id| self.fixtures.conversation(id)) {
            let unread = self.unread_in(conversation, watermark.as_ref());
            if let Some(why) = self.hidden_reason(conversation, unread, filter, open) {
                hidden.push(why, conversation.id.clone());
            }
        }
        hidden
    }

    /// Open `key`'s "More conversations" popover over the section as it is now: "Mark all
    /// as read" while anything in it is unread, then what its rows leave out.
    pub(super) fn open_more_conversations(
        &mut self,
        key: &SharedString,
        ids: &[ChannelId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hidden = self.hidden_conversations(key, ids);
        let watermark = self.read_watermark();
        let mark: Vec<ChannelId> = ids
            .iter()
            .filter(|id| {
                self.fixtures
                    .conversation(id)
                    .is_some_and(|c| self.unread_in(c, watermark.as_ref()).count > 0)
            })
            .cloned()
            .collect();
        let this = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, |menu, _, _| {
            more_conversations_menu(
                menu.min_w(px(300.)).max_w(px(300.)),
                &hidden,
                mark,
                |ids| self.menu_rows(ids),
                &this,
            )
        });
        let dismiss = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.section_search = None;
            cx.notify();
        });
        menu.focus_handle(cx).focus(window, cx);
        self.section_search = Some(SectionSearch {
            key: key.clone(),
            menu,
            _dismiss: dismiss,
        });
        cx.notify();
    }

    /// Why the section leaves `conversation` out of its rows, by Slack's rules in Slack's
    /// order; `None` keeps it. The open conversation always stays. A muted one leaves unless
    /// something mentions you there (Slack's "Mute and hide"); a collapsed section keeps
    /// only the conversations that mention you; an expanded one applies its filter. What
    /// leaves is grouped as the "More conversations" popover lists it: dormant (under
    /// "Active only"), then muted, then unread, then read.
    pub(super) fn hidden_reason(
        &self,
        conversation: &Conversation,
        unread: Unread,
        filter: SectionFilter,
        open: bool,
    ) -> Option<Hidden> {
        if conversation.id == self.selected {
            return None;
        }
        let muted = !self.home_prefs.show_muted
            && self.notify_level(&conversation.id) == Some(NotifyLevel::Muted);
        let listed = if muted || !open {
            unread.mentions > 0
        } else {
            match filter {
                SectionFilter::ActiveOnly => unread.count > 0 || self.active_recent(conversation),
                SectionFilter::All => true,
                SectionFilter::Unreads => unread.count > 0,
                SectionFilter::Mentions => unread.mentions > 0,
            }
        };
        if listed {
            return None;
        }
        Some(
            if filter == SectionFilter::ActiveOnly && !self.active_recent(conversation) {
                Hidden::Dormant
            } else if muted {
                Hidden::Muted
            } else if unread.count > 0 {
                Hidden::Unread
            } else {
                Hidden::Read
            },
        )
    }

    /// Slack's "Active only" rule (`sidebar_behavior: hide_inactive_channels`): Slack's
    /// client hides exactly the conversations the server marks dormant
    /// (`properties.is_dormant`, labelled "No messages in at least 30 days") and lists every
    /// other one however quiet, so a silent but undormant channel such as
    /// #partner_support_klarna stays.
    pub(super) fn active_recent(&self, conversation: &Conversation) -> bool {
        !self.active_only || !self.is_dormant(conversation)
    }

    /// Whether Slack marks the conversation dormant ("No messages in at least 30 days");
    /// the workspace's #general never counts as inactive.
    pub(super) fn is_dormant(&self, conversation: &Conversation) -> bool {
        !conversation.is_general
            && conversation
                .properties
                .as_ref()
                .and_then(|properties| properties.extra.get("is_dormant"))
                .and_then(|dormant| dormant.as_bool())
                .unwrap_or(false)
    }

    /// The gear menu's "Create a channel section": the dialog over a fresh name field.
    pub(crate) fn open_section_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.section_form_mode = SectionFormMode::Create;
        self.section_form
            .update(cx, |form, cx| form.open(window, cx));
        let form = self.section_form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Create a channel section")
                .w(px(440.))
                .child(form.clone())
        });
    }

    /// A section's "Rename": the same dialog seeded with the current name.
    pub(crate) fn open_rename_section(
        &mut self,
        section: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.section_form_mode = SectionFormMode::Rename(section);
        self.section_form
            .update(cx, |form, cx| form.open_editing(&name, window, cx));
        let form = self.section_form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Rename section")
                .w(px(440.))
                .child(form.clone())
        });
    }

    /// The row menu's "+ Create new section": the dialog, and the channel moves into the
    /// fresh section once it confirms.
    pub(crate) fn open_create_with_channel(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.section_form_mode = SectionFormMode::CreateWithChannel(channel);
        self.section_form
            .update(cx, |form, cx| form.open(window, cx));
        let form = self.section_form.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Create a channel section")
                .w(px(440.))
                .child(form.clone())
        });
    }

    /// "Delete section", behind Slack's confirmation: the section's channels fall back to
    /// their default buckets.
    pub(crate) fn ask_delete_section(
        &mut self,
        section: &ChannelSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        let id = section.id.clone();
        let name = section.name.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let (this, section_id) = (this.clone(), id.clone());
            dialog
                .title(format!("Delete {name}?"))
                .w(px(440.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(format!(
                            "Any channels in {name} will move back to their default sections."
                        )))
                        .child(
                            h_flex().gap_2().justify_end().child(
                                crate::preferences::form_button(
                                    Button::new("section-delete-cancel")
                                        .outline()
                                        .label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                        ),
                )
                // The confirm rides as the dialog's footer action so it reads as one.
                .child(
                    h_flex().justify_end().child(
                        crate::preferences::form_button(
                            Button::new("section-delete-confirm")
                                .danger()
                                .label("Delete section"),
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = this.update(cx, |_, cx| {
                                cx.emit(SidebarEvent::DeleteSection {
                                    section: section_id.clone(),
                                })
                            });
                        }),
                    ),
                )
        });
    }
}
