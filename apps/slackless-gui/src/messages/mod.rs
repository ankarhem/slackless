//! Message column: a bottom-anchored virtualized transcript (gpui-kit-guide section 4) with
//! date dividers, a new-messages divider, author grouping, reaction chips, and rich text
//! rendered to styled `TextRun`s with clickable link ranges and inline @mention chips (section 10).
//! App-authored Block Kit blocks and legacy attachments render through [`crate::blockkit`];
//! mrkdwn (fallback text, Block Kit text objects) through [`mrkdwn`].
//!
//! Message text participates in the window text selection (gpui-base: the root mounts the
//! selection layer): each paragraph is one [`SelectableParagraph`], registered in
//! `ts`-derived document order so drags span paragraphs and messages and Cmd+C copies the
//! selection as plain text. Links route through [`LinkTarget`] — @mentions open profiles,
//! #channels open the channel, permalinks open in-app — see the column's link handler.
//!
//! Hovering a row reveals the shared action bar and the right-click menu from
//! [`crate::actions`]: quick reactions, add reaction (emoji picker via the root), reply in
//! thread, forward, save for later, copy link, mark unread, and in-place edit / delete of the
//! signed-in user's messages, all applied to this view's fixture store.
//!
//! Design tokens: colors and fonts come from the active gpui-kit theme via `cx.theme()`; the
//! avatar identity chips resolve through [`crate::theme::avatar_color`], Slack's fixed
//! per-user hue table (theme-owned, not view-local). Spacing rides the 4px utility scale
//! (`gap_1`, `px_2`, `py_2`, ...); the row rhythm is the scroller's 2px gap plus dividers'
//! own padding. Typography: body at `text_sm`, meta text (timestamps, labels, counts) at
//! `text_xs`.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonRounded};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, Bounds, Context, Div, Entity, EventEmitter,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
    Render, SharedString, Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled as _,
    Subscription, WeakEntity, Window, div, px, size,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{Block, ChannelId, Message, MessageSubtype, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MessageAction, MessageChange};
use crate::blockkit::{self, BlockKitState, KitCx};
use crate::composer::{Composer, ComposerEvent};
use crate::emoji::EmojiChoice;
use crate::fixtures::Fixtures;
use crate::hover_card::{self, Trigger};
use crate::image_viewer::{self, ImageClick, ImageOrigin};
use crate::later::SaveRequest;

mod code_block;
mod render;
mod rows;
mod selectable;
mod time;
pub(crate) use render::is_deleted_ghost;
use render::{
    CivilTime, card_stamp, date_label, day_index, format_clock, format_clock_with, message_body,
};
pub(crate) use render::{
    clock, day_and_time, day_label, expand_shortcodes_in_text, file_day, hex_color, iso_date,
    list_stamp, local_offset_now, mrkdwn, mrkdwn_preview, mrkdwn_with_verbatim, now_secs,
    render_block, selectable_code, short_date, unescape,
};
use rows::transcript_row;
pub use rows::{Jump, message_row, message_row_linked};
pub(crate) use rows::{editing_avatar, reply_bar, same_group};
pub use time::set_timezone_override;
pub(crate) use time::{
    day_and_time_with, default_custom_datetime, local_utc_offset, parse_local_schedule,
};
#[cfg(test)]
mod tests;

/// Avatar chip size in px (Clean density).
const AVATAR_SIZE: f32 = 36.;
/// Avatar chip size in px (Compact density).
const COMPACT_AVATAR_SIZE: f32 = 28.;

pub(super) fn message_avatar_size(compact: bool) -> f32 {
    if compact {
        COMPACT_AVATAR_SIZE
    } else {
        AVATAR_SIZE
    }
}
/// How far Slack sets a message's avatar below the top of its header line (px): the avatar's
/// top sits a few px above the name's capitals, 8px into the row (measured, channel and
/// thread alike).
const AVATAR_TOP: f32 = 5.;
/// Horizontal gutter around the transcript (px); overrides the scroller's 12px default.
const GUTTER: f32 = 16.;
/// Space the scroller inserts between consecutive rows (px); dividers add their own padding.
const ROW_GAP: f32 = 2.;
/// Seconds after which consecutive messages by one author render as separate cards.
const GROUP_WINDOW_SECS: i64 = 300;
/// Inline custom-emoji image size in a text line (px).
pub(super) const INLINE_EMOJI: f32 = 18.;
/// The room an app chip's text leaves for its icon: five no-break spaces, about the icon and
/// Slack's gap after it at body size (an em space measured narrower in Lato).
pub(super) const APP_ICON_SLOT: &str = "\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}";
/// An app chip's icon size (px).
pub(super) const APP_ICON: f32 = 13.;
/// Rows from the top within which scrolling asks for the next older page: close enough that
/// the page lands before the reader reaches the edge.
const NEAR_TOP_ROWS: usize = 2;

pub(super) const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

// ---------------------------------------------------------------------------
// Links in message text (Slack's in-app navigation): message bodies route every link
// through their column's [`LinkHandler`]; the root resolves profiles, channels, threads
// and Slack permalinks in the app and opens the rest in the browser.
// ---------------------------------------------------------------------------

/// Where a link inside message text leads.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    /// A URL: opened in the browser, or in-app when it is a Slack permalink.
    Url(SharedString),
    /// An @user mention: that user's profile.
    Profile(UserId),
    /// A #channel mention: the channel.
    Channel(ChannelId),
    /// A thread reference (the "replied to a thread" line of a broadcast).
    Thread { channel: ChannelId, thread_ts: Ts },
}

/// Routes one link activation out of a message body.
pub type LinkHandler = Rc<dyn Fn(LinkTarget, &mut Window, &mut App)>;

/// What a link does without a column handler to route it (previews, static cards):
/// URLs open in the browser, in-app targets do nothing.
pub(super) fn open_link_default(target: LinkTarget, cx: &mut App) {
    if let LinkTarget::Url(url) = target {
        cx.open_url(url.as_ref());
    }
}

/// Activate `target` through `handler`, or the default when there is none.
pub(crate) fn route_link(
    handler: Option<&LinkHandler>,
    target: LinkTarget,
    window: &mut Window,
    cx: &mut App,
) {
    match handler {
        Some(handler) => handler(target, window, cx),
        None => open_link_default(target, cx),
    }
}

/// Paragraph slots each text theme of one message gets inside the message's selection
/// document order (see [`message_order`]).
pub(crate) const THEME_ORDER_STRIDE: u64 = 4096;

/// Selection document order of one message: its `ts` in milliseconds, shifted up to leave
/// room for the paragraphs inside the message. The transcript reads top-down in
/// chronological order, so `ts` order is reading order; the shift keeps paragraphs of
/// adjacent messages from interleaving. See [`SelectableParagraph`].
use crate::threads_cards::ts_just_before;
pub(crate) use render::unescape_cow;

pub(crate) fn message_order(ts: &Ts) -> u64 {
    /// `ts` (`secs.micros`) as milliseconds.
    fn millis(ts: &Ts) -> Option<u64> {
        let (secs, fraction) = ts.as_str().split_once('.')?;
        let millis: u64 = fraction.get(..3)?.parse().ok()?;
        secs.parse::<u64>()
            .ok()?
            .checked_mul(1000)?
            .checked_add(millis)
    }
    millis(ts).unwrap_or(0) << 20
}

pub enum MessageListEvent {
    OpenThread {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// An add-reaction control asked for the emoji picker at `position` (window coordinates);
    /// the root routes the pick back through [`MessageList::add_reaction`].
    PickReaction {
        channel: ChannelId,
        ts: Ts,
        position: Point<Pixels>,
    },
    /// The inline edit composer's emoji button asked for the picker; picks come back through
    /// [`MessageList::insert_edit_emoji`].
    PickEditEmoji(Point<Pixels>),
    /// Save for later / Remind me / unsave; the root keeps the Later list.
    Later(SaveRequest),
    /// The user reacted to, edited or deleted a message in `channel` (already shown here);
    /// the root posts the change to Slack.
    Changed {
        channel: ChannelId,
        change: MessageChange,
    },
    /// The transcript scrolled to (near) the top of `channel`; the root reads the next older
    /// page from the store, strictly older than `before` (the oldest message shown). Emitted
    /// at most once per oldest message until the window refreshes.
    NeedOlder {
        channel: ChannelId,
        before: Ts,
    },
    /// A message's avatar or name was clicked: show that user's profile.
    OpenProfile(UserId),
    /// A link in message text was clicked: profile, channel, thread or URL (permalinks
    /// resolved in-app by the root).
    OpenLink(LinkTarget),
    /// A message image (a file upload, a Block Kit image, an attachment image) was
    /// clicked: the root opens the image viewer over `images`, at `index`.
    OpenImage {
        channel: ChannelId,
        images: Rc<Vec<image_viewer::ViewerImage>>,
        index: usize,
    },
    /// Forward this message (its permalink): the root opens New message with it drafted.
    Forward(String),
    /// Join the live huddle from an inline `huddle_thread` card (`F-COLLAB-003`).
    JoinHuddle(ChannelId),
    /// Open the local SQLite revision history inspector for `(channel, ts)`.
    ViewHistory {
        channel: ChannelId,
        ts: Ts,
    },
    /// Add this message to a Slack List (`lists.addToList`, `F-COLLAB-231`).
    AddToList {
        channel: ChannelId,
        ts: Ts,
    },
    /// "Jump to…" needs history older than the loaded window: the root pages it in until it
    /// reaches `since` (unix seconds), then calls [`MessageList::finish_jump`].
    NeedHistorySince {
        channel: ChannelId,
        since: i64,
    },
}

/// Snapshot the `'static` virtual-list row renderer reads: the current conversation's history
/// plus its read cursor and the row-level UI state. [`MessageList`] writes it; the row
/// renderer reads it during paint.
struct MessageData {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    /// Oldest first, hidden events filtered out.
    messages: Vec<Message>,
    /// The conversation's read cursor; the first message past it gets the new-messages divider.
    last_read: Option<Ts>,
    /// Precomputed index of the first unread message from someone else (O(1) lookup per row).
    first_unread_ix: Option<usize>,
    /// Interactive Block Kit control state (selections, checks, input drafts).
    kit: Entity<BlockKitState>,
    /// Messages saved for later (fixture mode: in memory).
    saved: HashSet<Ts>,
    /// Per-message expand/collapse overrides for locally preserved deleted messages (XORed with
    /// `DisplayPrefs::show_deleted_messages`).
    toggled_deleted: HashSet<Ts>,
    /// The hovered message (Slack's one-key target while no field holds focus).
    active: Option<Ts>,
    /// Root of the thread open beside the list, tinted while it shows.
    open_thread: Option<Ts>,
    /// Message briefly highlighted after jumping to it (from search, Later, pins, or links).
    highlighted: Option<Ts>,
    /// The message being edited in place; its WYSIWYG draft is `edit_composer`.
    editing: Option<Ts>,
    edit_composer: Entity<Composer>,
}

impl MessageData {
    fn recompute_first_unread(&mut self) {
        self.first_unread_ix = self.last_read.as_ref().and_then(|read| {
            self.messages
                .iter()
                .position(|m| &m.ts > read && m.user.as_ref() != Some(&self.fixtures.self_user))
        });
    }
}

/// Collect remote non-avatar attachment/thumbnail URLs from `messages` so switching channels can
/// evict decoded channel media from GPUI's texture atlas without touching avatars or emoji.
fn collect_channel_media_urls(messages: &[Message]) -> Vec<String> {
    let mut urls = Vec::new();
    for m in messages {
        for f in &m.files {
            for candidate in [
                f.thumb_720.as_deref(),
                f.thumb_480.as_deref(),
                f.thumb_360.as_deref(),
                f.thumb_160.as_deref(),
                f.url_private.as_deref(),
            ]
            .into_iter()
            .flatten()
            {
                if candidate.starts_with("https://") {
                    urls.push(candidate.to_owned());
                }
            }
        }
        for att in &m.attachments {
            for candidate in [att.image_url.as_deref(), att.thumb_url.as_deref()]
                .into_iter()
                .flatten()
            {
                if candidate.starts_with("https://") {
                    urls.push(candidate.to_owned());
                }
            }
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

/// Maximum number of recently visited channels kept warm in [`MessageList`] so switching
/// back to a recent channel renders on frame 0 without flashing an empty transcript while
/// SQLite refreshes in the background.
const HISTORY_CACHE_CHANNELS: usize = 4;

pub struct MessageList {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    /// Per-conversation history, oldest first (fixture history or bounded recent-channel LRU).
    messages: HashMap<ChannelId, Vec<Message>>,
    /// Most-recently-used order for live cached channels (newest at the back).
    history_lru: Vec<ChannelId>,
    /// Projection of the current conversation that the row renderer reads.
    data: Entity<MessageData>,
    scroller: Entity<MessageScrollerState>,
    /// Oldest message a [`MessageListEvent::NeedOlder`] covered (in flight or landed); a
    /// window refresh forgets it so the top of the list re-asks.
    requested_older: Option<Ts>,
    /// A "Jump to…" target (unix seconds) waiting for older history to land.
    pending_jump: Option<i64>,
    /// Target message to highlight once a pending jump finishes loading.
    pending_highlight: Option<Ts>,
    /// Whether the current conversation still owes its one-time initial scroll to the first
    /// unread message (`mark_read == 0`). Cleared once scrolled or as soon as the user scrolls,
    /// so background backfill pages (`Event::StoreChanged`) never yank the reader back.
    pending_initial_unread_jump: bool,
    /// Timer that clears the jump-to highlight after a short flash.
    highlight_clear: gpui_kit::gpui::Task<()>,
    /// Non-avatar image URLs from the previously viewed channel queued for GPU texture eviction
    /// on the next `Render::render` (keeps long multi-channel sessions flat in RAM/VRAM).
    pending_evict_urls: Vec<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<MessageListEvent> for MessageList {}

impl MessageList {
    pub fn new(
        fixtures: Arc<Fixtures>,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<MessageList> {
        cx.new(|cx| {
            let messages = fixtures.messages.clone();
            let history = history_of(&messages, &channel);
            let count = history.len();
            let kit = cx.new(|_| BlockKitState::default());
            let edit_composer = Composer::new_edit(fixtures.clone(), window, cx);
            // Constructor enables FollowMode::Tail: the transcript starts at the bottom and
            // follows new content.
            let scroller = cx.new(|cx| MessageScrollerState::new(count, cx));
            // Control state and the edit composer live in their own entities: repaint the rows
            // that show them when either changes; the scroller is watched to page older
            // history in when the reader reaches the top.
            let subscriptions = vec![
                cx.observe(&kit, |_, _, cx| cx.notify()),
                cx.subscribe(
                    &edit_composer,
                    |list: &mut MessageList, _, event: &ComposerEvent, cx| match event {
                        ComposerEvent::Submit(content) => list.save_edit(content.clone(), cx),
                        ComposerEvent::Cancel => list.cancel_edit(cx),
                        ComposerEvent::OpenEmojiPicker(position) => {
                            cx.emit(MessageListEvent::PickEditEmoji(*position));
                        }
                        ComposerEvent::OpenGifPicker(_)
                        | ComposerEvent::SlashCommand(_)
                        | ComposerEvent::SubmitWithFiles { .. }
                        | ComposerEvent::Schedule { .. }
                        | ComposerEvent::ScheduleCustom(_)
                        | ComposerEvent::EditLast
                        | ComposerEvent::Typing => {}
                    },
                ),
                cx.observe(&scroller, |list: &mut MessageList, scroller, cx| {
                    if !scroller.read(cx).is_following_tail() {
                        list.pending_initial_unread_jump = false;
                    }
                    list.request_older_near_top(&scroller, cx)
                }),
            ];
            let data = cx.new(|_| {
                let mut data = MessageData {
                    fixtures: fixtures.clone(),
                    channel: channel.clone(),
                    messages: history,
                    last_read: fixtures.last_read(&channel).cloned(),
                    first_unread_ix: None,
                    kit,
                    saved: HashSet::new(),
                    toggled_deleted: HashSet::new(),
                    active: None,
                    open_thread: None,
                    highlighted: None,
                    editing: None,
                    edit_composer,
                };
                data.recompute_first_unread();
                data
            });
            MessageList {
                fixtures,
                channel,
                messages,
                history_lru: Vec::new(),
                data,
                scroller,
                requested_older: None,
                pending_jump: None,
                pending_highlight: None,
                pending_initial_unread_jump: false,
                highlight_clear: gpui_kit::gpui::Task::ready(()),
                pending_evict_urls: Vec::new(),
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: draw names, emoji and conversations from `fixtures`; the loaded history,
    /// scroll position and new-messages divider stay.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        let edit_composer = self.data.read(cx).edit_composer.clone();
        edit_composer.update(cx, |composer, cx| {
            composer.set_fixtures(fixtures.clone(), cx)
        });
        self.data.update(cx, |data, cx| {
            data.fixtures = fixtures.clone();
            cx.notify();
        });
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Store dynamically fetched `blocks.suggestions` options for an `external_select` control.
    pub fn set_block_suggestions(
        &mut self,
        key: String,
        options: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let kit = self.data.read(cx).kit.clone();
        kit.update(cx, |kit, cx| {
            kit.set_suggestions(key, options);
            cx.notify();
        });
        cx.notify();
    }

    /// Forward the Enter-to-send preference to the inline edit composer.
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        let edit_composer = self.data.read(cx).edit_composer.clone();
        edit_composer.update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
    }

    /// Insert a picked emoji into the inline edit composer.
    pub fn insert_edit_emoji(
        &mut self,
        choice: &EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let edit_composer = self.data.read(cx).edit_composer.clone();
        edit_composer.update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
    }

    fn touch_cache(&mut self, channel: &ChannelId) {
        if self.fixtures.sample {
            return;
        }
        self.history_lru.retain(|c| c != channel);
        self.history_lru.push(channel.clone());
        while self.history_lru.len() > HISTORY_CACHE_CHANNELS {
            let oldest = self.history_lru.remove(0);
            if oldest != self.channel {
                self.messages.remove(&oldest);
            }
        }
    }

    /// Return the newest `huddle_thread` message ts in the current conversation (falling back
    /// to the newest message in sample mode) so the huddle bar can open its thread (`F-COLLAB-014`).
    pub fn latest_huddle_thread_ts(&self, sample_fallback: bool, cx: &App) -> Option<Ts> {
        let messages = &self.data.read(cx).messages;
        messages
            .iter()
            .rev()
            .find(|m| m.subtype == Some(slackless_core::types::MessageSubtype::HuddleThread))
            .or_else(|| sample_fallback.then(|| messages.last()).flatten())
            .map(|m| m.ts.clone())
    }

    /// Look up an already-loaded message by timestamp (used to seed `ThreadPanel` on frame 0).
    pub fn message_by_ts(&self, channel: &ChannelId, ts: &Ts, cx: &App) -> Option<Message> {
        if *channel == self.channel
            && let Some(m) = self.data.read(cx).messages.iter().find(|m| &m.ts == ts)
        {
            return Some(m.clone());
        }
        self.messages
            .get(channel)
            .and_then(|history| history.iter().find(|m| &m.ts == ts))
            .cloned()
    }

    pub fn set_conversation(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        if self.channel != channel {
            let old_urls = collect_channel_media_urls(&self.data.read(cx).messages);
            if !old_urls.is_empty() {
                self.pending_evict_urls.extend(old_urls);
            }
            self.data.update(cx, |data, _| data.toggled_deleted.clear());
        }
        self.channel = channel.clone();
        self.touch_cache(&channel);
        self.pending_initial_unread_jump = true;
        self.sync(cx);
    }

    /// Toggle whether a single locally preserved deleted message is expanded or collapsed.
    pub fn toggle_deleted(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| {
            if !data.toggled_deleted.remove(ts) {
                data.toggled_deleted.insert(ts.clone());
            }
        });
        self.refresh(cx);
    }

    /// Clear per-message deleted overrides when the global preference changes so every deleted
    /// message follows the newly chosen default immediately.
    pub fn reset_deleted_overrides(&mut self, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| data.toggled_deleted.clear());
        self.refresh(cx);
    }

    /// Move the visible transcript's `"New"` divider cursor (e.g. pressing Esc clears it).
    pub fn set_last_read(&mut self, last_read: Option<Ts>, cx: &mut Context<Self>) {
        let changed = self.data.update(cx, |data, _| {
            if data.last_read != last_read {
                data.last_read = last_read;
                data.recompute_first_unread();
                true
            } else {
                false
            }
        });
        if changed {
            self.refresh(cx);
        }
    }

    /// Append a message to `channel`'s history (live event or optimistic local send). Appends
    /// to the visible conversation also advance the virtual list by one row, tail-anchored.
    pub fn append(&mut self, channel: ChannelId, message: Message, cx: &mut Context<Self>) {
        let visible = channel == self.channel && !message.hidden;
        if visible {
            self.data.update(cx, |data, _| {
                data.messages.push(message.clone());
                data.recompute_first_unread();
            });
        }
        self.messages.entry(channel).or_default().push(message);
        if visible {
            self.scroller.update(cx, |scroller, cx| {
                scroller.append(1, cx);
            });
        }
        cx.notify();
    }

    /// Replace `channel`'s history (live mode: re-read from the store after it changed). An
    /// unchanged history is a no-op; one that only grew keeps the scroll position. Pages
    /// older than the refreshed window (loaded by [`MessageList::prepend_history`]) survive
    /// the refresh, so scrolling back does not reload what the reader already paged in.
    pub fn set_history(
        &mut self,
        channel: ChannelId,
        history: Vec<Message>,
        cx: &mut Context<Self>,
    ) {
        // A refresh may overtake an in-flight older-page read; forget the request so the top
        // of the list re-asks.
        self.requested_older = None;
        let merged = match (
            self.messages.remove(&channel),
            history.first().map(|message| message.ts.clone()),
        ) {
            (Some(mut stored), Some(oldest)) => {
                let pages = stored
                    .iter()
                    .take_while(|message| message.ts < oldest)
                    .count();
                stored.truncate(pages);
                stored.extend(history);
                stored
            }
            _ => history,
        };
        let visible = channel == self.channel;
        self.messages.insert(channel.clone(), merged);
        self.touch_cache(&channel);
        if !visible {
            return;
        }
        let history = history_of(&self.messages, &self.channel);
        let (unchanged, prefix, tail_added, preserves_window) = {
            let shown = &self.data.read(cx).messages;
            if *shown == history {
                (true, 0, 0, true)
            } else if let Some(first_shown) = shown.first() {
                let prefix = history.iter().take_while(|m| m.ts < first_shown.ts).count();
                let matches_start = history.get(prefix).is_some_and(|m| m.ts == first_shown.ts);
                let preserves = matches_start && history.len() >= prefix + shown.len();
                let tail_added = (history.len().saturating_sub(prefix)).saturating_sub(shown.len());
                (false, prefix, tail_added, preserves)
            } else {
                (false, 0, 0, false)
            }
        };
        if unchanged {
            return;
        }
        if !preserves_window {
            return self.sync(cx);
        }
        self.data.update(cx, |data, _| {
            data.messages = history;
            data.recompute_first_unread();
        });
        self.scroller.update(cx, |scroller, cx| {
            if prefix > 0 {
                scroller.prepend(prefix, cx);
            }
            if tail_added > 0 {
                scroller.append(tail_added, cx);
            } else if prefix == 0 {
                scroller.remeasure(cx);
            }
        });
        cx.notify();
    }

    /// Prepend a page of `older` history to `channel` (a [`MessageListEvent::NeedOlder`]
    /// read, oldest first), keeping the reader's place: the scroller anchors to its top row,
    /// which shifts down by the rows added above it. Messages already stored are skipped, so
    /// a repeated or out-of-order page cannot reorder the list.
    pub fn prepend_history(
        &mut self,
        channel: ChannelId,
        older: Vec<Message>,
        cx: &mut Context<Self>,
    ) {
        let Some(history) = self.messages.get_mut(&channel) else {
            return;
        };
        for message in older {
            let at = history.partition_point(|shown| shown.ts < message.ts);
            if history.get(at).is_some_and(|shown| shown.ts == message.ts) {
                continue;
            }
            history.insert(at, message);
        }
        if channel != self.channel {
            return;
        }
        let shown = self.data.read(cx).messages.len();
        let history = history_of(&self.messages, &self.channel);
        let added = history.len().saturating_sub(shown);
        if added == 0 {
            return;
        }
        self.data.update(cx, |data, _| {
            data.messages = history;
            data.recompute_first_unread();
        });
        self.scroller
            .update(cx, |scroller, cx| scroller.prepend(added, cx));
        cx.notify();
    }

    /// Add the signed-in user's `name` reaction to `ts` in `channel` (emoji picker pick).
    pub fn add_reaction(
        &mut self,
        channel: &ChannelId,
        ts: &Ts,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        let me = self.fixtures.self_user.clone();
        let message = self
            .messages
            .get_mut(channel)
            .and_then(|history| history.iter_mut().find(|message| &message.ts == ts));
        if message.is_some_and(|message| actions::add_reaction(message, &me, name)) {
            cx.emit(MessageListEvent::Changed {
                channel: channel.clone(),
                change: MessageChange::React {
                    ts: ts.clone(),
                    name: name.to_owned(),
                    add: true,
                },
            });
        }
        if *channel == self.channel {
            self.refresh(cx);
        }
    }

    /// Rebuild the render snapshot for the current conversation and reset the list onto it.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.requested_older = None;
        let history = history_of(&self.messages, &self.channel);
        let last_read = self.fixtures.last_read(&self.channel).cloned();
        let count = history.len();
        self.data.update(cx, |data, _| {
            let switching = data.channel != self.channel;
            data.channel = self.channel.clone();
            data.messages = history;
            if switching || data.last_read.is_none() {
                data.last_read = last_read;
            }
            data.recompute_first_unread();
            data.editing = None;
            data.active = None;
        });
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(count, cx));
        cx.notify();
    }

    /// Re-project the current conversation after an in-place change: rows keep their place,
    /// heights are remeasured.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let history = history_of(&self.messages, &self.channel);
        self.data.update(cx, |data, _| {
            data.messages = history;
            data.recompute_first_unread();
        });
        self.scroller
            .update(cx, |scroller, cx| scroller.remeasure(cx));
        cx.notify();
    }

    /// The scroller moved: away from the live tail and near the top, ask the root for the
    /// next older page, once per oldest message. A list still following its tail (freshly
    /// reset, or shorter than the viewport) never asks.
    fn request_older_near_top(
        &mut self,
        scroller: &Entity<MessageScrollerState>,
        cx: &mut Context<Self>,
    ) {
        let near_top = {
            let state = scroller.read(cx);
            !state.is_following_tail() && state.logical_scroll_top().item_ix <= NEAR_TOP_ROWS
        };
        if !near_top {
            return;
        }
        let Some(before) = self.data.read(cx).messages.first().map(|m| m.ts.clone()) else {
            return;
        };
        if self.requested_older.as_ref() == Some(&before) {
            return;
        }
        self.requested_older = Some(before.clone());
        cx.emit(MessageListEvent::NeedOlder {
            channel: self.channel.clone(),
            before,
        });
    }

    fn current_mut(&mut self, ts: &Ts) -> Option<&mut Message> {
        self.messages
            .get_mut(&self.channel)?
            .iter_mut()
            .find(|message| &message.ts == ts)
    }

    pub fn run_action(
        &mut self,
        ts: &Ts,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = self.fixtures.self_user.clone();
        let channel = self.channel.clone();
        match action {
            MessageAction::React(name) => self.add_reaction(&channel, ts, &name, cx),
            MessageAction::ToggleReaction(name) => {
                if let Some(message) = self.current_mut(ts) {
                    let add = actions::toggle_reaction(message, &me, &name);
                    cx.emit(MessageListEvent::Changed {
                        channel,
                        change: MessageChange::React {
                            ts: ts.clone(),
                            name,
                            add,
                        },
                    });
                }
                self.refresh(cx);
            }
            MessageAction::PickReaction(position) => {
                cx.emit(MessageListEvent::PickReaction {
                    channel,
                    ts: ts.clone(),
                    position,
                });
            }
            MessageAction::ReplyInThread => {
                let thread_ts = self
                    .current_mut(ts)
                    .and_then(|message| message.thread_ts.clone())
                    .unwrap_or_else(|| ts.clone());
                cx.emit(MessageListEvent::OpenThread { channel, thread_ts });
            }
            MessageAction::CopyLink => actions::copy_link(&self.fixtures, &channel, ts, window, cx),
            MessageAction::CopyText => {
                let message = self
                    .data
                    .read(cx)
                    .messages
                    .iter()
                    .find(|m| &m.ts == ts)
                    .cloned();
                if let Some(message) = message {
                    actions::copy_message_text(&self.fixtures, &message, window, cx);
                }
            }
            MessageAction::Forward => cx.emit(MessageListEvent::Forward(actions::permalink(
                &self.fixtures,
                &self.channel,
                ts,
            ))),
            MessageAction::SaveForLater => {
                let saved = !self.data.read(cx).saved.contains(ts);
                let title = if saved {
                    "Saved for later"
                } else {
                    "Removed from Later"
                };
                self.save_for_later(ts, saved, None, title, window, cx);
            }
            MessageAction::RemindMe(remind) => {
                let title = format!("Reminder set: {}", remind.label().to_lowercase());
                self.save_for_later(
                    ts,
                    true,
                    Some(remind.at(crate::drafts::now_secs())),
                    &title,
                    window,
                    cx,
                );
            }
            MessageAction::RemindCustom => {
                let Some(message) = self
                    .data
                    .read(cx)
                    .messages
                    .iter()
                    .find(|m| &m.ts == ts)
                    .cloned()
                else {
                    return;
                };
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::open_custom_remind_for_message(
                    &fixtures,
                    channel,
                    &message,
                    window,
                    cx,
                    move |request, when, window, cx| {
                        let _ = this.update(cx, |list, cx| {
                            actions::notify(
                                &format!("Reminder set: {when}"),
                                request.text.clone(),
                                window,
                                cx,
                            );
                            let ts = request.ts.clone();
                            cx.emit(MessageListEvent::Later(request));
                            list.set_saved(&ts, true, cx);
                        });
                    },
                );
            }
            MessageAction::MarkUnread => self.mark_unread(ts, cx),
            MessageAction::Edit => self.begin_edit(ts, window, cx),
            MessageAction::Delete => {
                let data = self.data.read(cx);
                let Some(message) = data.messages.iter().find(|m| &m.ts == ts).cloned() else {
                    return;
                };
                let fixtures = data.fixtures.clone();
                let (this, ts) = (cx.entity().downgrade(), ts.clone());
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    // Gone only if the list itself was dropped meanwhile: nothing to delete.
                    let _ = this.update(cx, |list, cx| list.delete(&ts, cx));
                });
            }
            MessageAction::TogglePin => {
                let pinned = self
                    .current_mut(ts)
                    .map(|message| actions::toggle_pin(message, &channel));
                let Some(pinned) = pinned else {
                    return;
                };
                let preview = {
                    let data = self.data.read(cx);
                    data.messages
                        .iter()
                        .find(|m| &m.ts == ts)
                        .map(|m| self.fixtures.preview(m))
                        .unwrap_or_default()
                };
                actions::notify_pin_toggled(pinned, preview, window, cx);
                cx.emit(MessageListEvent::Changed {
                    channel,
                    change: MessageChange::Pin {
                        ts: ts.clone(),
                        pinned,
                    },
                });
            }
            MessageAction::DismissEphemeral => {
                let target = ts.clone();
                if let Some(history) = self.messages.get_mut(&self.channel) {
                    history.retain(|m| m.ts != target);
                }
                self.data.update(cx, |data, cx| {
                    data.messages.retain(|m| m.ts != target);
                    data.recompute_first_unread();
                    cx.notify();
                });
                cx.notify();
            }
            MessageAction::OpenProfile(user) => cx.emit(MessageListEvent::OpenProfile(user)),
            MessageAction::OpenImage(origin) => self.open_image(ts, &origin, cx),
            MessageAction::ViewHistory => cx.emit(MessageListEvent::ViewHistory {
                channel,
                ts: ts.clone(),
            }),
            MessageAction::AddToList => cx.emit(MessageListEvent::AddToList {
                channel,
                ts: ts.clone(),
            }),
            MessageAction::SendToChannel => {}
            MessageAction::JoinHuddle => cx.emit(MessageListEvent::JoinHuddle(channel)),
            MessageAction::BlockAction { action, attachment } => {
                let live = !self.fixtures.sample;
                if let Some(message) = self.current_mut(ts).cloned()
                    && let Some(change) = actions::dispatch_block_action(
                        live, &message, &channel, action, attachment, window, cx,
                    )
                {
                    cx.emit(MessageListEvent::Changed { channel, change });
                }
            }
        }
    }

    /// The newest message authored by the signed-in user in the open conversation (for
    /// quick unsend, `F-CORE-093`).
    pub fn latest_own_ts(&self, cx: &App) -> Option<Ts> {
        let me = &self.fixtures.self_user;
        self.data
            .read(cx)
            .messages
            .iter()
            .rev()
            .find(|m| !m.is_ephemeral && m.user.as_ref() == Some(me))
            .map(|m| m.ts.clone())
    }

    /// A message image was clicked: walk the conversation's messages into the viewer's
    /// image list and emit it with the clicked one's index; the root opens the viewer.
    fn open_image(&mut self, ts: &Ts, origin: &ImageOrigin, cx: &mut Context<Self>) {
        let found = {
            let data = self.data.read(cx);
            image_viewer::collect_viewer_images(&data.fixtures, &data.messages, ts, origin)
        };
        if let Some((images, index)) = found {
            cx.emit(MessageListEvent::OpenImage {
                channel: self.channel.clone(),
                images,
                index,
            });
        }
    }

    /// Slack's Up arrow in an empty composer: edit the signed-in user's newest message here.
    /// The open conversation's newest message (Esc marks the conversation read up to it).
    pub fn newest_ts(&self) -> Option<Ts> {
        let messages = self.messages.get(&self.channel)?;
        messages.last().map(|message| message.ts.clone())
    }

    pub fn edit_last_own(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = self.fixtures.self_user.clone();
        let newest = self
            .data
            .read(cx)
            .messages
            .iter()
            .rev()
            .find(|message| message.user.as_ref() == Some(&me))
            .map(|message| message.ts.clone());
        if let Some(ts) = newest {
            self.begin_edit(&ts, window, cx);
        }
    }

    /// Track the message currently under the pointer for Slack's one-key shortcuts, without
    /// re-rendering the transcript (row hover styling is handled natively in paint).
    fn set_focused(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        if self.data.read(cx).active.as_ref() != Some(ts) {
            self.data
                .update(cx, |data, _| data.active = Some(ts.clone()));
        }
    }

    fn clear_focused(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        if self.data.read(cx).active.as_ref() == Some(ts) {
            self.data.update(cx, |data, _| data.active = None);
        }
    }

    /// The focused message, if the mouse has hovered or keyboard focus stepped into the transcript.
    pub fn focused(&self, cx: &App) -> Option<Ts> {
        self.data.read(cx).active.clone()
    }

    /// Move keyboard focus up (`forward == false`) or down (`forward == true`) one message row
    /// in the transcript (`F-SHELL-273`), scrolling the newly focused message into view.
    pub fn step_focus(&mut self, forward: bool, cx: &mut Context<Self>) {
        let next = {
            let data = self.data.read(cx);
            let len = data.messages.len();
            if len == 0 {
                return;
            }
            let current = data
                .active
                .as_ref()
                .and_then(|ts| data.messages.iter().position(|m| &m.ts == ts));
            let ix = match current {
                Some(ix) if forward => (ix + 1).min(len - 1),
                Some(ix) => ix.saturating_sub(1),
                None => len - 1,
            };
            (ix, data.messages[ix].ts.clone())
        };
        let (ix, ts) = next;
        self.data.update(cx, |data, _| {
            data.active = Some(ts);
        });
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_focused_for_test(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        self.set_focused(ts, cx);
    }

    #[cfg(test)]
    pub(crate) fn is_editing_for_test(&self, cx: &App) -> bool {
        self.data.read(cx).editing.is_some()
    }

    #[cfg(test)]
    pub(crate) fn edit_text_for_test(&self, cx: &App) -> String {
        self.data.read(cx).edit_composer.read(cx).text()
    }

    #[cfg(test)]
    pub(crate) fn own_message_ts_for_test(&self) -> Option<Ts> {
        let self_user = &self.fixtures.self_user;
        self.messages
            .get(&self.channel)?
            .iter()
            .find(|m| m.user.as_ref() == Some(self_user))
            .map(|m| m.ts.clone())
    }

    #[cfg(test)]
    pub(crate) fn message_count_for_test(&self, cx: &App) -> usize {
        self.data.read(cx).messages.len()
    }

    #[cfg(test)]
    pub(crate) fn last_message_text_for_test(&self, cx: &App) -> Option<String> {
        self.data.read(cx).messages.last().map(|m| m.text.clone())
    }

    /// A one-key shortcut on the focused message, routed through the row's own
    /// [`MessageAction`] handler — the same one the hover bar and menu run. No-op while no
    /// message is focused (the mouse has not hovered the transcript yet) or the action does
    /// not apply: Edit and Delete are the signed-in user's only.
    pub fn focus_action(
        &mut self,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (ts, set) = {
            let data = self.data.read(cx);
            let Some(ts) = data.active.clone() else {
                return;
            };
            let set = match data.messages.iter().find(|message| message.ts == ts) {
                Some(message) => ActionSet {
                    own: message.user.as_ref() == Some(&self.fixtures.self_user),
                    reply: true,
                    saved: data.saved.contains(&message.ts),
                    pinned: message.pinned_to.contains(&self.channel),
                    in_thread: false,
                },
                None => return,
            };
            (ts, set)
        };
        if !actions::eligible(set, &action) {
            return;
        }
        self.run_action(&ts, action, window, cx);
    }

    /// The date divider's "Jump to…": scroll to the newest message, or to the first message
    /// at or after a point in time, asking the root to page older history in first when the
    /// loaded window starts after it.
    pub fn jump(&mut self, jump: Jump, cx: &mut Context<Self>) {
        const DAY: i64 = 86_400;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
            });
        let since = match jump {
            Jump::Recent => {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
                return;
            }
            Jump::Yesterday => now - DAY,
            Jump::LastWeek => now - 7 * DAY,
            Jump::LastMonth => now - 30 * DAY,
            Jump::Beginning => 0,
        };
        let starts_after = self
            .oldest_loaded(cx)
            .and_then(|ts| ts.secs())
            .is_some_and(|oldest| oldest > since);
        if starts_after {
            self.pending_jump = Some(since);
            cx.emit(MessageListEvent::NeedHistorySince {
                channel: self.channel.clone(),
                since,
            });
        }
        self.scroll_to_time(since, cx);
    }

    /// Jump to the first unread message in the open conversation (`Cmd+J`). Returns `true`
    /// when an unread message was found and scrolled into view.
    pub fn jump_to_unread(&mut self, cx: &mut Context<Self>) -> bool {
        self.pending_initial_unread_jump = false;
        self.scroll_to_first_unread(true, cx)
    }

    /// One-time unread scroll when opening a conversation (`mark_read == 0`): scrolls cleanly
    /// to the first unread message without flashing or remeasuring, and disarms itself so
    /// subsequent `Event::StoreChanged` refreshes (e.g. background backfill) never pull the
    /// reader back while scrolling.
    pub fn scroll_to_unread_once(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.pending_initial_unread_jump || self.data.read(cx).messages.is_empty() {
            return false;
        }
        self.pending_initial_unread_jump = false;
        self.scroll_to_first_unread(false, cx)
    }

    fn scroll_to_first_unread(&mut self, highlight: bool, cx: &mut Context<Self>) -> bool {
        let target = {
            let data = self.data.read(cx);
            let Some(last_read) = data.last_read.as_ref() else {
                return false;
            };
            data.messages
                .iter()
                .enumerate()
                .find(|(_, m)| m.ts > *last_read)
                .map(|(ix, m)| (ix, m.ts.clone()))
        };
        if let Some((ix, ts)) = target {
            self.scroller
                .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
            if highlight {
                self.flash_highlight(ts, cx);
            }
            true
        } else {
            false
        }
    }

    /// The jump's older history landed (or ran out): scroll to its target and highlight it.
    pub fn finish_jump(&mut self, cx: &mut Context<Self>) {
        if let Some(ts) = self.pending_highlight.take() {
            self.pending_jump = None;
            if let Some(ix) = self
                .data
                .read(cx)
                .messages
                .iter()
                .position(|message| message.ts >= ts)
            {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
                self.flash_highlight(ts, cx);
                return;
            }
        }
        if let Some(since) = self.pending_jump.take() {
            self.scroll_to_time(since, cx);
        }
    }

    /// A permalink, search hit, or saved item landed on `ts`: scroll the transcript onto it
    /// and briefly highlight the row, asking the root for older history first when the loaded
    /// window starts after it.
    pub fn reveal(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        let since = ts.secs().unwrap_or(0);
        let target = {
            let messages = &self.data.read(cx).messages;
            messages
                .iter()
                .position(|message| &message.ts >= ts)
                .map(|ix| (ix, messages[ix].ts.clone()))
        };
        match target {
            Some((ix, landed))
                if &landed == ts || self.oldest_loaded(cx).as_ref().is_none_or(|o| o <= ts) =>
            {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
                self.flash_highlight(landed, cx);
            }
            _ => {
                self.pending_jump = Some(since);
                self.pending_highlight = Some(ts.clone());
                cx.emit(MessageListEvent::NeedHistorySince {
                    channel: self.channel.clone(),
                    since,
                });
                self.scroll_to_time(since, cx);
            }
        }
    }

    fn flash_highlight(&mut self, ts: Ts, cx: &mut Context<Self>) {
        self.data
            .update(cx, |data, _| data.highlighted = Some(ts.clone()));
        self.refresh(cx);
        self.highlight_clear = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(2500))
                .await;
            let _ = this.update(cx, |list, cx| {
                if list.data.read(cx).highlighted.as_ref() == Some(&ts) {
                    list.data.update(cx, |data, _| data.highlighted = None);
                    list.refresh(cx);
                }
            });
        });
    }

    /// The oldest message of the open conversation's loaded window.
    pub fn oldest_loaded(&self, cx: &App) -> Option<Ts> {
        self.data
            .read(cx)
            .messages
            .first()
            .map(|message| message.ts.clone())
    }

    fn scroll_to_time(&mut self, since: i64, cx: &mut Context<Self>) {
        let ix = self
            .data
            .read(cx)
            .messages
            .iter()
            .position(|message| message.ts.secs().is_some_and(|secs| secs >= since))
            .unwrap_or(0);
        self.scroller.update(cx, |scroller, cx| {
            scroller.scroll_to_item(ix, cx);
        });
    }

    /// Mark or unmark `ts` saved (the Later list changed elsewhere).
    pub fn set_saved(&mut self, ts: &Ts, saved: bool, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| {
            if saved {
                data.saved.insert(ts.clone());
            } else {
                data.saved.remove(ts);
            }
        });
        self.refresh(cx);
    }

    pub fn replace_saved(&mut self, saved: HashSet<Ts>, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| {
            data.saved = saved;
        });
        self.refresh(cx);
    }

    /// Tint the root of the thread open beside the list (`None`: no thread is open).
    pub fn set_open_thread(&mut self, root: Option<Ts>, cx: &mut Context<Self>) {
        if self.data.read(cx).open_thread != root {
            self.data.update(cx, |data, _| data.open_thread = root);
            self.refresh(cx);
        }
    }

    /// Save (or unsave) `ts` in the open conversation, optionally with a reminder, and hand
    /// the request to the root's Later list.
    fn save_for_later(
        &mut self,
        ts: &Ts,
        saved: bool,
        due: Option<u64>,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(message) = self
            .messages
            .get(&self.channel)
            .and_then(|history| history.iter().find(|message| &message.ts == ts))
        else {
            return;
        };
        let request = SaveRequest {
            channel: self.channel.clone(),
            ts: ts.clone(),
            author: message
                .user
                .as_ref()
                .map(|user| self.fixtures.user_name(user).to_owned())
                .unwrap_or_default(),
            text: self.fixtures.preview(message),
            saved,
            due,
        };
        actions::notify(title, request.text.clone(), window, cx);
        cx.emit(MessageListEvent::Later(request));
        self.set_saved(ts, saved, cx);
    }

    /// Move the read cursor to just before `ts` so `ts` (and everything newer) is marked
    /// unread both locally (New divider above `ts`) and on Slack (`conversations.mark`).
    fn mark_unread(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        let cursor = self.data.update(cx, |data, _| {
            let before = data
                .messages
                .iter()
                .take_while(|message| &message.ts < ts)
                .last()
                .map(|message| message.ts.clone());
            let cursor = before.unwrap_or_else(|| ts_just_before(ts));
            data.last_read = Some(cursor.clone());
            data.recompute_first_unread();
            cursor
        });
        cx.emit(MessageListEvent::Changed {
            channel: self.channel.clone(),
            change: MessageChange::MarkUnread { ts: cursor },
        });
        self.refresh(cx);
    }

    /// Edit `ts` in place: the row body becomes the shared WYSIWYG edit composer, focused at
    /// the end and scrolled into view.
    fn begin_edit(&mut self, ts: &Ts, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self.current_mut(ts).cloned() else {
            return;
        };
        let composer = self.data.read(cx).edit_composer.clone();
        composer.update(cx, |composer, cx| {
            composer.load_message(&message, window, cx);
        });
        let row = self.data.read(cx).messages.iter().position(|m| &m.ts == ts);
        self.data
            .update(cx, |data, _| data.editing = Some(ts.clone()));
        self.refresh(cx);
        if let Some(ix) = row {
            self.scroller
                .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
        }
    }

    fn save_edit(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let Some(ts) = self.data.read(cx).editing.clone() else {
            return;
        };
        let me = self.fixtures.self_user.clone();
        if let Some(message) = self.current_mut(&ts) {
            actions::apply_edit(message, &me, content.clone());
            cx.emit(MessageListEvent::Changed {
                channel: self.channel.clone(),
                change: MessageChange::Edit {
                    ts: ts.clone(),
                    content,
                },
            });
        }
        self.data.update(cx, |data, _| data.editing = None);
        self.refresh(cx);
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| data.editing = None);
        self.refresh(cx);
    }

    /// Delete `ts` from the fixture store and drop its row from the virtual list.
    pub fn delete(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        if let Some(history) = self.messages.get_mut(&self.channel) {
            history.retain(|message| &message.ts != ts);
        }
        cx.emit(MessageListEvent::Changed {
            channel: self.channel.clone(),
            change: MessageChange::Delete { ts: ts.clone() },
        });
        let row = self
            .data
            .read(cx)
            .messages
            .iter()
            .position(|message| &message.ts == ts);
        let history = history_of(&self.messages, &self.channel);
        self.data.update(cx, |data, _| {
            data.messages = history;
            data.recompute_first_unread();
            data.saved.remove(ts);
            if data.editing.as_ref() == Some(ts) {
                data.editing = None;
            }
        });
        if let Some(row) = row {
            self.scroller.update(cx, |scroller, cx| {
                scroller.splice(row..row + 1, 0, cx);
            });
        }
        cx.notify();
    }
}

/// History snapshot for a conversation: oldest first, metadata-only events dropped.
fn history_of(messages: &HashMap<ChannelId, Vec<Message>>, channel: &ChannelId) -> Vec<Message> {
    messages
        .get(channel)
        .map(|history| {
            history
                .iter()
                .filter(|message| !message.hidden)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

impl Render for MessageList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.pending_evict_urls.is_empty() {
            for url in self.pending_evict_urls.drain(..) {
                crate::emoji::evict_gif_asset(&url, window, cx);
            }
        }
        let data = self.data.clone();
        let this = cx.entity();
        let scroller = self.scroller.clone();
        let now = CivilTime::now();
        let background = cx.theme().background;
        let row_gap = if crate::preferences::display_prefs(cx).compact {
            0.
        } else {
            ROW_GAP
        };

        MessageScroller::new("message-list", scroller, move |ix, window, cx| {
            // Block Kit text inputs need a window-created textarea before the row renders.
            let (kit, with_inputs) = {
                let store = data.read(cx);
                if store.messages.is_empty() {
                    return div();
                }
                let message = &store.messages[ix.min(store.messages.len() - 1)];
                let has_inputs = message.blocks.iter().any(
                    |block| matches!(block, Block::Unknown { r#type, .. } if r#type == "input"),
                );
                (store.kit.clone(), has_inputs.then(|| message.clone()))
            };
            if let Some(message) = with_inputs {
                blockkit::prepare_inputs(&kit, &message, window, cx);
            }
            let store = data.read(cx);
            let ix = ix.min(store.messages.len() - 1);
            transcript_row(store, ix, &this, &now, cx)
        })
        .jump_button(true)
        .with_row_style(
            StyleRefinement::default()
                .pb(px(row_gap))
                .pl(px(GUTTER))
                .pr(px(GUTTER)),
        )
        .with_bottom_fade(background)
    }
}
