//! The Activity page (the rail's Activity destination): Slack's own Activity inbox
//! (`activity.feed`, hydrated through `messages.list`), rendered full-width beside the rail
//! like the official app - a title, the All / DMs / Mentions / Threads tabs (client-side
//! filters over the loaded page), the Unreads toggle, day-grouped rows (avatar, author +
//! time, the message body through the shared mrkdwn renderer, a context line) with rose
//! unread pills and a hover action bar, newest first.
//!
//! Data: live mode asks the core for Slack's feed ([`slackless_core::Command::FetchActivity`],
//! answered by [`slackless_core::Event::ActivityView`]); the boot fetch fills the rail and
//! dock badges before the page is ever opened. Fixture mode (no session) derives the rows
//! from the workspace fixtures in the same layout:
//! - mentions: messages by others that @-mention the signed-in user; unread when newer than
//!   the conversation's `last_read`;
//! - reactions: the signed-in user's messages that others reacted to; the newest two unread;
//! - threads: threads the signed-in user started or replied in whose latest reply is by
//!   someone else; unread.
//!
//! `ActivityFeed::new(Arc<Fixtures>, &mut App) -> Entity<ActivityFeed>` emits
//! `ActivityEvent::Open { channel, thread_ts }` (thread rows open their thread),
//! `ActivityEvent::Mark { entries, read }` (the read toggles) and `ActivityEvent::Retry`.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
};
use slackless_core::api::{ActivityEntry, ActivityFeed as SlackFeed, ActivityItem, ActivityKind};
use slackless_core::types::{ChannelId, Message, Ts, UserId};

use crate::avatars::author_avatar;
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::messages::{day_label, list_stamp, mrkdwn_preview};
use crate::sidebar::mentions_user;

/// Rows kept beyond Slack's page are dropped (bounded memory; a page holds ~50 and the
/// fixture workspace fewer still).
const MAX_ITEMS: usize = 200;

/// Reaction items marked unread in fixture mode (the newest ones).
const UNREAD_REACTIONS: usize = 2;

/// Where a live fetch stands; the rows stay visible across refreshes, so the states only
/// matter before the first page lands.
#[derive(Default)]
enum Load {
    #[default]
    Idle,
    Loading,
    Failed(String),
}

/// The page's tabs; each filters the loaded page client-side (`F-SHELL-046`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Dms,
    Mentions,
    Threads,
    Reactions,
    Apps,
    Vips,
}

impl Filter {
    const ALL: [Filter; 7] = [
        Filter::All,
        Filter::Dms,
        Filter::Mentions,
        Filter::Threads,
        Filter::Reactions,
        Filter::Apps,
        Filter::Vips,
    ];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Dms => "DMs",
            Filter::Mentions => "Mentions",
            Filter::Threads => "Threads",
            Filter::Reactions => "Reactions",
            Filter::Apps => "Apps",
            Filter::Vips => "VIPs",
        }
    }

    /// Whether a row of `kind` shows under this tab.
    fn admits(self, kind: ActivityKind) -> bool {
        matches!(
            (self, kind),
            (Filter::All | Filter::Vips, _)
                | (Filter::Dms, ActivityKind::Dm | ActivityKind::App)
                | (Filter::Mentions, ActivityKind::Mention)
                | (Filter::Threads, ActivityKind::Thread)
                | (Filter::Reactions, ActivityKind::Reaction)
                | (Filter::Apps, ActivityKind::App)
        )
    }
}

/// The context line under a row's body.
#[derive(Clone)]
enum RowContext {
    /// "@ Mention in # eng-order-public thread".
    Mention { label: String, thread: bool },
    /// "Thread in # channel".
    Thread { label: String },
    /// "Reacted {reaction chip} to: …".
    Reaction,
    /// "Direct message".
    Dm,
    /// "App".
    App,
    /// "# channel".
    Channel { label: String },
}

/// One row, live or fixture-derived; every field the page renders.
#[derive(Clone)]
struct Item {
    kind: ActivityKind,
    channel: ChannelId,
    /// Thread root, for thread rows (opens the thread panel).
    thread_ts: Option<Ts>,
    /// When the row's message happened (the stamp and the day grouping).
    ts: Ts,
    /// The `activity.markRead` handles; `None` for fixture rows (nothing to mark).
    entry: Option<ActivityEntry>,
    /// Who the row is about (the mentioning author, the reactor).
    actor: Option<UserId>,
    /// Display name when no profile resolves (fixture mode joins several names).
    actor_label: String,
    /// The reaction shortcode, for the context chip.
    reaction: Option<String>,
    context: RowContext,
    /// The body, as mrkdwn.
    text: String,
    unread: bool,
    /// The unread pill's count (0 renders a dot).
    count: u32,
    /// The message the row points at: the avatar/name source, and the live body.
    message: Option<Message>,
}

pub enum ActivityEvent {
    /// A row was clicked: open its conversation (its thread for thread rows).
    Open {
        channel: ChannelId,
        thread_ts: Option<Ts>,
    },
    /// A row's read state was toggled; live mode forwards this to the core.
    Mark {
        entries: Vec<ActivityEntry>,
        read: bool,
    },
    /// The error state's Retry: fetch the page again.
    Retry,
    /// Open the Activity feed in a standalone pop-out window (`F-SHELL-149`).
    PopOut,
}

pub struct ActivityFeed {
    fixtures: Arc<Fixtures>,
    items: Vec<Item>,
    filter: Filter,
    unreads_only: bool,
    vip_users: String,
    load: Load,
    /// Keyboard-selected visible row index (`Up` / `Down` / `Enter` in Activity).
    selected: Option<usize>,
}

impl EventEmitter<ActivityEvent> for ActivityFeed {}

impl ActivityFeed {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<ActivityFeed> {
        cx.new(|_| ActivityFeed {
            // Live mode starts empty (the boot fetch and the page's own fetch bring Slack's
            // rows); the fixture workspace derives its rows right away.
            items: if fixtures.sample {
                fixture_items(&fixtures)
            } else {
                Vec::new()
            },
            fixtures,
            filter: Filter::All,
            unreads_only: false,
            vip_users: String::new(),
            load: Load::Idle,
            selected: None,
        })
    }

    /// Update the configured VIP list used by the `VIPs` filter tab (`F-SHELL-061`).
    pub fn set_vip_users(&mut self, vip_users: String, cx: &mut Context<Self>) {
        if self.vip_users != vip_users {
            self.vip_users = vip_users;
            cx.notify();
        }
    }

    /// Snapshot of currently visible items for a popped-out Activity window (`F-SHELL-149`).
    pub fn snapshot_for_popout(&self) -> Vec<crate::popout::PopoutActivityRow> {
        self.visible()
            .into_iter()
            .map(|ix| {
                let item = &self.items[ix];
                let title = item
                    .actor
                    .as_ref()
                    .and_then(|id| self.fixtures.user(id))
                    .map(|u| self.fixtures.name_of(u).to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| item.actor_label.clone());
                let body = match &item.reaction {
                    Some(emoji) => format!(":{emoji}: {}", item.text),
                    None => item.text.clone(),
                };
                let place = self
                    .fixtures
                    .conversation(&item.channel)
                    .map(|c| self.fixtures.conversation_label(c))
                    .unwrap_or_else(|| item.channel.to_string());
                crate::popout::PopoutActivityRow {
                    channel: item.channel.clone(),
                    thread_ts: item.thread_ts.clone(),
                    title,
                    body,
                    context: format!("{place} · {}", list_stamp(&item.ts)),
                    unread: item.unread,
                }
            })
            .collect()
    }

    /// Move keyboard selection up or down across visible Activity items (`F-SHELL-276`).
    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let vis = self.visible();
        if vis.is_empty() {
            self.selected = None;
            return;
        }
        let next = match self.selected {
            Some(pos) if forward => (pos + 1).min(vis.len() - 1),
            Some(pos) => pos.saturating_sub(1),
            None => 0,
        };
        self.selected = Some(next);
        cx.notify();
    }

    /// Open the currently keyboard-selected Activity item (`Enter`).
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let vis = self.visible();
        if let Some(pos) = self.selected {
            if let Some(&ix) = vis.get(pos) {
                self.open(ix, cx);
            }
        }
    }

    /// Snapshot refresh: the fixture workspace rebuilds its rows; a live snapshot never
    /// clobbers Slack's feed (labels re-resolve on the next remote page).
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        if self.fixtures.sample {
            self.items = fixture_items(&self.fixtures);
        }
        cx.notify();
    }

    /// A live fetch started; only the first page shows a loading state (refreshes keep the
    /// rows visible).
    pub fn start_loading(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            self.load = Load::Loading;
            cx.notify();
        }
    }

    /// Slack's page landed ([`slackless_core::Event::ActivityView`]).
    pub fn show_remote(&mut self, feed: SlackFeed, cx: &mut Context<Self>) {
        self.items = feed
            .items
            .into_iter()
            .map(|item| from_remote(item, &self.fixtures))
            .collect();
        // The page arrives newest-first; sort defensively, then cap.
        self.items.sort_by(|a, b| b.ts.cmp(&a.ts));
        self.items.truncate(MAX_ITEMS);
        self.load = Load::Idle;
        cx.notify();
    }

    /// A live fetch failed; the rows stay and the error shows until Retry.
    pub fn show_error(&mut self, text: &str, cx: &mut Context<Self>) {
        self.load = Load::Failed(text.to_owned());
        cx.notify();
    }

    /// Unread rows across every tab (the rail and dock badges).
    pub fn unread_count(&self) -> u32 {
        self.items.iter().filter(|item| item.unread).count() as u32
    }

    /// The rows the current tab and Unreads toggle show, in `items` order.
    fn visible(&self) -> Vec<usize> {
        let base = visible(&self.items, self.filter, self.unreads_only);
        if self.filter != Filter::Vips {
            return base;
        }
        let vip_settings = crate::preferences::Settings {
            vip_users: self.vip_users.clone(),
            ..Default::default()
        };
        base.into_iter()
            .filter(|&ix| {
                let item = &self.items[ix];
                let uid = item
                    .message
                    .as_ref()
                    .and_then(|m| m.user.as_ref())
                    .or(item.actor.as_ref());
                uid.is_some_and(|u| {
                    let user_obj = self.fixtures.user(u);
                    let handle = user_obj.map(|x| x.name.as_str()).unwrap_or("");
                    let display = user_obj.map(|x| x.display_name()).unwrap_or("");
                    vip_settings.is_vip(u.as_str(), handle, display)
                })
            })
            .collect()
    }

    fn toggle_read(&mut self, ix: usize, cx: &mut Context<Self>) {
        let item = &mut self.items[ix];
        item.unread = !item.unread;
        let read = item.unread;
        if let Some(entry) = item.entry.clone() {
            cx.emit(ActivityEvent::Mark {
                entries: vec![entry],
                read,
            });
        }
        cx.notify();
    }

    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        let item = &mut self.items[ix];
        let was_unread = item.unread;
        item.unread = false;
        if was_unread {
            if let Some(entry) = item.entry.clone() {
                cx.emit(ActivityEvent::Mark {
                    entries: vec![entry],
                    read: true,
                });
            }
        }
        cx.emit(ActivityEvent::Open {
            channel: item.channel.clone(),
            thread_ts: item.thread_ts.clone(),
        });
        cx.notify();
    }

    /// The row's display name: the bot's label, the author's profile, the actor, or the
    /// fixture-provided label.
    fn actor_name<'a>(&'a self, item: &'a Item) -> SharedString {
        if let Some(name) = item.message.as_ref().and_then(|m| m.username.as_deref()) {
            if !name.is_empty() {
                return SharedString::from(name.to_owned());
            }
        }
        if let Some(user) = item
            .message
            .as_ref()
            .and_then(|m| m.user.as_ref())
            .or(item.actor.as_ref())
        {
            return SharedString::from(self.fixtures.user_name(user).to_owned());
        }
        SharedString::from(item.actor_label.clone())
    }

    fn row(&self, ix: usize, item: &Item, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (dim, hover, fg, border) = (
            theme.muted_foreground,
            theme.accent,
            theme.foreground,
            theme.border,
        );
        let unread = item.unread;
        let name = self.actor_name(item);
        let avatar_key = item
            .actor
            .as_ref()
            .map(|user| user.as_str().to_owned())
            .unwrap_or_else(|| item.channel.to_string());
        let fallback = Message::default();
        let body = item
            .message
            .as_ref()
            .filter(|message| !message.text.is_empty())
            .map(|message| message.text.clone())
            .unwrap_or_else(|| item.text.clone());
        let group = SharedString::from(format!("activity-group-{ix}"));
        let is_selected = self
            .selected
            .and_then(|pos| self.visible().get(pos).copied())
            == Some(ix);
        let ring = cx.theme().ring;
        h_flex()
            .id(("activity-row", ix))
            .group(group.clone())
            .items_start()
            .gap_3()
            .px_4()
            .py_2()
            // Slack separates the items with hairlines.
            .border_b_1()
            .border_color(border)
            .when(is_selected, |row| {
                row.bg(hover).border_l_2().border_color(ring)
            })
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .child(author_avatar(
                &self.fixtures,
                item.message.as_ref().unwrap_or(&fallback),
                &avatar_key,
                &name,
                36.,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(if unread {
                                        FontWeight::BOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(dim)
                                    .child(list_stamp(&item.ts)),
                            ),
                    )
                    // Slack shows the first two lines of the message.
                    .child(
                        div()
                            .min_w_0()
                            .line_clamp(2)
                            // Inline link chips split the paragraph, so cap the height as well.
                            .max_h(px(40.))
                            .overflow_hidden()
                            .child(mrkdwn_preview(
                                &body,
                                &self.fixtures,
                                fg,
                                format!("activity-{ix}"),
                                cx,
                            )),
                    )
                    .child(context_line(item, dim, cx)),
            )
            .child(self.row_edge(ix, unread, cx))
            .on_click(cx.listener(move |this, _, _, cx| this.open(ix, cx)))
    }

    /// The row's right edge: the unread pill (or dot) and the hover action bar.
    fn row_edge(&self, ix: usize, unread: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let (badge_bg, badge_fg) = crate::theme::mention_badge(cx);
        // Slack counts an unread item as at least one.
        let count = if unread {
            self.items[ix].count.max(1)
        } else {
            self.items[ix].count
        };
        v_flex()
            .flex_none()
            .items_end()
            .gap_1()
            .child(div().mt_1().when(count > 0, |pill| {
                pill.min_w(px(18.))
                    .h(px(18.))
                    .px_1()
                    .rounded_full()
                    .bg(badge_bg)
                    .text_color(badge_fg)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(count.to_string())
            }))
            .child(
                div()
                    .opacity(0.)
                    .group_hover(SharedString::from(format!("activity-group-{ix}")), |el| {
                        el.opacity(1.)
                    })
                    .child(
                        Button::new(("activity-read", ix))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(if unread {
                                IconName::Check
                            } else {
                                IconName::Dot
                            })
                            .tooltip(if unread {
                                "Mark as read"
                            } else {
                                "Mark as unread"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_read(ix, cx);
                            })),
                    ),
            )
    }

    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, dim, accent, border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.border,
        );
        let (badge_bg, _) = crate::theme::mention_badge(cx);
        h_flex()
            .gap_1()
            .px_2()
            .border_b_1()
            .border_color(border)
            .children(Filter::ALL.map(|filter| {
                let active = filter == self.filter;
                h_flex()
                    .id(SharedString::from(format!(
                        "activity-tab-{}",
                        filter.label()
                    )))
                    .items_center()
                    .gap_1p5()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .text_color(if active { fg } else { dim })
                    .border_b_2()
                    .border_color(if active { accent } else { border })
                    .hover(|tab| tab.text_color(fg))
                    .child(filter.label())
                    .when(filter == Filter::All && self.unread_count() > 0, |tab| {
                        tab.child(div().size(px(6.)).rounded_full().bg(badge_bg))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.filter = filter;
                        cx.notify();
                    }))
            }))
    }

    /// The toolbar row under the tabs; of Slack's select / filter / search controls only
    /// the Unreads toggle does something real here, so it is the only one rendered.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, dim, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let on = self.unreads_only;
        h_flex().gap_2().px_4().py_2().child(
            h_flex()
                .id("activity-unreads")
                .items_center()
                .gap_1p5()
                .px_2p5()
                .py_1()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .border_1()
                .border_color(if on { fg } else { border })
                .text_color(if on { fg } else { dim })
                .when(on, |chip| chip.bg(theme.accent.opacity(0.35)))
                .child(Icon::new(IconName::Inbox).size(px(14.)))
                .child("Unreads")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.unreads_only = !this.unreads_only;
                    cx.notify();
                })),
        )
    }
}

impl Render for ActivityFeed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, dim, border) = (theme.foreground, theme.muted_foreground, theme.border);
        let rows: Vec<(String, Vec<usize>)> = day_groups(&self.items, &self.visible());
        let empty = rows.is_empty();
        let page = v_flex()
            .id("activity-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_2()
            .children(rows.iter().map(|(label, group)| {
                v_flex()
                    .child(
                        div()
                            .px_4()
                            .pt_3()
                            .pb_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(dim)
                            .child(label.clone()),
                    )
                    .children(group.iter().map(|&ix| self.row(ix, &self.items[ix], cx)))
            }));
        let page = match &self.load {
            Load::Loading if empty => page.child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .p_6()
                    .text_color(dim)
                    .child("Loading…"),
            ),
            Load::Failed(text) => page.child(
                h_flex()
                    .id("activity-error")
                    .mx_4()
                    .my_2()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .text_color(dim)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(format!("Could not load the activity feed: {text}")),
                    )
                    .child(
                        Button::new("activity-retry")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::RotateCw)
                            .tooltip("Try again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.emit(ActivityEvent::Retry);
                                this.start_loading(cx);
                            })),
                    ),
            ),
            _ => page,
        };

        v_flex()
            .size_full()
            .text_color(fg)
            .text_sm()
            .child(
                h_flex()
                    .px_4()
                    .pt_4()
                    .pb_1()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child("Activity"),
                    )
                    .child(
                        Button::new("activity-popout")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::ExternalLink)
                            .tooltip("Open Activity in new window")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ActivityEvent::PopOut))),
                    ),
            )
            .child(self.tabs(cx))
            .child(self.toolbar(cx))
            .child(page.when(empty && matches!(self.load, Load::Idle), |list| {
                list.child(
                    v_flex()
                        .items_center()
                        .gap_2()
                        .p_6()
                        .text_color(dim)
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .text_color(dim)
                                .size(px(28.)),
                        )
                        .child("You're all caught up"),
                )
            }))
    }
}

/// The context line under a row's body, Slack's wording per kind.
fn context_line(item: &Item, dim: Hsla, cx: &App) -> impl IntoElement {
    let row = h_flex().items_center().gap_1().text_xs().text_color(dim);
    match &item.context {
        RowContext::Mention { label, thread } => row.child(format!(
            "@ Mention in {label}{}",
            thread.then_some(" thread").unwrap_or_default()
        )),
        RowContext::Thread { label } => row.child(format!("Thread in {label}")),
        RowContext::Reaction => {
            let glyph = item
                .reaction
                .as_deref()
                .and_then(emoji::glyph)
                .map(str::to_owned)
                .or_else(|| item.reaction.clone().map(|name| format!(":{name}:")))
                .unwrap_or_default();
            row.child("Reacted")
                .child(
                    div()
                        .px_1p5()
                        .rounded_md()
                        .bg(cx.theme().accent.opacity(0.4))
                        .child(glyph),
                )
                .child("to your message")
        }
        RowContext::Dm => row.child("Direct message"),
        RowContext::App => row.child("App"),
        RowContext::Channel { label } => row.child(label.clone()),
    }
}

/// The rows the tab and Unreads toggle show, in `items` order.
fn visible(items: &[Item], filter: Filter, unreads_only: bool) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| filter.admits(item.kind) && (!unreads_only || item.unread))
        .map(|(ix, _)| ix)
        .collect()
}

/// Groups the visible rows under day headers, newest first: each `(label, rows)` pair is one
/// day of the page.
fn day_groups(items: &[Item], visible: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for &ix in visible {
        let label = day_label(&items[ix].ts);
        match groups.last_mut() {
            Some((known, rows)) if *known == label => rows.push(ix),
            _ => groups.push((label, vec![ix])),
        }
    }
    groups
}

/// A conversation's sidebar label, or its id when the snapshot does not know it yet.
fn channel_label(fixtures: &Fixtures, channel: &ChannelId) -> String {
    fixtures
        .conversation(channel)
        .map(|conversation| fixtures.conversation_label(conversation))
        .unwrap_or_else(|| channel.to_string())
}

/// One of Slack's rows, ready to render.
fn from_remote(item: ActivityItem, fixtures: &Fixtures) -> Item {
    let label = channel_label(fixtures, &item.channel);
    let context = match item.kind {
        ActivityKind::Mention => RowContext::Mention {
            label,
            thread: item.thread_ts.is_some(),
        },
        ActivityKind::Thread => RowContext::Thread { label },
        ActivityKind::Reaction => RowContext::Reaction,
        ActivityKind::Dm => RowContext::Dm,
        ActivityKind::App => RowContext::App,
        ActivityKind::Channel | ActivityKind::Unknown => RowContext::Channel { label },
    };
    let reaction = item.reaction.as_ref().map(|reaction| reaction.name.clone());
    let entry = (!item.key.is_empty()).then(|| item.entry());
    Item {
        kind: item.kind,
        channel: item.channel,
        thread_ts: item.thread_ts,
        ts: item.ts,
        entry,
        actor: item.actor,
        actor_label: String::new(),
        reaction,
        context,
        text: String::new(),
        unread: item.unread,
        count: item.count,
        message: item.message,
    }
}

/// Derives the fixture rows (see the module docs).
fn fixture_items(fixtures: &Fixtures) -> Vec<Item> {
    let me = &fixtures.self_user;
    let name = |message: &Message| {
        message
            .user
            .as_ref()
            .map(|user| fixtures.user_name(user).to_owned())
            .unwrap_or_default()
    };
    let mut items = Vec::new();
    let conversations = fixtures.channels.iter().chain(&fixtures.dms);

    let mut reactions = Vec::new();
    for conversation in conversations {
        let label = fixtures.conversation_label(conversation);
        let history = fixtures
            .messages
            .get(&conversation.id)
            .into_iter()
            .flatten();
        for message in history {
            let from_me = message.user.as_ref() == Some(me);
            if !from_me && mentions_user(message, me) {
                let unread = conversation
                    .last_read
                    .as_ref()
                    .is_none_or(|read| message.ts > *read);
                items.push(Item {
                    kind: ActivityKind::Mention,
                    channel: conversation.id.clone(),
                    thread_ts: None,
                    ts: message.ts.clone(),
                    entry: None,
                    actor: message.user.clone(),
                    actor_label: name(message),
                    reaction: None,
                    context: RowContext::Mention {
                        label: label.clone(),
                        thread: false,
                    },
                    text: fixtures.preview(message),
                    unread,
                    count: 0,
                    message: Some(message.clone()),
                });
            }
            let reactors: Vec<&UserId> = message
                .reactions
                .iter()
                .flat_map(|reaction| reaction.users.iter())
                .filter(|user| **user != *me)
                .collect();
            if from_me && !reactors.is_empty() {
                let newest = message
                    .reactions
                    .last()
                    .map(|reaction| reaction.name.as_str())
                    .unwrap_or("");
                let mut names: Vec<String> = Vec::new();
                for user in &reactors {
                    let reactor = fixtures.user_name(user).to_owned();
                    if !names.contains(&reactor) {
                        names.push(reactor);
                    }
                }
                reactions.push(Item {
                    kind: ActivityKind::Reaction,
                    channel: conversation.id.clone(),
                    thread_ts: None,
                    ts: message.ts.clone(),
                    entry: None,
                    actor: reactors.first().map(|user| (*user).clone()),
                    actor_label: names.join(", "),
                    reaction: Some(newest.to_owned()),
                    context: RowContext::Reaction,
                    text: fixtures.preview(message),
                    unread: false,
                    count: 0,
                    message: Some(message.clone()),
                });
            }
        }
    }
    reactions.sort_by(|a, b| b.ts.cmp(&a.ts));
    for item in reactions.iter_mut().take(UNREAD_REACTIONS) {
        item.unread = true;
    }
    items.extend(reactions);

    for ((channel, root_ts), replies) in &fixtures.threads {
        let root = fixtures
            .messages
            .get(channel)
            .and_then(|history| history.iter().find(|message| message.ts == *root_ts));
        let involved = root.is_some_and(|root| root.user.as_ref() == Some(me))
            || replies.iter().any(|reply| reply.user.as_ref() == Some(me));
        let Some(latest) = replies.iter().max_by(|a, b| a.ts.cmp(&b.ts)) else {
            continue;
        };
        if !involved || latest.user.as_ref() == Some(me) {
            continue;
        }
        let label = channel_label(fixtures, channel);
        items.push(Item {
            kind: ActivityKind::Thread,
            channel: channel.clone(),
            thread_ts: Some(root_ts.clone()),
            ts: latest.ts.clone(),
            entry: None,
            actor: latest.user.clone(),
            actor_label: name(latest),
            reaction: None,
            context: RowContext::Thread { label },
            text: fixtures.preview(latest),
            unread: true,
            count: 0,
            message: Some(latest.clone()),
        });
    }
    items.sort_by(|a, b| b.ts.cmp(&a.ts));
    items.truncate(MAX_ITEMS);
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: ActivityKind, ts: &str, unread: bool) -> Item {
        Item {
            kind,
            channel: ChannelId::from("C1"),
            thread_ts: None,
            ts: Ts::from(ts),
            entry: None,
            actor: None,
            actor_label: String::new(),
            reaction: None,
            context: RowContext::Dm,
            text: String::new(),
            unread,
            count: 0,
            message: None,
        }
    }

    /// 2026-11-19 and 2026-11-20, mid-day UTC (two calendar days in every local zone a
    /// test runner plausibly uses).
    const DAY_ONE: &str = "1795089600.000100";
    const DAY_ONE_LATER: &str = "1795093200.000200";
    const DAY_TWO: &str = "1795176000.000300";

    #[test]
    fn tabs_and_unreads_filter_the_rows() {
        let items = vec![
            item(ActivityKind::Mention, DAY_ONE, true),
            item(ActivityKind::Dm, DAY_ONE_LATER, false),
            item(ActivityKind::App, DAY_TWO, true),
            item(ActivityKind::Thread, DAY_TWO, false),
            item(ActivityKind::Reaction, DAY_TWO, false),
            item(ActivityKind::Channel, DAY_TWO, false),
        ];
        let rows = |filter, unreads_only| {
            visible(&items, filter, unreads_only)
                .into_iter()
                .map(|ix| items[ix].kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rows(Filter::All, false),
            vec![
                ActivityKind::Mention,
                ActivityKind::Dm,
                ActivityKind::App,
                ActivityKind::Thread,
                ActivityKind::Reaction,
                ActivityKind::Channel,
            ]
        );
        assert_eq!(
            rows(Filter::Dms, false),
            vec![ActivityKind::Dm, ActivityKind::App],
            "app DMs fold into the DMs tab"
        );
        assert_eq!(rows(Filter::Mentions, false), vec![ActivityKind::Mention]);
        assert_eq!(rows(Filter::Threads, false), vec![ActivityKind::Thread]);
        // Reactions and channel activity only show under All.
        assert_eq!(rows(Filter::Dms, true), vec![ActivityKind::App]);
        assert_eq!(
            rows(Filter::All, true),
            vec![ActivityKind::Mention, ActivityKind::App],
            "the Unreads toggle keeps only unread rows"
        );
    }

    #[test]
    fn rows_group_under_day_headers_in_order() {
        let items = vec![
            item(ActivityKind::Dm, DAY_TWO, false),
            item(ActivityKind::Mention, DAY_ONE_LATER, false),
            item(ActivityKind::Reaction, DAY_ONE, false),
        ];
        let visible = visible(&items, Filter::All, false);
        let groups = day_groups(&items, &visible);
        assert_eq!(groups.len(), 2, "two calendar days");
        assert_eq!(groups[0].1, vec![0], "the newest day first");
        assert_eq!(groups[1].1, vec![1, 2], "same-day rows share a header");
    }
}
