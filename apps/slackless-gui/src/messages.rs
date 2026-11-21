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

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::base::{TextSelectionHandle, TextSelectionRegistration, TextSelectionRun};
use gpui_kit::component::button::{Button, ButtonRounded};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, BorderStyle, Bounds, Context, Corners, CursorStyle,
    DispatchPhase, Div, Edges, Element, ElementId, Entity, EventEmitter, Focusable as _, Font,
    FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla, ImageSource,
    InspectorElementId, InteractiveElement as _, InteractiveText, IntoElement, LayoutId,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement as _, Pixels, Point,
    Render, RenderOnce, SharedString, Stateful, StatefulInteractiveElement as _,
    StrikethroughStyle, StyleRefinement, Styled as _, StyledText, Subscription, TextLayout,
    TextRun, TransformationMatrix, UnderlineStyle, WeakEntity, Window, deferred, div, px, relative,
    rgb, transparent_black,
};
use slackless_core::types::rich_text::{
    ListStyle, RichTextContainer, RichTextElement, RichTextList, RichTextPreformatted,
};
use slackless_core::types::{Block, ChannelId, Message, MessageSubtype, TextStyle, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MessageAction, MessageChange};
use crate::blockkit::{self, BlockDispatch, BlockKitState, KitCx};
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::image_viewer::{self, ImageClick, ImageOrigin};
use crate::later::{Remind, SaveRequest};

/// Avatar chip size in px.
const AVATAR_SIZE: f32 = 36.;
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
const INLINE_EMOJI: f32 = 18.;
/// The room an app chip's text leaves for its icon: five no-break spaces, about the icon and
/// Slack's gap after it at body size (an em space measured narrower in Lato).
const APP_ICON_SLOT: &str = "\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}";
/// An app chip's icon size (px).
const APP_ICON: f32 = 13.;
/// Rows from the top within which scrolling asks for the next older page: close enough that
/// the page lands before the reader reaches the edge.
const NEAR_TOP_ROWS: usize = 2;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

const DEFAULT_FLAGS: RunFlags = RunFlags {
    bold: false,
    semibold: false,
    italic: false,
    strike: false,
    underline: false,
    code: false,
};

/// Mention chip style: semibold link color on an accent wash (guide section 10).
fn mention_flags() -> RunFlags {
    RunFlags {
        semibold: true,
        ..DEFAULT_FLAGS
    }
}

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
fn open_link_default(target: LinkTarget, cx: &mut App) {
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
        images: Arc<Vec<image_viewer::ViewerImage>>,
        index: usize,
    },
    /// Forward this message (its permalink): the root opens New message with it drafted.
    Forward(String),
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
    /// Interactive Block Kit control state (selections, checks, input drafts).
    kit: Entity<BlockKitState>,
    /// Messages saved for later (fixture mode: in memory).
    saved: HashSet<Ts>,
    /// Root of the thread open beside the list, tinted while it shows.
    open_thread: Option<Ts>,
    /// The message being edited in place; its draft is `edit_draft`.
    editing: Option<Ts>,
    edit_draft: Entity<TextareaState>,
}

pub struct MessageList {
    fixtures: Arc<Fixtures>,
    channel: ChannelId,
    /// Per-conversation history, oldest first (fixture history plus local sends).
    messages: HashMap<ChannelId, Vec<Message>>,
    /// Projection of the current conversation that the row renderer reads.
    data: Entity<MessageData>,
    scroller: Entity<MessageScrollerState>,
    /// Oldest message a [`MessageListEvent::NeedOlder`] covered (in flight or landed); a
    /// window refresh forgets it so the top of the list re-asks.
    requested_older: Option<Ts>,
    /// A "Jump to…" target (unix seconds) waiting for older history to land.
    pending_jump: Option<i64>,
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
            let edit_draft = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Edit message")
                    .auto_grow(1, 8)
            });
            // Constructor enables FollowMode::Tail: the transcript starts at the bottom and
            // follows new content.
            let scroller = cx.new(|cx| MessageScrollerState::new(count, cx));
            // Control state and the edit draft live in their own entities: repaint the rows
            // that show them when either changes; the scroller is watched to page older
            // history in when the reader reaches the top.
            let subscriptions = vec![
                cx.observe(&kit, |_, _, cx| cx.notify()),
                cx.subscribe(&edit_draft, |_, _, _: &InputEvent, cx| cx.notify()),
                cx.observe(&scroller, |list: &mut MessageList, scroller, cx| {
                    list.request_older_near_top(&scroller, cx)
                }),
            ];
            let data = cx.new(|_| MessageData {
                fixtures: fixtures.clone(),
                channel: channel.clone(),
                messages: history,
                last_read: fixtures
                    .conversation(&channel)
                    .and_then(|conversation| conversation.last_read.clone()),
                kit,
                saved: HashSet::new(),
                open_thread: None,
                editing: None,
                edit_draft,
            });
            MessageList {
                fixtures,
                channel,
                messages,
                data,
                scroller,
                requested_older: None,
                pending_jump: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: draw names, emoji and conversations from `fixtures`; the loaded history,
    /// scroll position and new-messages divider stay.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.data.update(cx, |data, cx| {
            data.fixtures = fixtures.clone();
            cx.notify();
        });
        self.fixtures = fixtures;
        cx.notify();
    }

    pub fn set_conversation(&mut self, channel: ChannelId, cx: &mut Context<Self>) {
        self.channel = channel;
        self.sync(cx);
    }

    /// Append a message to `channel`'s history (live event or optimistic local send). Appends
    /// to the visible conversation also advance the virtual list by one row, tail-anchored.
    pub fn append(&mut self, channel: ChannelId, message: Message, cx: &mut Context<Self>) {
        let visible = channel == self.channel && !message.hidden;
        if visible {
            self.data
                .update(cx, |data, _| data.messages.push(message.clone()));
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
        // Live history lives in the store: keep only the open conversation's in memory.
        self.messages.retain(|kept, _| *kept == channel);
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
        self.messages.insert(channel, merged);
        if !visible {
            return;
        }
        let history = history_of(&self.messages, &self.channel);
        let (unchanged, extends, added) = {
            let shown = &self.data.read(cx).messages;
            (
                *shown == history,
                history.len() >= shown.len()
                    && history.first().map(|m| &m.ts) == shown.first().map(|m| &m.ts),
                history.len().saturating_sub(shown.len()),
            )
        };
        if unchanged {
            return;
        }
        if !extends {
            return self.sync(cx);
        }
        self.data.update(cx, |data, _| data.messages = history);
        self.scroller.update(cx, |scroller, cx| {
            if added > 0 {
                scroller.append(added, cx);
            } else {
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
        self.data.update(cx, |data, _| data.messages = history);
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
        let last_read = self
            .fixtures
            .conversation(&self.channel)
            .and_then(|conversation| conversation.last_read.clone());
        let count = history.len();
        self.data.update(cx, |data, _| {
            data.channel = self.channel.clone();
            data.messages = history;
            data.last_read = last_read;
            data.editing = None;
        });
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(count, cx));
        cx.notify();
    }

    /// Re-project the current conversation after an in-place change: rows keep their place,
    /// heights are remeasured.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let history = history_of(&self.messages, &self.channel);
        self.data.update(cx, |data, _| data.messages = history);
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

    fn run_action(
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
                let data = self.data.read(cx);
                if let Some(message) = data.messages.iter().find(|m| &m.ts == ts) {
                    let text = self.fixtures.plain_text(message);
                    cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(text));
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
                self.save_for_later(ts, true, Some(remind), &title, window, cx);
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
            MessageAction::OpenProfile(user) => cx.emit(MessageListEvent::OpenProfile(user)),
            MessageAction::OpenImage(origin) => self.open_image(ts, &origin, cx),
            MessageAction::BlockAction { action, attachment } => {
                let live = !self.fixtures.sample;
                let dispatch = self
                    .current_mut(ts)
                    .and_then(|message| {
                        BlockDispatch::new(message, &channel, action.clone(), attachment)
                    })
                    .filter(|_| live);
                match dispatch {
                    Some(dispatch) => cx.emit(MessageListEvent::Changed {
                        channel,
                        change: MessageChange::BlockAction(dispatch),
                    }),
                    None => blockkit::log_button(&action, window, cx),
                }
            }
        }
    }

    /// A message image was clicked: walk the conversation's messages into the viewer's
    /// image list and emit it with the clicked one's index; the root opens the viewer.
    fn open_image(&mut self, ts: &Ts, origin: &ImageOrigin, cx: &mut Context<Self>) {
        let (images, index) = {
            let data = self.data.read(cx);
            let fixtures = data.fixtures.clone();
            let mut images = Vec::new();
            let mut index = None;
            for message in &data.messages {
                for entry in image_viewer::message_images(&fixtures, message) {
                    if index.is_none() && &message.ts == ts && &entry.origin == origin {
                        index = Some(images.len());
                    }
                    images.push(entry.image);
                }
            }
            (images, index)
        };
        if let Some(index) = index {
            cx.emit(MessageListEvent::OpenImage {
                channel: self.channel.clone(),
                // The viewer is main-thread-only (GPUI): the Arc only shares the list, never
                // crosses threads.
                #[allow(clippy::arc_with_non_send_sync)]
                images: Arc::new(images),
                index,
            });
        }
    }

    /// Slack's Up arrow in an empty composer: edit the signed-in user's newest message here.
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

    /// The jump's older history landed (or ran out): scroll to its target.
    pub fn finish_jump(&mut self, cx: &mut Context<Self>) {
        if let Some(since) = self.pending_jump.take() {
            self.scroll_to_time(since, cx);
        }
    }

    /// A permalink landed on `ts` (the root's [`LinkTarget::Url`] handling): scroll the
    /// transcript onto it, asking the root for older history first when the loaded window
    /// starts after it — the date divider's "Jump to…" machinery.
    pub fn reveal(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        let since = ts.secs().unwrap_or(0);
        match self
            .data
            .read(cx)
            .messages
            .iter()
            .position(|message| &message.ts >= ts)
        {
            Some(ix) => {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
            }
            None => {
                self.pending_jump = Some(since);
                cx.emit(MessageListEvent::NeedHistorySince {
                    channel: self.channel.clone(),
                    since,
                });
                self.scroll_to_time(since, cx);
            }
        }
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
        remind: Option<Remind>,
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
            due: remind.map(|remind| remind.at(crate::drafts::now_secs())),
        };
        actions::notify(title, request.text.clone(), window, cx);
        cx.emit(MessageListEvent::Later(request));
        self.set_saved(ts, saved, cx);
    }

    /// Fixture-mode unread marker: move the read cursor to just before `ts`, so the New
    /// divider sits above it.
    fn mark_unread(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        self.data.update(cx, |data, _| {
            let before = data
                .messages
                .iter()
                .take_while(|message| &message.ts < ts)
                .last()
                .map(|message| message.ts.clone());
            data.last_read = Some(before.unwrap_or_else(|| Ts::new("0.000000")));
        });
        self.refresh(cx);
    }

    /// Edit `ts` in place: the row body becomes the shared edit draft, focused at the end.
    fn begin_edit(&mut self, ts: &Ts, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.current_mut(ts).map(|message| message.text.clone()) else {
            return;
        };
        let caret = text.len();
        let draft = self.data.read(cx).edit_draft.clone();
        draft.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_selected_range(caret..caret, cx);
        });
        draft.read(cx).focus_handle(cx).focus(window, cx);
        self.data
            .update(cx, |data, _| data.editing = Some(ts.clone()));
        self.refresh(cx);
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let (editing, draft) = {
            let data = self.data.read(cx);
            (data.editing.clone(), data.edit_draft.clone())
        };
        let Some(ts) = editing else {
            return;
        };
        let text = draft.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let me = self.fixtures.self_user.clone();
        if let Some(message) = self.current_mut(&ts) {
            actions::apply_edit(message, &me, text.clone());
            cx.emit(MessageListEvent::Changed {
                channel: self.channel.clone(),
                change: MessageChange::Edit {
                    ts: ts.clone(),
                    text,
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
    fn delete(&mut self, ts: &Ts, cx: &mut Context<Self>) {
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let data = self.data.clone();
        let this = cx.entity();
        let scroller = self.scroller.clone();
        let now = CivilTime::now();
        let background = cx.theme().background;

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
                .pb(px(ROW_GAP))
                .pl(px(GUTTER))
                .pr(px(GUTTER)),
        )
        .with_bottom_fade(background)
    }
}

/// One transcript row: optional date/new-messages divider above the (possibly grouped)
/// message card with its reactions, "N replies" link, hover action bar and context menu.
fn transcript_row(
    store: &MessageData,
    ix: usize,
    this: &Entity<MessageList>,
    now: &CivilTime,
    cx: &App,
) -> Div {
    let messages = &store.messages;
    let fixtures = &store.fixtures;
    let message = &messages[ix];
    let previous = ix.checked_sub(1).map(|previous| &messages[previous]);
    // Like Slack, a run of joins (or of leaves) reads as its first line, "<name> and N others
    // left #channel."; the rest of the run keeps an empty row (and so its scroll position).
    if previous.is_some_and(|prev| membership_run(prev, message)) {
        return div();
    }

    let date_break = previous.is_none_or(|prev| day_index(&prev.ts) != day_index(&message.ts));
    // Like Slack, your own messages never count as new: the line goes above the first unread
    // message from someone else (the scan back only crosses this conversation's unread tail).
    let own = |m: &Message| m.user.as_ref() == Some(&fixtures.self_user);
    let new_break = match store.last_read.as_ref() {
        Some(read) if !date_break && &message.ts > read && !own(message) => messages[..ix]
            .iter()
            .rev()
            .take_while(|prev| &prev.ts > read)
            .all(own),
        _ => false,
    };
    let grouped =
        !date_break && !new_break && previous.is_some_and(|prev| same_group(prev, message));
    let editing = store.editing.as_ref() == Some(&message.ts);
    let saved = store.saved.contains(&message.ts);

    let author_key = author_key(message);
    let author = author_name(message, fixtures);
    // Every link in this row's text routes out as one event; the root resolves it.
    let links: LinkHandler = {
        let this = this.clone();
        Rc::new(move |target, _window, cx| {
            this.update(cx, |_, cx| {
                cx.emit(MessageListEvent::OpenLink(target.clone()))
            });
        })
    };

    let mut row = v_flex().min_w_0();
    if date_break {
        let label = message
            .ts
            .secs()
            .map(|secs| date_label(&CivilTime::from_unix(secs), now))
            .unwrap_or_else(|| message.ts.as_str().to_owned());
        row = row.child(date_divider(
            label,
            format!("day-{}", message.ts.as_str()),
            this.downgrade(),
            cx,
        ));
    } else if new_break {
        row = row.child(new_messages_divider(cx));
    }
    if is_membership(message) {
        let others = messages[ix + 1..]
            .iter()
            .take_while(|next| membership_run(message, next))
            .count();
        return row.child(membership_line(message, &author, others, store, &links, cx));
    }

    let key = format!("msg-{}", message.ts.as_str());
    let handler: ActionHandler = {
        let this = this.clone();
        let ts = message.ts.clone();
        Rc::new(move |action, window, cx| {
            this.update(cx, |list, cx| list.run_action(&ts, action, window, cx));
        })
    };
    let image_open: ImageClick = {
        let this = this.clone();
        let ts = message.ts.clone();
        Rc::new(move |origin, _window, cx| {
            this.update(cx, |list, cx| list.open_image(&ts, &origin, cx));
        })
    };

    let mut body = v_flex().flex_1().min_w_0().gap_0p5().text_sm();
    if !grouped || editing {
        body = body.child(message_header(
            message,
            &author,
            format_clock(&message.ts),
            fixtures,
            Some(&handler),
            cx,
        ));
    }
    if editing {
        let (cancel, save) = (this.clone(), this.clone());
        body = body.child(actions::edit_box(
            &key,
            &store.edit_draft,
            move |_, cx| cancel.update(cx, |list, cx| list.cancel_edit(cx)),
            move |_, cx| save.update(cx, |list, cx| list.save_edit(cx)),
            cx,
        ));
    } else {
        let kcx = KitCx::new(fixtures, Some(&store.kit), &message.ts)
            .with_links(links.clone())
            .opening_images(image_open)
            .dispatching_actions(handler.clone());
        // The root as loaded here, so an edit to it shows as in Slack; else the copy the
        // broadcast carries.
        let broadcast_root = message
            .root
            .as_deref()
            .filter(|_| message.subtype == Some(MessageSubtype::ThreadBroadcast))
            .map(|root| messages.iter().find(|m| m.ts == root.ts).unwrap_or(root));
        if let Some(root) = broadcast_root.filter(|_| !grouped) {
            // Slack's broadcast reference opens the thread.
            let (open, channel, thread_ts) =
                (links.clone(), store.channel.clone(), root.ts.clone());
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "broadcast-{}",
                        message.ts.as_str()
                    )))
                    .min_w_0()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|el| el.underline())
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_none().child("replied to a thread:"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().link)
                            .child(fixtures.preview(root)),
                    )
                    .on_click(move |_, window, cx| {
                        open(
                            LinkTarget::Thread {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                            },
                            window,
                            cx,
                        )
                    }),
            );
        }
        body = body.child(message_body(message, &kcx, cx));
        if let Some(files) =
            crate::files::attachments(fixtures, message, kcx.image_open.as_ref(), cx)
        {
            body = body.child(files);
        }
        body = body.children(actions::reaction_chips(
            &key,
            &message.reactions,
            fixtures,
            Some(&handler),
            cx,
        ));
        if let Some(reply_count) = message.reply_count.filter(|&count| count > 0) {
            let reply = handler.clone();
            // Slack's thread summary (`.c-message__reply_bar`): the repliers' faces and the
            // reply count, 4px under the message. Hovering the message raises it into a card
            // (white, 13% foreground ring) with "Last reply …" and a chevron; hovering the
            // bar itself swaps that line for "View thread".
            let faces = message
                .reply_users
                .iter()
                .take(4)
                .map(|user| crate::avatars::user_chip(fixtures, user, 24.));
            let label = if reply_count == 1 {
                "1 reply".to_owned()
            } else {
                format!("{reply_count} replies")
            };
            let last_reply = message
                .latest_reply
                .as_ref()
                .and_then(|ts| ts.secs())
                .map(|secs| format!("Last reply {}", crate::image_viewer::relative_stamp(secs)));
            let theme = cx.theme();
            let (surface, edge, muted, link) = (
                theme.background,
                theme.foreground.opacity(0.13),
                theme.muted_foreground,
                theme.link,
            );
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "thread-{}",
                        message.ts.as_str()
                    )))
                    .group("reply-bar")
                    .max_w(px(600.))
                    .h(px(34.))
                    .mt_1()
                    .ml(px(-5.))
                    .p_1()
                    .gap_1()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(edge.opacity(0.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .group_hover("msg", |bar| bar.bg(surface).border_color(edge))
                    .children(faces)
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .text_color(link)
                            .child(label),
                    )
                    .child(
                        div()
                            .relative()
                            .min_w_0()
                            .ml_1()
                            .text_color(muted)
                            .whitespace_nowrap()
                            .opacity(0.)
                            .group_hover("msg", |line| line.opacity(1.))
                            .children(last_reply.map(|last| {
                                div()
                                    .group_hover("reply-bar", |line| line.opacity(0.))
                                    .child(last)
                            }))
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .opacity(0.)
                                    .group_hover("reply-bar", |line| line.opacity(1.))
                                    .child("View thread"),
                            ),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .opacity(0.)
                            .group_hover("msg", |arrow| arrow.opacity(1.))
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(px(16.))
                                    .text_color(muted),
                            ),
                    )
                    .on_click(move |_, window, cx| reply(MessageAction::ReplyInThread, window, cx)),
            );
        }
    }

    let gutter = if grouped && !editing {
        // Slack shows a small hover-only timestamp in place of the avatar on grouped rows.
        div()
            .w(px(AVATAR_SIZE))
            .flex_none()
            .flex()
            .items_start()
            .justify_center()
            .pt_0p5()
            .opacity(0.)
            .group_hover("msg", |slot| slot.opacity(1.))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_clock(&message.ts)),
            )
            .into_any_element()
    } else {
        avatar_gutter(
            fixtures,
            message,
            &author_key,
            &author,
            &key,
            Some(&handler),
            cx,
        )
    };

    let set = ActionSet {
        own: message.user.as_ref() == Some(&fixtures.self_user),
        reply: true,
        saved,
    };
    let theme = cx.theme();
    let hover_bg = theme.secondary.opacity(0.5);
    let saved_bg = theme.warning.opacity(0.08);
    let thread_open = !saved && store.open_thread.as_ref() == Some(&message.ts);
    let thread_bg = crate::theme::thread_root(cx);
    let menu_handler = handler.clone();
    let card = div()
        .id(SharedString::from(key.clone()))
        .group("msg")
        .relative()
        .min_w_0()
        .ml(px(-GUTTER))
        .mr(px(-GUTTER))
        .px(px(GUTTER))
        .py_0p5()
        .when(saved, |card| {
            card.bg(saved_bg).child(actions::saved_banner(cx))
        })
        .when(message.is_ephemeral, |card| {
            card.child(actions::ephemeral_banner(cx))
        })
        .when(thread_open, |card| card.bg(thread_bg))
        .when(!saved && !thread_open, |card| {
            card.hover(|card| card.bg(hover_bg))
        })
        .child(
            h_flex()
                .items_start()
                .gap_3()
                .min_w_0()
                .child(gutter)
                .child(body),
        )
        .when(!editing, |card| {
            card.child(actions::hover_toolbar(
                &key,
                &fixtures.quick_reactions,
                set,
                &handler,
                "msg".into(),
                cx,
            ))
        })
        .context_menu(move |menu, window, cx| {
            actions::actions_menu(menu, set, &menu_handler, window, cx)
        });
    row.child(card)
}

/// Two consecutive messages group into one card when they share an author, land on the same
/// day within [`GROUP_WINDOW_SECS`], and carry the same thread-ness.
pub(crate) fn same_group(prev: &Message, message: &Message) -> bool {
    let same_author = match (&prev.user, &message.user) {
        (Some(a), Some(b)) => a == b,
        (None, None) => prev.username == message.username,
        _ => false,
    };
    let close = match (prev.ts.secs(), message.ts.secs()) {
        (Some(prev_secs), Some(secs)) => secs.saturating_sub(prev_secs) <= GROUP_WINDOW_SECS,
        _ => false,
    };
    same_author && close && prev.is_thread_reply() == message.is_thread_reply()
}

fn author_key(message: &Message) -> String {
    message
        .user
        .as_ref()
        .map(|user| user.as_str().to_owned())
        .or_else(|| message.username.clone())
        .or_else(|| message.bot_id.as_ref().map(|bot| bot.as_str().to_owned()))
        .unwrap_or_else(|| "unknown".to_owned())
}

fn author_name(message: &Message, fixtures: &Fixtures) -> String {
    message
        .user
        .as_ref()
        .map(|user| fixtures.user_name(user).to_owned())
        .or_else(|| message.username.clone())
        // An app message without a username (Giphy's preview) goes by its bot's name.
        .or_else(|| {
            let name = &message.bot_profile.as_ref()?.name;
            (!name.is_empty()).then(|| name.clone())
        })
        .unwrap_or_else(|| author_key(message))
}

/// One full message card: avatar, author header, rich-text / Block Kit body and static
/// reaction chips. Shared with the thread panel and search so every column renders messages
/// identically. Cards without a link handler stay plain (not selectable, links open as
/// URLs).
pub fn message_row(
    fixtures: &Fixtures,
    message: &Message,
    handler: Option<&actions::ActionHandler>,
    cx: &App,
) -> Div {
    message_row_linked(fixtures, message, handler, None, None, cx)
}

/// [`message_row`] with its text selectable and links routed through `links` (the thread
/// panel: its events reach the root without a transcript). With `grouped` (the row's hover
/// group) the card continues the previous message's: no avatar or name, its time showing in
/// the gutter on hover, as in Slack.
pub fn message_row_linked(
    fixtures: &Fixtures,
    message: &Message,
    handler: Option<&actions::ActionHandler>,
    links: Option<&LinkHandler>,
    grouped: Option<SharedString>,
    cx: &App,
) -> Div {
    let author_key = author_key(message);
    let author = author_name(message, fixtures);
    let kcx = match links.cloned() {
        Some(links) => KitCx::new(fixtures, None, &message.ts).with_links(links),
        None => KitCx::new(fixtures, None, &message.ts),
    };
    // A row with a handler (the thread panel) opens its images through it too.
    let kcx = match handler {
        Some(handler) => {
            let open = handler.clone();
            kcx.dispatching_actions(handler.clone())
                .opening_images(Rc::new(move |origin, window, cx| {
                    open(MessageAction::OpenImage(origin), window, cx)
                }))
        }
        None => kcx,
    };
    let mut body = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .text_sm()
        .when(grouped.is_none(), |body| {
            body.child(message_header(
                message,
                &author,
                card_stamp(&message.ts),
                fixtures,
                handler,
                cx,
            ))
        })
        .child(message_body(message, &kcx, cx));
    if let Some(files) = crate::files::attachments(fixtures, message, kcx.image_open.as_ref(), cx) {
        body = body.child(files);
    }
    let key = format!("row-{}", message.ts.as_str());
    body = body.children(actions::reaction_chips(
        &key,
        &message.reactions,
        fixtures,
        None,
        cx,
    ));
    let gutter = match grouped {
        // Slack's continuation gutter: the time, shown while the row is hovered.
        Some(group) => div()
            .w(px(AVATAR_SIZE))
            .flex_none()
            .flex()
            .justify_center()
            .pt_0p5()
            .opacity(0.)
            .group_hover(group, |slot| slot.opacity(1.))
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format_clock(&message.ts))
            .into_any_element(),
        None => avatar_gutter(fixtures, message, &author_key, &author, &key, handler, cx),
    };
    let row = h_flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(gutter)
        .child(body);
    if message.is_ephemeral {
        v_flex()
            .min_w_0()
            .child(actions::ephemeral_banner(cx))
            .child(row)
    } else {
        row
    }
}

/// `element` as a link to `user`'s profile, like Slack's message avatars and names; plain
/// when there is no user or no action handler (search and pinned cards).
fn profile_link(
    element: impl IntoElement,
    id: String,
    user: Option<&UserId>,
    handler: Option<&actions::ActionHandler>,
) -> AnyElement {
    let (Some(user), Some(handler)) = (user, handler) else {
        return element.into_any_element();
    };
    let (user, handler) = (user.clone(), handler.clone());
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .hover(|link| link.underline())
        .child(element)
        .on_click(move |_, window, cx| {
            handler(MessageAction::OpenProfile(user.clone()), window, cx)
        })
        .into_any_element()
}

/// A message's avatar gutter: the author's avatar (badged when they are from another
/// organization) as a link to their profile, [`AVATAR_TOP`] down like Slack's.
fn avatar_gutter(
    fixtures: &Fixtures,
    message: &Message,
    author_key: &str,
    author: &str,
    key: &str,
    handler: Option<&actions::ActionHandler>,
    cx: &App,
) -> AnyElement {
    let avatar = crate::avatars::with_external_badge(
        fixtures,
        message.user.as_ref(),
        crate::avatars::author_avatar(fixtures, message, author_key, author, AVATAR_SIZE),
        AVATAR_SIZE,
        cx.theme().background,
    );
    div()
        .flex_none()
        .mt(px(AVATAR_TOP))
        .child(profile_link(
            avatar,
            format!("{key}-avatar"),
            message.user.as_ref(),
            handler,
        ))
        .into_any_element()
}

/// Slack's `.c-app_badge` after an app's name: 10px bold "APP" in the secondary text
/// color on a 14px tile with 2px corners.
fn app_badge(cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(px(14.))
        .px(px(3.))
        .rounded(px(2.))
        .bg(theme.secondary)
        .text_color(theme.muted_foreground)
        .text_size(px(10.))
        .line_height(px(12.))
        .font_weight(FontWeight::BOLD)
        .child("APP")
}

fn message_header(
    message: &Message,
    author: &str,
    stamp: String,
    fixtures: &Fixtures,
    handler: Option<&actions::ActionHandler>,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    // Like Slack, the author's current (unexpired) status emoji follows their name.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    let status = message
        .user
        .as_ref()
        .and_then(|user| fixtures.user(user))
        .map(|user| &user.profile)
        .filter(|profile| {
            profile
                .status_expiration
                .is_none_or(|end| end == 0 || end > now)
        })
        .and_then(|profile| profile.status_emoji.as_deref())
        .map(|code| code.trim_matches(':'))
        .filter(|code| !code.is_empty())
        .map(|code| crate::emoji::emoji_element(code, fixtures, px(14.)));
    h_flex()
        .items_baseline()
        .gap_2()
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(profile_link(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(author.to_owned()),
                    format!("author-{}", message.ts.as_str()),
                    message.user.as_ref(),
                    handler,
                ))
                // An app's own post; a person's post made through an app (`/giphy`) keeps
                // their name unbadged, as in Slack.
                .when(
                    message.bot_id.is_some()
                        && message
                            .user
                            .as_ref()
                            .and_then(|user| fixtures.user(user))
                            .is_none_or(|user| user.is_bot),
                    |name| name.child(app_badge(cx)),
                )
                .children(status),
        )
        .child(div().text_xs().text_color(muted).child(stamp))
}

/// Slack's day divider: a full-width rule with the day's pill (label, then a chevron) on it.
/// The date divider's "Jump to…" targets.
#[derive(Clone, Copy)]
pub enum Jump {
    Recent,
    LastWeek,
    LastMonth,
    Beginning,
}

fn date_divider(label: String, key: String, list: WeakEntity<MessageList>, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, background, fg) = (theme.border, theme.background, theme.foreground);
    // Slack's pill: 28px high, 13px bold, 16px before the label and 8px after its small
    // chevron, a 13% foreground ring and a faint drop shadow.
    let pill = Button::new(SharedString::from(key))
        .outline()
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(999.)))
        .h(px(28.))
        .pl(px(16.))
        .pr(px(8.))
        .bg(background)
        .border_color(fg.opacity(0.13))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .size(px(10.))
                .text_color(fg),
        );
    // `Button::shadow` toggles the kit's own shadow; this is the element's.
    let pill = gpui_kit::gpui::Styled::shadow(
        pill,
        vec![gpui_kit::gpui::BoxShadow {
            color: Hsla::black().opacity(0.08),
            offset: gpui_kit::gpui::point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }],
    );
    h_flex()
        .items_center()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(border))
        .child(
            pill.dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                menu = menu.label("Jump to…");
                for (jump, text) in [
                    (Jump::Recent, "Most recent"),
                    (Jump::LastWeek, "Last week"),
                    (Jump::LastMonth, "Last month"),
                    (Jump::Beginning, "The very beginning"),
                ] {
                    let list = list.clone();
                    menu = menu.item(PopupMenuItem::new(text).on_click(move |_, _, cx| {
                        let _ = list.update(cx, |list, cx| list.jump(jump, cx));
                    }));
                }
                menu
            }),
        )
        .child(div().flex_1().h(px(1.)).bg(border))
}

fn is_membership(message: &Message) -> bool {
    matches!(
        message.subtype,
        Some(
            MessageSubtype::ChannelJoin
                | MessageSubtype::ChannelLeave
                | MessageSubtype::GroupJoin
                | MessageSubtype::GroupLeave
        )
    )
}

/// Whether `next` continues `message`'s run of joins (or of leaves), which Slack shows as one
/// line.
fn membership_run(message: &Message, next: &Message) -> bool {
    let leave = |m: &Message| {
        matches!(
            m.subtype,
            Some(MessageSubtype::ChannelLeave | MessageSubtype::GroupLeave)
        )
    };
    is_membership(message) && is_membership(next) && leave(message) == leave(next)
}

/// Slack's compact join / leave line: a person icon in the avatar gutter, "<name> joined
/// #channel." ("<name> and N others left #channel." for a run of `others` more) and the
/// time, all muted. Like Slack, the name opens that member's profile and the channel the
/// conversation.
fn membership_line(
    message: &Message,
    author: &str,
    others: usize,
    store: &MessageData,
    links: &LinkHandler,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    let channel = store
        .fixtures
        .conversation(&store.channel)
        .map(|c| store.fixtures.conversation_label(c).replacen("# ", "#", 1))
        .unwrap_or_default();
    let verb = match message.subtype {
        Some(MessageSubtype::ChannelLeave | MessageSubtype::GroupLeave) => "left",
        _ => "joined",
    };
    // The clickable spans: the member who joined (or the first of a run), then the channel.
    let member_span = |name: &str| {
        let id = format!("member-{}", message.ts.as_str());
        message.user.clone().map_or_else(
            || div().child(name.to_owned()).into_any_element(),
            |user| {
                membership_link(id, name.to_owned(), LinkTarget::Profile(user), links)
                    .into_any_element()
            },
        )
    };
    let channel_span = membership_link(
        format!("channel-{}", message.ts.as_str()),
        channel.clone(),
        LinkTarget::Channel(store.channel.clone()),
        links,
    );
    h_flex()
        .items_center()
        .gap_1()
        .py_1()
        .text_sm()
        .text_color(muted)
        .child(
            div()
                .w(px(AVATAR_SIZE))
                .flex_none()
                .flex()
                .justify_center()
                .child(
                    Icon::new(IconName::UserRound)
                        .text_color(muted)
                        .size(px(15.)),
                ),
        )
        .child(match others {
            0 => h_flex()
                .gap_1()
                .child(member_span(author))
                .child(verb)
                .child(channel_span)
                .child("."),
            1 => h_flex()
                .gap_1()
                .child(member_span(author))
                .child("and 1 other")
                .child(verb)
                .child(channel_span)
                .child("."),
            n => h_flex()
                .gap_1()
                .child(member_span(author))
                .child(format!("and {n} others"))
                .child(verb)
                .child(channel_span)
                .child("."),
        })
        .child(div().text_xs().child(format_clock(&message.ts)))
}

/// One muted, hover-underlined clickable span of a join/leave line.
fn membership_link(
    id: String,
    label: String,
    target: LinkTarget,
    links: &LinkHandler,
) -> Stateful<Div> {
    let links = links.clone();
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .hover(|el| el.underline())
        .on_click(move |_, window, cx| links.clone()(target.clone(), window, cx))
        .child(label)
}

/// Slack's unread marker: a danger-colored line with the "New" label at its right end.
fn new_messages_divider(cx: &App) -> Div {
    let danger = cx.theme().danger;
    h_flex()
        .items_center()
        .gap_2()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(danger))
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("New"),
        )
}

// ---------------------------------------------------------------------------
// Rich text rendering (guide section 10): a paragraph is one `StyledText` with styled
// `TextRun`s; links are runs made clickable through `InteractiveText` ranges, so a
// paragraph flows as a single block of text. Only inline images split it.
// ---------------------------------------------------------------------------

/// Inline style flags one text run carries.
#[derive(Clone, Copy, Default)]
struct RunFlags {
    bold: bool,
    semibold: bool,
    italic: bool,
    strike: bool,
    underline: bool,
    code: bool,
}

/// Theme-resolved fonts and colors shared by every run of one message render.
struct RunTheme {
    base_font: Font,
    /// Code blocks' face, sized by the block.
    mono_font: Font,
    /// Inline code's face: smaller glyphs at the body size (see
    /// [`crate::theme::INLINE_CODE_FAMILY`]).
    code_font: Font,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    link: Hsla,
    mention_bg: Hsla,
    /// Slack's tint behind chip links: address-labelled links and app chips.
    link_wash: Hsla,
    code_bg: Hsla,
    /// Inline `code` text (Slack's orange / red, see [`crate::theme::code_foreground`]).
    code_fg: Hsla,
    /// The window-selection highlight of selectable paragraphs.
    selection: Hsla,
    /// Selection document order of this theme's first paragraph; `None` renders plain,
    /// unselectable text (previews).
    order: Option<u64>,
    /// Routing for this text's links; `None` falls back to [`open_link_default`].
    links: Option<LinkHandler>,
    /// Paragraphs rendered with this theme so far: keeps each paragraph's element id unique
    /// within its message (GPUI keys interaction and accessibility state by the id path)
    /// and counts its selection document order.
    paragraphs: Cell<usize>,
    /// The message this render belongs to; text ids are also unique across messages.
    key: SharedString,
}

impl RunTheme {
    fn from_app(
        cx: &App,
        key: SharedString,
        order: Option<u64>,
        links: Option<LinkHandler>,
    ) -> Self {
        let theme = cx.theme();
        let base_font = Font {
            family: theme.font_family.clone(),
            ..Default::default()
        };
        Self {
            mono_font: Font {
                family: theme.mono_font_family.clone(),
                ..base_font.clone()
            },
            code_font: Font {
                family: crate::theme::inline_code_family(cx),
                ..base_font.clone()
            },
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            border: theme.border,
            link: theme.link,
            mention_bg: theme.accent.opacity(0.25),
            link_wash: theme.link.opacity(0.1),
            code_bg: theme.secondary,
            code_fg: crate::theme::code_foreground(cx),
            selection: gpui_kit::base::Theme::global(cx).tokens.colors.selection,
            order,
            links,
            base_font,
            paragraphs: Cell::new(0),
            key,
        }
    }

    /// The theme of selectable message-body text: paragraphs participate in the window
    /// selection from document order `order` on, links route through `links`.
    fn body(cx: &App, key: SharedString, order: u64, links: Option<LinkHandler>) -> Self {
        Self::from_app(cx, key, Some(order), links)
    }

    /// The theme of preview text: not selectable, links open as URLs.
    fn plain(cx: &App, key: SharedString) -> Self {
        Self::from_app(cx, key, None, None)
    }

    fn run(
        &self,
        run_flags: RunFlags,
        color: Option<Hsla>,
        bg: Option<Hsla>,
        len: usize,
    ) -> TextRun {
        let mut font = if run_flags.code {
            self.code_font.clone()
        } else {
            self.base_font.clone()
        };
        if run_flags.bold {
            font.weight = FontWeight::BOLD;
        } else if run_flags.semibold {
            font.weight = FontWeight::SEMIBOLD;
        }
        if run_flags.italic {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color: color.unwrap_or(if run_flags.code {
                self.code_fg
            } else {
                self.foreground
            }),
            background_color: bg,
            underline: run_flags.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            strikethrough: run_flags.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
        }
    }
}

fn flags(style: Option<&TextStyle>) -> RunFlags {
    let Some(style) = style else {
        return RunFlags::default();
    };
    RunFlags {
        bold: style.bold,
        semibold: false,
        italic: style.italic,
        strike: style.strike,
        underline: style.underline,
        code: style.code,
    }
}

/// One inline chunk of a paragraph: styled text (with its clickable link ranges), a custom
/// emoji image, or a link chip.
enum Segment {
    Text {
        text: String,
        runs: Vec<TextRun>,
        links: Vec<(Range<usize>, LinkTarget)>,
    },
    Image(ImageSource),
    /// Slack's chip for a link labelled with its own address (a link icon) or for a link to a
    /// message (a speech bubble): the icon and the label on a tinted pill.
    Chip {
        url: SharedString,
        label: String,
        icon: IconName,
        icon_size: f32,
    },
}

/// Accumulates a paragraph's styled text and link ranges, split only at inline images.
struct ParagraphBuilder {
    text: String,
    runs: Vec<TextRun>,
    links: Vec<(Range<usize>, LinkTarget)>,
    segments: Vec<Segment>,
}

impl ParagraphBuilder {
    fn new() -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            links: Vec::new(),
            segments: Vec::new(),
        }
    }

    fn push_text(
        &mut self,
        text: &str,
        theme: &RunTheme,
        run_flags: RunFlags,
        color: Option<Hsla>,
        bg: Option<Hsla>,
    ) {
        if text.is_empty() {
            return;
        }
        self.runs.push(theme.run(run_flags, color, bg, text.len()));
        self.text.push_str(text);
    }

    /// Slack's edit marker: a muted run at the paragraph's end that flows and wraps with the
    /// text. (A per-run font size is not expressible: one `StyledText` carries a single text
    /// size from its element, so the marker is muted at body size.)
    fn push_edited(&mut self, theme: &RunTheme) {
        self.push_text(
            " (edited)",
            theme,
            RunFlags::default(),
            Some(theme.muted),
            None,
        );
    }

    /// A link inside the paragraph's text: a link-colored run, clickable through its range.
    /// Like Slack, a URL posted bare shows as itself, scheme and all, while a link labelled
    /// with its own address minus the scheme (`github.com/…`) gets the tinted chip look.
    /// `run` is the label's styling (Slack bolds `<url|*Error*>` and links inside `*…*`).
    fn push_link(&mut self, url: String, label: String, run: RunFlags, theme: &RunTheme) {
        let address = |s: &str| {
            s.split_once("://")
                .map_or(s, |(_, rest)| rest)
                .trim_end_matches('/')
                .to_owned()
        };
        if label != url && address(&label) == address(&url) {
            self.push_chip(url.into(), label, IconName::Link, 12.);
            return;
        }
        let target = LinkTarget::Url(url.into());
        let start = self.text.len();
        self.push_text(&label, theme, run, Some(theme.link), None);
        self.links.push((start..self.text.len(), target));
    }

    /// An in-app mention (an @user or #channel chip): the mention look, clickable through
    /// its range like a link.
    fn push_mention(
        &mut self,
        label: &str,
        target: LinkTarget,
        theme: &RunTheme,
        bg: Option<Hsla>,
    ) {
        let start = self.text.len();
        self.push_text(label, theme, mention_flags(), Some(theme.link), bg);
        self.links.push((start..self.text.len(), target));
    }

    /// An app's inline link chip (`attachment_mention`, e.g. a Jira issue): its title on the
    /// link wash after a slot the paragraph paints the app icon into, one clickable range.
    fn push_attachment(&mut self, url: String, label: &str, theme: &RunTheme) {
        let start = self.text.len();
        for text in [APP_ICON_SLOT, label] {
            self.push_text(
                text,
                theme,
                RunFlags::default(),
                Some(theme.link),
                Some(theme.link_wash),
            );
        }
        self.links
            .push((start..self.text.len(), LinkTarget::Url(url.into())));
    }

    fn push_image(&mut self, image: ImageSource) {
        self.flush();
        self.segments.push(Segment::Image(image));
    }

    fn push_chip(&mut self, url: SharedString, label: String, icon: IconName, icon_size: f32) {
        self.flush();
        self.segments.push(Segment::Chip {
            url,
            label,
            icon,
            icon_size,
        });
    }

    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        let runs = std::mem::take(&mut self.runs);
        let links = std::mem::take(&mut self.links);
        self.segments.push(Segment::Text { text, runs, links });
    }

    /// Drop the line breaks the paragraph ends with: Slack starts what follows (a code
    /// block, a quote, a list or the message's end) on its own line without them.
    fn trim_trailing_breaks(&mut self) {
        while self.text.ends_with('\n') {
            self.text.pop();
            if let Some(last) = self.runs.last_mut() {
                last.len -= 1;
                if last.len == 0 {
                    self.runs.pop();
                }
            }
        }
    }

    fn finish(mut self) -> Vec<Segment> {
        self.flush();
        self.segments
    }
}

/// One block of a paragraph after splitting at blank-line runs: its text, runs and links
/// (the same shape [`Segment::Text`] carries).
type ParagraphBlock = (String, Vec<TextRun>, Vec<(Range<usize>, LinkTarget)>);

/// Slack renders a run of one or more blank lines inside message text as a single short
/// paragraph break, not an empty text line: the web client emits an 8px block
/// (`.c-mrkdwn__br { height: 8px; display: block }`) between the pieces. Splits one
/// paragraph's text (with its byte-offset runs and link ranges) at those runs so the
/// pieces can stack with the break between them; text without blank lines comes back as
/// the single block it always was.
fn paragraph_break_split(
    text: &str,
    runs: &[TextRun],
    links: &[(Range<usize>, LinkTarget)],
) -> Vec<ParagraphBlock> {
    let slice_block = |range: Range<usize>| {
        let block_text = text[range.clone()].to_owned();
        let mut block_runs = Vec::new();
        let mut at = 0;
        for run in runs {
            let run_range = at..at + run.len;
            at = run_range.end;
            let start = run_range.start.max(range.start);
            let end = run_range.end.min(range.end);
            if start < end {
                block_runs.push(TextRun {
                    len: end - start,
                    ..run.clone()
                });
            }
        }
        let block_links = links
            .iter()
            .filter_map(|(link, target)| {
                let start = link.start.max(range.start);
                let end = link.end.min(range.end);
                (start < end).then(|| ((start - range.start)..(end - range.start), target.clone()))
            })
            .collect();
        (block_text, block_runs, block_links)
    };

    let bytes = text.as_bytes();
    let mut blocks = Vec::new();
    let mut start = 0;
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] != b'\n' {
            ix += 1;
            continue;
        }
        let mut end = ix;
        while end < bytes.len() && bytes[end] == b'\n' {
            end += 1;
        }
        if end - ix >= 2 {
            blocks.push(slice_block(start..ix));
            start = end;
        }
        ix = end;
    }
    blocks.push(slice_block(start..text.len()));
    blocks
}

/// One text piece of a paragraph: selectable in the transcript when `order_base` is set,
/// plain (links still clickable) otherwise. Takes its own slot of the theme's paragraph
/// counter, so its id and document order stay unique however many pieces a paragraph
/// splits into.
fn text_element(
    text: String,
    runs: Vec<TextRun>,
    links: Vec<(Range<usize>, LinkTarget)>,
    n: usize,
    theme: &RunTheme,
    order_base: Option<u64>,
    routed: &Option<LinkHandler>,
) -> AnyElement {
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    if let Some(base) = order_base {
        return SelectableParagraph::new(
            paragraph_id(&theme.key, paragraph, n, &text),
            text,
            runs,
            links,
            base + paragraph as u64,
            theme,
        )
        .into_any_element();
    }
    if links.is_empty() {
        return StyledText::new(text).with_runs(runs).into_any_element();
    }
    let id = paragraph_id(&theme.key, paragraph, n, &text);
    let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let styled = StyledText::new(text).with_runs(runs);
    let routed = routed.clone();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| {
            route_link(routed.as_ref(), targets[ix].clone(), window, cx)
        })
        .into_any_element()
}

fn render_segments(segments: Vec<Segment>, theme: &RunTheme) -> Div {
    // Message text participates in the window selection (one participant per paragraph,
    // ordered with the transcript's reading order); preview text stays plain.
    let order_base = theme.order;
    let routed = theme.links.clone();
    // Text wraps only inside a width-constrained block: a lone text run (most paragraphs)
    // renders in a full-width block; runs mixed with inline images shrink to wrap in the row.
    let single = segments.len() == 1;
    let mut row = h_flex().min_w_0().flex_wrap();
    for (n, segment) in segments.into_iter().enumerate() {
        match segment {
            Segment::Text {
                mut text,
                mut runs,
                mut links,
            } => {
                // In a row mixed with chips or images, a line break at a text's edge must end
                // the row: inside the text it would only add an empty line to its own box.
                let (lead, trail) = if single {
                    (false, false)
                } else {
                    trim_breaks(&mut text, &mut runs, &mut links)
                };
                let blocks = paragraph_break_split(&text, &runs, &links);
                if single && blocks.len() == 1 {
                    let (text, runs, links) = blocks.into_iter().next().unwrap();
                    return div().w_full().min_w_0().child(text_element(
                        text, runs, links, n, theme, order_base, &routed,
                    ));
                }
                if lead {
                    row = row.child(div().w_full().h_0());
                }
                for (b, (text, runs, links)) in blocks.into_iter().enumerate() {
                    // The break between the pieces: Slack's 8px `c-mrkdwn__br` block, which
                    // the wrap row stacks between the two texts.
                    if b > 0 {
                        row = row.child(div().w_full().h(px(8.)));
                    }
                    if !text.is_empty() {
                        let element =
                            text_element(text, runs, links, n, theme, order_base, &routed);
                        row = row.child(div().min_w_0().child(element));
                    }
                }
                if trail {
                    row = row.child(div().w_full().h_0());
                }
            }
            Segment::Image(image) => {
                // A Picture: animated custom emoji play, as in Slack.
                row = row.child(
                    div()
                        .flex_none()
                        .size(px(INLINE_EMOJI))
                        .child(crate::media::Picture::new(image).radius(px(2.))),
                );
                continue;
            }
            Segment::Chip {
                url,
                label,
                icon,
                icon_size,
            } => {
                let id = format!("chip-{}-{}-{n}", theme.key, theme.paragraphs.get());
                let target = LinkTarget::Url(url.clone());
                let routed = routed.clone();
                row = row.child(
                    // Slack's pill is 1.3em tall, inside the line.
                    h_flex()
                        .id(SharedString::from(id))
                        .min_w_0()
                        .items_center()
                        .line_height(relative(1.3))
                        .gap_0p5()
                        .px_0p5()
                        .rounded_sm()
                        .bg(theme.link_wash)
                        .text_color(theme.link)
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            route_link(routed.as_ref(), target.clone(), window, cx)
                        })
                        .child(
                            Icon::new(icon)
                                .flex_none()
                                .size(px(icon_size))
                                .text_color(theme.link),
                        )
                        .child(label),
                );
            }
        }
    }
    row
}

/// The stable element id of one rendered paragraph: its position in the message plus a hash
/// of its text, unique within the message and across messages.
fn paragraph_id(key: &SharedString, paragraph: usize, n: usize, text: &str) -> ElementId {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    ElementId::Name(SharedString::from(format!(
        "text-{key}-{paragraph}-{n}-{:x}",
        hasher.finish()
    )))
}

/// Strip one line break from each edge of a text segment (keeping its runs and link ranges in
/// step) and report which edges had one.
fn trim_breaks(
    text: &mut String,
    runs: &mut Vec<TextRun>,
    links: &mut [(Range<usize>, LinkTarget)],
) -> (bool, bool) {
    let trail = text.ends_with('\n');
    if trail {
        text.pop();
        if let Some(last) = runs.last_mut() {
            last.len -= 1;
        }
    }
    let lead = text.starts_with('\n');
    if lead {
        text.remove(0);
        if let Some(first) = runs.first_mut() {
            first.len -= 1;
        }
        for (range, _) in links.iter_mut() {
            *range = range.start.saturating_sub(1)..range.end.saturating_sub(1);
        }
    }
    runs.retain(|run| run.len > 0);
    (lead, trail)
}

// ---------------------------------------------------------------------------
// Selectable message text (gpui-base window selection): one paragraph is one participant
// with its own retained handle, registered in document order so drags span paragraphs and
// messages of the transcript; copy joins the selected paragraphs with newlines. Link
// ranges stay clickable (a press without a drag activates one) and underlines on hover.
// ---------------------------------------------------------------------------

/// One paragraph of message-body text that participates in the window text selection: a
/// [`StyledText`] with styled runs whose link ranges stay clickable (see
/// gpui-base's `SelectableText` for the plain-run original, and its `PlainSelectableText`
/// test element for this registration pattern).
struct SelectableParagraph {
    id: ElementId,
    text: SharedString,
    styled: StyledText,
    links: Vec<(Range<usize>, LinkTarget)>,
    handler: Option<LinkHandler>,
    document_order: u64,
    selection: Hsla,
    link: Hsla,
    /// Inline code spans, outlined like Slack's `.c-mrkdwn__code` (a run-level background
    /// cannot carry a border).
    code: Vec<Range<usize>>,
    /// Run backgrounds (inline code, mentions, app chips), painted under the selection: as
    /// run backgrounds GPUI paints them with the text, over the highlight.
    washes: Vec<(Range<usize>, Hsla)>,
    code_border: Hsla,
    /// App chips' icon slots ([`APP_ICON_SLOT`]): text offsets the icons are painted at.
    icons: Vec<usize>,
}

/// What one paragraph keeps between frames: its selection handle (and the subscription
/// that redraws the window when a drag moves its selection), the link it hovers and the
/// link a press started in.
struct ParagraphState {
    handle: TextSelectionHandle,
    hovered_link: Rc<Cell<Option<usize>>>,
    mouse_down: Rc<Cell<Option<usize>>>,
    _refresh: Subscription,
}

impl SelectableParagraph {
    fn new(
        id: ElementId,
        text: String,
        mut runs: Vec<TextRun>,
        links: Vec<(Range<usize>, LinkTarget)>,
        document_order: u64,
        theme: &RunTheme,
    ) -> Self {
        let text: SharedString = text.into();
        // Inline code is the inline-code run with a wash; code blocks carry no run
        // background. An app chip's icon slot is its own run on the link wash.
        let mut code: Vec<Range<usize>> = Vec::new();
        let mut washes = Vec::new();
        let mut icons = Vec::new();
        let mut at = 0;
        for run in &mut runs {
            let span = at..at + run.len;
            at = span.end;
            let Some(wash) = run.background_color.take() else {
                continue;
            };
            if wash == theme.link_wash && text.get(span.clone()) == Some(APP_ICON_SLOT) {
                icons.push(span.start);
            }
            if run.font.family == theme.code_font.family {
                match code.last_mut() {
                    Some(last) if last.end == span.start => last.end = span.end,
                    _ => code.push(span.clone()),
                }
            }
            washes.push((span, wash));
        }
        Self {
            code,
            washes,
            icons,
            code_border: theme.foreground.opacity(0.13),
            styled: StyledText::new(text.clone()).with_runs(runs),
            id,
            text,
            links,
            handler: theme.links.clone(),
            document_order,
            selection: theme.selection,
            link: theme.link,
        }
    }

    /// The line-box quads a range covers: one per wrapped line between its ends (gpui-base's
    /// own selection painting, replicated because it is private there).
    fn span_quads(
        start: Point<Pixels>,
        end: Point<Pixels>,
        bounds: Bounds<Pixels>,
        line_height: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        if start.y == end.y {
            return vec![Bounds::from_corners(
                start,
                Point::new(end.x, end.y + line_height),
            )];
        }
        let mut quads = vec![Bounds::from_corners(
            start,
            Point::new(bounds.right(), start.y + line_height),
        )];
        if end.y > start.y + line_height {
            quads.push(Bounds::from_corners(
                Point::new(bounds.left(), start.y + line_height),
                Point::new(bounds.right(), end.y),
            ));
        }
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), end.y),
            Point::new(end.x, end.y + line_height),
        ));
        quads
    }

    /// Slack's inline-code outline: a 1px 13% foreground ring with 3px corners, a little
    /// wider than the glyphs and a little shorter than the line.
    fn paint_code_outline(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        for bounds in self.range_quads(layout, range) {
            let outline = Bounds::from_corners(
                Point::new(bounds.left() - px(2.), bounds.top() + px(1.)),
                Point::new(bounds.right() + px(2.), bounds.bottom() - px(1.)),
            );
            window.paint_quad(PaintQuad {
                bounds: outline,
                background: transparent_black().into(),
                corner_radii: Corners::all(px(3.)),
                border_widths: Edges::all(px(1.)),
                border_color: self.code_border,
                border_style: BorderStyle::default(),
            });
        }
    }

    /// An app chip's icon in its slot at `ix`, centered on the line in the link color. Slack
    /// draws the app's own glyph (Jira's mark); the bundled link glyph stands in for it.
    fn paint_app_icon(&self, layout: &TextLayout, ix: usize, window: &mut Window, cx: &App) {
        let Some(at) = layout.position_for_index(ix) else {
            return;
        };
        let side = px(APP_ICON);
        let bounds = Bounds::new(
            Point::new(at.x + px(1.), at.y + (layout.line_height() - side) / 2.),
            gpui_kit::gpui::Size::new(side, side),
        );
        let icon = IconName::Link.path();
        if let Err(e) = window.paint_svg(
            bounds,
            icon.clone(),
            None,
            TransformationMatrix::unit(),
            self.link,
            cx,
        ) {
            eprintln!("slackless: could not paint the {icon} icon ({e})");
        }
    }

    /// A run wash under the text, laid out as GPUI lays out a run background.
    fn paint_wash(
        &self,
        layout: &TextLayout,
        range: Range<usize>,
        wash: Hsla,
        window: &mut Window,
    ) {
        for bounds in self.range_quads(layout, range) {
            window.paint_quad(PaintQuad {
                bounds,
                background: wash.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The boxes a text range covers, one per wrapped line, each ending where that line's
    /// text ends (a selection instead runs on to the paragraph's edge).
    fn range_quads(&self, layout: &TextLayout, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return Vec::new();
        };
        let line_height = layout.line_height();
        let quad = |y: Pixels, left: Pixels, right: Pixels| {
            Bounds::from_corners(Point::new(left, y), Point::new(right, y + line_height))
        };
        if start.y == end.y {
            return vec![quad(start.y, start.x, end.x)];
        }
        // A wrapped span. The layout places a wrap boundary at the end of the line it closes,
        // so the farthest position found on a line is where that line's text ends; a line the
        // span only touches at such a boundary gets no box.
        let left = layout.bounds().left();
        let mut quads = Vec::new();
        let (mut y, mut from, mut to) = (start.y, start.x, start.x);
        for ix in (range.start..=range.end).filter(|ix| self.text.is_char_boundary(*ix)) {
            let Some(at) = layout.position_for_index(ix) else {
                continue;
            };
            if at.y == y {
                to = to.max(at.x);
                continue;
            }
            if to - from > px(1.) {
                quads.push(quad(y, from, to));
            }
            (y, from, to) = (at.y, left, at.x);
        }
        if to - from > px(1.) {
            quads.push(quad(y, from, to));
        }
        quads
    }

    /// The selection highlight behind the text.
    fn paint_selection(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        for bounds in Self::span_quads(start, end, layout.bounds(), layout.line_height()) {
            window.paint_quad(PaintQuad {
                bounds,
                background: self.selection.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// Slack's hover affordance on URL links: the hovered link underlines (mentions keep
    /// their highlight). Painted over the text, so it needs no re-layout.
    fn paint_hover_underline(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        let line_height = layout.line_height();
        for bounds in Self::span_quads(start, end, layout.bounds(), line_height) {
            let underline = Bounds::new(
                Point::new(bounds.origin.x, bounds.bottom() - px(4.)),
                gpui_kit::gpui::Size::new(bounds.size.width, px(1.)),
            );
            window.paint_quad(PaintQuad {
                bounds: underline,
                background: self.link.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The paragraph's link interactions: the pointing cursor over a link, the hover
    /// underline (with a redraw when the hovered link changes) and the click — a press and
    /// a release inside one link range, so a drag that selects never activates the link.
    #[allow(clippy::too_many_arguments)]
    fn paint_links(
        &self,
        global_id: Option<&GlobalElementId>,
        layout: &TextLayout,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let hovered_now = layout
            .index_for_position(window.mouse_position())
            .ok()
            .and_then(|ix| self.links.iter().position(|(range, _)| range.contains(&ix)));
        if hovered_now.is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        if let Some(ix) = hovered_now {
            if let Some((range, LinkTarget::Url(_))) = self.links.get(ix) {
                self.paint_hover_underline(layout, range.clone(), window);
            }
        }
        let (mouse_down, hovered_link) = window.with_element_state::<ParagraphState, _>(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |state, _| {
                let state = state.expect("request_layout retained the paragraph state");
                let cells = (state.mouse_down.clone(), state.hovered_link.clone());
                (cells, state)
            },
        );
        let down = {
            let layout = layout.clone();
            let mouse_down = mouse_down.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window: &mut Window, _: &mut App| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    if let Ok(ix) = layout.index_for_position(event.position) {
                        mouse_down.set(Some(ix));
                    }
                }
            }
        };
        window.on_mouse_event(down);
        let up = {
            let (layout, links, handler) =
                (layout.clone(), self.links.clone(), self.handler.clone());
            let hitbox = hitbox.clone();
            move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let Some(down) = mouse_down.take() else {
                    return;
                };
                let Ok(release) = layout.index_for_position(event.position) else {
                    return;
                };
                if let Some((_, target)) = links
                    .iter()
                    .find(|(range, _)| range.contains(&down) && range.contains(&release))
                {
                    route_link(handler.as_ref(), target.clone(), window, cx);
                }
            }
        };
        window.on_mouse_event(up);
        let r#move = {
            let (layout, links) = (layout.clone(), self.links.clone());
            let hitbox = hitbox.clone();
            move |event: &MouseMoveEvent, phase, window: &mut Window, _: &mut App| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let now = layout
                    .index_for_position(event.position)
                    .ok()
                    .and_then(|ix| links.iter().position(|(range, _)| range.contains(&ix)));
                if hovered_link.get() != now {
                    hovered_link.set(now);
                    window.refresh();
                }
            }
        };
        window.on_mouse_event(r#move);
    }
}

impl gpui_kit::gpui::IntoElement for SelectableParagraph {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableParagraph {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |retained: Option<ParagraphState>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(self.text.clone(), cx);
                    let refresh = handle.refresh_window_on_change(window, cx);
                    ParagraphState {
                        handle,
                        hovered_link: Rc::new(Cell::new(None)),
                        mouse_down: Rc::new(Cell::new(None)),
                        _refresh: refresh,
                    }
                });
                (retained.handle.clone(), retained)
            },
        );
        let (layout_id, ()) = self
            .styled
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.document_order)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        #[cfg(test)]
        tests::record_painted_paragraph(&self.text, &layout);
        let projection = handle.update_runs(
            &[
                TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)
                    .with_document_order(self.document_order),
            ],
            cx,
        );
        for (range, wash) in &self.washes {
            self.paint_wash(&layout, range.clone(), *wash, window);
        }
        for range in projection.ranges().iter().flatten().cloned() {
            self.paint_selection(&layout, range, window);
        }
        self.styled.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
        for range in &self.code {
            self.paint_code_outline(&layout, range.clone(), window);
        }
        for &ix in &self.icons {
            self.paint_app_icon(&layout, ix, window, cx);
        }
        // Selectable text takes the text cursor, as in Slack; links switch it to a hand.
        let over_link = layout
            .index_for_position(window.mouse_position())
            .ok()
            .is_some_and(|ix| self.links.iter().any(|(range, _)| range.contains(&ix)));
        if !over_link {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
        }
        if !self.links.is_empty() {
            self.paint_links(global_id, &layout, hitbox, window);
        }
    }
}

fn paragraph(
    elements: &[RichTextElement],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Option<Div> {
    let mut builder = ParagraphBuilder::new();
    for element in elements {
        push_element(&mut builder, element, fixtures, theme);
    }
    builder.trim_trailing_breaks();
    if edited {
        builder.push_edited(theme);
    }
    let segments = builder.finish();
    if segments.is_empty() {
        return None;
    }
    Some(render_segments(segments, theme))
}

fn push_element(
    builder: &mut ParagraphBuilder,
    element: &RichTextElement,
    fixtures: &Fixtures,
    theme: &RunTheme,
) {
    match element {
        RichTextElement::Text(el) => {
            // Inline code gets the same wash as mrkdwn's backticks.
            let run = flags(el.style.as_ref());
            let bg = run.code.then_some(theme.code_bg);
            builder.push_text(&el.text, theme, run, None, bg);
        }
        RichTextElement::Link(el) => {
            let label = el.text.clone().unwrap_or_else(|| el.url.clone());
            // Slack draws a link inside backticks as plain code, not a link.
            let run = flags(el.style.as_ref());
            if run.code {
                builder.push_text(&label, theme, run, None, Some(theme.code_bg));
            } else {
                builder.push_link(el.url.clone(), label, run, theme);
            }
        }
        RichTextElement::User(el) => {
            let label = format!("@{}", fixtures.mention_name(&el.user_id));
            builder.push_mention(
                &label,
                LinkTarget::Profile(el.user_id.clone()),
                theme,
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Channel(el) => {
            let label = fixtures
                .conversation(&el.channel_id)
                .map(|conversation| fixtures.conversation_label(conversation))
                .unwrap_or_else(|| format!("#{}", el.channel_id));
            builder.push_mention(
                &label,
                LinkTarget::Channel(el.channel_id.clone()),
                theme,
                None,
            );
        }
        RichTextElement::Usergroup(_) => {
            builder.push_text(
                "@group",
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Team(_) => {
            builder.push_text(
                "@workspace",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Broadcast(el) => {
            let label = format!("@{}", el.range.as_str());
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Emoji(el) => {
            let glyph = el
                .unicode
                .as_deref()
                .and_then(unicode_glyph)
                .or_else(|| emoji::display_glyph(&el.name));
            match (glyph, fixtures.custom_emoji(&el.name)) {
                (Some(glyph), _) => {
                    builder.push_text(&glyph, theme, flags(el.style.as_ref()), None, None);
                }
                (None, Some(image)) => builder.push_image(image.source()),
                (None, None) => {
                    let literal = format!(":{}:", el.name);
                    builder.push_text(&literal, theme, flags(el.style.as_ref()), None, None);
                }
            }
        }
        RichTextElement::Date(el) => {
            let text = el.fallback.clone().unwrap_or_else(|| {
                if el.timestamp > 0 {
                    date_label_absolute(&CivilTime::from_unix(el.timestamp))
                } else {
                    el.format.clone()
                }
            });
            builder.push_text(&text, theme, RunFlags::default(), None, None);
        }
        RichTextElement::Color(el) => {
            builder.push_text(
                &el.value,
                theme,
                RunFlags::default(),
                hex_color(&el.value),
                None,
            );
        }
        RichTextElement::MessageMention(el) => {
            // Slack's chip for a link to a message: a speech bubble and "Author in #channel",
            // opening the message (its thread, for a reply).
            let author = el
                .author_id
                .as_ref()
                .and_then(|user| fixtures.user(user))
                .map(|user| fixtures.name_of(user).to_owned());
            let place = fixtures
                .conversation(&el.channel_id)
                .and_then(|conversation| conversation.name.as_ref())
                .map(|name| format!("#{name}"));
            let label = match (author, place) {
                (Some(author), Some(place)) => format!("{author} in {place}"),
                (Some(label), None) | (None, Some(label)) => label,
                (None, None) => el.text.clone().unwrap_or_else(|| "a message".to_owned()),
            };
            match &el.url {
                Some(url) => {
                    builder.push_chip(url.clone().into(), label, IconName::MessageCircle, 15.)
                }
                None => {
                    builder.push_text(&label, theme, RunFlags::default(), Some(theme.link), None)
                }
            }
        }
        RichTextElement::AttachmentMention(el) => {
            builder.push_attachment(el.url.clone(), el.text.as_deref().unwrap_or(&el.url), theme);
        }
        RichTextElement::Canvas(_) => {
            builder.push_text(
                "[canvas]",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Unknown { .. } => {}
    }
}

/// Block-level rich text containers: paragraphs, lists, quotes and code blocks. With
/// `edited`, the final container's plain text (a section, or a list's last item) carries the
/// edit marker inline at its end; boxed text (quotes) and code blocks leave it to the body's
/// own trailing line.
fn render_containers(
    containers: &[RichTextContainer],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Vec<AnyElement> {
    containers
        .iter()
        .enumerate()
        .filter_map(|(n, container)| {
            let edited = edited && n + 1 == containers.len();
            match container {
                RichTextContainer::Section(section) => {
                    paragraph(&section.elements, fixtures, theme, edited)
                        .map(IntoElement::into_any_element)
                }
                RichTextContainer::List(list) if !list.elements.is_empty() => {
                    Some(render_list(list, fixtures, theme, edited).into_any_element())
                }
                RichTextContainer::Quote(quote) => {
                    paragraph(&quote.elements, fixtures, theme, false).map(|paragraph| {
                        div()
                            .border_l_2()
                            .border_color(theme.border)
                            .pl_3()
                            .child(paragraph)
                            .into_any_element()
                    })
                }
                RichTextContainer::Preformatted(preformatted) => {
                    Some(render_code(preformatted, theme).into_any_element())
                }
                _ => None,
            }
        })
        .collect()
}

fn render_list(list: &RichTextList, fixtures: &Fixtures, theme: &RunTheme, edited: bool) -> Div {
    let depth = list
        .indent
        .unwrap_or(0)
        .saturating_add(list.offset.unwrap_or(0));
    // Slack's disc sits 10px in, its text 28px in, nesting 22px a level.
    let indent = px(10. + 22. * depth as f32);
    let mut column = v_flex().gap_1();
    for (n, item) in list.elements.iter().enumerate() {
        let elements: &[RichTextElement] = match item {
            RichTextContainer::Section(section) => &section.elements,
            _ => &[],
        };
        if elements.is_empty() {
            continue;
        }
        let ordered = matches!(list.style, ListStyle::Ordered);
        let marker = match &list.style {
            ListStyle::Ordered => format!("{}.", n + 1),
            _ => "•".to_owned(),
        };
        let last = n + 1 == list.elements.len();
        if let Some(item_row) = paragraph(elements, fixtures, theme, edited && last) {
            column = column.child(
                h_flex()
                    .items_start()
                    .pl(indent)
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .when(!ordered, |marker| {
                                marker.text_size(px(20.)).line_height(px(22.))
                            })
                            .text_color(theme.foreground)
                            .child(marker),
                    )
                    .child(item_row),
            );
        }
    }
    column
}

fn render_code(block: &RichTextPreformatted, theme: &RunTheme) -> Div {
    let code = block
        .elements
        .iter()
        .filter_map(|element| match element {
            RichTextElement::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<String>();
    code_block(
        code.trim_end_matches('\n').to_owned(),
        block
            .language
            .clone()
            .filter(|language| !language.is_empty()),
        theme,
    )
}

/// Slack's pre block: 12px code on an 18px line, 8px in from a 13% foreground ring with 4px
/// corners, 32px kept clear at the right for the copy button a hover shows there.
fn code_block(code: String, language: Option<String>, theme: &RunTheme) -> Div {
    let run_len = code.len();
    // Code blocks select like the paragraphs around them: one participant, its own order.
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    let order = theme.order.map(|base| base + paragraph as u64);
    let mut run = theme.run(
        RunFlags {
            code: true,
            ..DEFAULT_FLAGS
        },
        // Code blocks keep the body color; only inline code is tinted.
        Some(theme.foreground),
        None,
        run_len,
    );
    // The block's own 12px sizes it; the smaller inline face is for body-size text.
    run.font = theme.mono_font.clone();
    let group = SharedString::from(format!("code-{}-{paragraph}", theme.key));
    let copy = CopyCode {
        id: ElementId::Name(format!("copy-{}-{paragraph}", theme.key).into()),
        group: group.clone(),
        code: code.clone(),
    };
    let text = match order {
        Some(order) => SelectableParagraph::new(
            paragraph_id(&theme.key, paragraph, 0, &code),
            code,
            vec![run],
            Vec::new(),
            order,
            theme,
        )
        .into_any_element(),
        None => StyledText::new(code)
            .with_runs(vec![run])
            .into_any_element(),
    };
    v_flex()
        .min_w_0()
        .gap_1()
        .when_some(language, |column, language| {
            column.child(div().text_xs().text_color(theme.muted).child(language))
        })
        .child(
            div()
                .group(group)
                .relative()
                .w_full()
                .min_w_0()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.foreground.opacity(0.13))
                .bg(theme.code_bg)
                .p_2()
                .pr(px(32.))
                .text_size(px(12.))
                .line_height(px(18.))
                .child(text)
                .child(copy),
        )
}

/// Slack's copy button on a pre block: the block's hover reveals it with a "Copy" tooltip;
/// a click copies the code and shows a check in a focus ring under "Copied!" until the
/// pointer leaves.
#[derive(IntoElement)]
struct CopyCode {
    id: ElementId,
    /// The pre block's hover group, which reveals the button.
    group: SharedString,
    code: String,
}

#[derive(Clone, Copy, Default)]
struct CopyCodeState {
    hovered: bool,
    copied: bool,
    /// Clicks so far: a click's timer clears `copied` only if no later click restarted it.
    copies: u32,
}

/// How long "Copied!" stays up after a click, the pointer gone or not.
const COPIED_FOR: Duration = Duration::from_secs(2);

impl RenderOnce for CopyCode {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| CopyCodeState::default());
        let CopyCodeState {
            hovered, copied, ..
        } = *state.read(cx);
        let theme = cx.theme();
        let (fg, muted, ring) = (theme.foreground, theme.muted_foreground, theme.ring);
        // The pre block's own fill (`RunTheme::code_bg`).
        let block_bg = theme.secondary;
        let hover_bg = if copied { block_bg } else { fg.opacity(0.08) };
        let tip = (hovered || copied).then(|| {
            deferred(code_tooltip(if copied { "Copied!" } else { "Copy" }, cx)).with_priority(1)
        });
        div()
            .id(self.id)
            .absolute()
            .top(px(3.))
            .right(px(3.))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .opacity(0.)
            .group_hover(self.group, |button| button.opacity(1.))
            .when(hovered || copied, |button| button.opacity(1.))
            .hover(move |button| button.bg(hover_bg))
            .when(copied, |button| {
                // Slack's focus ring: a 1px line in a translucent 4px halo. GPUI paints a
                // shadow under the whole box, so an opaque fill keeps the halo outside it.
                button
                    .bg(block_bg)
                    .border_1()
                    .border_color(ring)
                    .shadow(vec![gpui_kit::gpui::BoxShadow {
                        color: ring.opacity(0.3),
                        offset: gpui_kit::gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(4.),
                        inset: false,
                    }])
            })
            .on_hover({
                let state = state.clone();
                move |hovered, window, cx| {
                    state.update(cx, |state, _| state.hovered = *hovered);
                    // Notifying the state alone repaints the transcript one event late.
                    window.refresh();
                }
            })
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(self.code.clone()));
                let copy = state.update(cx, |state, _| {
                    state.copied = true;
                    state.copies += 1;
                    state.copies
                });
                window.refresh();
                let (state, window) = (state.clone(), window.window_handle());
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    state.update(cx, |state, _| state.copied &= state.copies != copy);
                    // A closed window has no button left to update.
                    let _ = window.update(cx, |_, window, _| window.refresh());
                })
                .detach();
            })
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(px(14.))
                .text_color(if hovered || copied { fg } else { muted }),
            )
            .children(tip)
    }
}

/// The tooltip over a pre block's copy button: Slack's bubble, 13px bold in a 1px ring, its
/// arrow pointing down at the button 2px below.
fn code_tooltip(text: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    let (fill, ink, edge) = (
        theme.popover,
        theme.popover_foreground,
        theme.foreground.opacity(0.2),
    );
    div()
        .absolute()
        .bottom(px(30.))
        .left(px(-46.))
        .w(px(120.))
        .flex()
        .justify_center()
        .child(
            v_flex()
                .items_center()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(edge)
                        .bg(fill)
                        .text_color(ink)
                        .text_size(px(13.))
                        .line_height(px(16.25))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_nowrap()
                        .child(text),
                )
                .child(
                    // The arrow keeps the ring: a fill triangle 1.5px above a ring-colored
                    // one leaves a 1px edge and covers the bubble's ring where they meet.
                    div()
                        .relative()
                        .mt(px(-1.))
                        .child(crate::sidebar::tooltip_arrow(edge, false))
                        .child(
                            div()
                                .absolute()
                                .top(px(-1.5))
                                .left_0()
                                .child(crate::sidebar::tooltip_arrow(fill, false)),
                        ),
                ),
        )
}

/// Whether the body's final rendered paragraph is plain text: a rich-text section or list
/// at the end of the blocks, or the mrkdwn fallback when no block renders anything. Slack's
/// edit marker rides inline at that paragraph's end; any other body (Block Kit block,
/// attachment, image) puts the marker on its own line after the body.
fn body_ends_in_text(message: &Message) -> bool {
    if !message.attachments.is_empty() {
        return false;
    }
    let fallback_text = message
        .blocks
        .iter()
        .all(|block| matches!(block, Block::RichText(rich) if rich.elements.is_empty()))
        && !message.text.trim().is_empty();
    let ends_in_paragraph = matches!(
        message.blocks.last(),
        Some(Block::RichText(rich))
            if matches!(
                rich.elements.last(),
                // After a list Slack puts the edit marker on its own line.
                Some(RichTextContainer::Section(_))
            )
    );
    fallback_text || ends_in_paragraph
}

fn message_body(message: &Message, kcx: &KitCx, cx: &App) -> Div {
    let has_kit = message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Unknown { .. }));
    let mut column = v_flex()
        .min_w_0()
        .gap_1()
        .text_sm()
        .when(has_kit || !message.attachments.is_empty(), |column| {
            column.gap_2()
        });
    let edited = message.edited.is_some();
    let inline_edited = edited && body_ends_in_text(message);
    let attachments = blockkit::attachments(message, kcx, cx);
    let mut rendered = 0;
    let last_block = message.blocks.len().saturating_sub(1);
    for (ix, block) in message.blocks.iter().enumerate() {
        let elements = render_block(block, ix, kcx, inline_edited && ix == last_block, cx);
        rendered += elements.len();
        column = column.children(elements);
    }
    if rendered == 0 && !message.text.is_empty() {
        column = column.child(mrkdwn(&message.text, kcx, false, inline_edited, cx));
    }
    if edited && !inline_edited {
        let muted = cx.theme().muted_foreground;
        column = column.child(div().text_xs().text_color(muted).child("(edited)"));
    }
    column.children(attachments)
}

/// One block of a message (or attachment): `rich_text` here, everything else through
/// [`blockkit::block`].
pub(crate) fn render_block(
    block: &Block,
    ix: usize,
    kcx: &KitCx,
    edited: bool,
    cx: &App,
) -> Vec<AnyElement> {
    match block {
        Block::RichText(rich) => {
            let (key, order) = kcx.text_theme();
            render_containers(
                &rich.elements,
                kcx.fixtures,
                &RunTheme::body(cx, key.into(), order, kcx.links.clone()),
                edited,
            )
        }
        Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
            .into_iter()
            .collect(),
    }
}

/// Renders Slack mrkdwn (fallback text, Block Kit text objects, attachment text), one line
/// per paragraph: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<@user>` / `<#channel>` /
/// `<!here>` chips, `<url|label>` links and `:emoji:` (custom emoji as images). `muted`
/// renders plain runs in the muted color (Block Kit context blocks). Unknown markup stays
/// literal. With a link handler (see [`KitCx`]) the text is selectable and its links route
/// through it.
pub(crate) fn mrkdwn(text: &str, kcx: &KitCx, muted: bool, edited: bool, cx: &App) -> Div {
    let (key, order) = kcx.text_theme();
    let mut theme = RunTheme::body(cx, key.into(), order, kcx.links.clone());
    if muted {
        theme.foreground = theme.muted;
    }
    let pieces = mrkdwn_fences(text);
    let mut column = v_flex().min_w_0();
    for (n, (fenced, piece)) in pieces.iter().enumerate() {
        column = if *fenced {
            column.child(
                div()
                    .my_1()
                    .child(code_block(unescape(piece), None, &theme)),
            )
        } else {
            mrkdwn_lines(
                column,
                piece,
                kcx.fixtures,
                &theme,
                edited && n + 1 == pieces.len(),
            )
        };
    }
    // A body ending in a pre block puts the edit marker on its own line after it.
    if edited && pieces.last().is_some_and(|(fenced, _)| *fenced) {
        column = column.child(div().text_xs().text_color(theme.muted).child("(edited)"));
    }
    column
}

/// mrkdwn split at its ``` fences into `(fenced, text)` pieces: Slack renders fenced text
/// as a pre block, raw, dropping the line break just inside and outside each fence. An
/// unclosed fence stays literal; empty text pieces are left out.
fn mrkdwn_fences(text: &str) -> Vec<(bool, &str)> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let inner = &rest[open + 3..];
        let Some(close) = inner.find("```") else {
            break;
        };
        let before = &rest[..open];
        pieces.push((false, before.strip_suffix('\n').unwrap_or(before)));
        let code = &inner[..close];
        let code = code.strip_prefix('\n').unwrap_or(code);
        pieces.push((true, code.strip_suffix('\n').unwrap_or(code)));
        let after = &inner[close + 3..];
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    pieces.push((false, rest));
    pieces.retain(|(fenced, piece)| *fenced || !piece.is_empty());
    pieces
}

/// Lines of mrkdwn text into `column`; with `edited`, the edit marker rides inline at the
/// end of the last line that renders text.
fn mrkdwn_lines(
    mut column: Div,
    text: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Div {
    let lines: Vec<&str> = text.split('\n').collect();
    let last_text_line = lines.iter().rposition(|line| !line.is_empty());
    let mut blank = false;
    for (n, line) in lines.iter().enumerate() {
        // A run of blank lines is one 8px break, like the rich-text path's `c-mrkdwn__br`.
        if line.is_empty() {
            if !blank {
                column = column.child(div().h(px(8.)));
            }
            blank = true;
            continue;
        }
        blank = false;
        if let Some(row) = mrkdwn_line(line, fixtures, theme, edited && Some(n) == last_text_line) {
            column = column.child(row);
        }
    }
    column
}

/// A one-paragraph mrkdwn preview (the DM list): formatting, links and emoji as the
/// transcript renders them, plain runs in `color`, line breaks flattened. Preview text is
/// not selectable; links open as URLs.
pub(crate) fn mrkdwn_preview(
    text: &str,
    fixtures: &Fixtures,
    color: Hsla,
    key: String,
    cx: &App,
) -> Div {
    let mut theme = RunTheme::plain(cx, key.into());
    theme.foreground = color;
    mrkdwn_line(&text.replace('\n', " "), fixtures, &theme, false).unwrap_or_else(div)
}

fn mrkdwn_line(line: &str, fixtures: &Fixtures, theme: &RunTheme, edited: bool) -> Option<Div> {
    let mut builder = ParagraphBuilder::new();
    let mut run = RunFlags::default();
    let mut rest = line;
    let mut prev: Option<char> = None;
    while let Some(start) = rest.find(['<', ':', '*', '_', '~', '`']) {
        let (before, tail) = rest.split_at(start);
        push_mrkdwn_text(&mut builder, before, theme, run);
        prev = before.chars().last().or(prev);
        rest = tail;
        let marker = rest.as_bytes()[0] as char;
        let handled = if matches!(marker, '*' | '_' | '~' | '`') {
            toggle_marker(&mut run, marker, prev, &rest[1..]).then_some(1)
        } else if run.code {
            None
        } else if marker == '<' {
            angle_token(&mut builder, rest, fixtures, theme, run)
        } else {
            emoji_token(&mut builder, rest, fixtures, theme, run)
        };
        let len = handled.unwrap_or_else(|| {
            push_mrkdwn_text(&mut builder, &rest[..1], theme, run);
            1
        });
        prev = rest[..len].chars().last();
        rest = &rest[len..];
    }
    push_mrkdwn_text(&mut builder, rest, theme, run);
    if edited {
        builder.push_edited(theme);
    }
    let segments = builder.finish();
    if segments.is_empty() {
        return None;
    }
    Some(render_segments(segments, theme))
}

/// `text` with Slack's three mrkdwn HTML escapes undone.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Plain mrkdwn text with Slack's HTML escapes undone; inline code gets its wash.
fn push_mrkdwn_text(builder: &mut ParagraphBuilder, text: &str, theme: &RunTheme, run: RunFlags) {
    let bg = run.code.then_some(theme.code_bg);
    builder.push_text(&unescape(text), theme, run, None, bg);
}

/// Open or close the style `marker` stands for. A marker opens at a word start when a closing
/// marker follows, and closes at a word end; anything else is literal.
fn toggle_marker(run: &mut RunFlags, marker: char, prev: Option<char>, after: &str) -> bool {
    if run.code && marker != '`' {
        return false;
    }
    let flag = match marker {
        '*' => &mut run.bold,
        '_' => &mut run.italic,
        '~' => &mut run.strike,
        _ => &mut run.code,
    };
    if *flag {
        let ends_word = after.chars().next().is_none_or(|c| !c.is_alphanumeric());
        if ends_word && prev.is_some_and(|c| !c.is_whitespace()) {
            *flag = false;
            return true;
        }
        return false;
    }
    let starts_word = prev.is_none_or(|c| !c.is_alphanumeric());
    let opens = after.chars().next().is_some_and(|c| !c.is_whitespace()) && after.contains(marker);
    if starts_word && opens {
        *flag = true;
    }
    starts_word && opens
}

/// A `<...>` token at the start of `rest`: user / channel / special mentions and links. A
/// link takes the surrounding styling `run` plus its label's own.
fn angle_token(
    builder: &mut ParagraphBuilder,
    rest: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    run: RunFlags,
) -> Option<usize> {
    let close = rest.find('>')?;
    let inner = &rest[1..close];
    let (target, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(label)),
        None => (inner, None),
    };
    if let Some(user) = target.strip_prefix('@') {
        let name = format!("@{}", fixtures.mention_name(&UserId::from(user)));
        builder.push_mention(
            &name,
            LinkTarget::Profile(UserId::from(user)),
            theme,
            Some(theme.mention_bg),
        );
    } else if let Some(channel) = target.strip_prefix('#') {
        let name = label
            .map(|label| format!("#{label}"))
            .or_else(|| {
                fixtures
                    .conversation(&ChannelId::from(channel))
                    .map(|conversation| fixtures.conversation_label(conversation))
            })
            .unwrap_or_else(|| format!("#{channel}"));
        builder.push_mention(
            &name,
            LinkTarget::Channel(ChannelId::from(channel)),
            theme,
            None,
        );
    } else if let Some(special) = target.strip_prefix('!') {
        if special.starts_with("date") {
            builder.push_text(
                label.unwrap_or(special),
                theme,
                RunFlags::default(),
                None,
                None,
            );
        } else {
            let name = label
                .map(str::to_owned)
                .unwrap_or_else(|| format!("@{}", special.split('^').next().unwrap_or(special)));
            builder.push_text(
                &name,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
    } else if target.contains("://") || target.starts_with("mailto:") {
        let (label, run) = styled_label(label.unwrap_or(target), run);
        builder.push_link(unescape(target), unescape(label), run, theme);
    } else {
        return None;
    }
    Some(close + 1)
}

/// A link label's own styling: Slack renders `<url|*TypeError*>` as a bold link. Markers
/// wrapping the whole label add their style to `run`; any others stay literal.
fn styled_label(mut label: &str, mut run: RunFlags) -> (&str, RunFlags) {
    while let Some(marker) = label
        .chars()
        .next()
        .filter(|c| matches!(c, '*' | '_' | '~'))
    {
        if label.len() < 3 || !label.ends_with(marker) {
            break;
        }
        match marker {
            '*' => run.bold = true,
            '_' => run.italic = true,
            _ => run.strike = true,
        }
        label = &label[1..label.len() - 1];
    }
    (label, run)
}

/// A `:shortcode:` at the start of `rest`: a glyph run, or a custom emoji image.
fn emoji_token(
    builder: &mut ParagraphBuilder,
    rest: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    run: RunFlags,
) -> Option<usize> {
    let (len, code) = emoji::parse_shortcode(rest)?;
    match emoji::display_glyph(&code) {
        Some(glyph) => builder.push_text(&glyph, theme, run, None, None),
        None => builder.push_image(fixtures.custom_emoji(&code)?.source()),
    }
    Some(len)
}

/// Slack packs emoji code points as hex joined by `-` (`1f389`, skin tones appended).
fn unicode_glyph(hex: &str) -> Option<String> {
    let points = hex
        .split('-')
        .map(|point| u32::from_str_radix(point, 16).ok())
        .collect::<Option<Vec<u32>>>()?;
    Some(
        points
            .iter()
            .filter_map(|&point| char::from_u32(point))
            .collect(),
    )
}

pub(crate) fn hex_color(value: &str) -> Option<Hsla> {
    let hex = value.trim_start_matches('#');
    if hex.len() == 6 {
        u32::from_str_radix(hex, 16)
            .ok()
            .map(|color| Hsla::from(rgb(color)))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Local-time formatting: no chrono/time dependency, so the platform `localtime`
// provides the UTC offset and the civil conversion is done by hand.
// ---------------------------------------------------------------------------

/// Local wall-clock fields of a Unix timestamp, used for clock and date-divider labels.
struct CivilTime {
    year: i32,
    month: u32,
    day: u32,
    /// Local days since the Unix epoch; date breaks compare this.
    day_index: i64,
    hour: u32,
    minute: u32,
}

impl CivilTime {
    fn from_unix(secs: i64) -> Self {
        let shifted = secs + local_utc_offset(secs);
        let day_index = shifted.div_euclid(86_400);
        let second_of_day = shifted.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(day_index);
        CivilTime {
            year,
            month,
            day,
            day_index,
            hour: u32::try_from(second_of_day / 3_600).unwrap_or(0),
            minute: u32::try_from((second_of_day % 3_600) / 60).unwrap_or(0),
        }
    }

    fn now() -> Self {
        Self::from_unix(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(0),
        )
    }
}

fn day_index(ts: &Ts) -> Option<i64> {
    ts.secs()
        .map(|secs| (secs + local_utc_offset(secs)).div_euclid(86_400))
}

fn format_clock(ts: &Ts) -> String {
    match ts.secs() {
        Some(secs) => {
            let time = CivilTime::from_unix(secs);
            let hour12 = match time.hour % 12 {
                0 => 12,
                hour => hour,
            };
            let meridiem = if time.hour < 12 { "AM" } else { "PM" };
            format!("{}:{:02} {}", hour12, time.minute, meridiem)
        }
        None => ts.as_str().to_owned(),
    }
}

/// Current Unix seconds.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// `3:42 PM` for `secs` on this machine's clock, or in the zone `utc_offset` seconds from UTC.
pub(crate) fn clock(secs: i64, utc_offset: Option<i64>) -> String {
    let shifted = secs + utc_offset.unwrap_or_else(|| local_utc_offset(secs));
    let second_of_day = shifted.rem_euclid(86_400);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let hour12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    let meridiem = if hour < 12 { "AM" } else { "PM" };
    format!("{hour12}:{minute:02} {meridiem}")
}

/// This machine's offset from UTC right now, in seconds.
pub(crate) fn local_offset_now() -> i64 {
    local_utc_offset(now_secs())
}

/// Compact stamp for list rows (DM list, activity, search): the clock time for today's
/// messages, just the weekday within the past week ("Monday"), the divider's day label
/// otherwise.
pub(crate) fn list_stamp(ts: &Ts) -> String {
    let now = CivilTime::now();
    match ts.secs().map(CivilTime::from_unix) {
        Some(time) if time.day_index == now.day_index => format_clock(ts),
        // A week back still reads as the weekday ("Friday" on a Friday), as Slack shows it.
        Some(time) if now.day_index - time.day_index <= 7 => {
            let label = date_label(&time, &now);
            match label.split_once(',') {
                Some((weekday, _)) => weekday.to_owned(),
                None => label,
            }
        }
        Some(time) => date_label(&time, &now),
        None => ts.as_str().to_owned(),
    }
}

/// When a file was created, as the Files view words it after "Created": "today",
/// "yesterday", "on October 1st", or "on January 19th, 2025" outside the current year.
pub(crate) fn file_day(secs: i64) -> String {
    let now = CivilTime::now();
    let time = CivilTime::from_unix(secs);
    let label = date_label(&time, &now);
    if time.day_index == now.day_index || time.day_index + 1 == now.day_index {
        return label.to_lowercase();
    }
    // Within the current year the label leads with the weekday: "Monday, October 1st".
    match label.split_once(", ") {
        Some((_, day)) if time.year == now.year => format!("on {day}"),
        _ => format!("on {label}"),
    }
}

/// The stamp on a message card outside the day-divided transcript (threads, search, pins):
/// like Slack, the time for today's messages, "Yesterday at …", the weekday within the past
/// week ("Tuesday at 9:01 AM"), the full date before that.
fn card_stamp(ts: &Ts) -> String {
    let Some(secs) = ts.secs() else {
        return ts.as_str().to_owned();
    };
    let day = CivilTime::from_unix(secs).day_index;
    match CivilTime::now().day_index - day {
        0 => format_clock(ts),
        2..=6 => format!("{} at {}", weekday_name(day), format_clock(ts)),
        _ => day_and_time(ts),
    }
}

/// The weekday of a day counted from the Unix epoch, which began on a Thursday.
fn weekday_name(day_index: i64) -> &'static str {
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    WEEKDAYS[(day_index + 4).rem_euclid(7) as usize]
}

/// The day header of a day-grouped list (the Activity page): the divider's day label
/// (`Today`, `Yesterday`, `Tuesday, November 10th`), or the raw ts when it does not parse.
pub(crate) fn day_label(ts: &Ts) -> String {
    ts.secs()
        .map(|secs| date_label(&CivilTime::from_unix(secs), &CivilTime::now()))
        .unwrap_or_else(|| ts.as_str().to_owned())
}

/// Slack's long stamp (attachment footers, thread replies from other days):
/// `Yesterday at 9:25 AM`, `Today at 3:01 PM`, `Tuesday, November 10th at 8:00 AM`.
pub(crate) fn day_and_time(ts: &Ts) -> String {
    match ts.secs() {
        Some(secs) => format!(
            "{} at {}",
            date_label(&CivilTime::from_unix(secs), &CivilTime::now()),
            format_clock(ts)
        ),
        None => ts.as_str().to_owned(),
    }
}

/// `2026-11-19` (local date) for Unix `secs`; Block Kit date pickers use this format.
pub(crate) fn iso_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
}

/// Slack's divider label: `Today` / `Yesterday` / `Tuesday, November 10th`, plus `, 2025`
/// outside the current year.
fn date_label(time: &CivilTime, now: &CivilTime) -> String {
    const MONTH_NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    if time.day_index == now.day_index {
        return "Today".to_owned();
    }
    if time.day_index + 1 == now.day_index {
        return "Yesterday".to_owned();
    }
    let weekday = weekday_name(time.day_index);
    let month = MONTH_NAMES[(time.month - 1) as usize];
    let day = time.day;
    let suffix = ordinal_suffix(day);
    // Slack names the weekday only within the current year: "October 25th, 2022".
    if time.year == now.year {
        format!("{weekday}, {month} {day}{suffix}")
    } else {
        format!("{month} {day}{suffix}, {}", time.year)
    }
}

/// `st`, `nd`, `rd` or `th` after `day`.
fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// A shared message's day as Slack's card for it shows it: `Nov 18th`, plus `, 2025` outside
/// the current year.
pub(crate) fn short_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    let day = format!(
        "{} {}{}",
        MONTHS[(time.month - 1) as usize],
        time.day,
        ordinal_suffix(time.day)
    );
    if time.year == CivilTime::now().year {
        day
    } else {
        format!("{day}, {}", time.year)
    }
}

fn date_label_absolute(time: &CivilTime) -> String {
    format!("{} {}", MONTHS[(time.month - 1) as usize], time.day)
}

/// Offset from UTC in seconds at `unix_secs`, honoring DST, from the platform `localtime`.
#[cfg(unix)]
fn local_utc_offset(unix_secs: i64) -> i64 {
    /// `struct tm` as laid out by glibc and Darwin: nine `int` fields then
    /// `tm_gmtoff`/`tm_zone`.
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }
    extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: `tm` is a valid, fully initialized `struct tm` owned by this call, and `unix_secs`
    // outlives it.
    let ok = unsafe { !localtime_r(&unix_secs, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff
    } else {
        // The rest of `struct tm` exists for the C ABI; reading it keeps every field live.
        let _ = (
            tm.tm_sec, tm.tm_min, tm.tm_hour, tm.tm_mday, tm.tm_mon, tm.tm_year,
        );
        let _ = (tm.tm_wday, tm.tm_yday, tm.tm_isdst, tm.tm_zone);
        0
    }
}

#[cfg(not(unix))]
fn local_utc_offset(_unix_secs: i64) -> i64 {
    0
}

/// (year, month, day) from days since the Unix epoch (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_shift + 2) / 5 + 1) as u32;
    let month = if month_shift < 10 {
        month_shift + 3
    } else {
        month_shift - 9
    } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui_kit::base::TextSelection;
    use gpui_kit::gpui::{
        Entity, Modifiers, MouseButton, Render, TestAppContext, VisualTestContext, Window, point,
        px,
    };

    use super::*;

    // The paragraphs recent frames painted, with their layouts, so tests can find where
    // rendered text sits (gpui-base's own tests read this off its private window state).
    thread_local! {
        static PAINTED: RefCell<Vec<(SharedString, TextLayout)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn record_painted_paragraph(text: &SharedString, layout: &TextLayout) {
        PAINTED.with(|painted| painted.borrow_mut().push((text.clone(), layout.clone())));
    }

    /// The middle of the character at `index` of the painted `text`.
    fn painted_position(text: &str, index: usize, window: &Window, cx: &App) -> Point<Pixels> {
        let layout = PAINTED
            .with(|painted| {
                painted
                    .borrow()
                    .iter()
                    .rev()
                    .find(|(painted, _)| painted.as_ref() == text)
                    .map(|(_, layout)| layout.clone())
            })
            .unwrap_or_else(|| panic!("{text:?} was not painted"));
        let position = layout
            .position_for_index(index)
            .unwrap_or_else(|| panic!("{text:?} has no character {index}"));
        let _ = (window, cx);
        point(position.x + px(1.), position.y + layout.line_height() / 2.)
    }

    fn selected_text(cx: &mut VisualTestContext) -> String {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            TextSelection::selected_text(window, cx)
        })
    }

    fn press_at(
        text: &str,
        index: usize,
        click_count: usize,
        cx: &mut VisualTestContext,
    ) -> Point<Pixels> {
        let position = cx.update(|window, cx| painted_position(text, index, window, cx));
        cx.simulate_event(gpui_kit::gpui::MouseDownEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
            first_mouse: false,
        });
        position
    }

    fn drag_to(text: &str, index: usize, cx: &mut VisualTestContext) -> Point<Pixels> {
        let position = cx.update(|window, cx| painted_position(text, index, window, cx));
        cx.simulate_event(gpui_kit::gpui::MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        position
    }

    fn release_at(position: Point<Pixels>, click_count: usize, cx: &mut VisualTestContext) {
        cx.simulate_event(gpui_kit::gpui::MouseUpEvent {
            position,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count,
        });
    }

    /// A transcript of one channel, mounted like the app does (the kit's root mounts the
    /// selection layer; the harness stands in for it), recording routed links.
    struct TranscriptHarness {
        links: Rc<RefCell<Vec<LinkTarget>>>,
        list: Entity<MessageList>,
        _subscription: Subscription,
    }

    impl TranscriptHarness {
        fn open(channel: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let fixtures = Arc::new(crate::fixtures::load());
            let list = MessageList::new(fixtures, ChannelId::from(channel), window, cx);
            let links = Rc::new(RefCell::new(Vec::new()));
            let subscription = {
                let links = links.clone();
                cx.subscribe(&list, move |_, _, event: &MessageListEvent, _| {
                    if let MessageListEvent::OpenLink(target) = event {
                        links.borrow_mut().push(target.clone());
                    }
                })
            };
            Self {
                links,
                list,
                _subscription: subscription,
            }
        }
    }

    impl Render for TranscriptHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(gpui_kit::base::TextSelectionLayer)
                .child(self.list.clone())
        }
    }

    /// The channel's transcript window, drawn once, then `run` with its harness and test
    /// context (`add_window_view` lends the context only for the borrow).
    fn with_transcript<R>(
        channel: &str,
        cx: &mut TestAppContext,
        run: impl FnOnce(Entity<TranscriptHarness>, &mut VisualTestContext) -> R,
    ) -> R {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let (harness, cx) =
            cx.add_window_view(|window, cx| TranscriptHarness::open(channel, window, cx));
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        run(harness, cx)
    }

    #[test]
    fn message_order_separates_messages_and_their_paragraphs() {
        let first = message_order(&Ts::new("1000000.001100"));
        let second = message_order(&Ts::new("1000000.002200"));
        assert!(first < second);
        // A message owns room for several blocks' worth of paragraphs.
        assert!(first + THEME_ORDER_STRIDE * 4 < second);
    }

    #[gpui::test]
    fn drag_selects_across_paragraphs_and_messages(cx: &mut TestAppContext) {
        // The deploy report: a rich-text section followed by a code block, then the latency
        // message's own paragraph.
        let section = "Deploy of api v2.4 is green on staging: dashboard";
        let code = "cargo deploy --service api --env staging";
        let next =
            "I'm seeing elevated p99 latency on /search since the deploy. Thread for details.";
        with_transcript("C0ENG", cx, |harness, cx| {
            // The transcript opens tail-anchored on the huge release report; scroll onto
            // the deploy report the test drags across.
            harness.update(cx, |harness, cx| {
                harness
                    .list
                    .update(cx, |list, cx| list.jump(Jump::Beginning, cx));
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let _ = window.draw(cx);
            });
            assert_eq!(selected_text(cx), "");
            assert!(!PAINTED.with(|painted| painted.borrow().is_empty()));

            // Within one message: from the section into its code block.
            press_at(section, 7, 1, cx);
            let end = drag_to(code, code.len() - 8, cx);
            release_at(end, 1, cx);
            assert_eq!(
                selected_text(cx),
                format!("{}\n{}", &section[7..], &code[..code.len() - 8]),
                "a drag joins the message's paragraphs with a newline"
            );

            // Across messages: from the code block into the next message's paragraph.
            press_at(code, 6, 1, cx);
            let end = drag_to(next, 12, cx);
            release_at(end, 1, cx);
            assert_eq!(
                selected_text(cx),
                format!("{}\n{}", &code[6..], &next[..12]),
                "a drag spans messages in reading order"
            );
            drop(harness);
        });
    }

    #[test]
    fn mrkdwn_fences_split_pre_blocks_out_of_the_text() {
        // Sentry's alert section: the whole text is one fence.
        assert_eq!(
            mrkdwn_fences("```undefined is not an object```"),
            vec![(true, "undefined is not an object")]
        );
        assert_eq!(
            mrkdwn_fences("Run:\n```\ncargo test\n```\nthen ship"),
            vec![(false, "Run:"), (true, "cargo test"), (false, "then ship")]
        );
        assert_eq!(mrkdwn_fences("a ``` b"), vec![(false, "a ``` b")]);
    }

    #[test]
    fn link_labels_take_the_styles_wrapping_them() {
        let (label, run) = styled_label("*TypeError*", RunFlags::default());
        assert_eq!(label, "TypeError");
        assert!(run.bold && !run.italic);
        let (label, run) = styled_label("_*both*_", RunFlags::default());
        assert_eq!(label, "both");
        assert!(run.bold && run.italic);
        let (label, run) = styled_label("a *b*", RunFlags::default());
        assert_eq!(label, "a *b*");
        assert!(!run.bold);
    }

    #[gpui::test]
    fn blank_lines_render_as_a_short_paragraph_break(cx: &mut TestAppContext) {
        // Slack's blank line is an 8px `c-mrkdwn__br` block, not an empty text line
        // (measured on the official client: line pitch ~22.5px, blank-line pitch ~30px,
        // CSS height 8px). The fixture's "\n\n" sits inside one run, so splitting at the
        // break must slice runs and keep styling on both sides.
        let first = "Rollout notes:";
        let second = "Stage one is done, stage two next.";
        with_transcript("C0ENG", cx, |_, _| {
            // The blank-line message is the channel's last, so the tail-anchored transcript
            // paints it without jumping.
            let (across_break, line_height) = PAINTED.with(|painted| {
                let painted = painted.borrow();
                let find = |text: &str| {
                    painted
                        .iter()
                        .rev()
                        .find(|(painted, _)| painted.as_ref() == text)
                        .map(|(_, layout)| layout.clone())
                        .unwrap_or_else(|| panic!("{text:?} was not painted"))
                };
                let first = find(first);
                let second = find(second);
                let line_height = first.line_height();
                let across_break = second.position_for_index(0).unwrap().y
                    - first.position_for_index(0).unwrap().y;
                (across_break, line_height)
            });
            assert_eq!(
                across_break - (line_height + px(8.)),
                px(0.),
                "a blank line is Slack's short 8px break, not an empty text line"
            );
        });
    }

    #[gpui::test]
    fn double_click_selects_the_word_under_the_cursor(cx: &mut TestAppContext) {
        let line = "Count me in.";
        with_transcript("C0RANDOM", cx, |_, cx| {
            let word = line.find("me").expect("the word is in the line");
            let at = press_at(line, word + 1, 2, cx);
            release_at(at, 2, cx);
            assert_eq!(selected_text(cx), "me");
        });
    }

    #[gpui::test]
    fn clicking_a_mention_routes_to_the_profile(cx: &mut TestAppContext) {
        let fixtures = crate::fixtures::load();
        let line = format!(
            "@{} anyone up for lunch at the ramen place?",
            fixtures.mention_name(&UserId::from("U0ME"))
        );
        with_transcript("C0RANDOM", cx, |harness, cx| {
            let at = press_at(&line, 1, 1, cx);
            release_at(at, 1, cx);
            assert_eq!(
                harness.update(cx, |harness, _| harness.links.borrow().clone()),
                vec![LinkTarget::Profile(UserId::from("U0ME"))],
                "the @mention chip opens that user's profile"
            );
            // The click that opened the link left no selection behind.
            assert_eq!(selected_text(cx), "");
        });
    }

    #[gpui::test]
    fn a_drag_over_a_link_selects_without_activating_it(cx: &mut TestAppContext) {
        // "dashboard" links to the grafana URL; dragging across it must select its text
        // without routing the link.
        let section = "Deploy of api v2.4 is green on staging: dashboard";
        with_transcript("C0ENG", cx, |harness, cx| {
            harness.update(cx, |harness, cx| {
                harness
                    .list
                    .update(cx, |list, cx| list.jump(Jump::Beginning, cx));
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let _ = window.draw(cx);
            });
            press_at(section, 40, 1, cx);
            let end = drag_to(section, section.len(), cx);
            release_at(end, 1, cx);
            assert_eq!(selected_text(cx), &section[40..]);
            assert_eq!(
                harness.update(cx, |harness, _| harness.links.borrow().clone()),
                Vec::<LinkTarget>::new(),
                "a drag that selects text does not open the link it crossed"
            );
        });
    }
}
