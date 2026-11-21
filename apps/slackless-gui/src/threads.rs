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
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, Focusable as _,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    div, px,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, ConversationKind, Message, Ts, UserId};

use crate::actions::{self, ActionHandler, ActionSet, MessageAction, MessageChange};
use crate::blockkit::BlockDispatch;
use crate::composer::{AlsoSendTarget, Composer, ComposerEvent, ContextBar};
use crate::emoji::EmojiChoice;
use crate::fixtures::Fixtures;
use crate::later::{Remind, SaveRequest};
use crate::messages::{LinkHandler, LinkTarget, message_row_linked, same_group};
use crate::slash::{Invocation, SHRUG, SlashKind};

pub enum ThreadPanelEvent {
    Close,
    /// A message's avatar or name was clicked: show that user's profile.
    OpenProfile(UserId),
    /// A thread image was clicked: the root opens the image viewer over the thread's
    /// `images`, at `index`.
    OpenImage {
        channel: ChannelId,
        images: Arc<Vec<crate::image_viewer::ViewerImage>>,
        index: usize,
    },
    /// A link in message text was clicked: profile, channel, thread or URL (permalinks
    /// resolved in-app by the root).
    OpenLink(LinkTarget),
    /// Forward this message (its permalink): the root opens New message with it drafted.
    Forward(String),
    /// The header menu followed or unfollowed the open thread (already shown here); the root
    /// tells Slack. `last_read` is the thread's newest message.
    SetSubscribed {
        channel: ChannelId,
        thread_ts: Ts,
        last_read: Ts,
        subscribed: bool,
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
    /// An add-reaction control asked for the emoji picker at `position`; the root routes the
    /// pick back through [`ThreadPanel::add_reaction`].
    PickReaction {
        ts: Ts,
        position: Point<Pixels>,
    },
    /// The reply composer's emoji button asked for the picker; picks come back through
    /// [`ThreadPanel::insert_emoji`].
    PickEmoji(Point<Pixels>),
    /// A slash command other than /shrug (which replies in the thread) was sent from the
    /// reply composer; the root dispatches it.
    SlashCommand(Invocation),
    /// Save for later / Remind me / unsave; the root keeps the Later list.
    Later(SaveRequest),
}

/// Avatar chip size and the text-column inset (avatar plus its gap), mirroring `messages.rs`
/// so reaction chips align with the message text column.
const AVATAR_SIZE: f32 = 36.;
const AVATAR_GAP: f32 = 12.;
const TEXT_INSET: f32 = AVATAR_SIZE + AVATAR_GAP;

pub struct ThreadPanel {
    fixtures: Arc<Fixtures>,
    /// `(channel, root ts)` of the open thread.
    thread: Option<(ChannelId, Ts)>,
    root: Option<Message>,
    replies: Vec<Message>,
    /// Whether the "Replying in thread" hint above the composer shows (Reply in thread action).
    reply_hint: bool,
    /// Message currently edited in place; its draft lives in `edit_draft`.
    editing: Option<Ts>,
    edit_draft: Entity<TextareaState>,
    /// Fixture-mode unread marker: the New divider renders above this message.
    unread_after: Option<Ts>,
    /// Messages saved for later (fixture mode: in memory).
    saved: HashSet<Ts>,
    composer: Entity<Composer>,
    /// The thread body's scroll, so a linked reply can be scrolled into view.
    scroll: ScrollHandle,
    /// A reply permalink's message, scrolled into view once it is loaded.
    reveal: Option<Ts>,
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
            composer.update(cx, |composer, cx| composer.set_scheduling(false, cx));
            let edit_draft = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Edit message")
                    .auto_grow(1, 8)
            });
            let subscriptions = vec![
                cx.subscribe_in(
                    &composer,
                    window,
                    |this: &mut ThreadPanel, _, event: &ComposerEvent, window, cx| match event {
                        ComposerEvent::Submit(content) => this.reply(content.clone(), cx),
                        ComposerEvent::OpenEmojiPicker(position) => {
                            cx.emit(ThreadPanelEvent::PickEmoji(*position));
                        }
                        ComposerEvent::SlashCommand(invocation) => {
                            match invocation.command.map(|command| command.kind) {
                                Some(SlashKind::Shrug) if invocation.args.is_empty() => {
                                    this.reply(MessageContent::plain(SHRUG), cx);
                                }
                                Some(SlashKind::Shrug) => {
                                    let text = format!("{} {SHRUG}", invocation.args);
                                    this.reply(MessageContent::plain(text), cx);
                                }
                                _ => cx.emit(ThreadPanelEvent::SlashCommand(invocation.clone())),
                            }
                        }
                        // Scheduling is turned off in the reply composer.
                        ComposerEvent::Schedule { .. } => {}
                        ComposerEvent::EditLast => this.edit_last_reply(window, cx),
                    },
                ),
                // The edit box's Save button tracks whether the draft is empty.
                cx.subscribe(&edit_draft, |_: &mut ThreadPanel, _, _: &InputEvent, cx| {
                    cx.notify()
                }),
            ];
            ThreadPanel {
                fixtures,
                thread: None,
                root: None,
                replies: Vec::new(),
                reply_hint: false,
                editing: None,
                edit_draft,
                unread_after: None,
                saved: HashSet::new(),
                composer,
                scroll: ScrollHandle::new(),
                reveal: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: the open thread stays open, drawn from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| {
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

    /// Forward the Enter-to-send preference to the reply composer.
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.set_enter_sends(enter_sends, cx));
    }

    pub fn open(&mut self, channel: ChannelId, thread_ts: Ts, cx: &mut Context<Self>) {
        self.root = self
            .fixtures
            .messages
            .get(&channel)
            .and_then(|history| history.iter().find(|m| m.ts == thread_ts))
            .cloned();
        let target =
            self.fixtures
                .conversation(&channel)
                .map(|conversation| match conversation.kind() {
                    ConversationKind::Im | ConversationKind::Mpim => AlsoSendTarget::DirectMessage,
                    kind => AlsoSendTarget::Channel {
                        name: conversation.name.clone().unwrap_or_default().into(),
                        private: kind == ConversationKind::PrivateChannel,
                    },
                });
        self.composer
            .update(cx, |composer, cx| composer.set_also_send(target, cx));
        let key = (channel, thread_ts);
        self.replies = self.fixtures.threads.get(&key).cloned().unwrap_or_default();
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
        if self.thread.as_ref() != Some(&(channel, thread_ts.clone())) {
            return;
        }
        self.root = messages
            .first()
            .filter(|message| message.ts == thread_ts)
            .cloned();
        self.replies = match self.root.is_some() {
            true => messages.into_iter().skip(1).collect(),
            false => messages,
        };
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

    /// Save (or unsave) `ts` of the open thread, optionally with a reminder, and hand the
    /// request to the root's Later list.
    fn save_for_later(
        &mut self,
        ts: &Ts,
        saved: bool,
        remind: Option<Remind>,
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
        let request = SaveRequest {
            channel,
            ts: ts.clone(),
            author: message
                .user
                .as_ref()
                .map(|user| fixtures.user_name(user).to_owned())
                .unwrap_or_default(),
            text: fixtures.preview(message),
            saved,
            due: remind.map(|remind| remind.at(crate::drafts::now_secs())),
        };
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
        // The local echo shows the same blocks the reply posts with.
        let mut message =
            self.fixtures
                .local_message(&channel, content.text.clone(), Some(thread_ts.clone()));
        message.blocks = content.blocks.clone();
        self.replies.push(message);
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
                if let Some(message) = self.message_at_mut(ts).map(|message| message.clone()) {
                    let text = self.fixtures.plain_text(&message);
                    cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(text));
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
                self.save_for_later(ts, true, Some(remind), &title, window, cx);
            }
            MessageAction::MarkUnread => self.mark_unread(ts.clone(), cx),
            MessageAction::Edit => self.begin_edit(ts.clone(), window, cx),
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
            MessageAction::OpenProfile(user) => cx.emit(ThreadPanelEvent::OpenProfile(user)),
            MessageAction::OpenImage(origin) => {
                // The viewer pages through the thread's images, root first.
                let Some((channel, _)) = self.thread.clone() else {
                    return;
                };
                let mut images = Vec::new();
                let mut index = None;
                for message in self.root.iter().chain(&self.replies) {
                    for entry in crate::image_viewer::message_images(&self.fixtures, message) {
                        if index.is_none() && &message.ts == ts && entry.origin == origin {
                            index = Some(images.len());
                        }
                        images.push(entry.image);
                    }
                }
                if let Some(index) = index {
                    cx.emit(ThreadPanelEvent::OpenImage {
                        channel,
                        // Main-thread only, like the transcript's: the Arc only shares it.
                        #[allow(clippy::arc_with_non_send_sync)]
                        images: Arc::new(images),
                        index,
                    });
                }
            }
            MessageAction::BlockAction { action, attachment } => {
                let live = !self.fixtures.sample;
                let channel = self.thread.as_ref().map(|(channel, _)| channel.clone());
                let dispatch = channel
                    .zip(self.message_at_mut(ts))
                    .and_then(|(channel, message)| {
                        BlockDispatch::new(message, &channel, action.clone(), attachment)
                    })
                    .filter(|_| live);
                match dispatch {
                    Some(dispatch) => self.post(MessageChange::BlockAction(dispatch), cx),
                    None => crate::blockkit::log_button(&action, window, cx),
                }
            }
        }
    }

    /// Edit `ts` in place: swap the row body for the shared edit draft, focused with the caret
    /// at the end.
    fn begin_edit(&mut self, ts: Ts, window: &mut Window, cx: &mut Context<Self>) {
        let Some(message) = self.message_at_mut(&ts) else {
            return;
        };
        let text = message.text.clone();
        self.reply_hint = false;
        self.editing = Some(ts);
        let caret = text.len();
        self.edit_draft.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_selected_range(caret..caret, cx);
        });
        self.edit_draft.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some(ts) = self.editing.clone() else {
            return;
        };
        let text = self.edit_draft.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let me = self.fixtures.self_user.clone();
        if let Some(message) = self.message_at_mut(&ts) {
            actions::apply_edit(message, &me, text.clone());
            self.post(MessageChange::Edit { ts, text }, cx);
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
        self.unread_after = Some(ts);
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
            let (cancel, save) = (this.clone(), this);
            return div()
                .px_2()
                .child(actions::edit_box(
                    &key,
                    &self.edit_draft,
                    move |_, cx| cancel.update(cx, ThreadPanel::cancel_edit),
                    move |_, cx| save.update(cx, ThreadPanel::save_edit),
                    cx,
                ))
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
        };
        let row_group = SharedString::from(format!("thread-row-group-{}", ts.as_str()));
        let hover_bg = cx.theme().secondary.opacity(0.5);

        // `message_row` renders its own static chips; strip them here and render the
        // interactive chips (toggle + add control) below the card instead.
        let mut display = message.clone();
        let reactions = std::mem::take(&mut display.reactions);
        // Like Slack, a reply continuing the previous author's run drops its avatar and name.
        let card = message_row_linked(
            &self.fixtures,
            &display,
            Some(&handler),
            Some(&links),
            grouped.then(|| row_group.clone()),
            cx,
        );
        let chips = actions::reaction_chips(&key, &reactions, &self.fixtures, Some(&handler), cx)
            .map(|chips| chips.pl(px(TEXT_INSET)));

        let toolbar = actions::hover_toolbar(
            &key,
            &self.fixtures.quick_reactions,
            set,
            &handler,
            row_group.clone(),
            cx,
        );
        let menu_handler = handler;
        div()
            .id(SharedString::from(key))
            .group(row_group)
            .relative()
            .w_full()
            .min_w_0()
            .px_2()
            .when(!grouped, |row| row.pt_1())
            .pb_1()
            .rounded_md()
            .hover(|row| row.bg(hover_bg))
            .when(saved, |row| row.child(actions::saved_banner(cx)))
            .child(v_flex().min_w_0().child(card).children(chips))
            .child(toolbar)
            .context_menu(move |menu, window, cx| {
                actions::actions_menu(menu, set, &menu_handler, window, cx)
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
            let following = root.subscribed == Some(true);
            Button::new("thread-more")
                .ghost()
                .with_size(Size::Small)
                .icon(IconName::EllipsisVertical)
                .tooltip("More actions")
                .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                    let (toggle, copy, root_ts) = (panel.clone(), panel.clone(), root_ts.clone());
                    menu.item(
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
                    )
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
                h_flex().items_center().gap_1().children(more).child(
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
                if !self.replies.is_empty() {
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
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows);

        let composer_area = v_flex()
            .flex_none()
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
