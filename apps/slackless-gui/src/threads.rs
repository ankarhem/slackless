//! Right-hand thread panel: the root message, its replies and a dedicated reply composer.
//!
//! Messages render through [`crate::messages::message_row`] (the shared card renderer), so the
//! thread column and the transcript column render identically; the interactive layer comes from
//! [`crate::actions`], shared with the transcript (gpui-kit-guide section 6):
//!
//! - reaction chips that toggle the signed-in user's reaction on click, plus an add-reaction
//!   control; picking from the emoji picker goes through the root
//!   ([`ThreadPanelEvent::PickReaction`], then [`ThreadPanel::add_reaction`]);
//! - the hover action bar and a right-click `ContextMenu` sharing one action set: Add
//!   reaction, Reply in thread, Copy link (clipboard + toast), Forward, Save for later and
//!   Remind me (handed to the root's Later list), Mark unread (New divider above the chosen message), and Edit (inline draft in place of the
//!   row) / Delete for the signed-in user's messages;
//! - a "Replying in thread" hint above the composer while the reply box is summoned.
//!
//! Design tokens: colors and fonts come from the active gpui-kit theme via `cx.theme()`.
//! Spacing rides the 4px utility scale; the only layout constants are the avatar metrics
//! copied from `messages.rs`. Typography: body at `text_sm`, meta text at `text_xs`.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MessageAction, MessageChange};
use crate::composer::{
    AlsoSendTarget, Composer, ComposerEvent, ContextBar, PendingAttachment, TypingNames,
};
use crate::emoji::EmojiChoice;
use crate::fixtures::Fixtures;
use crate::later::SaveRequest;
use crate::messages::{LinkHandler, LinkTarget, editing_avatar, message_row_linked, same_group};
use crate::slash::{Invocation, SlashKind};

pub enum ThreadPanelEvent {
    Close,
    /// The user is typing a reply: the root tells the thread.
    Typing,
    /// A message's avatar or name was clicked: show that user's profile.
    OpenProfile(UserId),
    /// A thread image was clicked: the root opens the image viewer over the thread's
    /// `images`, at `index`.
    OpenImage {
        channel: ChannelId,
        images: Rc<Vec<crate::image_viewer::ViewerImage>>,
        index: usize,
    },
    /// A link in message text was clicked: profile, channel, thread or URL (permalinks
    /// resolved in-app by the root).
    OpenLink(LinkTarget),
    /// Forward this message (its permalink): the root opens New message with it drafted.
    Forward(String),
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
    /// Export the open thread (root + replies) to Markdown (`markdown = true`) or JSON.
    Export {
        channel: ChannelId,
        thread_ts: Ts,
        markdown: bool,
    },
    /// Summarize the open thread (root + replies) using the configured local/BYO-key AI provider.
    Summarize {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// Broadcast an existing thread reply back into the parent channel (`F-CORE-106`).
    SendToChannel {
        channel: ChannelId,
        thread_ts: Ts,
        content: MessageContent,
    },
    /// The header menu followed or unfollowed the open thread (already shown here); the root
    /// tells Slack. `last_read` is the thread's newest message.
    SetSubscribed {
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
    },
    /// User marked a message inside the thread unread; move the thread's read cursor back to `ts`.
    MarkThreadRead {
        channel: ChannelId,
        thread_ts: Ts,
        ts: Ts,
    },
    /// The user reacted to, edited or deleted a message of the open thread (already shown
    /// here); the root posts the change to Slack.
    Changed {
        channel: ChannelId,
        change: MessageChange,
    },
    /// The user replied; the panel has already shown the reply locally. `broadcast`: "Also
    /// send to" was ticked, so the reply posts to the conversation too.
    Reply {
        channel: ChannelId,
        thread_ts: Ts,
        content: MessageContent,
        broadcast: bool,
    },
    /// The user replied with attached files (and optional message text).
    ReplyWithFiles {
        channel: ChannelId,
        thread_ts: Ts,
        content: Option<MessageContent>,
        files: Vec<PendingAttachment>,
    },
    /// An add-reaction control asked for the emoji picker at `position`; the root routes the
    /// pick back through [`ThreadPanel::add_reaction`].
    PickReaction {
        ts: Ts,
        position: Point<Pixels>,
    },
    /// The reply composer's emoji button asked for the picker; picks come back through
    /// [`ThreadPanel::insert_emoji`].
    PickEmoji(Point<Pixels>),
    /// `+` -> `GIF` in the reply composer: open the picker on the GIFs tab.
    PickGif(Point<Pixels>),
    /// The inline edit composer's emoji button asked for the picker; picks come back through
    /// [`ThreadPanel::insert_edit_emoji`].
    PickEditEmoji(Point<Pixels>),
    /// A slash command other than /shrug (which replies in the thread) was sent from the
    /// reply composer; the root dispatches it.
    SlashCommand(Invocation),
    /// The reply composer scheduled a thread reply for later.
    Schedule {
        channel: ChannelId,
        thread_ts: Ts,
        text: String,
        when: crate::drafts::SendLater,
    },
    /// Open the open thread in a separate native GPUI window (`F-CORE-109`).
    PopOut {
        channel: ChannelId,
        thread_ts: Ts,
    },
    /// The reply composer asked for the Custom Schedule Send dialog.
    ScheduleCustom {
        channel: ChannelId,
        thread_ts: Ts,
        text: String,
    },
    /// Save for later / Remind me / unsave; the root keeps the Later list.
    Later(SaveRequest),
    /// Retry a durable V13 outbox reply shown inline at the bottom of this thread.
    RetryOutbox(i64),
    /// Discard a durable V13 outbox reply shown inline at the bottom of this thread.
    DiscardOutbox(i64),
}

/// Avatar chip size and the text-column inset (avatar plus its gap), mirroring `messages.rs`
/// so reaction chips align with the message text column.
const AVATAR_SIZE: f32 = 36.;
const AVATAR_GAP: f32 = 12.;
const TEXT_INSET: f32 = AVATAR_SIZE + AVATAR_GAP;

/// Recently opened threads cached in memory so reopening a thread renders on frame 0.
const THREAD_CACHE_MAX: usize = 16;

pub struct ThreadPanel {
    fixtures: Arc<Fixtures>,
    /// `(channel, root ts)` of the open thread.
    thread: Option<(ChannelId, Ts)>,
    root: Option<Message>,
    replies: Vec<Message>,
    /// True while waiting for the first SQLite/remote replies load on an uncached live thread.
    loading: bool,
    /// Bounded LRU cache of `(root, replies)` per `(channel, thread_ts)`.
    cache: std::collections::HashMap<(ChannelId, Ts), (Option<Message>, Vec<Message>)>,
    cache_lru: Vec<(ChannelId, Ts)>,
    /// Whether the "Replying in thread" hint above the composer shows (Reply in thread action).
    reply_hint: bool,
    /// Message currently edited in place; its WYSIWYG draft lives in `edit_composer`.
    editing: Option<Ts>,
    edit_composer: Entity<Composer>,
    /// Fixture-mode unread marker: the New divider renders above this message.
    unread_after: Option<Ts>,
    /// Messages saved for later (fixture mode: in memory).
    saved: HashSet<Ts>,
    /// Per-message expand/collapse overrides for deleted thread replies.
    toggled_deleted: HashSet<Ts>,
    composer: Entity<Composer>,
    /// The thread body's scroll, so a linked reply can be scrolled into view.
    scroll: ScrollHandle,
    /// A reply permalink's message, scrolled into view once it is loaded.
    reveal: Option<Ts>,
    /// Durable V13 outbox items for the active workspace (filtered to this thread on render).
    outbox: Vec<slackless_core::store::OutboxItem>,
    /// True when the live workspace RTM connection is offline (skips optimistic local echo so
    /// the inline outbox card does not duplicate the reply).
    offline: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ThreadPanelEvent> for ThreadPanel {}

impl ThreadPanel {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<ThreadPanel> {
        cx.new(|cx| {
            let composer = Composer::new("Reply...".into(), fixtures.clone(), window, cx);
            composer.update(cx, |composer, cx| composer.set_scheduling(true, cx));
            let edit_composer = Composer::new_edit(fixtures.clone(), window, cx);
            let subscriptions = vec![
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut ThreadPanel, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(content) => this.reply(content.clone(), cx),
                        ComposerEvent::SubmitWithFiles { content, files } => {
                            if let Some((channel, thread_ts)) = this.thread.clone() {
                                cx.emit(ThreadPanelEvent::ReplyWithFiles {
                                    channel,
                                    thread_ts,
                                    content: content.clone(),
                                    files: files.clone(),
                                });
                            }
                        }
                        ComposerEvent::OpenEmojiPicker(position) => {
                            cx.emit(ThreadPanelEvent::PickEmoji(*position));
                        }
                        ComposerEvent::OpenGifPicker(position) => {
                            cx.emit(ThreadPanelEvent::PickGif(*position));
                        }
                        ComposerEvent::SlashCommand(invocation) => {
                            match invocation.command.map(|command| command.kind) {
                                Some(SlashKind::Shrug) => {
                                    let text = crate::composer::format_shrug(&invocation.args);
                                    this.reply(MessageContent::plain(text), cx);
                                }
                                _ => cx.emit(ThreadPanelEvent::SlashCommand(invocation.clone())),
                            }
                        }
                        ComposerEvent::Schedule { text, when } => {
                            if let Some((channel, thread_ts)) = this.thread.clone() {
                                cx.emit(ThreadPanelEvent::Schedule {
                                    channel,
                                    thread_ts,
                                    text: text.clone(),
                                    when: *when,
                                });
                            }
                        }
                        ComposerEvent::ScheduleCustom(text) => {
                            if let Some((channel, thread_ts)) = this.thread.clone() {
                                cx.emit(ThreadPanelEvent::ScheduleCustom {
                                    channel,
                                    thread_ts,
                                    text: text.clone(),
                                });
                            }
                        }
                        ComposerEvent::Cancel => {}
                        ComposerEvent::EditLast => this.edit_last_reply(window, cx),
                        ComposerEvent::Typing => cx.emit(ThreadPanelEvent::Typing),
                    },
                ),
                cx.subscribe(
                    &edit_composer,
                    |this: &mut ThreadPanel, _, event: &ComposerEvent, cx| match event {
                        ComposerEvent::Submit(content) => this.save_edit(content.clone(), cx),
                        ComposerEvent::Cancel => this.cancel_edit(cx),
                        ComposerEvent::OpenEmojiPicker(position) => {
                            cx.emit(ThreadPanelEvent::PickEditEmoji(*position));
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
            ];
            ThreadPanel {
                fixtures,
                thread: None,
                root: None,
                replies: Vec::new(),
                loading: false,
                cache: std::collections::HashMap::new(),
                cache_lru: Vec::new(),
                reply_hint: false,
                editing: None,
                edit_composer,
                unread_after: None,
                saved: HashSet::new(),
                toggled_deleted: HashSet::new(),
                composer,
                scroll: ScrollHandle::new(),
                reveal: None,
                outbox: Vec::new(),
                offline: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Update the workspace's durable V13 outbox snapshot shown inline at the bottom of the thread.
    pub fn set_outbox(
        &mut self,
        items: Vec<slackless_core::store::OutboxItem>,
        cx: &mut Context<Self>,
    ) {
        self.outbox = items;
        cx.notify();
    }

    /// Track live connection offline state so offline replies render exclusively via outbox cards.
    pub fn set_offline(&mut self, offline: bool, cx: &mut Context<Self>) {
        if self.offline != offline {
            self.offline = offline;
            cx.notify();
        }
    }

    /// Clear per-message deleted overrides when the global setting changes.
    pub fn reset_deleted_overrides(&mut self, cx: &mut Context<Self>) {
        self.toggled_deleted.clear();
        cx.notify();
    }

    fn toggle_deleted(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        if !self.toggled_deleted.remove(ts) {
            self.toggled_deleted.insert(ts.clone());
        }
        cx.notify();
    }

    /// Live refresh: the open thread stays open, drawn from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| {
            composer.set_fixtures(fixtures.clone(), cx)
        });
        self.edit_composer.update(cx, |composer, cx| {
            composer.set_fixtures(fixtures.clone(), cx)
        });
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Show the conversation's context bar (Slack Connect notice) over the reply composer.
    pub fn set_context_bar(&mut self, bar: Option<ContextBar>, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_context_bar(bar, cx));
    }

    /// Forward the Enter-to-send preference to the reply and edit composers.
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
        self.edit_composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
    }

    /// Clear the reply composer's draft (e.g. after confirming a Custom Schedule Send).
    pub fn clear_composer(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.load_draft("", cx));
    }

    /// Insert a picked emoji into the inline edit composer.
    pub fn insert_edit_emoji(
        &mut self,
        choice: &EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_composer
            .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
    }

    fn cache_put(&mut self, key: (ChannelId, Ts), root: Option<Message>, replies: Vec<Message>) {
        self.cache.insert(key.clone(), (root, replies));
        self.cache_lru.retain(|k| k != &key);
        self.cache_lru.push(key);
        while self.cache_lru.len() > THREAD_CACHE_MAX {
            let oldest = self.cache_lru.remove(0);
            self.cache.remove(&oldest);
        }
    }

    pub fn open(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        seed_root: Option<Message>,
        cx: &mut Context<Self>,
    ) {
        let key = (channel.clone(), thread_ts.clone());
        let cached = self.cache.get(&key).cloned();
        let fixture_root = self
            .fixtures
            .messages
            .get(&channel)
            .and_then(|history| history.iter().find(|m| m.ts == thread_ts))
            .cloned();
        if let Some((cached_root, cached_replies)) = cached {
            self.root = cached_root.or(seed_root).or(fixture_root);
            self.replies = cached_replies;
            self.loading = false;
        } else {
            self.root = seed_root.or(fixture_root);
            self.replies = self.fixtures.threads.get(&key).cloned().unwrap_or_default();
            self.loading = !self.fixtures.sample && self.replies.is_empty();
        }
        let target = AlsoSendTarget::for_channel(&self.fixtures, &channel);
        self.composer
            .update(cx, |composer, cx| composer.set_also_send(target, cx));
        self.thread = Some(key);
        self.reply_hint = false;
        self.editing = None;
        self.unread_after = None;
        self.reveal = None;
        cx.notify();
    }

    /// Scroll `ts` of the open thread into view once it is loaded, as Slack does for a
    /// reply's permalink.
    pub fn reveal(&mut self, ts: Ts, cx: &mut Context<Self>) {
        self.reveal = Some(ts);
        cx.notify();
    }

    /// `(channel, root ts)` of the open thread.
    /// Show who is typing in the open thread under the reply box.
    pub fn set_typing(&self, typing: Option<TypingNames>, cx: &mut App) {
        self.composer
            .update(cx, |composer, cx| composer.set_typing(typing, cx));
    }

    pub fn open_thread(&self) -> Option<(ChannelId, Ts)> {
        self.thread.clone()
    }

    /// Replace the open thread's messages (live mode: re-read from the store after it
    /// changed; the read is oldest first, so the root leads). A read for another thread, or
    /// one landing after the panel closed, is ignored.
    pub fn set_thread(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        messages: Vec<Message>,
        cx: &mut Context<Self>,
    ) {
        let key = (channel, thread_ts.clone());
        if self.thread.as_ref() != Some(&key) {
            return;
        }
        let loaded_root = messages
            .first()
            .filter(|message| message.ts == thread_ts)
            .cloned();
        let loaded_replies: Vec<Message> = match loaded_root.is_some() {
            true => messages.into_iter().skip(1).collect(),
            false => messages,
        };
        if loaded_root.is_some() {
            self.root = loaded_root;
        }
        if !loaded_replies.is_empty() || !self.loading {
            self.replies = loaded_replies;
        }
        self.loading = false;
        self.cache_put(key, self.root.clone(), self.replies.clone());
        cx.notify();
    }

    /// Mark or unmark `ts` saved (the Later list changed elsewhere).
    pub fn set_saved(&mut self, ts: &Ts, saved: bool, cx: &mut Context<Self>) {
        if saved {
            self.saved.insert(ts.clone());
        } else {
            self.saved.remove(ts);
        }
        cx.notify();
    }

    pub fn replace_saved(&mut self, saved: HashSet<Ts>, cx: &mut Context<Self>) {
        self.saved = saved;
        cx.notify();
    }

    /// Save (or unsave) `ts` of the open thread, optionally with a reminder, and hand the
    /// request to the root's Later list.
    fn save_for_later(
        &mut self,
        ts: &Ts,
        saved: bool,
        due: Option<u64>,
        title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(channel) = self.thread.as_ref().map(|(channel, _)| channel.clone()) else {
            return;
        };
        let fixtures = self.fixtures.clone();
        let Some(message) = self.message_at_mut(ts) else {
            return;
        };
        let request = actions::build_save_request(&fixtures, channel, message, saved, due);
        actions::notify(title, request.text.clone(), window, cx);
        cx.emit(ThreadPanelEvent::Later(request));
        self.set_saved(ts, saved, cx);
    }

    /// Insert an emoji pick into the reply composer.
    pub fn insert_emoji(
        &mut self,
        choice: &EmojiChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.insert_emoji(choice, window, cx));
    }

    /// Stage a GIF pick inside the reply composer.
    pub fn stage_gif(
        &mut self,
        gif: slackless_core::api::GifItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.composer
            .update(cx, |composer, cx| composer.stage_gif(gif, window, cx));
    }

    /// Up in the empty reply box: edit the signed-in user's latest reply in this thread.
    fn edit_last_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let me = &self.fixtures.self_user;
        let latest = self
            .replies
            .iter()
            .rev()
            .find(|reply| reply.user.as_ref() == Some(me));
        if let Some(ts) = latest.map(|reply| reply.ts.clone()) {
            self.begin_edit(ts, window, cx);
        }
    }

    /// Add the signed-in user's `name` reaction if they have not already reacted with it.
    pub fn add_reaction(&mut self, ts: &Ts, name: &str, cx: &mut Context<Self>) {
        let me = self.fixtures.self_user.clone();
        let added = self
            .message_at_mut(ts)
            .is_some_and(|message| actions::add_reaction(message, &me, name));
        if added {
            self.post(
                MessageChange::React {
                    ts: ts.clone(),
                    name: name.to_owned(),
                    add: true,
                },
                cx,
            );
        }
        cx.notify();
    }

    /// Hand a change of the open thread's messages to the root, to post to Slack.
    fn post(&self, change: MessageChange, cx: &mut Context<Self>) {
        if let Some((channel, _)) = self.thread.clone() {
            cx.emit(ThreadPanelEvent::Changed { channel, change });
        }
    }

    fn reply(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let Some((channel, thread_ts)) = self.thread.clone() else {
            return;
        };
        // Skip synthetic optimistic echo while offline in live mode so the durable V13 inline
        // outbox card renders cleanly without duplicating the reply.
        if self.fixtures.sample || !self.offline {
            let mut message = self.fixtures.local_message(
                &channel,
                content.text.clone(),
                Some(thread_ts.clone()),
            );
            message.blocks = content.blocks.clone();
            message.attachments = content.attachments.clone();
            self.replies.push(message);
        }
        self.reply_hint = false;
        let broadcast = self
            .composer
            .update(cx, |composer, cx| composer.take_also_send(cx));
        cx.emit(ThreadPanelEvent::Reply {
            channel,
            thread_ts,
            content,
            broadcast,
        });
        cx.notify();
    }

    fn message_at_mut(&mut self, ts: &Ts) -> Option<&mut Message> {
        if self.root.as_ref().is_some_and(|root| &root.ts == ts) {
            return self.root.as_mut();
        }
        self.replies.iter_mut().find(|message| &message.ts == ts)
    }

    fn run_action(
        &mut self,
        ts: &Ts,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = self.fixtures.self_user.clone();
        match action {
            MessageAction::React(name) => self.add_reaction(ts, &name, cx),
            MessageAction::ToggleReaction(name) => {
                let toggled = self
                    .message_at_mut(ts)
                    .map(|message| actions::toggle_reaction(message, &me, &name));
                if let Some(add) = toggled {
                    self.post(
                        MessageChange::React {
                            ts: ts.clone(),
                            name,
                            add,
                        },
                        cx,
                    );
                }
                cx.notify();
            }
            MessageAction::PickReaction(position) => {
                cx.emit(ThreadPanelEvent::PickReaction {
                    ts: ts.clone(),
                    position,
                });
            }
            MessageAction::ReplyInThread => self.prompt_reply(cx),
            MessageAction::CopyLink => {
                if let Some((channel, _)) = &self.thread {
                    actions::copy_link(&self.fixtures, channel, ts, window, cx);
                }
            }
            MessageAction::CopyText => {
                if let Some(message) = self.message_at_mut(ts).cloned() {
                    actions::copy_message_text(&self.fixtures, &message, window, cx);
                }
            }
            MessageAction::Forward => {
                if let Some((channel, _)) = &self.thread {
                    let link = actions::permalink(&self.fixtures, channel, ts);
                    cx.emit(ThreadPanelEvent::Forward(link));
                }
            }
            MessageAction::SaveForLater => {
                let saved = !self.saved.contains(ts);
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
                let Some((channel, _)) = self.thread.clone() else {
                    return;
                };
                let Some(message) = self.message_at_mut(ts).cloned() else {
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
                        let _ = this.update(cx, |panel, cx| {
                            actions::notify(
                                &format!("Reminder set: {when}"),
                                request.text.clone(),
                                window,
                                cx,
                            );
                            let ts = request.ts.clone();
                            cx.emit(ThreadPanelEvent::Later(request));
                            panel.set_saved(&ts, true, cx);
                        });
                    },
                );
            }
            MessageAction::MarkUnread => self.mark_unread(ts.clone(), cx),
            MessageAction::Edit => self.begin_edit(ts.clone(), window, cx),
            MessageAction::TogglePin => {
                let Some((channel, _)) = &self.thread else {
                    return;
                };
                let channel = channel.clone();
                let pinned = self
                    .message_at_mut(ts)
                    .map(|message| actions::toggle_pin(message, &channel));
                let Some(pinned) = pinned else {
                    return;
                };
                let preview = self
                    .root
                    .iter()
                    .chain(&self.replies)
                    .find(|message| &message.ts == ts)
                    .map(|message| self.fixtures.preview(message))
                    .unwrap_or_default();
                actions::notify_pin_toggled(pinned, preview, window, cx);
                self.post(
                    MessageChange::Pin {
                        ts: ts.clone(),
                        pinned,
                    },
                    cx,
                );
            }
            MessageAction::Delete => {
                let Some(message) = self.message_at_mut(ts).map(|message| message.clone()) else {
                    return;
                };
                let fixtures = self.fixtures.clone();
                let (this, ts) = (cx.entity().downgrade(), ts.clone());
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    // Gone only if the panel itself was dropped meanwhile: nothing to delete.
                    let _ = this.update(cx, |panel, cx| panel.delete_message(&ts, cx));
                });
            }
            MessageAction::DismissEphemeral => {
                let target = ts.clone();
                self.replies.retain(|m| m.ts != target);
                cx.notify();
            }
            MessageAction::OpenProfile(user) => cx.emit(ThreadPanelEvent::OpenProfile(user)),
            MessageAction::ViewHistory => {
                if let Some((channel, _)) = &self.thread {
                    cx.emit(ThreadPanelEvent::ViewHistory {
                        channel: channel.clone(),
                        ts: ts.clone(),
                    });
                }
            }
            MessageAction::AddToList => {
                if let Some((channel, _)) = &self.thread {
                    cx.emit(ThreadPanelEvent::AddToList {
                        channel: channel.clone(),
                        ts: ts.clone(),
                    });
                }
            }
            MessageAction::SendToChannel => {
                if let Some((channel, thread_ts)) = self.thread.clone()
                    && let Some(reply) = self.replies.iter().find(|m| m.ts == *ts)
                {
                    let content = crate::composer::content_of(&reply.text, &self.fixtures);
                    cx.emit(ThreadPanelEvent::SendToChannel {
                        channel,
                        thread_ts,
                        content,
                    });
                }
            }
            MessageAction::JoinHuddle => {}
            MessageAction::OpenImage(origin) => {
                // The viewer pages through the thread's images, root first.
                let Some((channel, _)) = self.thread.clone() else {
                    return;
                };
                if let Some((images, index)) = crate::image_viewer::collect_viewer_images(
                    &self.fixtures,
                    self.root.iter().chain(&self.replies),
                    ts,
                    &origin,
                ) {
                    cx.emit(ThreadPanelEvent::OpenImage {
                        channel,
                        images,
                        index,
                    });
                }
            }
            MessageAction::BlockAction { action, attachment } => {
                let live = !self.fixtures.sample;
                if let Some((channel, _)) = self.thread.clone()
                    && let Some(message) = self.message_at_mut(ts).cloned()
                    && let Some(change) = actions::dispatch_block_action(
                        live, &message, &channel, action, attachment, window, cx,
                    )
                {
                    self.post(change, cx);
                }
            }
        }
    }

    /// Edit `ts` in place: swap the row body for the shared WYSIWYG edit composer, focused
    /// with the caret at the end.
    fn begin_edit(&mut self, ts: Ts, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self.message_at_mut(&ts).cloned() else {
            return;
        };
        self.reply_hint = false;
        self.editing = Some(ts);
        self.edit_composer.update(cx, |composer, cx| {
            composer.load_message(&message, window, cx);
        });
        cx.notify();
    }

    fn save_edit(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let Some(ts) = self.editing.clone() else {
            return;
        };
        let me = self.fixtures.self_user.clone();
        if let Some(message) = self.message_at_mut(&ts) {
            actions::apply_edit(message, &me, content.clone());
            self.post(MessageChange::Edit { ts, content }, cx);
        }
        self.editing = None;
        cx.notify();
    }

    fn cancel_edit(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        cx.notify();
    }

    /// The header menu's toggle: flip the root's follow state at once, as Slack does, and hand
    /// the change to the root to send.
    fn toggle_subscribed(&mut self, cx: &mut Context<Self>) {
        let Some((channel, thread_ts)) = self.thread.clone() else {
            return;
        };
        let last_read = self
            .replies
            .last()
            .map_or_else(|| thread_ts.clone(), |reply| reply.ts.clone());
        let Some(root) = self.root.as_mut() else {
            return;
        };
        let subscribed = root.subscribed != Some(true);
        root.subscribed = Some(subscribed);
        cx.emit(ThreadPanelEvent::SetSubscribed {
            channel,
            thread_ts,
            last_read,
            subscribed,
        });
        cx.notify();
    }

    /// Summon the reply box: dismiss any inline edit and show the hint above the composer.
    fn prompt_reply(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.reply_hint = true;
        cx.notify();
    }

    fn mark_unread(&mut self, ts: Ts, cx: &mut Context<Self>) {
        let cursor = self
            .root
            .iter()
            .chain(&self.replies)
            .take_while(|m| m.ts < ts)
            .last()
            .map(|m| m.ts.clone())
            .unwrap_or_else(|| crate::threads_cards::ts_just_before(&ts));
        self.unread_after = Some(ts);
        if let Some((channel, thread_ts)) = self.thread.clone() {
            cx.emit(ThreadPanelEvent::MarkThreadRead {
                channel,
                thread_ts,
                ts: cursor,
            });
        }
        cx.notify();
    }

    fn delete_message(&mut self, ts: &Ts, cx: &mut Context<Self>) {
        self.post(MessageChange::Delete { ts: ts.clone() }, cx);
        if self.root.as_ref().is_some_and(|root| &root.ts == ts) {
            // Deleting the root discards the whole thread and closes the panel.
            self.root = None;
            self.replies.clear();
            self.thread = None;
            self.editing = None;
            self.unread_after = None;
            self.reply_hint = false;
            cx.emit(ThreadPanelEvent::Close);
            cx.notify();
            return;
        }
        self.replies.retain(|message| &message.ts != ts);
        if self.editing.as_ref() == Some(ts) {
            self.editing = None;
        }
        if self.unread_after.as_ref() == Some(ts) {
            self.unread_after = None;
        }
        self.saved.remove(ts);
        cx.notify();
    }

    /// Replies at or after the unread marker: the fixture-mode "N new" hint.
    fn unread_new_count(&self) -> usize {
        match &self.unread_after {
            Some(mark) => self
                .replies
                .iter()
                .filter(|message| message.ts >= *mark)
                .count(),
            None => 0,
        }
    }

    fn empty_state(&self, cx: &App) -> Div {
        let muted = cx.theme().muted_foreground;
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_1()
            .px_6()
            .child(
                Icon::new(IconName::MessageSquareText)
                    .text_color(muted)
                    .size(px(24.)),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("No thread selected"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Pick Reply in thread on a message to open one here."),
            )
    }

    /// One thread message: the shared card renderer (reactions stripped and re-rendered as
    /// interactive chips), the hover action bar and the right-click menu. While the row is
    /// being edited, the card is replaced by the inline edit draft.
    fn thread_message_row(
        &self,
        message: &Message,
        is_root: bool,
        grouped: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ts = message.ts.clone();
        let key = format!("thread-row-{}", ts.as_str());
        let this = cx.entity();
        if self.editing.as_ref() == Some(&ts) {
            return div()
                .w_full()
                .min_w_0()
                .px_2()
                .py_2()
                .bg(crate::theme::editing_row(cx))
                .child(
                    h_flex()
                        .items_start()
                        .gap_3()
                        .min_w_0()
                        .child(editing_avatar(&self.fixtures, message, &key, cx))
                        .child(div().flex_1().min_w_0().child(self.edit_composer.clone())),
                )
                .into_any_element();
        }

        let prefs = crate::preferences::display_prefs(cx);
        let deleted_ghost = crate::messages::is_deleted_ghost(message);
        let deleted_expanded =
            !deleted_ghost || (prefs.show_deleted_messages ^ self.toggled_deleted.contains(&ts));
        if deleted_ghost && !deleted_expanded {
            let (muted, fg, hover_bg) = (
                cx.theme().muted_foreground,
                cx.theme().foreground,
                cx.theme().secondary.opacity(0.38),
            );
            let author = message
                .user
                .as_ref()
                .map(|u| self.fixtures.user_name(u).to_owned())
                .unwrap_or_else(|| "Unknown".to_owned());
            let stamp = crate::messages::list_stamp(&ts);
            let toggle_ts = ts.clone();
            return h_flex()
                .id(SharedString::from(format!(
                    "thread-deleted-collapsed-{}",
                    ts.as_str()
                )))
                .group("thread-deleted-row")
                .w_full()
                .min_w_0()
                .items_center()
                .gap_3()
                .px_2()
                .py_1()
                .cursor_pointer()
                .hover(move |el| el.bg(hover_bg))
                .child(
                    div()
                        .w(px(36.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Trash)
                                .size(px(12.))
                                .text_color(muted.opacity(0.55)),
                        ),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .child(
                            div()
                                .italic()
                                .text_color(muted.opacity(0.8))
                                .child(format!("Deleted message \u{b7} {author} \u{b7} {stamp}")),
                        )
                        .child(
                            div()
                                .text_color(muted)
                                .group_hover("thread-deleted-row", move |el| {
                                    el.text_color(fg).underline()
                                })
                                .child("Show"),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.toggle_deleted(&toggle_ts, cx);
                }))
                .into_any_element();
        }

        let handler: ActionHandler = {
            let (this, ts) = (this.clone(), ts.clone());
            Rc::new(move |action, window, cx| {
                this.update(cx, |panel, cx| panel.run_action(&ts, action, window, cx));
            })
        };
        // The panel's text links route out as one event; the root resolves them.
        let links: LinkHandler = {
            let panel = this.clone();
            Rc::new(move |target, _window, cx| {
                panel.update(cx, |_, cx| {
                    cx.emit(ThreadPanelEvent::OpenLink(target.clone()))
                });
            })
        };
        let saved = self.saved.contains(&ts);
        let set = ActionSet {
            own: message.user.as_ref() == Some(&self.fixtures.self_user),
            reply: !is_root,
            saved,
            pinned: self
                .thread
                .as_ref()
                .is_some_and(|(channel, _)| message.pinned_to.contains(channel)),
            in_thread: !is_root,
        };
        let row_group = SharedString::from(format!("thread-row-group-{}", ts.as_str()));
        let hover_bg = cx.theme().secondary.opacity(0.5);

        // Like Slack, a reply continuing the previous author's run drops its avatar and name.
        let card = message_row_linked(
            &self.fixtures,
            message,
            Some(&handler),
            Some(&links),
            grouped.then(|| row_group.clone()),
            cx,
        );
        let text_inset = if prefs.compact { 36. } else { TEXT_INSET };
        let card = if deleted_ghost {
            let (muted, fg) = (cx.theme().muted_foreground, cx.theme().foreground);
            let toggle_ts = ts.clone();
            v_flex()
                .min_w_0()
                .child(div().opacity(0.65).child(card))
                .child(
                    h_flex()
                        .pl(px(text_inset))
                        .pt_0p5()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .child(div().italic().text_color(muted).child("deleted"))
                        .child(div().text_color(muted).child("\u{b7}"))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "thread-deleted-hide-{}",
                                    ts.as_str()
                                )))
                                .text_color(muted)
                                .cursor_pointer()
                                .hover(move |el| el.text_color(fg).underline())
                                .child("Hide")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_deleted(&toggle_ts, cx);
                                })),
                        ),
                )
        } else {
            card
        };
        // Straddling the row's top edge would cut the root's bar off at the panel's top, so
        // Slack keeps it inside the row, 3pt down.
        let quick = crate::preferences::active_quick_reactions(&self.fixtures.quick_reactions, cx);
        let menu_handler = handler.clone();
        let row_key = key.clone();
        actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
            let revealed = hover.revealed(cx);
            div()
                .id(SharedString::from(row_key.clone()))
                .group(row_group)
                .relative()
                .w_full()
                .min_w_0()
                .px_2()
                .when(!grouped, |row| row.pt_1())
                .pb_1()
                .when(revealed, |row| row.bg(hover_bg))
                .when(saved, |row| row.child(actions::saved_banner(cx)))
                .child(card)
                .when(revealed, |row| {
                    row.child(
                        actions::hover_toolbar(&row_key, &quick, set, &handler, &hover, cx)
                            .when(is_root, |bar| bar.top(px(3.))),
                    )
                })
                .on_hover(hover.host_on_hover())
                .context_menu(move |menu, window, cx| {
                    actions::actions_menu(
                        menu,
                        set,
                        &menu_handler,
                        actions::MenuOrigin::Context,
                        window,
                        cx,
                    )
                })
                .into_any_element()
        })
        .into_any_element()
    }
}

impl Render for ThreadPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let background = theme.background;
        let danger = theme.danger;
        // Slack's header menu ("More actions"): follow or unfollow the thread, and copy its
        // link. Its other rows (AI summary, thread huddle, split view, new window) have
        // nothing behind them here, so they are left out.
        let more = self.root.as_ref().map(|root| {
            let (panel, root_ts) = (cx.entity().downgrade(), root.ts.clone());
            let open_thread = self.thread.clone();
            let following = root.subscribed == Some(true);
            Button::new("thread-more")
                .ghost()
                .with_size(Size::Small)
                .icon(IconName::EllipsisVertical)
                .tooltip("More actions")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                    let (toggle, copy, root_ts) = (panel.clone(), panel.clone(), root_ts.clone());
                    menu = menu
                        .item(
                            PopupMenuItem::new(if following {
                                "Turn off notifications for replies"
                            } else {
                                "Get notified about new replies"
                            })
                            .on_click(move |_, _, cx| {
                                let _ = toggle.update(cx, |panel, cx| panel.toggle_subscribed(cx));
                            }),
                        )
                        .item(
                            PopupMenuItem::new("Copy link").on_click(move |_, window, cx| {
                                let _ = copy.update(cx, |panel, cx| {
                                    panel.run_action(&root_ts, MessageAction::CopyLink, window, cx)
                                });
                            }),
                        );
                    if let Some((ch, tts)) = open_thread.clone() {
                        let (sum_panel, ch_sum, tts_sum) = (panel.clone(), ch.clone(), tts.clone());
                        let (exp_md, ch_md, tts_md) = (panel.clone(), ch.clone(), tts.clone());
                        let (exp_json, ch_json, tts_json) = (panel.clone(), ch, tts);
                        menu = menu
                            .separator()
                            .item(
                                PopupMenuItem::new("Summarize thread")
                                    .icon(IconName::Sparkles)
                                    .on_click(move |_, _, cx| {
                                        let _ = sum_panel.update(cx, |_, cx| {
                                            cx.emit(ThreadPanelEvent::Summarize {
                                                channel: ch_sum.clone(),
                                                thread_ts: tts_sum.clone(),
                                            });
                                        });
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new("Export thread (Markdown\u{2026})")
                                    .icon(IconName::Download)
                                    .on_click(move |_, _, cx| {
                                        let _ = exp_md.update(cx, |_, cx| {
                                            cx.emit(ThreadPanelEvent::Export {
                                                channel: ch_md.clone(),
                                                thread_ts: tts_md.clone(),
                                                markdown: true,
                                            });
                                        });
                                    }),
                            )
                            .item(
                                PopupMenuItem::new("Export thread (JSON\u{2026})")
                                    .icon(IconName::FileCode)
                                    .on_click(move |_, _, cx| {
                                        let _ = exp_json.update(cx, |_, cx| {
                                            cx.emit(ThreadPanelEvent::Export {
                                                channel: ch_json.clone(),
                                                thread_ts: tts_json.clone(),
                                                markdown: false,
                                            });
                                        });
                                    }),
                            );
                    }
                    menu
                })
        });
        // Slack's header is just the bold title; the conversation is not repeated under it.
        let header = h_flex()
            .justify_between()
            .items_center()
            .px_4()
            .py_2()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Thread"),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .when_some(self.thread.clone(), |bar, (ch, ts)| {
                        let (ch_sum, ts_sum) = (ch.clone(), ts.clone());
                        bar.child(
                            Button::new("summarize-thread")
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::Sparkles)
                                .tooltip("Summarize thread")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ThreadPanelEvent::Summarize {
                                        channel: ch_sum.clone(),
                                        thread_ts: ts_sum.clone(),
                                    });
                                })),
                        )
                        .child(
                            Button::new("popout-thread")
                                .ghost()
                                .with_size(Size::Small)
                                .icon(IconName::ExternalLink)
                                .tooltip("Open thread in new window")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(ThreadPanelEvent::PopOut {
                                        channel: ch.clone(),
                                        thread_ts: ts.clone(),
                                    });
                                })),
                        )
                    })
                    .children(more)
                    .child(
                        Button::new("close-thread")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ThreadPanelEvent::Close))),
                    ),
            );

        // The body's rows; `revealed` is the index of the row a permalink asked to show.
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut revealed = None;
        match self.root.as_ref() {
            Some(root) => {
                if self.unread_after.as_ref() == Some(&root.ts) {
                    rows.push(new_messages_divider(cx).into_any_element());
                }
                if self.reveal.as_ref() == Some(&root.ts) {
                    revealed = Some(rows.len());
                }
                rows.push(
                    self.thread_message_row(root, true, false, cx)
                        .into_any_element(),
                );
                // Slack's "16 replies ——" rule under the root; none before the first reply.
                let new_count = self.unread_new_count();
                let replies = match self.replies.len() {
                    1 => "1 reply".to_owned(),
                    n => format!("{n} replies"),
                };
                if self.loading && self.replies.is_empty() {
                    rows.push(
                        div()
                            .px_4()
                            .py_2()
                            .text_xs()
                            .text_color(muted)
                            .child("Loading replies…")
                            .into_any_element(),
                    );
                } else if !self.replies.is_empty() {
                    rows.push(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py_1()
                            .child(div().text_xs().text_color(muted).child(replies))
                            .when(new_count > 0, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(danger)
                                        .child(format!("{new_count} new")),
                                )
                            })
                            .child(div().flex_1().h(px(1.)).bg(border))
                            .into_any_element(),
                    );
                }
                let mut previous: Option<&Message> = None;
                for message in &self.replies {
                    let divider = self.unread_after.as_ref() == Some(&message.ts);
                    if divider {
                        rows.push(new_messages_divider(cx).into_any_element());
                    }
                    let grouped =
                        !divider && previous.is_some_and(|prev| same_group(prev, message));
                    if self.reveal.as_ref() == Some(&message.ts) {
                        revealed = Some(rows.len());
                    }
                    rows.push(
                        self.thread_message_row(message, false, grouped, cx)
                            .into_any_element(),
                    );
                    previous = Some(message);
                }
            }
            None => rows.push(self.empty_state(cx).into_any_element()),
        }
        if let Some(ix) = revealed {
            self.scroll.scroll_to_item(ix);
            self.reveal = None;
        }
        let body = v_flex()
            .id("thread-body")
            .flex_1()
            .min_h_0()
            .gap_1()
            .pb_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows);

        let outbox_strip = self.thread.as_ref().and_then(|(ch, root_ts)| {
            let panel_retry = cx.entity().downgrade();
            let panel_discard = cx.entity().downgrade();
            crate::outbox::inline_outbox_strip(
                &self.outbox,
                ch,
                Some(root_ts),
                crate::outbox::OutboxActions {
                    on_retry: Rc::new(move |id, _window, cx| {
                        let _ = panel_retry
                            .update(cx, |_, cx| cx.emit(ThreadPanelEvent::RetryOutbox(id)));
                    }),
                    on_discard: Rc::new(move |id, _window, cx| {
                        let _ = panel_discard
                            .update(cx, |_, cx| cx.emit(ThreadPanelEvent::DiscardOutbox(id)));
                    }),
                },
                "thread",
                cx,
            )
        });

        let composer_area = v_flex()
            .flex_none()
            .children(outbox_strip)
            .when(self.reply_hint, |column| {
                column.child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .px_4()
                        .pt_1()
                        .child(
                            Icon::new(IconName::CornerUpLeft)
                                .text_color(muted)
                                .size(px(14.)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(muted)
                                .child("Replying in thread"),
                        ),
                )
            })
            .child(self.composer.clone());

        v_flex()
            .size_full()
            .bg(background)
            .border_l_1()
            .border_color(border)
            .on_drop::<gpui_kit::gpui::ExternalPaths>(cx.listener(
                |this, paths: &gpui_kit::gpui::ExternalPaths, _, cx| {
                    if !paths.paths().is_empty() {
                        this.composer.update(cx, |composer, cx| {
                            composer.attach_paths(paths.paths().to_vec(), cx);
                        });
                    }
                },
            ))
            .drag_over::<gpui_kit::gpui::ExternalPaths>(|style, _, _, cx| {
                style.border_2().border_color(cx.theme().link)
            })
            .child(header)
            .child(body)
            .child(composer_area)
    }
}

/// The fixture-mode unread divider, styled like the transcript's in `messages.rs`.
fn new_messages_divider(cx: &App) -> Div {
    let danger = cx.theme().danger;
    h_flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(danger))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("New"),
        )
        .child(div().flex_1().h(px(1.)).bg(danger))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use gpui_kit::gpui::TestAppContext;

    #[gpui::test]
    fn thread_panel_open_lru_cache_set_thread_reply_actions_subscribe_and_unread_count(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });

        let mut fx = crate::fixtures::load();
        fx.sample = false;
        let me = fx.self_user.clone();
        let fx = Arc::new(fx);

        let events: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let subscribed_events: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));
        let reply_broadcasts: Rc<RefCell<Vec<bool>>> = Rc::new(RefCell::new(Vec::new()));

        let panel_slot: Rc<RefCell<Option<Entity<ThreadPanel>>>> = Rc::new(RefCell::new(None));
        let slot = panel_slot.clone();
        let fx_clone = fx.clone();
        let ev_log = events.clone();
        let sub_log = subscribed_events.clone();
        let bc_log = reply_broadcasts.clone();

        struct PanelHost {
            panel: Entity<ThreadPanel>,
            _sub: Subscription,
        }
        impl Render for PanelHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().child(self.panel.clone())
            }
        }

        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let panel = ThreadPanel::new(fx_clone, window, cx);
            *slot.borrow_mut() = Some(panel.clone());
            let host = cx.new(|cx| {
                let sub = cx.subscribe(&panel, move |_, _, ev: &ThreadPanelEvent, _| match ev {
                    ThreadPanelEvent::Reply { broadcast, .. } => {
                        ev_log.borrow_mut().push("reply");
                        bc_log.borrow_mut().push(*broadcast);
                    }
                    ThreadPanelEvent::Changed { change, .. } => match change {
                        MessageChange::React { .. } => ev_log.borrow_mut().push("react"),
                        MessageChange::Pin { .. } => ev_log.borrow_mut().push("pin"),
                        MessageChange::Edit { .. } => ev_log.borrow_mut().push("edit"),
                        MessageChange::Delete { .. } => ev_log.borrow_mut().push("delete"),
                        _ => ev_log.borrow_mut().push("changed"),
                    },
                    ThreadPanelEvent::Later(_) => ev_log.borrow_mut().push("later"),
                    ThreadPanelEvent::SetSubscribed { subscribed, .. } => {
                        ev_log.borrow_mut().push("subscribed");
                        sub_log.borrow_mut().push(*subscribed);
                    }
                    ThreadPanelEvent::Close => ev_log.borrow_mut().push("close"),
                    _ => {}
                });
                PanelHost { panel, _sub: sub }
            });
            gpui_kit::base::Root::new(host, window, cx)
        });
        let panel = panel_slot.borrow_mut().take().unwrap();
        cx.run_until_parked();

        let ch = ChannelId::from("C0ENG");
        let root_ts = Ts::from("1700000100.000100");
        let r1_ts = Ts::from("1700000101.000100");
        let r2_ts = Ts::from("1700000102.000100");

        let root_msg = Message {
            ts: root_ts.clone(),
            user: Some(me.clone()),
            text: "root post".into(),
            ..Default::default()
        };
        let reply1 = Message {
            ts: r1_ts.clone(),
            thread_ts: Some(root_ts.clone()),
            user: Some(me.clone()),
            text: "first reply".into(),
            ..Default::default()
        };
        let reply2 = Message {
            ts: r2_ts.clone(),
            thread_ts: Some(root_ts.clone()),
            user: Some(UserId::from("U0OTHER")),
            text: "second reply".into(),
            ..Default::default()
        };

        // 1. Open uncached live thread -> loading = true; set_thread populates & caches; wrong thread ignored.
        cx.update(|window, cx| {
            panel.update(cx, |p, cx| {
                p.open(ch.clone(), root_ts.clone(), Some(root_msg.clone()), cx);
                assert!(p.loading);
                assert_eq!(p.open_thread(), Some((ch.clone(), root_ts.clone())));

                // Wrong thread ts is ignored.
                p.set_thread(
                    ch.clone(),
                    Ts::from("9999999999.000000"),
                    vec![root_msg.clone()],
                    cx,
                );
                assert!(p.loading);

                // Matching thread populates root + replies and clears loading.
                p.set_thread(
                    ch.clone(),
                    root_ts.clone(),
                    vec![root_msg.clone(), reply1.clone(), reply2.clone()],
                    cx,
                );
                assert!(!p.loading);
                assert_eq!(p.replies.len(), 2);
                assert!(p.cache.contains_key(&(ch.clone(), root_ts.clone())));

                // 2. LRU cache eviction past THREAD_CACHE_MAX (16).
                for i in 0..(THREAD_CACHE_MAX + 3) {
                    let k = (ch.clone(), Ts::from(format!("1700001{i:03}.000100")));
                    p.cache_put(k, Some(root_msg.clone()), Vec::new());
                }
                assert_eq!(p.cache.len(), THREAD_CACHE_MAX);
                assert_eq!(p.cache_lru.len(), THREAD_CACHE_MAX);
                assert!(
                    !p.cache.contains_key(&(ch.clone(), root_ts.clone())),
                    "oldest cached thread evicted past THREAD_CACHE_MAX"
                );

                // Re-seed our thread in cache and reopen -> instant cache hit (`loading == false`).
                p.cache_put(
                    (ch.clone(), root_ts.clone()),
                    Some(root_msg.clone()),
                    vec![reply1.clone(), reply2.clone()],
                );
                p.open(ch.clone(), root_ts.clone(), None, cx);
                assert!(!p.loading);
                assert_eq!(p.replies.len(), 2);

                // 3. unread_new_count counts replies >= unread_after.
                assert_eq!(p.unread_new_count(), 0);
                p.mark_unread(r2_ts.clone(), cx);
                assert_eq!(p.unread_new_count(), 1);
                p.mark_unread(r1_ts.clone(), cx);
                assert_eq!(p.unread_new_count(), 2);

                // 4. reply appends local echo and emits Reply event.
                p.reply(MessageContent::plain("third reply"), cx);
                assert_eq!(p.replies.len(), 3);

                // 5. toggle_subscribed flips root.subscribed and emits SetSubscribed.
                p.toggle_subscribed(cx);
                assert_eq!(p.root.as_ref().unwrap().subscribed, Some(true));
                p.toggle_subscribed(cx);
                assert_eq!(p.root.as_ref().unwrap().subscribed, Some(false));

                // 6. run_action: reaction, toggle reaction, pin, save for later, edit + save_edit, delete reply & root.
                p.run_action(&r1_ts, MessageAction::React("thumbsup".into()), window, cx);
                assert_eq!(p.replies[0].reactions.len(), 1);
                p.run_action(
                    &r1_ts,
                    MessageAction::ToggleReaction("thumbsup".into()),
                    window,
                    cx,
                );
                assert!(p.replies[0].reactions.is_empty());

                p.run_action(&r1_ts, MessageAction::TogglePin, window, cx);
                assert!(p.replies[0].pinned_to.contains(&ch));

                p.run_action(&r1_ts, MessageAction::SaveForLater, window, cx);
                assert!(p.saved.contains(&r1_ts));

                p.run_action(&r1_ts, MessageAction::Edit, window, cx);
                assert_eq!(p.editing.as_ref(), Some(&r1_ts));
                p.save_edit(MessageContent::plain("edited reply"), cx);
                assert_eq!(p.editing, None);
                assert_eq!(p.replies[0].text, "edited reply");

                // Deleting a reply removes only that reply; deleting root clears panel and emits Close.
                p.delete_message(&r1_ts, cx);
                assert_eq!(p.replies.len(), 2);
                assert!(!p.saved.contains(&r1_ts));

                p.delete_message(&root_ts, cx);
                assert!(p.open_thread().is_none());
                assert!(p.root.is_none());
                assert!(p.replies.is_empty());
            });
        });

        assert_eq!(*subscribed_events.borrow(), vec![true, false]);
        assert_eq!(*reply_broadcasts.borrow(), vec![false]);
        let evs = events.borrow();
        assert!(evs.contains(&"reply"));
        assert!(evs.contains(&"react"));
        assert!(evs.contains(&"pin"));
        assert!(evs.contains(&"later"));
        assert!(evs.contains(&"edit"));
        assert!(evs.contains(&"delete"));
        assert!(evs.contains(&"close"));
    }
}
