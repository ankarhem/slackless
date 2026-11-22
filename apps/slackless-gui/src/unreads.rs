//! The All Unreads view (Slack's `p-unreads_view`, Cmd+Shift+A and the Home sidebar's
//! "Unreads" row): every conversation with unread messages as one collapsible card - the
//! conversation header with its mention pill and unread count, then the unread messages
//! themselves, oldest first, rendered like any transcript row with working links
//! (@mentions and their hover cards, #channels, URLs). "Mark as Read" moves Slack's read
//! cursor to the group's newest message through the root's existing mark path; "Mark All
//! Messages Read" routes through the root too, so the user's confirmation preference
//! holds.
//!
//! Data: the sidebar's own unread order picks the channels; each group derives from the
//! bundled workspace or the live snapshot ([`local_groups`]), and live mode re-reads the
//! local store off the UI thread ([`store_groups`]) with `client.counts`' read cursors. A
//! channel Slack counts unread whose stored rows lag shows a placeholder line instead -
//! opening the conversation loads them, as in Slack. Muted conversations stay out unless
//! they mention the signed-in user.
//!
//! `UnreadsView::new(Arc<Fixtures>, &mut App) -> Entity<UnreadsView>`
//! `UnreadsView::show_local / show_store / set_fixtures`
//! emits `UnreadsEvent::OpenConversation(ChannelId)` / `OpenLink(LinkTarget)`
//! / `MarkRead { channel, ts }` / `MarkAllRead`

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::NotifyLevel;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, ConversationKind, Message, TeamId, Ts};

use crate::avatars::conversation_avatar;
use crate::fixtures::Fixtures;
use crate::messages::{LinkHandler, LinkTarget, message_row_linked};
use crate::sidebar::mentions_user;

/// Channel-level rows scanned per conversation in live mode; beyond it the conversation is
/// read where it stands.
const STORE_SCAN_LIMIT: usize = 100;

/// One conversation's unread messages, as one card.
#[derive(Clone)]
pub struct UnreadGroup {
    pub channel: ChannelId,
    /// The unread messages, oldest first: reading top to bottom is reading them in order.
    pub messages: Vec<Message>,
    /// Where "Mark as Read" moves the read cursor: the newest unread timestamp. Falls back
    /// to `client.counts`' `latest` while neither the snapshot nor the store has that
    /// message, and never sits behind it (a stale row would leave Slack's own cursor short
    /// of the real newest message).
    pub newest_ts: Ts,
    pub mention_count: u32,
}

/// Whether a message renders in the view at all: metadata rows never do, and nothing at or
/// behind the read cursor does.
fn unread_row(message: &Message, last_read: Option<&Ts>) -> bool {
    !message.hidden && !message.is_ephemeral && last_read.is_none_or(|read| message.ts > *read)
}

/// The unread groups of `order` (the sidebar's unread order) from the bundled workspace or
/// the live snapshot. One's own messages are read by definition, so they never group. A
/// conversation Slack counts unread whose rows lag (live snapshots keep channel histories
/// out) still groups, its newest timestamp from `client.counts`; a muted conversation only
/// groups while it mentions the signed-in user, as Slack lists it.
pub fn local_groups(fixtures: &Fixtures, order: &[ChannelId]) -> Vec<UnreadGroup> {
    let mut groups = Vec::new();
    for channel in order {
        let Some(conversation) = fixtures.conversation(channel) else {
            continue;
        };
        let counts = fixtures.counts.get(channel);
        let last_read = counts
            .and_then(|counts| counts.last_read.as_ref())
            .or(conversation.last_read.as_ref());
        let messages: Vec<Message> = fixtures
            .messages
            .get(channel)
            .into_iter()
            .flatten()
            .filter(|message| {
                unread_row(message, last_read)
                    && !message
                        .user
                        .as_ref()
                        .is_some_and(|user| *user == fixtures.self_user)
            })
            .cloned()
            .collect();
        let mention_count = counts.map_or_else(
            || {
                messages
                    .iter()
                    .filter(|message| mentions_user(message, &fixtures.self_user))
                    .count() as u32
            },
            |counts| counts.mention_count,
        );
        if fixtures.notify_levels.get(channel) == Some(&NotifyLevel::Muted) && mention_count == 0 {
            continue;
        }
        let counted = counts.is_some_and(|c| c.has_unreads || c.mention_count > 0);
        let Some(newest_ts) = messages
            .last()
            .map(|message| message.ts.clone())
            .or_else(|| counts.and_then(|counts| counts.latest.clone()))
        else {
            continue;
        };
        if !messages.is_empty() || counted {
            groups.push(UnreadGroup {
                channel: channel.clone(),
                messages,
                newest_ts,
                mention_count,
            });
        }
    }
    groups
}

/// The live store's unread groups for `targets`: each channel Slack counts unread with its
/// read cursor, its newest message (`client.counts`) and its mention count. A channel whose
/// stored rows lag behind `latest` still groups - its card asks for the conversation to be
/// opened - and the read cursor still moves to `latest`.
pub fn store_groups(
    store: &Store,
    team: &TeamId,
    targets: &[(ChannelId, Option<Ts>, Option<Ts>, u32)],
) -> Vec<UnreadGroup> {
    let mut groups = Vec::new();
    for (channel, last_read, latest, mention_count) in targets {
        let Ok(rows) = store.channel_history(team, channel, None, STORE_SCAN_LIMIT) else {
            continue;
        };
        let messages: Vec<Message> = rows
            .iter()
            .filter(|row| !row.deleted && unread_row(&row.message, last_read.as_ref()))
            .map(|row| row.message.clone())
            .collect();
        // `latest` at or behind the cursor means Slack itself counts nothing unread there.
        let unread_latest = match (latest, last_read) {
            (Some(latest), Some(read)) => latest > read,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if messages.is_empty() && !unread_latest {
            continue;
        }
        let Some(newest_ts) = messages
            .last()
            .map(|message| message.ts.clone())
            .into_iter()
            .chain(latest.clone())
            .max()
        else {
            continue;
        };
        groups.push(UnreadGroup {
            channel: channel.clone(),
            messages,
            newest_ts,
            mention_count: *mention_count,
        });
    }
    groups
}

/// The store targets for the channels `groups` names: each one's read cursor, newest
/// message and mention count, as the snapshot's `client.counts` copy reported them.
pub fn store_targets(
    fixtures: &Fixtures,
    groups: &[UnreadGroup],
) -> Vec<(ChannelId, Option<Ts>, Option<Ts>, u32)> {
    groups
        .iter()
        .map(|group| {
            let counts = fixtures.counts.get(&group.channel);
            let last_read = counts
                .and_then(|counts| counts.last_read.clone())
                .or_else(|| {
                    fixtures
                        .conversation(&group.channel)
                        .and_then(|conversation| conversation.last_read.clone())
                });
            (
                group.channel.clone(),
                last_read,
                counts.and_then(|counts| counts.latest.clone()),
                group.mention_count,
            )
        })
        .collect()
}

/// What the All Unreads view asks the root for.
pub enum UnreadsEvent {
    /// A card's conversation (its name, or the placeholder of a channel without rows) was
    /// clicked: open it.
    OpenConversation(ChannelId),
    /// A link inside a message (@mention, #channel, URL) was activated.
    OpenLink(LinkTarget),
    /// The group's "Mark as Read" (or Esc): move the read cursor to `ts`.
    MarkRead { channel: ChannelId, ts: Ts },
    /// "Mark All Messages Read": the root runs it through the user's confirmation setting.
    MarkAllRead,
}

pub struct UnreadsView {
    fixtures: Arc<Fixtures>,
    groups: Vec<UnreadGroup>,
    /// Conversations whose cards the user collapsed.
    collapsed: HashSet<ChannelId>,
    /// The keyboard-focused group (Up/Down move it, Left/Right fold it, Esc marks it read).
    focused: Option<usize>,
}

impl EventEmitter<UnreadsEvent> for UnreadsView {}

impl UnreadsView {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<UnreadsView> {
        cx.new(|_| UnreadsView {
            fixtures,
            groups: Vec::new(),
            collapsed: HashSet::new(),
            focused: None,
        })
    }

    /// Show `groups` (the fixture or snapshot derivation); call when the view opens or the
    /// workspace reloads.
    pub fn show_local(
        &mut self,
        fixtures: Arc<Fixtures>,
        groups: Vec<UnreadGroup>,
        cx: &mut Context<Self>,
    ) {
        self.fixtures = fixtures;
        self.set_groups(groups, cx);
    }

    /// Fold in `groups` (the store derivation) for the conversations `keep` still counts
    /// unread, in `keep`'s order: shown conversations take their stored rows, one the store
    /// missed keeps the rows it has (a mark raced the query), and conversations no longer
    /// unread drop out.
    pub fn show_store(
        &mut self,
        groups: Vec<UnreadGroup>,
        keep: &[ChannelId],
        cx: &mut Context<Self>,
    ) {
        let mut fresh: Vec<Option<UnreadGroup>> = groups.into_iter().map(Some).collect();
        let take = |fresh: &mut [Option<UnreadGroup>], channel: &ChannelId| {
            let at = fresh
                .iter()
                .position(|group| group.as_ref().is_some_and(|g| &g.channel == channel))?;
            fresh[at].take()
        };
        let mut merged: Vec<UnreadGroup> = self
            .groups
            .drain(..)
            .filter(|group| keep.contains(&group.channel))
            .map(|group| take(&mut fresh, &group.channel).unwrap_or(group))
            .collect();
        for channel in keep {
            if merged.iter().any(|group| &group.channel == channel) {
                continue;
            }
            if let Some(group) = take(&mut fresh, channel) {
                merged.push(group);
            }
        }
        merged.sort_by_key(|group| keep.iter().position(|channel| *channel == group.channel));
        self.set_groups(merged, cx);
    }

    fn set_groups(&mut self, groups: Vec<UnreadGroup>, cx: &mut Context<Self>) {
        self.groups = groups;
        self.collapsed
            .retain(|channel| self.groups.iter().any(|group| &group.channel == channel));
        self.focused = self.focused.filter(|ix| *ix < self.groups.len());
        cx.notify();
    }

    /// Live refresh: keep the groups, draw them from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// The view's keys (Slack's "Navigate unread messages"): Up/Down move the focus,
    /// Left/Right collapse or expand the focused group, Esc marks it read. Returns whether
    /// the key was taken; with nothing unread no key is.
    pub fn handle_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "up" | "down" if !self.groups.is_empty() => {
                self.focused = match self.focused {
                    None => Some(0),
                    Some(ix) if key == "down" => Some((ix + 1).min(self.groups.len() - 1)),
                    Some(ix) => Some(ix.saturating_sub(1)),
                };
                cx.notify();
                true
            }
            "left" | "right" if !self.groups.is_empty() => {
                let ix = self
                    .focused
                    .filter(|ix| *ix < self.groups.len())
                    .unwrap_or(0);
                self.focused = Some(ix);
                let channel = self.groups[ix].channel.clone();
                if key == "left" {
                    self.collapsed.insert(channel);
                } else {
                    self.collapsed.remove(&channel);
                }
                cx.notify();
                true
            }
            "escape" if !self.groups.is_empty() => {
                self.mark_read(self.focused.unwrap_or(0), cx);
                true
            }
            _ => false,
        }
    }

    /// Collapse or expand group `ix` (its chevron), focusing it as a click does.
    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.focused = Some(ix);
        if let Some(group) = self.groups.get(ix) {
            let channel = group.channel.clone();
            if !self.collapsed.remove(&channel) {
                self.collapsed.insert(channel);
            }
        }
        cx.notify();
    }

    /// Mark group `ix` read: drop its card, pass the focus to the group that takes its
    /// place, and tell the root where Slack's read cursor moves.
    fn mark_read(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get(ix) else {
            return;
        };
        let (channel, ts) = (group.channel.clone(), group.newest_ts.clone());
        self.groups.remove(ix);
        self.collapsed.remove(&channel);
        self.focused = (!self.groups.is_empty() && self.focused.is_some())
            .then(|| ix.min(self.groups.len() - 1));
        cx.emit(UnreadsEvent::MarkRead { channel, ts });
        cx.notify();
    }
}

impl Render for UnreadsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let background = theme.background;
        let groups: Vec<UnreadGroup> = self.groups.clone();
        let focused = self.focused;
        let collapsed: HashSet<ChannelId> = self.collapsed.clone();
        let fixtures = self.fixtures.clone();

        // Slack's header: the bold title beside its conversation count, and the outline
        // "Mark All Messages Read" once anything is left.
        let header = h_flex()
            .flex_none()
            .h(px(50.))
            .px_4()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .child("Unreads"),
                    )
                    .when(!groups.is_empty(), |title| {
                        title.child(div().text_sm().text_color(muted).child(format!(
                            "{} conversation{}",
                            groups.len(),
                            if groups.len() == 1 { "" } else { "s" }
                        )))
                    }),
            )
            .when(!groups.is_empty(), |header| {
                header.child(
                    Button::new("unreads-mark-all-top")
                        .outline()
                        .with_size(Size::Small)
                        .label("Mark All Messages Read")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(UnreadsEvent::MarkAllRead))),
                )
            });

        let body = if groups.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::CheckCheck)
                                .text_color(muted)
                                .size(px(28.)),
                        )
                        .child(
                            div()
                                .text_size(px(20.))
                                .font_weight(FontWeight::BOLD)
                                .child("No more unread messages. All clear."),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("You're all caught up."),
                        ),
                )
                .into_any_element()
        } else {
            v_flex()
                .id("unreads-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_4()
                .py_3()
                .gap_3()
                .children((0..groups.len()).map(|ix| {
                    card_view(
                        &fixtures,
                        ix,
                        &groups[ix],
                        collapsed.contains(&groups[ix].channel),
                        focused == Some(ix),
                        background,
                        cx,
                    )
                }))
                .child(
                    h_flex().flex_none().justify_center().py_6().child(
                        Button::new("unreads-mark-all-bottom")
                            .outline()
                            .with_size(Size::Small)
                            .label("Mark All Messages Read")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(UnreadsEvent::MarkAllRead)),
                            ),
                    ),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(background)
            .child(header)
            .child(body)
    }
}

/// One conversation's card: the header (collapse chevron, conversation badge, bold name,
/// mention pill, unread count, "Mark as Read") over its unread messages, oldest first - or,
/// while collapsed, nothing, and while its rows lag, the line that loads them.
fn card_view(
    fixtures: &Fixtures,
    ix: usize,
    group: &UnreadGroup,
    collapsed: bool,
    focused: bool,
    background: Hsla,
    cx: &mut Context<UnreadsView>,
) -> AnyElement {
    let theme = cx.theme();
    let (border, muted, danger, danger_foreground, link, hover) = (
        theme.border,
        theme.muted_foreground,
        theme.danger,
        theme.danger_foreground,
        theme.link,
        theme.accent,
    );
    let conversation = fixtures.conversation(&group.channel);
    let label = conversation
        .map(|conversation| fixtures.conversation_label(conversation))
        .unwrap_or_else(|| group.channel.to_string());
    let kind = conversation.map(|conversation| conversation.kind());
    // The conversation badge: # for public channels, a lock for private ones, the peer
    // avatar for DMs - as every row in the app renders it.
    let badge = match kind {
        Some(ConversationKind::Im) | Some(ConversationKind::Mpim) => conversation
            .map(|conversation| {
                conversation_avatar(fixtures, conversation, 20., background, cx).into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element()),
        Some(ConversationKind::PrivateChannel) => Icon::new(IconName::Lock)
            .text_color(muted)
            .size(px(16.))
            .into_any_element(),
        _ => Icon::new(IconName::Hash)
            .text_color(muted)
            .size(px(16.))
            .into_any_element(),
    };
    let name = match kind {
        Some(ConversationKind::PublicChannel) | Some(ConversationKind::PrivateChannel) => {
            label.trim_start_matches("# ").to_owned()
        }
        _ => label,
    };
    let open = group.channel.clone();

    let header = h_flex()
        .flex_none()
        .h(px(48.))
        .px_4()
        .items_center()
        .justify_between()
        .gap_2()
        .when(!collapsed, |header| {
            header.border_b_1().border_color(border)
        })
        .child(
            h_flex()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .id(("unreads-collapse", ix))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|toggle| toggle.bg(hover))
                        .child(
                            Icon::new(if collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .text_color(muted)
                            .size(px(16.)),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(ix, cx))),
                )
                .child(
                    h_flex()
                        .id(("unreads-open", ix))
                        .min_w_0()
                        .items_center()
                        .gap_1()
                        .rounded_md()
                        .px_1()
                        .cursor_pointer()
                        .hover(|name| name.bg(hover))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(UnreadsEvent::OpenConversation(open.clone()))
                        }))
                        .child(badge)
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::BOLD)
                                .truncate()
                                .child(name),
                        )
                        .when(group.mention_count > 0, |row| {
                            row.child(
                                div()
                                    .rounded_full()
                                    .bg(danger)
                                    .text_xs()
                                    .px_1p5()
                                    .text_color(danger_foreground)
                                    .child(group.mention_count.to_string()),
                            )
                        })
                        .when(!group.messages.is_empty(), |row| {
                            row.child(div().text_sm().text_color(muted).child(
                                if group.messages.len() == 1 {
                                    "1 message".to_owned()
                                } else {
                                    format!("{} messages", group.messages.len())
                                },
                            ))
                        }),
                ),
        )
        .child(
            Button::new(("unreads-mark-read", ix))
                .ghost()
                .with_size(Size::Small)
                .icon(IconName::Check)
                .label("Mark as Read")
                .on_click(cx.listener(move |this, _, _, cx| this.mark_read(ix, cx))),
        );

    let channel = group.channel.clone();
    let body = if collapsed {
        None
    } else if group.messages.is_empty() {
        // Live mode, before the conversation's history lands in the store.
        Some(
            div()
                .id(("unreads-load", ix))
                .px_4()
                .py_3()
                .text_sm()
                .text_color(muted)
                .cursor_pointer()
                .hover(|row| row.underline())
                .child("Open conversation to load messages.")
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(UnreadsEvent::OpenConversation(channel.clone()))
                }))
                .into_any_element(),
        )
    } else {
        // The card's rows share one link handler: @mentions (and their hover cards),
        // #channels and URLs route out as the view's own event.
        let view = cx.entity();
        let links: LinkHandler = Rc::new(move |target, _window, cx| {
            view.update(cx, |_, cx| cx.emit(UnreadsEvent::OpenLink(target.clone())));
        });
        Some(
            v_flex()
                .px_4()
                .py_3()
                .gap_2()
                .children(group.messages.iter().map(|message| {
                    message_row_linked(fixtures, message, None, Some(&links), None, cx)
                }))
                .into_any_element(),
        )
    };

    v_flex()
        .id(("unreads-card", ix))
        .flex_none()
        .rounded(px(12.))
        .border_1()
        .border_color(if focused { link } else { border })
        .bg(cx.theme().background)
        .overflow_hidden()
        .child(header)
        .children(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::gpui::{Subscription, TestAppContext};

    use super::*;
    use crate::fixtures;

    /// A `client.counts` entry, as the snapshot keeps it.
    fn counts(
        channel: &str,
        last_read: Option<&str>,
        latest: Option<&str>,
        mentions: u32,
        unreads: bool,
    ) -> slackless_core::api::ConversationCounts {
        slackless_core::api::ConversationCounts {
            id: ChannelId::from(channel),
            last_read: last_read.map(Ts::from),
            latest: latest.map(Ts::from),
            mention_count: mentions,
            has_unreads: unreads,
            extra: Default::default(),
        }
    }

    /// A group of bare messages at `timestamps`, for view-state tests.
    fn group(channel: &str, timestamps: &[&str]) -> UnreadGroup {
        UnreadGroup {
            channel: ChannelId::from(channel),
            messages: timestamps
                .iter()
                .map(|ts| Message {
                    ts: Ts::from(*ts),
                    ..Default::default()
                })
                .collect(),
            newest_ts: Ts::from(timestamps.last().copied().unwrap_or("0")),
            mention_count: 0,
        }
    }

    #[test]
    fn local_groups_keep_only_newer_rows_oldest_first() {
        let fixtures = fixtures::load();
        // D0ALICE is read up to its first message; the reply after it (and only it - the
        // middle one is the user's own) is unread.
        let last_read = fixtures
            .conversation(&ChannelId::from("D0ALICE"))
            .and_then(|dm| dm.last_read.clone())
            .unwrap();
        let groups = local_groups(&fixtures, &[ChannelId::from("D0ALICE")]);
        let unread = groups
            .iter()
            .find(|group| group.channel == ChannelId::from("D0ALICE"))
            .expect("the unread DM groups");
        assert!(unread.messages.iter().all(|message| message.ts > last_read));
        assert!(
            unread
                .messages
                .iter()
                .all(|message| message.user.as_ref() != Some(&fixtures.self_user))
        );
        assert_eq!(
            unread.messages.len(),
            1,
            "one reply, one own message behind"
        );
        // #general has no cursor: every message but the user's own shows, oldest first.
        let general = local_groups(&fixtures, &[ChannelId::from("C0GENERAL")])
            .pop()
            .unwrap();
        assert!(
            general
                .messages
                .windows(2)
                .all(|pair| pair[0].ts < pair[1].ts)
        );
        assert!(
            general
                .messages
                .iter()
                .all(|message| message.user.as_ref() != Some(&fixtures.self_user))
        );
        assert_eq!(general.newest_ts, general.messages.last().unwrap().ts);
    }

    #[test]
    fn muted_channels_list_only_with_mentions() {
        let mut fixtures = fixtures::load();
        fixtures.counts.clear();
        let muted = ChannelId::from("C0DESIGN");
        assert_eq!(
            fixtures.notify_levels.get(&muted),
            Some(&NotifyLevel::Muted),
            "the sample workspace mutes #design"
        );
        fixtures.counts.insert(
            ChannelId::from("C0DESIGN"),
            counts("C0DESIGN", None, Some("3000.000000"), 0, true),
        );
        assert!(
            local_groups(&fixtures, std::slice::from_ref(&muted)).is_empty(),
            "a muted channel without mentions stays out, as Slack lists it"
        );
        fixtures.counts.insert(
            ChannelId::from("C0DESIGN"),
            counts("C0DESIGN", None, Some("3000.000000"), 2, true),
        );
        let groups = local_groups(&fixtures, std::slice::from_ref(&muted));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].mention_count, 2);
    }

    #[test]
    fn counted_unreads_group_without_stored_rows() {
        let mut fixtures = fixtures::load();
        let channel = ChannelId::from("C0ENG");
        fixtures.messages.remove(&channel);
        fixtures.counts.insert(
            ChannelId::from("C0ENG"),
            counts("C0ENG", Some("1500.000000"), Some("3000.000000"), 1, true),
        );
        let groups = local_groups(&fixtures, std::slice::from_ref(&channel));
        let unread = groups.first().expect("the counted channel groups");
        assert!(unread.messages.is_empty(), "no rows are stored for it");
        assert_eq!(unread.newest_ts, Ts::from("3000.000000"));
        assert_eq!(unread.mention_count, 1);
        // The store targets keep the cursor the counts reported.
        let targets = store_targets(&fixtures, groups.as_slice());
        assert_eq!(
            targets,
            vec![(
                channel,
                Some(Ts::from("1500.000000")),
                Some(Ts::from("3000.000000")),
                1
            )]
        );
    }

    #[test]
    fn read_conversations_do_not_group() {
        let mut fixtures = fixtures::load();
        let channel = ChannelId::from("C0GENERAL");
        let last_read = fixtures
            .messages
            .get(&channel)
            .unwrap()
            .last()
            .unwrap()
            .ts
            .clone();
        if let Some(conversation) = fixtures.channels.iter_mut().find(|c| c.id == channel) {
            conversation.last_read = Some(last_read.clone());
        }
        let groups = local_groups(&fixtures, std::slice::from_ref(&channel));
        assert!(
            groups.is_empty(),
            "a cursor at the newest message leaves nothing to read"
        );
        let targets = vec![(channel, Some(last_read), None, 0)];
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        assert!(
            store_groups(&store.lock().unwrap(), &team, &targets).is_empty(),
            "the store agrees: no row is newer"
        );
    }

    #[test]
    fn store_groups_read_the_watermark_and_reach_the_newest_known() {
        let fixtures = fixtures::load();
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        let channel = ChannelId::from("C0GENERAL");
        let history = fixtures.messages.get(&channel).unwrap();
        let last_read = history.first().unwrap().ts.clone();
        // `client.counts` knows a newest message the store has not synced.
        let latest = Ts::from("9999999999.000100");
        let targets = vec![(
            channel.clone(),
            Some(last_read.clone()),
            Some(latest.clone()),
            0,
        )];
        let groups = store_groups(&store.lock().unwrap(), &team, &targets);
        let unread = groups.first().expect("the channel groups");
        assert!(unread.messages.iter().all(|message| message.ts > last_read));
        assert!(
            unread
                .messages
                .windows(2)
                .all(|pair| pair[0].ts < pair[1].ts)
        );
        assert_eq!(unread.newest_ts, latest);
    }

    #[test]
    fn store_groups_keep_channels_the_store_has_not_synced() {
        let fixtures = fixtures::load();
        let store =
            crate::search::fixture_archive(&fixtures).expect("the fixture archive seeds a store");
        let team = fixtures.workspaces[0].id.clone();
        // A directory channel: the conversation is stored, its history is not.
        let unsynced = ChannelId::from("C0OPS");
        let ahead = vec![(
            unsynced.clone(),
            Some(Ts::from("1000.000000")),
            Some(Ts::from("2000.000000")),
            3,
        )];
        let groups = store_groups(&store.lock().unwrap(), &team, &ahead);
        let unread = groups.first().expect("the counted channel groups");
        assert!(unread.messages.is_empty());
        assert_eq!(unread.newest_ts, Ts::from("2000.000000"));
        assert_eq!(unread.mention_count, 3);
        // A `latest` at or behind the cursor means nothing unread: no group.
        let behind = vec![(
            unsynced,
            Some(Ts::from("2000.000000")),
            Some(Ts::from("1000.000000")),
            0,
        )];
        assert!(store_groups(&store.lock().unwrap(), &team, &behind).is_empty());
    }

    #[gpui::test]
    fn keyboard_walks_collapses_and_marks_read(cx: &mut TestAppContext) {
        let fixtures = Arc::new(fixtures::load());
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let view = cx.update(|cx| UnreadsView::new(fixtures.clone(), cx));
        let seen = events.clone();
        let recorder = cx.new(|cx| {
            struct Recorder(#[allow(dead_code)] Subscription);
            let subscription = cx.subscribe(&view, move |_, _, event: &UnreadsEvent, _| {
                if let UnreadsEvent::MarkRead { channel, ts } = event {
                    seen.borrow_mut().push((channel.clone(), ts.clone()));
                }
            });
            Recorder(subscription)
        });

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show_local(
                    fixtures.clone(),
                    vec![
                        group("C1", &["100.000001"]),
                        group("C2", &["200.000001"]),
                        group("C3", &["300.000001"]),
                    ],
                    cx,
                );
                // Left right after opening (before Up/Down) focuses and collapses the first card.
                assert!(view.handle_key("left", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.collapsed.contains(&ChannelId::from("C1")));
                assert!(view.handle_key("right", cx));
                assert!(!view.collapsed.contains(&ChannelId::from("C1")));
                view.focused = None;

                // Down focuses the first group; the focus walks both ways and stops at the
                // ends.
                assert!(view.handle_key("down", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.handle_key("up", cx));
                assert_eq!(view.focused, Some(0));
                assert!(view.handle_key("down", cx));
                assert_eq!(view.focused, Some(1));
                // Left collapses the focused group, Right expands it.
                assert!(view.handle_key("left", cx));
                assert!(view.collapsed.contains(&ChannelId::from("C2")));
                assert!(view.handle_key("right", cx));
                assert!(!view.collapsed.contains(&ChannelId::from("C2")));
            });
        });

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                // Esc marks the focused group read; the group below takes its place and the
                // focus stays on it.
                assert!(view.handle_key("escape", cx));
            });
        });
        assert_eq!(
            events.borrow().as_slice(),
            [(ChannelId::from("C2"), Ts::from("200.000001"))],
            "Esc marks the focused group read up to its newest message"
        );
        cx.update(|cx| {
            let view = view.read(cx);
            assert_eq!(view.groups.len(), 2);
            assert_eq!(view.groups[0].channel, ChannelId::from("C1"));
            assert_eq!(
                view.focused,
                Some(1),
                "the group below the marked one takes its place and the focus"
            );
        });

        // Marking the last group read passes the focus back.
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.handle_key("down", cx);
                view.handle_key("escape", cx);
                view.handle_key("escape", cx);
            });
        });
        cx.update(|cx| {
            let view = view.read(cx);
            assert!(view.groups.is_empty());
            assert_eq!(view.focused, None);
        });
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                assert!(
                    !view.handle_key("escape", cx),
                    "with nothing left, Esc is not taken"
                );
            });
        });
        drop(recorder);
    }

    #[gpui::test]
    fn store_folds_refresh_without_resurrecting_marks(cx: &mut TestAppContext) {
        let fixtures = Arc::new(fixtures::load());
        let view = cx.update(|cx| UnreadsView::new(fixtures.clone(), cx));
        let c2 = ChannelId::from("C2");
        let c3 = ChannelId::from("C3");
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.show_local(
                    fixtures.clone(),
                    vec![group("C1", &["100.000001"]), group("C2", &["200.000001"])],
                    cx,
                );
                view.mark_read(0, cx);
                assert_eq!(view.groups.len(), 1, "the marked group left");
                // The store derivation still lists C1 (the read raced the query); only
                // channels Slack still counts unread take part.
                view.show_store(
                    vec![
                        group("C1", &["100.000001", "100.000002"]),
                        group("C2", &["200.000001", "200.000002"]),
                        group("C3", &["300.000001"]),
                    ],
                    &[c2.clone(), c3.clone()],
                    cx,
                );
                assert_eq!(
                    view.groups
                        .iter()
                        .map(|group| group.channel.clone())
                        .collect::<Vec<_>>(),
                    vec![c2.clone(), c3.clone()],
                    "the dismissed group stays gone; the fresh one joins in unread order"
                );
                assert_eq!(
                    view.groups[0].messages.len(),
                    2,
                    "the shown group took its stored rows"
                );
            });
        });
    }
}
