//! Message column: a bottom-anchored virtualized transcript (gpui-kit-guide section 4) with
//! date dividers, a new-messages divider, author grouping, reaction chips, and rich text
//! rendered to styled `TextRun`s with clickable link ranges and inline @mention chips (section 10).
//! App-authored Block Kit blocks and legacy attachments render through [`crate::blockkit`];
//! mrkdwn (fallback text, Block Kit text objects) through [`mrkdwn`].
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
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, Focusable as _, Font,
    FontStyle, FontWeight, Hsla, ImageSource, InteractiveElement as _, InteractiveText,
    IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement as _, StrikethroughStyle, StyleRefinement, Styled as _, StyledText,
    Subscription, TextRun, UnderlineStyle, Window, div, img, px, rgb,
};
use slackless_core::types::rich_text::{
    ListStyle, RichTextContainer, RichTextElement, RichTextList, RichTextPreformatted,
};
use slackless_core::types::{Block, ChannelId, Message, MessageSubtype, TextStyle, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MessageAction};
use crate::blockkit::{self, BlockKitState, KitCx};
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::later::{Remind, SaveRequest};

/// Avatar chip size in px.
const AVATAR_SIZE: f32 = 36.;
/// Horizontal gutter around the transcript (px); overrides the scroller's 12px default.
const GUTTER: f32 = 16.;
/// Space the scroller inserts between consecutive rows (px); dividers add their own padding.
const ROW_GAP: f32 = 2.;
/// Seconds after which consecutive messages by one author render as separate cards.
const GROUP_WINDOW_SECS: i64 = 300;
/// Inline custom-emoji image size in a text line (px).
const INLINE_EMOJI: f32 = 18.;
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

fn quote_flags() -> RunFlags {
    RunFlags {
        italic: true,
        ..DEFAULT_FLAGS
    }
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
    /// The transcript scrolled to (near) the top of `channel`; the root reads the next older
    /// page from the store, strictly older than `before` (the oldest message shown). Emitted
    /// at most once per oldest message until the window refreshes.
    NeedOlder {
        channel: ChannelId,
        before: Ts,
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
                _subscriptions: subscriptions,
            }
        })
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
        if let Some(message) = message {
            actions::add_reaction(message, &me, name);
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
                    actions::toggle_reaction(message, &me, &name);
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
            MessageAction::Forward => {
                eprintln!("slackless: forward {channel}/{ts} (needs a live workspace)");
                actions::notify(
                    "Forward message",
                    "Forwarding needs a live workspace.",
                    window,
                    cx,
                );
            }
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
            MessageAction::Delete => self.delete(ts, cx),
        }
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
            actions::apply_edit(message, &me, text);
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

    let date_break = previous.is_none_or(|prev| day_index(&prev.ts) != day_index(&message.ts));
    let new_break = match store.last_read.as_ref() {
        Some(read) if !date_break => {
            &message.ts > read && previous.is_none_or(|prev| &prev.ts <= read)
        }
        _ => false,
    };
    let grouped =
        !date_break && !new_break && previous.is_some_and(|prev| same_group(prev, message));
    let editing = store.editing.as_ref() == Some(&message.ts);
    let saved = store.saved.contains(&message.ts);

    let author_key = author_key(message);
    let author = author_name(message, fixtures);

    let mut row = v_flex().min_w_0();
    if date_break {
        let label = message
            .ts
            .secs()
            .map(|secs| date_label(&CivilTime::from_unix(secs), now))
            .unwrap_or_else(|| message.ts.as_str().to_owned());
        row = row.child(date_divider(label, cx));
    } else if new_break {
        row = row.child(new_messages_divider(cx));
    }
    if is_membership(message) {
        return row.child(membership_line(message, &author, store, cx));
    }

    let key = format!("msg-{}", message.ts.as_str());
    let handler: ActionHandler = {
        let this = this.clone();
        let ts = message.ts.clone();
        Rc::new(move |action, window, cx| {
            this.update(cx, |list, cx| list.run_action(&ts, action, window, cx));
        })
    };

    let mut body = v_flex().flex_1().min_w_0().gap_0p5().text_sm();
    if !grouped || editing {
        body = body.child(message_header(message, &author, fixtures, cx));
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
        let kcx = KitCx {
            fixtures,
            kit: Some(&store.kit),
            ts: &message.ts,
        };
        let broadcast_root = message
            .root
            .as_deref()
            .filter(|_| message.subtype == Some(MessageSubtype::ThreadBroadcast));
        if let Some(root) = broadcast_root.filter(|_| !grouped) {
            body = body.child(
                h_flex()
                    .min_w_0()
                    .gap_1()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_none().child("replied to a thread:"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().link)
                            .child(fixtures.preview(root)),
                    ),
            );
        }
        body = body.child(message_body(message, &kcx, cx));
        if let Some(files) = crate::files::attachments(fixtures, message, cx) {
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
            // Slack's thread summary: the repliers' faces, then the reply count.
            let faces = message
                .reply_users
                .iter()
                .take(4)
                .map(|user| crate::avatars::user_chip(fixtures, user, 22.));
            let label = if reply_count == 1 {
                "1 reply".to_owned()
            } else {
                format!("{reply_count} replies")
            };
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "thread-{}",
                        message.ts.as_str()
                    )))
                    .items_center()
                    .gap_1()
                    .pb_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().link)
                    .cursor_pointer()
                    .hover(|el| el.opacity(0.8))
                    .children(faces)
                    .child(div().ml_1().child(label))
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
    } else {
        crate::avatars::author_avatar(fixtures, message, &author_key, &author, AVATAR_SIZE)
    };

    let set = ActionSet {
        own: message.user.as_ref() == Some(&fixtures.self_user),
        reply: true,
        saved,
    };
    let theme = cx.theme();
    let hover_bg = theme.secondary.opacity(0.5);
    let saved_bg = theme.warning.opacity(0.08);
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
        .when(!saved, |card| card.hover(|card| card.bg(hover_bg)))
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
fn same_group(prev: &Message, message: &Message) -> bool {
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
        .unwrap_or_else(|| author_key(message))
}

/// One full message card: avatar, author header, rich-text / Block Kit body and static
/// reaction chips. Shared with the thread panel and search so every column renders messages
/// identically.
pub fn message_row(fixtures: &Fixtures, message: &Message, cx: &App) -> Div {
    let author_key = author_key(message);
    let author = author_name(message, fixtures);
    let kcx = KitCx {
        fixtures,
        kit: None,
        ts: &message.ts,
    };
    let mut body = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .text_sm()
        .child(message_header(message, &author, fixtures, cx))
        .child(message_body(message, &kcx, cx));
    if let Some(files) = crate::files::attachments(fixtures, message, cx) {
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
    h_flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(crate::avatars::author_avatar(
            fixtures,
            message,
            &author_key,
            &author,
            AVATAR_SIZE,
        ))
        .child(body)
}

fn message_header(message: &Message, author: &str, fixtures: &Fixtures, cx: &App) -> Div {
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
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(author.to_owned()),
                )
                .children(status),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format_clock(&message.ts)),
        )
}

/// Slack's day divider: a full-width rule with the day's pill (label, then a chevron) on it.
fn date_divider(label: String, cx: &App) -> Div {
    let border = cx.theme().border;
    let muted = cx.theme().muted_foreground;
    let background = cx.theme().background;
    h_flex()
        .items_center()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(border))
        .child(
            h_flex()
                .flex_none()
                .items_center()
                .gap_1()
                .px_3()
                .py_0p5()
                .rounded_full()
                .border_1()
                .border_color(border)
                .bg(background)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child(label)
                .child(
                    Icon::new(IconName::ChevronDown)
                        .text_color(muted)
                        .size(px(12.)),
                ),
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

/// Slack's compact join / leave line: a person icon in the avatar gutter, "<name> joined
/// #channel." and the time, all muted.
fn membership_line(message: &Message, author: &str, store: &MessageData, cx: &App) -> Div {
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
    h_flex()
        .items_center()
        .gap_3()
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
        .child(format!("{author} {verb} {channel}."))
        .child(div().text_xs().child(format_clock(&message.ts)))
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
    mono_font: Font,
    foreground: Hsla,
    muted: Hsla,
    border: Hsla,
    link: Hsla,
    mention_bg: Hsla,
    code_bg: Hsla,
    /// Inline `code` text (Slack's orange / red, see [`crate::theme::code_foreground`]).
    code_fg: Hsla,
    /// Paragraphs rendered with this theme so far: keeps each paragraph's element id unique
    /// within its message (GPUI keys interaction and accessibility state by the id path).
    paragraphs: Cell<usize>,
}

impl RunTheme {
    fn from_app(cx: &App) -> Self {
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
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            border: theme.border,
            link: theme.link,
            mention_bg: theme.accent.opacity(0.25),
            code_bg: theme.secondary,
            code_fg: crate::theme::code_foreground(cx),
            base_font,
            paragraphs: Cell::new(0),
        }
    }

    fn run(
        &self,
        run_flags: RunFlags,
        color: Option<Hsla>,
        bg: Option<Hsla>,
        len: usize,
    ) -> TextRun {
        let mut font = if run_flags.code {
            self.mono_font.clone()
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

/// One inline chunk of a paragraph: styled text (with its clickable link ranges) or a custom
/// emoji image.
enum Segment {
    Text {
        text: String,
        runs: Vec<TextRun>,
        links: Vec<(Range<usize>, String)>,
    },
    Image(ImageSource),
}

/// Accumulates a paragraph's styled text and link ranges, split only at inline images.
struct ParagraphBuilder {
    text: String,
    runs: Vec<TextRun>,
    links: Vec<(Range<usize>, String)>,
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
    /// A bare URL (label equal to the URL) gets Slack's chip look: no scheme, tinted wash.
    fn push_link(&mut self, url: String, label: String, theme: &RunTheme) {
        let address = |s: &str| {
            s.split_once("://")
                .map_or(s, |(_, rest)| rest)
                .trim_end_matches('/')
                .to_owned()
        };
        // Labelled with its own address (with or without the scheme): Slack's link chip.
        let bare = address(&label) == address(&url);
        let label = if bare {
            url.split_once("://")
                .map_or(url.as_str(), |(_, rest)| rest)
                .trim_end_matches('/')
                .to_owned()
        } else {
            label
        };
        let start = self.text.len();
        let bg = bare.then_some(theme.code_bg);
        self.push_text(&label, theme, RunFlags::default(), Some(theme.link), bg);
        self.links.push((start..self.text.len(), url));
    }

    fn push_image(&mut self, image: ImageSource) {
        self.flush();
        self.segments.push(Segment::Image(image));
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

    fn finish(mut self) -> Vec<Segment> {
        self.flush();
        self.segments
    }
}

fn render_segments(segments: Vec<Segment>, theme: &RunTheme) -> Div {
    // Stable across frames and unique within the message: the paragraph's position under this
    // theme plus its text (separate themes render separate blocks of one message).
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    // Text wraps only inside a width-constrained block: a lone text run (most paragraphs)
    // renders in a full-width block; runs mixed with inline images shrink to wrap in the row.
    let single = segments.len() == 1;
    let mut row = h_flex().min_w_0().flex_wrap();
    for (n, segment) in segments.into_iter().enumerate() {
        let text = match segment {
            Segment::Text { text, runs, links } if links.is_empty() => {
                StyledText::new(text).with_runs(runs).into_any_element()
            }
            Segment::Text { text, runs, links } => {
                let mut hasher = DefaultHasher::new();
                text.hash(&mut hasher);
                let id = format!("text-{paragraph}-{n}-{:x}", hasher.finish());
                let (ranges, urls): (Vec<_>, Vec<_>) = links.into_iter().unzip();
                let styled = StyledText::new(text).with_runs(runs);
                InteractiveText::new(SharedString::from(id), styled)
                    .on_click(ranges, move |ix, _, cx| cx.open_url(&urls[ix]))
                    .into_any_element()
            }
            Segment::Image(image) => {
                row = row.child(img(image).flex_none().size(px(INLINE_EMOJI)).rounded_sm());
                continue;
            }
        };
        if single {
            return div().w_full().min_w_0().child(text);
        }
        row = row.child(div().min_w_0().child(text));
    }
    row
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
            builder.push_text(&el.text, theme, flags(el.style.as_ref()), None, None);
        }
        RichTextElement::Link(el) => {
            let label = el.text.clone().unwrap_or_else(|| el.url.clone());
            builder.push_link(el.url.clone(), label, theme);
        }
        RichTextElement::User(el) => {
            let label = format!("@{}", fixtures.mention_name(&el.user_id));
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Channel(el) => {
            let label = fixtures
                .conversation(&el.channel_id)
                .map(|conversation| fixtures.conversation_label(conversation))
                .unwrap_or_else(|| format!("#{}", el.channel_id));
            builder.push_text(&label, theme, mention_flags(), Some(theme.link), None);
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
            let label = format!("> {}", el.text.as_deref().unwrap_or("a message"));
            builder.push_text(&label, theme, quote_flags(), Some(theme.muted), None);
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
    let indent = px(16. + 20. * depth as f32);
    let mut column = v_flex().gap_1();
    for (n, item) in list.elements.iter().enumerate() {
        let elements: &[RichTextElement] = match item {
            RichTextContainer::Section(section) => &section.elements,
            _ => &[],
        };
        if elements.is_empty() {
            continue;
        }
        let marker = match &list.style {
            ListStyle::Ordered => format!("{}.", n + 1),
            _ => "•".to_owned(),
        };
        let last = n + 1 == list.elements.len();
        if let Some(item_row) = paragraph(elements, fixtures, theme, edited && last) {
            column = column.child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .pl(indent)
                    .child(div().flex_none().text_color(theme.muted).child(marker))
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
        .collect::<String>()
        .trim_end_matches('\n')
        .to_owned();
    let run_len = code.len();
    v_flex()
        .min_w_0()
        .gap_1()
        .when_some(
            block
                .language
                .clone()
                .filter(|language| !language.is_empty()),
            |column, language| {
                column.child(div().text_xs().text_color(theme.muted).child(language))
            },
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .rounded_md()
                .bg(theme.code_bg)
                .px_2()
                .py_1p5()
                .child(StyledText::new(code).with_runs(vec![theme.run(
                    RunFlags {
                        code: true,
                        ..DEFAULT_FLAGS
                    },
                    // Code blocks keep the body color; only inline code is tinted.
                    Some(theme.foreground),
                    None,
                    run_len,
                )])),
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
                Some(RichTextContainer::Section(_) | RichTextContainer::List(_))
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
        column = column.child(mrkdwn(
            &message.text,
            kcx.fixtures,
            false,
            inline_edited,
            cx,
        ));
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
        Block::RichText(rich) => render_containers(
            &rich.elements,
            kcx.fixtures,
            &RunTheme::from_app(cx),
            edited,
        ),
        Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
            .into_iter()
            .collect(),
    }
}

/// Renders Slack mrkdwn (fallback text, Block Kit text objects, attachment text), one line
/// per paragraph: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<@user>` / `<#channel>` /
/// `<!here>` chips, `<url|label>` links and `:emoji:` (custom emoji as images). `muted`
/// renders plain runs in the muted color (Block Kit context blocks). Unknown markup stays
/// literal.
pub(crate) fn mrkdwn(text: &str, fixtures: &Fixtures, muted: bool, edited: bool, cx: &App) -> Div {
    let mut theme = RunTheme::from_app(cx);
    if muted {
        theme.foreground = theme.muted;
    }
    // The edit marker rides inline at the end of the last line that renders text.
    let lines: Vec<&str> = text.split('\n').collect();
    let last_text_line = lines.iter().rposition(|line| !line.is_empty());
    v_flex()
        .min_w_0()
        .children(lines.iter().enumerate().map(|(n, line)| {
            mrkdwn_line(line, fixtures, &theme, edited && Some(n) == last_text_line)
                .unwrap_or_else(|| div().h(px(6.)))
        }))
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
            angle_token(&mut builder, rest, fixtures, theme)
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

/// Plain mrkdwn text with Slack's three HTML escapes undone; inline code gets its wash.
fn push_mrkdwn_text(builder: &mut ParagraphBuilder, text: &str, theme: &RunTheme, run: RunFlags) {
    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    let bg = run.code.then_some(theme.code_bg);
    builder.push_text(&text, theme, run, None, bg);
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

/// A `<...>` token at the start of `rest`: user / channel / special mentions and links.
fn angle_token(
    builder: &mut ParagraphBuilder,
    rest: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
) -> Option<usize> {
    let close = rest.find('>')?;
    let inner = &rest[1..close];
    let (target, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(label)),
        None => (inner, None),
    };
    if let Some(user) = target.strip_prefix('@') {
        let name = format!("@{}", fixtures.mention_name(&UserId::from(user)));
        builder.push_text(
            &name,
            theme,
            mention_flags(),
            Some(theme.link),
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
        builder.push_text(&name, theme, mention_flags(), Some(theme.link), None);
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
        builder.push_link(target.to_owned(), label.unwrap_or(target).to_owned(), theme);
    } else {
        return None;
    }
    Some(close + 1)
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
/// messages, the divider's day label otherwise.
pub(crate) fn list_stamp(ts: &Ts) -> String {
    let now = CivilTime::now();
    match ts.secs() {
        Some(secs) if CivilTime::from_unix(secs).day_index == now.day_index => format_clock(ts),
        Some(secs) => date_label(&CivilTime::from_unix(secs), &now),
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
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
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
    // Day 0 of the Unix epoch (1970-01-01) was a Thursday.
    let weekday = WEEKDAYS[(time.day_index + 4).rem_euclid(7) as usize];
    let month = MONTH_NAMES[(time.month - 1) as usize];
    let day = time.day;
    let suffix = match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    if time.year == now.year {
        format!("{weekday}, {month} {day}{suffix}")
    } else {
        format!("{weekday}, {month} {day}{suffix}, {}", time.year)
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
