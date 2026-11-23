//! Popped-out secondary GPUI windows for a conversation, thread, Activity feed, or Saved/Later
//! (`F-CORE-010`, `F-CORE-026`, `F-CORE-109`, `F-SHELL-149`): lightweight native window that
//! observes the main window's live entities for real-time updates and jumps back to the main
//! window when an Activity or Saved item is clicked.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, Bounds, ClipboardItem, Context, DisplayId, Edges, Entity,
    Focusable as _, FontWeight, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, anchored, deferred, div, point, px, size,
};
use gpui_kit::open_window;
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin, MessageAction};
use crate::activity::ActivityFeed;
use crate::composer::{self, Composer, ComposerEvent};
use crate::emoji::{EmojiChoice, EmojiPicker, EmojiPickerEvent, PickerTab};
use crate::fixtures::Fixtures;
use crate::image_viewer::{self, ImageOrigin, ImageViewer, ViewerEvent};
use crate::later::{LaterItem, LaterView, SaveRequest};
use crate::messages::{
    LinkHandler, LinkTarget, MessageList, message_row_linked, reply_bar, same_group,
};

/// Summary row for a popped-out Activity window (`F-SHELL-149`).
#[derive(Clone)]
pub struct PopoutActivityRow {
    pub channel: ChannelId,
    pub thread_ts: Option<Ts>,
    pub title: String,
    pub body: String,
    pub context: String,
    pub unread: bool,
}

/// Callback invoked when clicking a row in a popped-out Activity or Saved window:
/// activates the main window and navigates to `(channel, ts, is_thread)`.
pub type JumpHandler = Rc<dyn Fn(ChannelId, Option<Ts>, bool, &mut App)>;

/// What this popped-out native window displays.
#[derive(Clone)]
pub enum PopoutKind {
    Conversation {
        channel: ChannelId,
        source: WeakEntity<MessageList>,
        on_jump: JumpHandler,
    },
    Thread {
        channel: ChannelId,
        thread_ts: Ts,
        source: WeakEntity<MessageList>,
        on_jump: JumpHandler,
    },
    Activity {
        source: WeakEntity<ActivityFeed>,
        on_jump: JumpHandler,
    },
    Later {
        source: WeakEntity<LaterView>,
        on_jump: JumpHandler,
    },
    /// Popped-out Slack Canvas window (`F-COLLAB-128`, `F-SHELL-148`).
    Canvas {
        doc: crate::canvas::CanvasDoc,
        file_id: Option<String>,
        open_browser_url: Option<String>,
        on_jump: JumpHandler,
    },
    /// Popped-out Files browser window (`F-COLLAB-127`, `F-SHELL-150`).
    Files {
        source: WeakEntity<crate::files_page::FilesPage>,
        on_jump: JumpHandler,
    },
}

/// Callback that forwards commands (`SendMessage`, etc.) to the workspace's live `CoreBridge`.
pub type CommandForward = Arc<dyn Fn(Command) + Send + Sync>;

static CASCADE_STEP: AtomicUsize = AtomicUsize::new(0);

fn popout_window_options(
    title: &str,
    display_id: Option<DisplayId>,
    cascade_slot: usize,
    cx: &App,
) -> WindowOptions {
    let mut bounds = Bounds::centered(display_id, size(px(720.), px(640.)), cx);
    let offset = px((cascade_slot % 5) as f32 * 28.);
    bounds.origin.x += offset;
    bounds.origin.y += offset;
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title.to_owned())),
            traffic_light_position: Some(point(px(8.), px(12.))),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    }
}

/// Open a new native GPUI window showing `kind`, centered (with cascade offset) on `display_id`
/// (the main window's display) or the primary display if `None`.
#[allow(clippy::too_many_arguments)]
pub fn open_popout(
    kind: PopoutKind,
    title: String,
    fixtures: Arc<Fixtures>,
    store: Option<SharedStore>,
    team: Option<TeamId>,
    send: CommandForward,
    later_sync: Option<(WeakEntity<LaterView>, usize)>,
    display_id: Option<DisplayId>,
    cx: &mut App,
) {
    let slot = CASCADE_STEP.fetch_add(1, Ordering::Relaxed);
    let options = popout_window_options(&title, display_id, slot, cx);
    let _ = open_window(options, cx, move |window, cx| {
        window.set_window_title(&format!("{title} — slackless"));
        window.activate_window();
        cx.new(|cx| {
            PopoutWindow::new(
                kind, title, fixtures, store, team, send, later_sync, window, cx,
            )
        })
    });
}

enum PopoutPickerTarget {
    Composer,
    Message { channel: ChannelId, ts: Ts },
}

pub struct PopoutWindow {
    kind: PopoutKind,
    title: String,
    fixtures: Arc<Fixtures>,
    store: Option<SharedStore>,
    team: Option<TeamId>,
    send: CommandForward,
    later_sync: Option<(WeakEntity<LaterView>, usize)>,
    messages: Vec<Message>,
    saved: HashSet<Ts>,
    activity_rows: Vec<PopoutActivityRow>,
    later_items: Vec<LaterItem>,
    files_items: Vec<(String, String, Option<String>)>,
    composer: Entity<Composer>,
    emoji_picker: Entity<EmojiPicker>,
    picker: Option<(PopoutPickerTarget, Point<Pixels>)>,
    editing: Option<(ChannelId, Ts, Entity<TextareaState>)>,
    scroll: ScrollHandle,
    image_viewer: Option<Entity<ImageViewer>>,
    _viewer_sub: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl PopoutWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: PopoutKind,
        title: String,
        fixtures: Arc<Fixtures>,
        store: Option<SharedStore>,
        team: Option<TeamId>,
        send: CommandForward,
        later_sync: Option<(WeakEntity<LaterView>, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match &kind {
            PopoutKind::Conversation { .. } => format!("Message {title}"),
            PopoutKind::Thread { .. } => "Reply in thread…".to_owned(),
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. } => String::new(),
        };
        let has_composer = matches!(
            &kind,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. }
        );
        let composer = Composer::new(placeholder.into(), fixtures.clone(), window, cx);
        let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
        let initial = load_messages(&kind, &fixtures, store.as_ref(), team.as_ref());
        let mut subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this: &mut Self, _, event: &ComposerEvent, window, cx| match event {
                    ComposerEvent::Submit(content) => this.send_content(content.clone(), cx),
                    ComposerEvent::OpenEmojiPicker(pos) => {
                        this.open_picker(PopoutPickerTarget::Composer, *pos, window, cx);
                    }
                    ComposerEvent::EditLast => {
                        if let Some(ch) = match &this.kind {
                            PopoutKind::Conversation { channel, .. }
                            | PopoutKind::Thread { channel, .. } => Some(channel.clone()),
                            _ => None,
                        } {
                            if let Some(last_own) = this
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m.user.as_ref() == Some(&this.fixtures.self_user))
                                .cloned()
                            {
                                this.start_edit(ch, &last_own, window, cx);
                            }
                        }
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &emoji_picker,
                window,
                |this: &mut Self, _, event: &EmojiPickerEvent, window, cx| {
                    if let EmojiPickerEvent::Pick(choice) = event {
                        this.pick_emoji(choice, window, cx);
                    }
                },
            ),
        ];
        let mut saved = HashSet::new();
        let active_channel = match &kind {
            PopoutKind::Conversation { channel, .. } | PopoutKind::Thread { channel, .. } => {
                Some(channel.clone())
            }
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. } => None,
        };
        if let (Some(ch), Some((weak_later, workspace))) = (active_channel.clone(), &later_sync) {
            if let Some(later) = weak_later.upgrade() {
                saved = later
                    .read(cx)
                    .saved_in(*workspace)
                    .into_iter()
                    .filter(|(c, _)| *c == ch)
                    .map(|(_, ts)| ts)
                    .collect();
                let workspace = *workspace;
                subscriptions.push(cx.observe(&later, move |this, later, cx| {
                    this.saved = later
                        .read(cx)
                        .saved_in(workspace)
                        .into_iter()
                        .filter(|(c, _)| *c == ch)
                        .map(|(_, ts)| ts)
                        .collect();
                    cx.notify();
                }));
            }
        }
        let mut activity_rows = Vec::new();
        let mut later_items = Vec::new();
        let mut files_items = Vec::new();
        match &kind {
            PopoutKind::Conversation { source, .. } | PopoutKind::Thread { source, .. } => {
                if let Some(list) = source.upgrade() {
                    subscriptions.push(cx.observe(&list, |this, _, cx| {
                        this.reload_from_store(cx);
                    }));
                }
            }
            PopoutKind::Activity { source, .. } => {
                if let Some(feed) = source.upgrade() {
                    activity_rows = feed.read(cx).snapshot_for_popout();
                    subscriptions.push(cx.observe(&feed, |this, feed, cx| {
                        this.activity_rows = feed.read(cx).snapshot_for_popout();
                        cx.notify();
                    }));
                }
            }
            PopoutKind::Later { source, .. } => {
                if let Some(later) = source.upgrade() {
                    later_items = later.read(cx).items_for_popout();
                    subscriptions.push(cx.observe(&later, |this, later, cx| {
                        this.later_items = later.read(cx).items_for_popout();
                        cx.notify();
                    }));
                }
            }
            PopoutKind::Files { source, .. } => {
                if let Some(page) = source.upgrade() {
                    files_items = page.read(cx).snapshot_for_popout();
                    subscriptions.push(cx.observe(&page, |this, page, cx| {
                        this.files_items = page.read(cx).snapshot_for_popout();
                        cx.notify();
                    }));
                }
            }
            PopoutKind::Canvas { .. } => {}
        }
        // Fallback periodic refresh from the shared SQLite store for conversations that are not
        // currently selected in the main window's `MessageList`.
        if store.is_some() && has_composer {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let keep = this
                        .update(cx, |win, cx| {
                            win.reload_from_store(cx);
                        })
                        .is_ok();
                    if !keep {
                        break;
                    }
                }
            })
            .detach();
        }
        let scroll = ScrollHandle::new();
        if !initial.is_empty() {
            scroll.scroll_to_bottom();
        }
        Self {
            kind,
            title,
            fixtures,
            store,
            team,
            send,
            later_sync,
            messages: initial,
            saved,
            activity_rows,
            later_items,
            files_items,
            composer,
            emoji_picker,
            picker: None,
            editing: None,
            scroll,
            image_viewer: None,
            _viewer_sub: None,
            _subscriptions: subscriptions,
        }
    }

    fn open_picker(
        &mut self,
        target: PopoutPickerTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.emoji_picker.update(cx, |picker, cx| {
            picker.open_tab(PickerTab::Emoji, false, window, cx);
        });
        self.picker = Some((target, position));
        cx.notify();
    }

    fn pick_emoji(&mut self, choice: &EmojiChoice, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, _)) = self.picker.take() else {
            return;
        };
        match target {
            PopoutPickerTarget::Composer => {
                self.composer
                    .update(cx, |c, cx| c.insert_emoji(choice, window, cx));
            }
            PopoutPickerTarget::Message { channel, ts } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    if actions::add_reaction(msg, &self.fixtures.self_user, &choice.name) {
                        (self.send)(Command::AddReaction {
                            channel,
                            ts,
                            name: choice.name.clone(),
                        });
                    }
                }
            }
        }
        cx.notify();
    }

    fn start_edit(
        &mut self,
        channel: ChannelId,
        message: &Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = self.fixtures.plain_text(message);
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .placeholder("Edit message…")
                .auto_grow(1, 6);
            state.set_value(initial, window, cx);
            state
        });
        input.read(cx).focus_handle(cx).focus(window, cx);
        self.editing = Some((channel, message.ts.clone(), input));
        cx.notify();
    }

    fn save_edit(&mut self, cx: &mut Context<Self>) {
        let Some((channel, ts, input)) = self.editing.take() else {
            return;
        };
        let text = input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            cx.notify();
            return;
        }
        let content = composer::content_of(&text, &self.fixtures);
        if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
            actions::apply_edit(msg, &self.fixtures.self_user, content.clone());
        }
        (self.send)(Command::EditMessage {
            channel,
            ts,
            content,
        });
        cx.notify();
    }

    fn sync_later_item(
        &self,
        channel: &ChannelId,
        message: &Message,
        saved: bool,
        due: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some((weak_later, workspace)) = &self.later_sync else {
            return;
        };
        let Some(later) = weak_later.upgrade() else {
            return;
        };
        let author = message
            .user
            .as_ref()
            .map(|u| self.fixtures.user_name(u).to_owned())
            .or_else(|| message.username.clone())
            .unwrap_or_else(|| "Message".to_owned());
        let text = self.fixtures.preview(message);
        let request = SaveRequest {
            channel: channel.clone(),
            ts: message.ts.clone(),
            author,
            text,
            saved,
            due,
        };
        let label = self.title.clone();
        let workspace = *workspace;
        later.update(cx, |later, cx| later.save(request, workspace, label, cx));
    }

    fn jump_handler(&self) -> JumpHandler {
        match &self.kind {
            PopoutKind::Conversation { on_jump, .. }
            | PopoutKind::Thread { on_jump, .. }
            | PopoutKind::Activity { on_jump, .. }
            | PopoutKind::Later { on_jump, .. }
            | PopoutKind::Canvas { on_jump, .. }
            | PopoutKind::Files { on_jump, .. } => on_jump.clone(),
        }
    }

    fn open_image_viewer(
        &mut self,
        ts: &Ts,
        origin: ImageOrigin,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut images = Vec::new();
        let mut index = None;
        for message in &self.messages {
            for entry in image_viewer::message_images(&self.fixtures, message) {
                if message.ts == *ts && entry.origin == origin && index.is_none() {
                    index = Some(images.len());
                }
                images.push(entry.image);
            }
        }
        let Some(index) = index else {
            return;
        };
        #[allow(clippy::arc_with_non_send_sync)]
        let viewer = ImageViewer::new(
            self.fixtures.clone(),
            self.title.clone(),
            Arc::new(images),
            index,
            cx,
        );
        self._viewer_sub = Some(cx.subscribe_in(
            &viewer,
            window,
            |this: &mut Self, _, event: &ViewerEvent, _, cx| {
                if matches!(event, ViewerEvent::Close) {
                    this.close_image_viewer(cx);
                }
            },
        ));
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    fn close_image_viewer(&mut self, cx: &mut Context<Self>) {
        if self.image_viewer.take().is_some() {
            self._viewer_sub = None;
            cx.notify();
        }
    }

    fn handle_message_action(
        &mut self,
        channel: ChannelId,
        message: Message,
        action: MessageAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ts = message.ts.clone();
        match action {
            MessageAction::React(name) => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    if actions::add_reaction(msg, &self.fixtures.self_user, &name) {
                        (self.send)(Command::AddReaction { channel, ts, name });
                        cx.notify();
                    }
                }
            }
            MessageAction::ToggleReaction(name) => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    let add = actions::toggle_reaction(msg, &self.fixtures.self_user, &name);
                    if add {
                        (self.send)(Command::AddReaction { channel, ts, name });
                    } else {
                        (self.send)(Command::RemoveReaction { channel, ts, name });
                    }
                    cx.notify();
                }
            }
            MessageAction::PickReaction(position) => {
                self.open_picker(
                    PopoutPickerTarget::Message { channel, ts },
                    position,
                    window,
                    cx,
                );
            }
            MessageAction::ReplyInThread => {
                let thread_ts = message.thread_ts.clone().unwrap_or(ts);
                let jump = self.jump_handler();
                jump(channel, Some(thread_ts), true, cx);
            }
            MessageAction::CopyLink => {
                actions::copy_link(&self.fixtures, &channel, &ts, window, cx);
            }
            MessageAction::CopyText => {
                let text = self.fixtures.plain_text(&message);
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                actions::notify("Copied", "Message text copied.", window, cx);
            }
            MessageAction::Forward => {
                let link = actions::permalink(&self.fixtures, &channel, &ts);
                cx.write_to_clipboard(ClipboardItem::new_string(link));
                actions::notify(
                    "Link copied",
                    "Paste in any channel to forward this message.",
                    window,
                    cx,
                );
            }
            MessageAction::SaveForLater => {
                let saved = self.saved.insert(ts.clone());
                if saved {
                    (self.send)(Command::SaveItem {
                        channel: channel.clone(),
                        ts: ts.clone(),
                        date_due: None,
                    });
                } else {
                    self.saved.remove(&ts);
                    (self.send)(Command::DeleteSavedItem {
                        channel: channel.clone(),
                        ts: ts.clone(),
                    });
                }
                self.sync_later_item(&channel, &message, saved, None, cx);
                actions::notify(
                    if saved {
                        "Saved for later"
                    } else {
                        "Removed from Later"
                    },
                    String::new(),
                    window,
                    cx,
                );
                cx.notify();
            }
            MessageAction::RemindMe(remind) => {
                let due = remind.at(crate::drafts::now_secs());
                self.saved.insert(ts.clone());
                (self.send)(Command::SaveItem {
                    channel: channel.clone(),
                    ts,
                    date_due: Some(due),
                });
                self.sync_later_item(&channel, &message, true, Some(due), cx);
                actions::notify("Reminder set", remind.label().to_lowercase(), window, cx);
                cx.notify();
            }
            MessageAction::RemindCustom => {
                let this = cx.entity().downgrade();
                let msg_clone = message.clone();
                crate::later::open_custom_remind_dialog(window, cx, move |due, window, cx| {
                    let _ = this.update(cx, |win, cx| {
                        win.saved.insert(ts.clone());
                        (win.send)(Command::SaveItem {
                            channel: channel.clone(),
                            ts: ts.clone(),
                            date_due: Some(due),
                        });
                        win.sync_later_item(&channel, &msg_clone, true, Some(due), cx);
                        let when = crate::drafts::when_label(due, crate::drafts::now_secs());
                        actions::notify("Reminder set", when, window, cx);
                        cx.notify();
                    });
                });
            }
            MessageAction::TogglePin => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    let pinned = actions::toggle_pin(msg, &channel);
                    (self.send)(Command::SetPinned {
                        channel,
                        ts,
                        pinned,
                    });
                    actions::notify(
                        if pinned {
                            "Pinned to conversation"
                        } else {
                            "Unpinned from conversation"
                        },
                        String::new(),
                        window,
                        cx,
                    );
                    cx.notify();
                }
            }
            MessageAction::MarkUnread => {
                match &self.kind {
                    PopoutKind::Thread { thread_ts, .. } => {
                        let cursor = self
                            .messages
                            .iter()
                            .take_while(|m| m.ts < ts)
                            .last()
                            .map(|m| m.ts.clone())
                            .unwrap_or_else(|| crate::threads_cards::ts_just_before(&ts));
                        (self.send)(Command::MarkThreadRead {
                            channel,
                            thread_ts: thread_ts.clone(),
                            ts: cursor,
                        });
                    }
                    _ => {
                        (self.send)(Command::MarkRead { channel, ts });
                    }
                }
                actions::notify("Marked unread", String::new(), window, cx);
            }
            MessageAction::Edit => {
                self.start_edit(channel, &message, window, cx);
            }
            MessageAction::ViewHistory => {
                let jump = self.jump_handler();
                jump(channel, Some(ts), false, cx);
            }
            MessageAction::Delete => {
                let this = cx.entity().downgrade();
                let fixtures = self.fixtures.clone();
                actions::confirm_delete(message, fixtures, window, cx, move |cx| {
                    let _ = this.update(cx, |win, cx| {
                        win.messages.retain(|m| m.ts != ts);
                        (win.send)(Command::DeleteMessage {
                            channel: channel.clone(),
                            ts: ts.clone(),
                        });
                        cx.notify();
                    });
                });
            }
            MessageAction::OpenProfile(_) => {
                let jump = self.jump_handler();
                jump(channel, Some(ts), false, cx);
            }
            MessageAction::OpenImage(origin) => {
                self.open_image_viewer(&ts, origin, window, cx);
            }
            MessageAction::BlockAction { action, attachment } => {
                if let Some(dispatch) = crate::blockkit::BlockDispatch::new(
                    &message,
                    &channel,
                    action.clone(),
                    attachment,
                ) {
                    (self.send)(Command::BlockAction {
                        bot: dispatch.bot,
                        app_id: dispatch.app_id,
                        team: dispatch.team,
                        action: dispatch.action,
                        container: dispatch.container,
                    });
                } else {
                    crate::blockkit::log_button(&action, window, cx);
                }
            }
            MessageAction::DismissEphemeral => {
                self.messages.retain(|m| m.ts != ts);
                cx.notify();
            }
            MessageAction::AddToList => {}
        }
    }

    fn reload_from_store(&mut self, cx: &mut Context<Self>) {
        match &self.kind {
            PopoutKind::Activity { source, .. } => {
                if let Some(feed) = source.upgrade() {
                    self.activity_rows = feed.read(cx).snapshot_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Later { source, .. } => {
                if let Some(later) = source.upgrade() {
                    self.later_items = later.read(cx).items_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Files { source, .. } => {
                if let Some(page) = source.upgrade() {
                    self.files_items = page.read(cx).snapshot_for_popout();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Canvas { .. } => return,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. } => {}
        }
        let fresh = load_messages(
            &self.kind,
            &self.fixtures,
            self.store.as_ref(),
            self.team.as_ref(),
        );
        if fresh.len() != self.messages.len()
            || fresh.last().map(|m| &m.ts) != self.messages.last().map(|m| &m.ts)
        {
            self.messages = fresh;
            self.scroll.scroll_to_bottom();
            cx.notify();
        }
    }

    fn send_content(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        let (channel, thread_ts) = match &self.kind {
            PopoutKind::Conversation { channel, .. } => (channel.clone(), None),
            PopoutKind::Thread {
                channel, thread_ts, ..
            } => (channel.clone(), Some(thread_ts.clone())),
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. } => return,
        };
        let mut local =
            self.fixtures
                .local_message(&channel, content.text.clone(), thread_ts.clone());
        local.blocks = content.blocks.clone();
        local.attachments = content.attachments.clone();
        self.messages.push(local);
        self.scroll.scroll_to_bottom();
        (self.send)(Command::SendMessage {
            channel,
            content,
            thread_ts,
            broadcast: false,
        });
        cx.notify();
    }
}

fn load_messages(
    kind: &PopoutKind,
    fixtures: &Fixtures,
    store: Option<&SharedStore>,
    team: Option<&TeamId>,
) -> Vec<Message> {
    if matches!(
        kind,
        PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. }
    ) {
        return Vec::new();
    }
    if let (Some(store), Some(team)) = (store, team) {
        if let Ok(guard) = store.lock() {
            let rows = match kind {
                PopoutKind::Conversation { channel, .. } => {
                    guard.channel_history(team, channel, None, 200)
                }
                PopoutKind::Thread {
                    channel, thread_ts, ..
                } => guard.thread_replies(team, channel, thread_ts),
                PopoutKind::Activity { .. }
                | PopoutKind::Later { .. }
                | PopoutKind::Canvas { .. }
                | PopoutKind::Files { .. } => Ok(Vec::new()),
            };
            if let Ok(rows) = rows {
                return rows
                    .into_iter()
                    .filter(|r| !r.deleted)
                    .map(|r| r.message)
                    .collect();
            }
        }
    }
    match kind {
        PopoutKind::Conversation { channel, .. } => {
            fixtures.messages.get(channel).cloned().unwrap_or_default()
        }
        PopoutKind::Thread {
            channel, thread_ts, ..
        } => fixtures
            .threads
            .get(&(channel.clone(), thread_ts.clone()))
            .cloned()
            .unwrap_or_default(),
        PopoutKind::Activity { .. }
        | PopoutKind::Later { .. }
        | PopoutKind::Canvas { .. }
        | PopoutKind::Files { .. } => Vec::new(),
    }
}

impl Render for PopoutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted, fg, accent, hover_bg, saved_bg) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.accent,
            theme.secondary.opacity(0.5),
            theme.warning.opacity(0.08),
        );
        let jump_for_links = self.jump_handler();
        let links: LinkHandler = Rc::new(move |target, _, cx| match target {
            LinkTarget::Url(url) => cx.open_url(url.as_ref()),
            LinkTarget::Channel(channel) => jump_for_links(channel.clone(), None, false, cx),
            LinkTarget::Thread { channel, thread_ts } => {
                jump_for_links(channel.clone(), Some(thread_ts.clone()), true, cx)
            }
            LinkTarget::Profile(_) => {}
        });
        let kind_label = match &self.kind {
            PopoutKind::Conversation { .. } => "Conversation",
            PopoutKind::Thread { .. } => "Thread",
            PopoutKind::Activity { .. } => "Activity · Live",
            PopoutKind::Later { .. } => "Later · Live",
            PopoutKind::Canvas { .. } => "Canvas",
            PopoutKind::Files { .. } => "Files · Live",
        };
        let has_composer = matches!(
            &self.kind,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. }
        );
        let header = TitleBar::new()
            .h(px(40.))
            .bg(bg)
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_3()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(kind_label),
                            ),
                    )
                    .child(
                        Button::new("popout-refresh")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::RefreshCw)
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.reload_from_store(cx))),
                    ),
            );
        let body = match &self.kind {
            PopoutKind::Conversation {
                channel, on_jump, ..
            }
            | PopoutKind::Thread {
                channel, on_jump, ..
            } => {
                let is_conversation = matches!(self.kind, PopoutKind::Conversation { .. });
                let channel = channel.clone();
                let on_jump = on_jump.clone();
                let view = cx.entity();
                let mut rows: Vec<gpui_kit::gpui::AnyElement> =
                    Vec::with_capacity(self.messages.len());
                let mut prev_msg: Option<&Message> = None;
                let editing = self.editing.clone();
                for (ix, msg) in self.messages.iter().enumerate() {
                    if let Some((_, edit_ts, edit_input)) = &editing {
                        if *edit_ts == msg.ts {
                            let edit_row = v_flex()
                                .id(SharedString::from(format!("popout-edit-{ix}")))
                                .mx_4()
                                .my_1()
                                .p_2p5()
                                .gap_2()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.theme().ring)
                                .bg(bg)
                                .child(Textarea::new(edit_input).appearance(false).bordered(false))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            Button::new("popout-edit-cancel")
                                                .outline()
                                                .with_size(Size::XSmall)
                                                .label("Cancel")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.editing = None;
                                                    cx.notify();
                                                })),
                                        )
                                        .child(
                                            Button::new("popout-edit-save")
                                                .primary()
                                                .with_size(Size::XSmall)
                                                .label("Save Changes")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.save_edit(cx);
                                                })),
                                        ),
                                );
                            rows.push(edit_row.into_any_element());
                            prev_msg = None;
                            continue;
                        }
                    }
                    let msg_group = SharedString::from(format!("popout-msg-{ix}"));
                    let key = format!("popout-row-{}", msg.ts.as_str());
                    let grouped = prev_msg
                        .filter(|prev| same_group(prev, msg))
                        .map(|_| msg_group.clone());
                    let is_saved = self.saved.contains(&msg.ts);
                    let view_for_action = view.clone();
                    let ch_for_action = channel.clone();
                    let msg_for_action = msg.clone();
                    let handler: ActionHandler = Rc::new(move |action, window, cx| {
                        let ch = ch_for_action.clone();
                        let m = msg_for_action.clone();
                        view_for_action.update(cx, |this, cx| {
                            this.handle_message_action(ch, m, action, window, cx);
                        });
                    });
                    let set = ActionSet {
                        own: msg.user.as_ref() == Some(&self.fixtures.self_user),
                        reply: is_conversation,
                        saved: is_saved,
                        pinned: msg.pinned_to.contains(&channel),
                    };
                    let mut display = msg.clone();
                    let reactions = std::mem::take(&mut display.reactions);
                    let chips = actions::reaction_chips(
                        &key,
                        &reactions,
                        &self.fixtures,
                        Some(&handler),
                        cx,
                    )
                    .map(|c| c.pl(px(48.)));
                    let row_body = message_row_linked(
                        &self.fixtures,
                        &display,
                        Some(&handler),
                        Some(&links),
                        grouped,
                        cx,
                    );
                    let reply_bar_el = if is_conversation {
                        let jump = on_jump.clone();
                        let ch = channel.clone();
                        let ts = msg.ts.clone();
                        let reply_group = SharedString::from(format!("popout-reply-{ix}"));
                        reply_bar(
                            &self.fixtures,
                            msg,
                            ("popout-thread-chip", ix),
                            msg_group.clone(),
                            reply_group,
                            move |_, cx| jump(ch.clone(), Some(ts.clone()), true, cx),
                            cx,
                        )
                        .map(|bar| div().pl(px(48.)).child(bar))
                    } else {
                        None
                    };
                    let quick = self.fixtures.quick_reactions.clone();
                    let menu_handler = handler.clone();
                    let row_key = key.clone();
                    let keep_inside = ix == 0;
                    let item =
                        actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
                            let revealed = hover.revealed(cx);
                            v_flex()
                                .id(SharedString::from(row_key.clone()))
                                .group(msg_group)
                                .relative()
                                .px_4()
                                .py_0p5()
                                .when(is_saved, |row| {
                                    row.bg(saved_bg).child(actions::saved_banner(cx))
                                })
                                .when(!is_saved && revealed, |row| row.bg(hover_bg))
                                .child(row_body)
                                .children(chips)
                                .children(reply_bar_el)
                                .when(revealed, |row| {
                                    row.child(
                                        actions::hover_toolbar(
                                            &row_key, &quick, set, &handler, &hover, cx,
                                        )
                                        .when(keep_inside, |bar| bar.top(px(3.))),
                                    )
                                })
                                .on_hover(hover.host_on_hover())
                                .context_menu(move |menu, window, cx| {
                                    actions::actions_menu(
                                        menu,
                                        set,
                                        &menu_handler,
                                        MenuOrigin::Context,
                                        window,
                                        cx,
                                    )
                                })
                                .into_any_element()
                        });
                    rows.push(item.into_any_element());
                    prev_msg = Some(msg);
                }
                v_flex()
                    .id("popout-messages")
                    .flex_1()
                    .min_h_0()
                    .py_3()
                    .gap_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows)
            }
            PopoutKind::Activity { on_jump, .. } => {
                let on_jump = on_jump.clone();
                let mut list = v_flex()
                    .id("popout-activity")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .overflow_y_scroll();
                if self.activity_rows.is_empty() {
                    list = list.child(
                        div()
                            .py_6()
                            .text_sm()
                            .text_color(muted)
                            .child("No activity items."),
                    );
                } else {
                    list =
                        list.children(self.activity_rows.iter().enumerate().map(|(ix, item)| {
                            let jump = on_jump.clone();
                            let channel = item.channel.clone();
                            let thread_ts = item.thread_ts.clone();
                            let is_thread = thread_ts.is_some();
                            v_flex()
                                .id(SharedString::from(format!("popout-act-{ix}")))
                                .gap_1()
                                .p_3()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(if item.unread { accent.opacity(0.2) } else { bg })
                                .cursor_pointer()
                                .hover(|el| el.bg(accent.opacity(0.28)))
                                .on_click(move |_, _, cx| {
                                    jump(channel.clone(), thread_ts.clone(), is_thread, cx);
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(item.title.clone()),
                                )
                                .child(div().text_sm().child(item.body.clone()))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(item.context.clone()),
                                )
                        }));
                }
                list
            }
            PopoutKind::Later { on_jump, .. } => {
                let on_jump = on_jump.clone();
                let mut list = v_flex()
                    .id("popout-later")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .overflow_y_scroll();
                if self.later_items.is_empty() {
                    list = list.child(
                        div()
                            .py_6()
                            .text_sm()
                            .text_color(muted)
                            .child("No saved items."),
                    );
                } else {
                    list = list.children(self.later_items.iter().enumerate().map(|(ix, item)| {
                        let jump = on_jump.clone();
                        let channel = item.channel.clone();
                        let ts = item.ts.clone();
                        v_flex()
                            .id(SharedString::from(format!("popout-later-{ix}")))
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .cursor_pointer()
                            .hover(|el| el.bg(accent.opacity(0.22)))
                            .on_click(move |_, _, cx| {
                                jump(channel.clone(), Some(ts.clone()), false, cx);
                            })
                            .child(
                                h_flex()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(item.author.clone()),
                                    )
                                    .child(
                                        div().text_xs().text_color(muted).child(item.label.clone()),
                                    ),
                            )
                            .child(div().text_sm().child(item.text.clone()))
                    }));
                }
                list
            }
            PopoutKind::Canvas {
                doc,
                file_id,
                open_browser_url,
                ..
            } => {
                let open_browser = open_browser_url.clone().map(|url| {
                    Rc::new(move |_: &mut Window, cx: &mut App| cx.open_url(&url))
                        as crate::canvas::OpenBrowserFn
                });
                let on_edit = file_id.clone().map(|fid| {
                    let send = self.send.clone();
                    Rc::new(move |op, _: &mut Window, _: &mut App| {
                        (send)(Command::EditCanvas {
                            file_id: fid.clone(),
                            op,
                        });
                    }) as crate::canvas::CanvasEditFn
                });
                let key = file_id.as_deref().unwrap_or("popout-canvas");
                crate::canvas::render_popout_canvas(
                    key,
                    doc,
                    "",
                    None,
                    crate::canvas::CanvasActions {
                        open_browser,
                        on_edit,
                        on_request_access: None,
                        on_pop_out: None,
                    },
                    file_id.is_none(),
                    cx,
                )
            }
            PopoutKind::Files { .. } => {
                let mut list = v_flex()
                    .id("popout-files")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .gap_2()
                    .overflow_y_scroll();
                if self.files_items.is_empty() {
                    list = list.child(
                        div()
                            .py_6()
                            .text_sm()
                            .text_color(muted)
                            .child("No files visible."),
                    );
                } else {
                    list = list.children(self.files_items.iter().enumerate().map(
                        |(ix, (name, meta, url))| {
                            let url_opt = url.clone();
                            v_flex()
                                .id(SharedString::from(format!("popout-file-{ix}")))
                                .gap_0p5()
                                .p_3()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .when_some(url_opt, |card, link| {
                                    card.cursor_pointer()
                                        .hover(|el| el.bg(accent.opacity(0.22)))
                                        .on_click(move |_, _, cx| cx.open_url(&link))
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name.clone()),
                                )
                                .child(div().text_xs().text_color(muted).child(meta.clone()))
                        },
                    ));
                }
                list
            }
        };

        let picker_pos = self.picker.as_ref().map(|(_, pos)| *pos);

        let mut root = v_flex()
            .relative()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        if this.image_viewer.is_some() {
                            this.close_image_viewer(cx);
                        } else if this.picker.take().is_some() || this.editing.take().is_some() {
                            cx.notify();
                        }
                    }
                }),
            )
            .child(header)
            .child(body);
        if has_composer {
            root = root.child(div().flex_none().p_2().child(self.composer.clone()));
        }
        root.when_some(picker_pos, |root, pos| {
            let viewport = _window.viewport_size();
            let anchor = match (pos.y > viewport.height / 2., pos.x > viewport.width / 2.) {
                (true, true) => Anchor::BottomRight,
                (true, false) => Anchor::BottomLeft,
                (false, true) => Anchor::TopRight,
                (false, false) => Anchor::TopLeft,
            };
            root.child(
                div()
                    .id("popout-picker-backdrop")
                    .occlude()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.picker = None;
                            cx.notify();
                        }),
                    ),
            )
            .child(deferred(
                anchored()
                    .position(pos)
                    .anchor(anchor)
                    .snap_to_window_with_margin(Edges::all(px(8.)))
                    .child(
                        div()
                            .id("popout-picker-overlay")
                            .occlude()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .child(self.emoji_picker.clone()),
                    ),
            ))
        })
        .when_some(self.image_viewer.clone(), |root, viewer| {
            root.child(
                div()
                    .id("popout-image-viewer-overlay")
                    .occlude()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .absolute()
                    .top(px(40.))
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(viewer),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui_kit::gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn popout_message_actions_mutate_state_and_forward_commands(cx: &mut TestAppContext) {
        let fixtures = Arc::new(crate::fixtures::load());
        let channel = ChannelId::from("C0GENERAL");
        let sent: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sent_cb = sent.clone();
        let send: CommandForward = Arc::new(move |cmd| {
            let tag = match cmd {
                Command::AddReaction { name, .. } => format!("add:{name}"),
                Command::RemoveReaction { name, .. } => format!("rm:{name}"),
                Command::EditMessage { content, .. } => format!("edit:{}", content.text),
                Command::SaveItem { .. } => "save".to_owned(),
                Command::DeleteSavedItem { .. } => "unsave".to_owned(),
                Command::SetPinned { pinned, .. } => format!("pin:{pinned}"),
                _ => "other".to_owned(),
            };
            sent_cb.lock().unwrap().push(tag);
        });
        type JumpRecord = (ChannelId, Option<Ts>, bool);
        let jumped: Arc<Mutex<Vec<JumpRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let jumped_cb = jumped.clone();
        let on_jump: JumpHandler = Rc::new(move |ch, ts, is_thread, _| {
            jumped_cb.lock().unwrap().push((ch, ts, is_thread));
        });

        cx.update(gpui_kit::init);
        let later = cx.update(LaterView::new);
        let mut popout_slot: Option<Entity<PopoutWindow>> = None;
        let vcx = cx.add_window(|window, cx| {
            let list = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
            let win = cx.new(|cx| {
                PopoutWindow::new(
                    PopoutKind::Conversation {
                        channel: channel.clone(),
                        source: list.downgrade(),
                        on_jump,
                    },
                    "# general".to_owned(),
                    fixtures.clone(),
                    None,
                    None,
                    send,
                    Some((later.downgrade(), 0)),
                    window,
                    cx,
                )
            });
            popout_slot = Some(win.clone());
            gpui_kit::component::Root::new(win, window, cx)
        });
        let popout = popout_slot.unwrap();
        let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);

        let first = vcx.read(|cx| popout.read(cx).messages[0].clone());
        let ts = first.ts.clone();
        vcx.update(|window, cx| {
            popout.update(cx, |win, cx| {
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::React("eyes".to_owned()),
                    window,
                    cx,
                );
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::ToggleReaction("eyes".to_owned()),
                    window,
                    cx,
                );
                // Pick reaction via in-window EmojiPicker
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::PickReaction(point(px(200.), px(200.))),
                    window,
                    cx,
                );
                assert!(win.picker.is_some());
                win.pick_emoji(
                    &EmojiChoice {
                        name: "rocket".to_owned(),
                        glyph: Some("🚀".to_owned()),
                    },
                    window,
                    cx,
                );
                assert!(win.picker.is_none());
                // Inline edit inside the popped-out window
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::Edit,
                    window,
                    cx,
                );
                let input = win.editing.as_ref().unwrap().2.clone();
                input.update(cx, |s, cx| {
                    s.set_value("Updated inline".to_owned(), window, cx)
                });
                win.save_edit(cx);
                assert!(win.editing.is_none());
                assert_eq!(win.messages[0].text, "Updated inline");

                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::SaveForLater,
                    window,
                    cx,
                );
                assert!(win.saved.contains(&ts));
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::SaveForLater,
                    window,
                    cx,
                );
                assert!(!win.saved.contains(&ts));
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::TogglePin,
                    window,
                    cx,
                );
                win.handle_message_action(
                    channel.clone(),
                    first.clone(),
                    MessageAction::ReplyInThread,
                    window,
                    cx,
                );
            });
        });

        assert_eq!(
            *sent.lock().unwrap(),
            vec![
                "add:eyes",
                "rm:eyes",
                "add:rocket",
                "edit:Updated inline",
                "save",
                "unsave",
                "pin:true"
            ]
        );
        assert_eq!(*jumped.lock().unwrap(), vec![(channel, Some(ts), true)]);
    }

    #[gpui::test]
    fn open_popout_targets_the_requested_display(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let second_display = DisplayId::from(2u64);
        cx.simulate_display_added(second_display, None);

        cx.update(|cx| {
            let opts0 = popout_window_options("# general", Some(second_display), 0, cx);
            let opts1 = popout_window_options("# general", Some(second_display), 1, cx);
            assert_eq!(opts0.display_id, Some(second_display));
            assert_eq!(opts1.display_id, Some(second_display));
            let b0 = opts0.window_bounds.unwrap().get_bounds();
            let b1 = opts1.window_bounds.unwrap().get_bounds();
            assert_eq!(b1.origin.x - b0.origin.x, px(28.));
            assert_eq!(b1.origin.y - b0.origin.y, px(28.));

            let default_opts = popout_window_options("# general", None, 0, cx);
            assert_eq!(default_opts.display_id, None);
        });
    }

    #[gpui::test]
    fn popout_emoji_picker_occludes_scroll_wheel_from_messages(cx: &mut TestAppContext) {
        let fixtures = Arc::new(crate::fixtures::load());
        let channel = ChannelId::from("C0GENERAL");
        let send: CommandForward = Arc::new(|_| {});
        let on_jump: JumpHandler = Rc::new(|_, _, _, _| {});

        cx.update(gpui_kit::init);
        let mut popout_slot: Option<Entity<PopoutWindow>> = None;
        let vcx = cx.add_window(|window, cx| {
            let list = MessageList::new(fixtures.clone(), channel.clone(), window, cx);
            let win = cx.new(|cx| {
                let mut w = PopoutWindow::new(
                    PopoutKind::Conversation {
                        channel: channel.clone(),
                        source: list.downgrade(),
                        on_jump,
                    },
                    "# general".to_owned(),
                    fixtures.clone(),
                    None,
                    None,
                    send,
                    None,
                    window,
                    cx,
                );
                // Ensure the transcript is taller than the viewport so it is scrollable.
                let seed = w.messages.clone();
                for i in 0..8 {
                    for m in &seed {
                        let mut dup = m.clone();
                        dup.ts = Ts::from(format!("17600{i}0000.{}", m.ts.as_str()));
                        w.messages.push(dup);
                    }
                }
                w
            });
            popout_slot = Some(win.clone());
            gpui_kit::component::Root::new(win, window, cx)
        });
        let popout = popout_slot.unwrap();
        let mut vcx = gpui_kit::VisualTestContext::from_window(vcx.into(), cx);
        vcx.run_until_parked();

        // Open the emoji picker inside the popout window and settle layout.
        vcx.update(|window, cx| {
            popout.update(cx, |win, cx| {
                win.open_picker(
                    PopoutPickerTarget::Composer,
                    point(px(360.), px(420.)),
                    window,
                    cx,
                );
            });
        });
        vcx.run_until_parked();

        let offset_before = vcx.read(|cx| popout.read(cx).scroll.offset());
        // Scroll inside the emoji picker overlay; the underlying message list must not move.
        vcx.simulate_event(gpui::ScrollEvent {
            position: point(px(260.), px(260.)),
            delta: gpui::ScrollDelta::Pixels(point(px(0.), px(120.))),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        vcx.run_until_parked();
        let offset_after = vcx.read(|cx| popout.read(cx).scroll.offset());
        assert_eq!(
            offset_before, offset_after,
            "scrolling over the popout emoji picker must not scroll the underlying messages"
        );
    }
}
