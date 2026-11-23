//! Popped-out secondary GPUI windows for a conversation, thread, Activity feed, or Saved/Later
//! (`F-CORE-010`, `F-CORE-026`, `F-CORE-109`, `F-SHELL-149`): lightweight native window that
//! observes the main window's live entities for real-time updates and jumps back to the main
//! window when an Activity or Saved item is clicked.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
    Focusable as _, FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render, Rgba,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, anchored, canvas, deferred,
    div, hsla, point, px, rgb, rgba, size,
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

/// Callback invoked when opening a huddle's attached notes canvas from a popped-out Huddle window.
pub type CanvasOpenHandler = Rc<dyn Fn(String, &mut App)>;

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
    /// Popped-out native Huddle window (`F-COLLAB-001..059`): live participant tiles, active
    /// speaker rings, audio controls, device switcher, invite picker, thread & notes canvas links.
    Huddle {
        source: WeakEntity<crate::huddle_dock::HuddleDock>,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
    },
}

/// Callback that forwards commands (`SendMessage`, etc.) to the workspace's live `CoreBridge`.
pub type CommandForward = Arc<dyn Fn(Command) + Send + Sync>;

static CASCADE_STEP: AtomicUsize = AtomicUsize::new(0);

fn popout_window_options(
    title: &str,
    is_huddle: bool,
    display_id: Option<DisplayId>,
    cascade_slot: usize,
    cx: &App,
) -> WindowOptions {
    let dim = if is_huddle {
        size(px(900.), px(620.))
    } else {
        size(px(720.), px(640.))
    };
    let mut bounds = Bounds::centered(display_id, dim, cx);
    let offset = px((cascade_slot % 5) as f32 * 28.);
    bounds.origin.x += offset;
    bounds.origin.y += offset;
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title.to_owned())),
            traffic_light_position: Some(point(px(12.), px(14.))),
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
    let is_huddle = matches!(kind, PopoutKind::Huddle { .. });
    let options = popout_window_options(&title, is_huddle, display_id, slot, cx);
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

/// Side-by-side right-hand drawer inside the Pop-out Huddle Window (`PopoutKind::Huddle`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HuddleDrawer {
    Thread,
    Notes,
    Diagnostics,
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
    huddle_state: Option<crate::huddle_dock::HuddleViewState>,
    huddle_invite_open: bool,
    huddle_devices_open: bool,
    huddle_share_open: bool,
    huddle_reactions_open: bool,
    huddle_theme_open: bool,
    huddle_stage_expanded: bool,
    huddle_draw_mode: bool,
    huddle_draw_hue: i32,
    huddle_drawing_active: bool,
    huddle_draw_last_pt: Option<(f64, f64)>,
    huddle_stage_bounds: Arc<Mutex<Bounds<Pixels>>>,
    huddle_focused_screen: Option<usize>,
    huddle_topic_editing: Option<Entity<TextareaState>>,
    huddle_drawer: Option<HuddleDrawer>,
    huddle_thread_input: Entity<TextareaState>,
    huddle_thread_local: Vec<Message>,
    huddle_devices: slackless_core::huddle::AudioDevices,
    huddle_screens: Vec<slackless_core::huddle::ScreenSource>,
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
            | PopoutKind::Files { .. }
            | PopoutKind::Huddle { .. } => String::new(),
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
            | PopoutKind::Files { .. }
            | PopoutKind::Huddle { .. } => None,
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
        let mut huddle_state = None;
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
            PopoutKind::Huddle { source, .. } => {
                if let Some(dock) = source.upgrade() {
                    huddle_state = Some(dock.read(cx).view_state());
                    subscriptions.push(cx.observe(&dock, |this, dock, cx| {
                        this.huddle_state = Some(dock.read(cx).view_state());
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
        let huddle_thread_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Reply in huddle thread…")
                .auto_grow(1, 4)
        });
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
            huddle_state,
            huddle_invite_open: false,
            huddle_devices_open: false,
            huddle_share_open: false,
            huddle_reactions_open: false,
            huddle_theme_open: false,
            huddle_stage_expanded: false,
            huddle_draw_mode: false,
            huddle_draw_hue: 0,
            huddle_drawing_active: false,
            huddle_draw_last_pt: None,
            huddle_stage_bounds: Arc::new(Mutex::new(Bounds::default())),
            huddle_focused_screen: None,
            huddle_topic_editing: None,
            huddle_drawer: None,
            huddle_thread_input,
            huddle_thread_local: Vec::new(),
            huddle_devices: slackless_core::huddle::AudioDevices::default(),
            huddle_screens: Vec::new(),
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
            | PopoutKind::Files { on_jump, .. }
            | PopoutKind::Huddle { on_jump, .. } => on_jump.clone(),
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
            MessageAction::SendToChannel => {
                if let PopoutKind::Thread {
                    channel, thread_ts, ..
                } = &self.kind
                {
                    let content = crate::composer::content_of(&message.text, &self.fixtures);
                    (self.send)(Command::SendMessage {
                        channel: channel.clone(),
                        content,
                        thread_ts: Some(thread_ts.clone()),
                        broadcast: true,
                    });
                }
            }
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
            PopoutKind::Huddle { source, .. } => {
                if let Some(dock) = source.upgrade() {
                    self.huddle_state = Some(dock.read(cx).view_state());
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
            | PopoutKind::Files { .. }
            | PopoutKind::Huddle { .. } => return,
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

    fn huddle_dock_entity(&self) -> Option<Entity<crate::huddle_dock::HuddleDock>> {
        match &self.kind {
            PopoutKind::Huddle { source, .. } => source.upgrade(),
            _ => None,
        }
    }

    fn close_huddle_popovers(&mut self) {
        self.huddle_invite_open = false;
        self.huddle_devices_open = false;
        self.huddle_share_open = false;
        self.huddle_reactions_open = false;
        self.huddle_theme_open = false;
    }

    fn render_huddle_window(
        &self,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{HuddlePhase, VideoTileKey};

        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let emerald = rgb(0x2BAC76);
        let amber = rgb(0xECB22E);
        let crimson = rgb(0xE01E5A);

        let bg_id = self
            .huddle_state
            .as_ref()
            .and_then(|s| s.session.background_id.as_deref());
        let (stage_bg, tile_bg) = huddle_theme_surface(bg_id);

        let Some(state) = self.huddle_state.clone() else {
            return v_flex()
                .id("popout-huddle-ended")
                .size_full()
                .bg(stage_bg)
                .text_color(white)
                .child(
                    TitleBar::new()
                        .h(px(44.))
                        .bg(stage_bg)
                        .border_b_1()
                        .border_color(rgba(0xFFFFFF12))
                        .child(
                            h_flex()
                                .flex_1()
                                .h_full()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::Headphones)
                                        .size(px(14.))
                                        .text_color(muted),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(white)
                                        .child(self.title.clone()),
                                ),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Headphones)
                                .size(px(32.))
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("This huddle has ended."),
                        ),
                );
        };

        let snap = state.session.clone();
        let channel = snap.channel.clone();
        let mut people = state.participants.clone();
        if !people.contains(&self.fixtures.self_user) {
            people.push(self.fixtures.self_user.clone());
        }
        let (status_color, status_text) = match snap.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => (amber, "Connecting…".to_owned()),
            HuddlePhase::Connected => (
                emerald,
                crate::huddle_dock::format_elapsed(state.elapsed_secs),
            ),
            HuddlePhase::Reconnecting => (amber, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (muted, "Leaving…".to_owned()),
            HuddlePhase::Failed => (crimson, "Failed".to_owned()),
            HuddlePhase::Idle => (muted, "Ended".to_owned()),
        };

        let header = self.render_huddle_titlebar(&snap, status_color, status_text, stage_bg, cx);

        let has_screen_share = state
            .video_frames
            .keys()
            .any(|k| matches!(k, VideoTileKey::RemoteScreen(_) | VideoTileKey::LocalScreen));

        let mut stage = v_flex()
            .id("popout-huddle-stage-container")
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_3();

        if snap.audio_only {
            let send_resume = self.send.clone();
            stage = stage.child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .rounded_xl()
                    .bg(amber.opacity(0.16))
                    .border_1()
                    .border_color(amber.opacity(0.45))
                    .text_xs()
                    .text_color(white)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(amber),
                            )
                            .child("Audio-only mode — video paused to save bandwidth"),
                    )
                    .child(
                        div()
                            .id("popout-huddle-resume-video")
                            .cursor_pointer()
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(rgba(0xFFFFFF18))
                            .hover(|el| el.bg(rgba(0xFFFFFF2A)))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Resume video")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_resume)(Command::SetHuddleAudioOnly(false));
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        dock.update(cx, |d, cx| d.set_audio_only_local(false, cx));
                                    }
                                }
                            })),
                    ),
            );
        }

        if let Some(err) = snap.error.clone() {
            stage = stage.child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .rounded_xl()
                    .bg(crimson.opacity(0.18))
                    .border_1()
                    .border_color(crimson.opacity(0.5))
                    .text_xs()
                    .text_color(white)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(14.))
                            .text_color(crimson),
                    )
                    .child(err),
            );
        }

        if has_screen_share {
            if let Some(screen_stage) = self.render_huddle_stage(&state, cx) {
                stage = stage.child(screen_stage);
            }
            stage = stage.child(self.render_huddle_filmstrip(&state, &people, tile_bg, cx));
        } else {
            let has_bottom_overlay = state.session.captions_enabled
                || !state.captions.is_empty()
                || !state.reactions.is_empty();
            stage = stage.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(has_bottom_overlay, |el| el.pb(px(48.)))
                    .items_center()
                    .justify_center()
                    .child(self.render_huddle_tiles(&state, &people, tile_bg, cx)),
            );
        }

        // Floating effect burst banner (top-center, never shifts stage layout)
        if let Some((uid, effect_name, _)) = &state.active_effect {
            let who = self.fixtures.user_name(uid).to_owned();
            let (glyph, label) = effect_label(effect_name);
            stage = stage.child(
                h_flex()
                    .absolute()
                    .top(px(12.))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(
                        h_flex()
                            .px_4()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .rounded_full()
                            .bg(rgba(0x14171CE6))
                            .border_1()
                            .border_color(emerald.opacity(0.65))
                            .shadow_xl()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(format!(
                                "{glyph} {glyph} {who} triggered {label}! {glyph} {glyph}"
                            )),
                    ),
            );
        }

        // Floating stacking reactions (bottom-left above dock)
        if let Some(reactions_el) = self.render_huddle_floating_reactions(&state) {
            stage = stage.child(reactions_el);
        }

        // Floating live captions subtitle overlay (bottom-center above dock)
        if let Some(captions_el) = self.render_huddle_floating_captions(&state) {
            stage = stage.child(captions_el);
        }

        // Floating popover panel anchored right above the bottom dock
        if let Some(popover) = self.render_huddle_active_popover(&snap, &channel, &people, cx) {
            stage = stage.child(
                h_flex()
                    .absolute()
                    .bottom(px(10.))
                    .left_0()
                    .right_0()
                    .justify_center()
                    .px_4()
                    .child(popover),
            );
        }

        let body = h_flex().flex_1().min_h_0().w_full().child(stage).when_some(
            self.huddle_drawer,
            |row, drawer| {
                row.child(self.render_huddle_drawer(
                    drawer,
                    &state,
                    on_jump.clone(),
                    on_open_canvas.clone(),
                    cx,
                ))
            },
        );

        v_flex()
            .id("popout-huddle-root")
            .relative()
            .size_full()
            .bg(stage_bg)
            .text_color(white)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && (this.huddle_topic_editing.take().is_some()
                            || this.huddle_share_open
                            || this.huddle_reactions_open
                            || this.huddle_theme_open
                            || this.huddle_invite_open
                            || this.huddle_devices_open
                            || this.huddle_drawer.take().is_some())
                    {
                        this.close_huddle_popovers();
                        cx.notify();
                    }
                }),
            )
            .child(header)
            .child(body)
            .child(self.render_huddle_controls(&snap, stage_bg, cx))
    }

    fn render_huddle_drawer(
        &self,
        drawer: HuddleDrawer,
        state: &crate::huddle_dock::HuddleViewState,
        on_jump: JumpHandler,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let content: gpui_kit::gpui::AnyElement = match drawer {
            HuddleDrawer::Thread => self
                .render_huddle_thread_drawer(&state.session, on_jump, cx)
                .into_any_element(),
            HuddleDrawer::Notes => self
                .render_huddle_notes_drawer(&state.session, on_open_canvas, cx)
                .into_any_element(),
            HuddleDrawer::Diagnostics => self
                .render_huddle_diagnostics_drawer(state, cx)
                .into_any_element(),
        };
        v_flex()
            .flex_none()
            .w(px(340.))
            .h_full()
            .bg(rgb(0x1B1D21))
            .border_l_1()
            .border_color(rgba(0xFFFFFF16))
            .text_color(rgb(0xFFFFFF))
            .child(content)
    }

    fn huddle_thread_messages(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
    ) -> Vec<Message> {
        let mut out = if let (Some(store), Some(team)) = (&self.store, &self.team) {
            store
                .lock()
                .ok()
                .and_then(|guard| {
                    let rows = if let Some(root_ts) = &snap.thread_root_ts {
                        guard.thread_replies(team, &snap.channel, root_ts)
                    } else {
                        guard.channel_history(team, &snap.channel, None, 20)
                    };
                    rows.ok().map(|list| {
                        list.into_iter()
                            .filter(|r| !r.deleted)
                            .map(|r| r.message)
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default()
        } else if let Some(root_ts) = &snap.thread_root_ts {
            self.fixtures
                .threads
                .get(&(snap.channel.clone(), root_ts.clone()))
                .cloned()
                .unwrap_or_default()
        } else {
            self.fixtures
                .messages
                .get(&snap.channel)
                .map(|list| list.iter().rev().take(12).rev().cloned().collect())
                .unwrap_or_default()
        };
        for local in &self.huddle_thread_local {
            if !out.iter().any(|m| m.ts == local.ts) {
                out.push(local.clone());
            }
        }
        out
    }

    fn render_huddle_thread_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        on_jump: JumpHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let (white, muted, border, emerald) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x007A5A),
        );
        let channel = snap.channel.clone();
        let thread_ts = snap.thread_root_ts.clone();
        let jump_ch = channel.clone();
        let jump_ts = thread_ts.clone();
        let messages = self.huddle_thread_messages(snap);
        let send = self.send.clone();
        let reply_ch = channel.clone();
        let reply_ts = thread_ts.clone();

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::MessageSquare)
                                    .size(px(14.))
                                    .text_color(muted),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Huddle thread"),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .id("huddle-drawer-thread-pop")
                                    .size(px(24.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .size(px(13.))
                                            .text_color(muted),
                                    )
                                    .on_click(move |_, _, cx| {
                                        on_jump(
                                            jump_ch.clone(),
                                            jump_ts.clone(),
                                            jump_ts.is_some(),
                                            cx,
                                        );
                                    }),
                            )
                            .child(
                                div()
                                    .id("huddle-drawer-close")
                                    .size(px(24.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                                    .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer = None;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .id("huddle-drawer-thread-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .py_2p5()
                    .gap_2p5()
                    .when(messages.is_empty(), |col| {
                        col.child(div().py_6().text_xs().text_color(muted).child(
                            "No messages in this huddle thread yet. Drop a link or note below.",
                        ))
                    })
                    .children(messages.into_iter().map(|msg| {
                        let author = msg
                            .user
                            .as_ref()
                            .map(|u| self.fixtures.user_name(u).to_owned())
                            .or_else(|| msg.username.clone())
                            .unwrap_or_else(|| "Slack".to_owned());
                        v_flex()
                            .p_2()
                            .gap_1()
                            .rounded_lg()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(rgba(0xFFFFFF0F))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(white)
                                    .child(author),
                            )
                            .child(div().text_xs().text_color(rgb(0xD1D2D3)).child(msg.text))
                    })),
            )
            .child(
                v_flex()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgba(0xFFFFFF22))
                            .bg(rgb(0x222529))
                            .px_2()
                            .py_1()
                            .child(Textarea::new(&self.huddle_thread_input).appearance(false)),
                    )
                    .child(
                        h_flex().justify_end().child(
                            div()
                                .id("huddle-drawer-thread-send")
                                .cursor_pointer()
                                .px_3()
                                .py_1()
                                .rounded_md()
                                .bg(emerald)
                                .hover(|el| el.bg(rgb(0x008E69)))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
                                .child("Send")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let raw = this.huddle_thread_input.read(cx).value().to_string();
                                    let trimmed = raw.trim();
                                    if trimmed.is_empty() {
                                        return;
                                    }
                                    let content =
                                        crate::composer::content_of(trimmed, &this.fixtures);
                                    (send)(Command::SendMessage {
                                        channel: reply_ch.clone(),
                                        content,
                                        thread_ts: reply_ts.clone(),
                                        broadcast: false,
                                    });
                                    this.huddle_thread_local.push(Message {
                                        ts: Ts::from(format!(
                                            "{}.000001",
                                            crate::drafts::now_secs()
                                        )),
                                        user: Some(this.fixtures.self_user.clone()),
                                        text: trimmed.to_owned(),
                                        ..Default::default()
                                    });
                                    this.huddle_thread_input.update(cx, |s, cx| {
                                        s.set_value("", window, cx);
                                    });
                                    cx.notify();
                                })),
                        ),
                    ),
            )
    }

    fn render_huddle_notes_drawer(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        on_open_canvas: CanvasOpenHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;

        let (white, muted, border, emerald) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x007A5A),
        );
        let send_notes = self.send.clone();
        let notes_ch = snap.channel.clone();
        let canvas_id = snap.canvas_file_id.clone();
        let topic_title = snap
            .topic
            .clone()
            .map(|t| format!("Huddle notes: {t}"))
            .unwrap_or_else(|| "Huddle notes".to_owned());

        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::FileText).size(px(14.)).text_color(muted))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Huddle notes"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-notes-close")
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                            .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.huddle_drawer = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_4()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(topic_title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(if canvas_id.is_some() {
                                "A collaborative Slack canvas is attached to this huddle. Open it in the main window to edit sections and action items live."
                            } else {
                                "Start a shared Slack canvas for this huddle to capture notes, links, and follow-up tasks."
                            }),
                    )
                    .child(if let Some(fid) = canvas_id {
                        h_flex()
                            .id("huddle-drawer-open-canvas")
                            .cursor_pointer()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(emerald)
                            .hover(|el| el.bg(rgb(0x008E69)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(Icon::new(IconName::FileText).size(px(14.)).text_color(white))
                            .child("Open huddle canvas")
                            .on_click(move |_, _, cx| on_open_canvas(fid.clone(), cx))
                    } else {
                        h_flex()
                            .id("huddle-drawer-create-canvas")
                            .cursor_pointer()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .items_center()
                            .justify_center()
                            .rounded_lg()
                            .bg(emerald)
                            .hover(|el| el.bg(rgb(0x008E69)))
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(Icon::new(IconName::Plus).size(px(14.)).text_color(white))
                            .child("Create huddle notes canvas")
                            .on_click(move |_, window, cx| {
                                (send_notes)(Command::CreateCanvas {
                                    title: topic_title.clone(),
                                    markdown: "# Huddle notes\n\n- ".to_owned(),
                                    channel: Some(notes_ch.clone()),
                                });
                                actions::notify(
                                    "Creating huddle notes",
                                    "Opening a shared canvas for this huddle.",
                                    window,
                                    cx,
                                );
                            })
                    }),
            )
    }

    /// Troubleshooting, Network Stats HUD & Self-Test drawer (`F-COLLAB-074`, `075`, `076`).
    fn render_huddle_diagnostics_drawer(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::{
            HuddleChime, HuddlePhase, play_huddle_chime, play_speaker_test_tone,
        };

        let (white, muted, border, emerald, amber, crimson) = (
            rgb(0xFFFFFF),
            rgb(0xABABAD),
            rgba(0xFFFFFF16),
            rgb(0x2BAC76),
            rgb(0xECB22E),
            rgb(0xE01E5A),
        );
        let snap = &state.session;
        let (phase_color, phase_label) = match snap.phase {
            HuddlePhase::Connected => (emerald, "Connected (UDP TURN Relay)"),
            HuddlePhase::Joining | HuddlePhase::Connecting => (amber, "Connecting…"),
            HuddlePhase::Reconnecting => (amber, "Reconnecting…"),
            HuddlePhase::Failed if snap.udp_blocked => (crimson, "UDP Blocked — Check Firewall"),
            HuddlePhase::Failed => (crimson, "Connection Failed"),
            HuddlePhase::Leaving => (muted, "Leaving…"),
            HuddlePhase::Idle => (muted, "Idle"),
        };
        let video_codec = snap
            .active_video_codec
            .as_deref()
            .unwrap_or(if snap.video || snap.sharing {
                "video/H264 (OpenH264)"
            } else {
                "Idle (AV1 / H.264 ready)"
            })
            .to_owned();
        let tx_kb = snap.bytes_sent as f64 / 1024.0;
        let rx_kb = snap.bytes_received as f64 / 1024.0;
        let rms_pct = (state.levels.local_rms.clamp(0.0, 1.0) * 100.0) as u32;
        let audio_only = snap.audio_only;
        let send_ao = self.send.clone();
        let test_out = snap.output_device.clone();
        let chime_out = snap.output_device.clone();

        let stat_row = |label: &'static str, value: String| {
            h_flex()
                .justify_between()
                .items_center()
                .text_xs()
                .child(div().text_color(muted).child(label))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(white)
                        .child(value),
                )
        };
        let dsp_pill = |label: &'static str, on: bool| {
            div()
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(if on {
                    emerald.opacity(0.2)
                } else {
                    rgba(0xFFFFFF12)
                })
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if on { emerald } else { muted })
                .child(label)
        };

        v_flex()
            .id("huddle-drawer-diag-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                h_flex()
                    .px_3p5()
                    .py_2p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Activity)
                                    .size(px(14.))
                                    .text_color(phase_color),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Connection & diagnostics"),
                            ),
                    )
                    .child(
                        div()
                            .id("huddle-drawer-diag-close")
                            .size(px(24.))
                            .rounded_md()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|el| el.bg(rgba(0xFFFFFF1A)))
                            .child(Icon::new(IconName::X).size(px(13.)).text_color(muted))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.huddle_drawer = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .p_3p5()
                    .gap_3()
                    .child(
                        v_flex()
                            .p_3()
                            .gap_1p5()
                            .rounded_xl()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .pb_1()
                                    .child(div().size(px(8.)).rounded_full().bg(phase_color))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(white)
                                            .child(phase_label),
                                    ),
                            )
                            .child(stat_row("Audio codec", "Opus 48 kHz Stereo".to_owned()))
                            .child(stat_row("Video codec", video_codec))
                            .child(stat_row(
                                "Outbound (TX)",
                                format!("{} pkts ({tx_kb:.1} KB)", snap.packets_sent),
                            ))
                            .child(stat_row(
                                "Inbound (RX)",
                                format!("{} pkts ({rx_kb:.1} KB)", snap.packets_received),
                            ))
                            .child(stat_row(
                                "Signaling keepalive",
                                format!("{} pongs", snap.pongs_sent),
                            ))
                            .child(stat_row(
                                "Screen audio",
                                snap.screen_audio_device
                                    .clone()
                                    .unwrap_or_else(|| "None (install BlackHole 2ch)".to_owned()),
                            )),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Sonora Audio Processing"),
                            )
                            .child(
                                h_flex()
                                    .gap_1p5()
                                    .child(dsp_pill("AEC3", snap.aec))
                                    .child(dsp_pill("Noise Supp", snap.ns))
                                    .child(dsp_pill("Auto Gain", snap.agc)),
                            ),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        v_flex()
                                            .gap_0p5()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(white)
                                                    .child("Audio-only mode"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child("Pause video to save bandwidth"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("huddle-diag-audio-only")
                                            .cursor_pointer()
                                            .px_2p5()
                                            .py_1()
                                            .rounded_full()
                                            .bg(if audio_only { amber } else { rgba(0xFFFFFF18) })
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if audio_only {
                                                rgb(0x14171C)
                                            } else {
                                                white
                                            })
                                            .child(if audio_only { "On" } else { "Off" })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                let next = !audio_only;
                                                (send_ao)(Command::SetHuddleAudioOnly(next));
                                                if this.fixtures.sample {
                                                    if let Some(dock) = this.huddle_dock_entity() {
                                                        dock.update(cx, |d, cx| {
                                                            d.set_audio_only_local(next, cx)
                                                        });
                                                    }
                                                }
                                            })),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .p_3()
                            .gap_2()
                            .rounded_xl()
                            .bg(rgba(0xFFFFFF0A))
                            .border_1()
                            .border_color(border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child("Audio self-test"),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(stat_row("Microphone level", format!("{rms_pct}%")))
                                    .child(
                                        div()
                                            .w_full()
                                            .h(px(6.))
                                            .rounded_full()
                                            .bg(rgba(0xFFFFFF18))
                                            .overflow_hidden()
                                            .child(
                                                div()
                                                    .h_full()
                                                    .w(px((2.8 * rms_pct as f32).clamp(4., 280.)))
                                                    .rounded_full()
                                                    .bg(emerald),
                                            ),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .pt_1()
                                    .child(
                                        div()
                                            .id("huddle-diag-test-tone")
                                            .flex_1()
                                            .cursor_pointer()
                                            .py_1p5()
                                            .rounded_lg()
                                            .bg(rgba(0xFFFFFF14))
                                            .hover(|el| el.bg(rgba(0xFFFFFF24)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(white)
                                            .child("Test speaker")
                                            .on_click(move |_, _, _| {
                                                let _ = play_speaker_test_tone(test_out.as_deref());
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id("huddle-diag-test-chime")
                                            .flex_1()
                                            .cursor_pointer()
                                            .py_1p5()
                                            .rounded_lg()
                                            .bg(rgba(0xFFFFFF14))
                                            .hover(|el| el.bg(rgba(0xFFFFFF24)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_xs()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(white)
                                            .child("Test chime")
                                            .on_click(move |_, _, _| {
                                                let _ = play_huddle_chime(
                                                    HuddleChime::Join,
                                                    chime_out.as_deref(),
                                                );
                                            }),
                                    ),
                            ),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_huddle_titlebar(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        status_color: Rgba,
        status_text: String,
        bar_bg: Rgba,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::Icon;

        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let channel = snap.channel.clone();
        let browser_url = snap.huddle_link.clone().unwrap_or_else(|| {
            let team = self
                .fixtures
                .workspaces
                .first()
                .map(|t| t.id.to_string())
                .unwrap_or_default();
            format!("https://app.slack.com/huddle/{team}/{channel}")
        });
        let send_copy = self.send.clone();
        let copy_ch = channel.clone();
        let notes_open = self.huddle_drawer == Some(HuddleDrawer::Notes);
        let thread_open = self.huddle_drawer == Some(HuddleDrawer::Thread);
        let diag_open = self.huddle_drawer == Some(HuddleDrawer::Diagnostics);

        let notes_pill = h_flex()
            .id("popout-huddle-canvas")
            .h(px(28.))
            .px_2p5()
            .gap_1p5()
            .items_center()
            .rounded_lg()
            .bg(if notes_open {
                rgba(0xFFFFFF2A)
            } else {
                rgba(0xFFFFFF12)
            })
            .hover(|el| el.bg(rgba(0xFFFFFF22)))
            .cursor_pointer()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(white)
            .child(
                Icon::new(IconName::FileText)
                    .size(px(13.))
                    .text_color(white),
            )
            .child("Notes")
            .on_click(cx.listener(|this, _, _, cx| {
                this.huddle_drawer = if this.huddle_drawer == Some(HuddleDrawer::Notes) {
                    None
                } else {
                    Some(HuddleDrawer::Notes)
                };
                cx.notify();
            }));

        TitleBar::new()
            .h(px(44.))
            .bg(bar_bg)
            .border_b_1()
            .border_color(rgba(0xFFFFFF10))
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_3()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(14.))
                                    .text_color(status_color),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white)
                                    .child(self.title.clone()),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(status_color.opacity(0.18))
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(status_color)
                                    .child(status_text),
                            )
                            .child(self.render_huddle_topic_bar(snap.topic.as_deref(), cx)),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .child(
                                h_flex()
                                    .id("popout-huddle-header-invite")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1p5()
                                    .items_center()
                                    .rounded_lg()
                                    .bg(if self.huddle_invite_open {
                                        rgba(0xFFFFFF28)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(white)
                                    .child(
                                        Icon::new(IconName::UserPlus)
                                            .size(px(13.))
                                            .text_color(white),
                                    )
                                    .child("Invite")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let next = !this.huddle_invite_open;
                                        this.close_huddle_popovers();
                                        this.huddle_invite_open = next;
                                        cx.notify();
                                    })),
                            )
                            .child(notes_pill)
                            .child(
                                h_flex()
                                    .id("popout-huddle-thread")
                                    .h(px(28.))
                                    .px_2p5()
                                    .gap_1p5()
                                    .items_center()
                                    .rounded_lg()
                                    .bg(if thread_open {
                                        rgba(0xFFFFFF2A)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(white)
                                    .child(
                                        Icon::new(IconName::MessageSquare)
                                            .size(px(13.))
                                            .text_color(white),
                                    )
                                    .child("Thread")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer =
                                            if this.huddle_drawer == Some(HuddleDrawer::Thread) {
                                                None
                                            } else {
                                                Some(HuddleDrawer::Thread)
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-diag")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(if diag_open {
                                        rgba(0xFFFFFF2A)
                                    } else {
                                        rgba(0xFFFFFF12)
                                    })
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Activity)
                                            .size(px(13.))
                                            .text_color(status_color),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.huddle_drawer = if this.huddle_drawer
                                            == Some(HuddleDrawer::Diagnostics)
                                        {
                                            None
                                        } else {
                                            Some(HuddleDrawer::Diagnostics)
                                        };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-copy-link")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::Link).size(px(13.)).text_color(muted),
                                    )
                                    .on_click(move |_, _, _| {
                                        (send_copy)(Command::CopyHuddleLink {
                                            channel: copy_ch.clone(),
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-browser")
                                    .size(px(28.))
                                    .rounded_lg()
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::ExternalLink)
                                            .size(px(13.))
                                            .text_color(muted),
                                    )
                                    .on_click(move |_, _, cx| cx.open_url(&browser_url)),
                            ),
                    ),
            )
    }

    fn render_huddle_topic_bar(
        &self,
        topic: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        if let Some(input) = &self.huddle_topic_editing {
            return h_flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_0p5()
                .rounded_lg()
                .border_1()
                .border_color(rgba(0xFFFFFF28))
                .bg(rgb(0x2B2E33))
                .child(
                    div()
                        .w(px(180.))
                        .text_xs()
                        .text_color(white)
                        .child(Textarea::new(input).appearance(false).bordered(false)),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-save")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(rgb(0x007A5A))
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(white)
                        .child("Save")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(inp) = this.huddle_topic_editing.take() {
                                let text = inp.read(cx).value().trim().to_owned();
                                (this.send)(Command::SetHuddleTopic(text.clone()));
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        let me = this.fixtures.self_user.clone();
                                        dock.update(cx, |d, cx| {
                                            d.apply_data_event(
                                                &slackless_core::huddle::HuddleDataEvent::Topic {
                                                    user: me,
                                                    topic: text,
                                                },
                                                cx,
                                            );
                                        });
                                    }
                                }
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-cancel")
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .hover(|el| el.bg(rgba(0xFFFFFF16)))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(muted)
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.huddle_topic_editing = None;
                            cx.notify();
                        })),
                );
        }
        let initial = topic.unwrap_or_default().to_owned();
        h_flex().min_w_0().items_center().child(
            h_flex()
                .id("popout-huddle-topic-edit")
                .px_2()
                .py_0p5()
                .gap_1()
                .items_center()
                .rounded_md()
                .hover(|el| el.bg(rgba(0xFFFFFF12)))
                .cursor_pointer()
                .text_xs()
                .text_color(if topic.is_some() { white } else { muted })
                .child(
                    div().max_w(px(220.)).truncate().child(
                        topic
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| "Add a topic".to_owned()),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let init = initial.clone();
                    let input = cx.new(|cx| {
                        let mut st = TextareaState::new(window, cx)
                            .placeholder("What are you discussing?")
                            .auto_grow(1, 1);
                        st.set_value(init, window, cx);
                        st
                    });
                    input.read(cx).focus_handle(cx).focus(window, cx);
                    this.huddle_topic_editing = Some(input);
                    cx.notify();
                })),
        )
    }

    fn render_huddle_stage(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        use slackless_core::huddle::{HuddleDrawMessage, VideoTileKey};

        let mut screens: Vec<(String, String, Arc<gpui_kit::gpui::RenderImage>)> = Vec::new();
        for (k, v) in &state.video_frames {
            if let VideoTileKey::RemoteScreen(uid) = k {
                screens.push((
                    uid.to_string(),
                    format!("{} is sharing their screen", self.fixtures.user_name(uid)),
                    v.clone(),
                ));
            }
        }
        screens.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(v) = state.video_frames.get(&VideoTileKey::LocalScreen) {
            let local_item = if !state.session.attendee_id.is_empty() {
                state.session.attendee_id.clone()
            } else {
                self.fixtures.self_user.to_string()
            };
            screens.push((
                local_item,
                "You are sharing your screen".to_owned(),
                v.clone(),
            ));
        }
        if screens.is_empty() {
            return None;
        }

        let white = rgb(0xFFFFFF);
        let emerald = rgb(0x2BAC76);
        let focused = self.huddle_focused_screen.filter(|&i| i < screens.len());
        let primary_item_id = screens
            .get(focused.unwrap_or(0))
            .map(|s| s.0.clone())
            .unwrap_or_default();
        let header_title = if let Some(ix) = focused {
            screens[ix].1.clone()
        } else if screens.len() > 1 {
            format!("{} screens shared", screens.len())
        } else {
            screens[0].1.clone()
        };

        const HUES: [(i32, &str); 6] = [
            (0, "Red"),
            (35, "Orange"),
            (55, "Yellow"),
            (135, "Green"),
            (210, "Blue"),
            (285, "Purple"),
        ];

        let draw_btn = {
            let item = primary_item_id.clone();
            let hue = self.huddle_draw_hue;
            h_flex()
                .id("popout-huddle-stage-draw")
                .h(px(26.))
                .px_2p5()
                .gap_1()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .bg(if self.huddle_draw_mode {
                    rgb(0x007A5A)
                } else {
                    rgba(0xFFFFFF16)
                })
                .hover(|el| {
                    el.bg(if self.huddle_draw_mode {
                        emerald
                    } else {
                        rgba(0xFFFFFF28)
                    })
                })
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(white)
                .child(
                    gpui_kit::component::Icon::new(IconName::Pencil)
                        .size(px(12.))
                        .text_color(white),
                )
                .child(if self.huddle_draw_mode {
                    "Drawing"
                } else {
                    "Draw"
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.huddle_draw_mode = !this.huddle_draw_mode;
                    this.huddle_drawing_active = false;
                    this.huddle_draw_last_pt = None;
                    if this.huddle_draw_mode {
                        let msg = HuddleDrawMessage::ChangeHue {
                            item_id: item.clone(),
                            hue,
                        };
                        (this.send)(Command::SendHuddleDraw(msg.clone()));
                        this.apply_local_draw(msg, cx);
                    }
                    cx.notify();
                }))
        };

        let swatches = self.huddle_draw_mode.then(|| {
            h_flex()
                .items_center()
                .gap_1()
                .px_1()
                .children(HUES.iter().enumerate().map(|(ix, &(hue, _label))| {
                    let selected = self.huddle_draw_hue == hue;
                    let item = primary_item_id.clone();
                    div()
                        .id(("popout-draw-hue", ix))
                        .size(px(15.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(hsla(hue as f32 / 360.0, 0.88, 0.56, 1.0))
                        .border_2()
                        .border_color(if selected { white } else { rgba(0xFFFFFF30) })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_draw_hue = hue;
                            let msg = HuddleDrawMessage::ChangeHue {
                                item_id: item.clone(),
                                hue,
                            };
                            (this.send)(Command::SendHuddleDraw(msg.clone()));
                            this.apply_local_draw(msg, cx);
                            cx.notify();
                        }))
                }))
        });

        let screen_tabs = (screens.len() > 1).then(|| {
            h_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .id("popout-huddle-split-all")
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if focused.is_none() {
                            rgba(0xFFFFFF2A)
                        } else {
                            rgba(0xFFFFFF10)
                        })
                        .text_xs()
                        .text_color(white)
                        .child("Split")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.huddle_focused_screen = None;
                            cx.notify();
                        })),
                )
                .children(screens.iter().enumerate().map(|(ix, (_, lbl, _))| {
                    let is_f = focused == Some(ix);
                    let short = lbl
                        .split(" is ")
                        .next()
                        .unwrap_or("Screen")
                        .replace("You are sharing your screen", "You");
                    div()
                        .id(("popout-huddle-screen-tab", ix))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .cursor_pointer()
                        .bg(if is_f {
                            rgba(0xFFFFFF2A)
                        } else {
                            rgba(0xFFFFFF10)
                        })
                        .text_xs()
                        .text_color(white)
                        .child(short)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_focused_screen = Some(ix);
                            cx.notify();
                        }))
                }))
        });

        let visible_screens: Vec<(String, String, Arc<gpui_kit::gpui::RenderImage>)> =
            if let Some(ix) = focused {
                vec![screens[ix].clone()]
            } else {
                screens
            };

        Some(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .rounded_2xl()
                .overflow_hidden()
                .border_1()
                .border_color(rgba(0xFFFFFF18))
                .bg(rgb(0x0B0D10))
                .child(h_flex().size_full().gap_1p5().children(
                    visible_screens.into_iter().enumerate().map(
                        |(ix, (item_id, _lbl, img_frame))| {
                            self.render_huddle_screen_pane(ix, item_id, img_frame, state, cx)
                        },
                    ),
                ))
                .child(
                    h_flex()
                        .absolute()
                        .left(px(12.))
                        .top(px(12.))
                        .px_2p5()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .rounded_full()
                        .bg(rgba(0x0E1014CC))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1A))
                        .child(
                            gpui_kit::component::Icon::new(IconName::ScreenShare)
                                .size(px(12.))
                                .text_color(emerald),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
                                .child(header_title),
                        )
                        .children(screen_tabs),
                )
                .child(
                    h_flex()
                        .absolute()
                        .right(px(12.))
                        .top(px(12.))
                        .p_1()
                        .gap_1()
                        .items_center()
                        .rounded_full()
                        .bg(rgba(0x0E1014CC))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1A))
                        .children(swatches)
                        .child(draw_btn)
                        .child(
                            div()
                                .id("popout-huddle-stage-expand")
                                .h(px(26.))
                                .px_2()
                                .rounded_full()
                                .cursor_pointer()
                                .bg(rgba(0xFFFFFF12))
                                .hover(|el| el.bg(rgba(0xFFFFFF24)))
                                .flex()
                                .items_center()
                                .text_xs()
                                .text_color(white)
                                .child(if self.huddle_stage_expanded {
                                    "Compact"
                                } else {
                                    "Expand"
                                })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.huddle_stage_expanded = !this.huddle_stage_expanded;
                                    cx.notify();
                                })),
                        ),
                ),
        )
    }

    fn apply_local_draw(
        &self,
        message: slackless_core::huddle::HuddleDrawMessage,
        cx: &mut Context<Self>,
    ) {
        if let Some(dock) = self.huddle_dock_entity() {
            let me = self.fixtures.self_user.clone();
            dock.update(cx, |d, cx| {
                d.apply_data_event(
                    &slackless_core::huddle::HuddleDataEvent::Draw {
                        user: me,
                        message,
                        at_ms: 0,
                    },
                    cx,
                );
            });
        }
    }

    fn render_huddle_screen_pane(
        &self,
        pane_ix: usize,
        item_id: String,
        img_frame: Arc<gpui_kit::gpui::RenderImage>,
        state: &crate::huddle_dock::HuddleViewState,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};
        use slackless_core::huddle::HuddleDrawMessage;

        let filter_id = item_id.clone();
        let strokes: Vec<_> = state
            .draw_strokes
            .iter()
            .filter(|s| s.item_id.is_empty() || s.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let cursors: Vec<_> = state
            .draw_cursors
            .values()
            .filter(|c| c.item_id.is_empty() || c.item_id == filter_id || pane_ix == 0)
            .cloned()
            .collect();
        let bounds_cell = Arc::clone(&self.huddle_stage_bounds);
        let draw_overlay = canvas(
            move |bounds, _, _| {
                if pane_ix == 0 {
                    if let Ok(mut guard) = bounds_cell.lock() {
                        *guard = bounds;
                    }
                }
            },
            move |bounds, _, window, _| {
                let left: f32 = bounds.origin.x.into();
                let top: f32 = bounds.origin.y.into();
                let width: f32 = bounds.size.width.into();
                let height: f32 = bounds.size.height.into();
                if width <= 1.0 || height <= 1.0 {
                    return;
                }
                let to_pt = |[nx, ny]: [f64; 2]| {
                    point(px(left + nx as f32 * width), px(top + ny as f32 * height))
                };
                for stroke in &strokes {
                    if stroke.segments.is_empty() {
                        continue;
                    }
                    let color = hsla(
                        (stroke.hue.rem_euclid(360)) as f32 / 360.0,
                        0.88,
                        0.56,
                        0.92,
                    );
                    let mut path = PathBuilder::stroke(px(3.5));
                    for (ix, seg) in stroke.segments.iter().enumerate() {
                        if ix == 0 {
                            path.move_to(to_pt(seg.a));
                        }
                        path.cubic_bezier_to(to_pt(seg.d), to_pt(seg.b), to_pt(seg.c));
                    }
                    if let Ok(built) = path.build() {
                        window.paint_path(built, color);
                    }
                }
                for cur in &cursors {
                    let color = hsla((cur.hue.rem_euclid(360)) as f32 / 360.0, 0.9, 0.58, 0.95);
                    let center = to_pt([cur.x, cur.y]);
                    let r = px(6.);
                    let mut dot = PathBuilder::fill();
                    dot.move_to(point(center.x - r, center.y));
                    dot.line_to(point(center.x, center.y - r));
                    dot.line_to(point(center.x + r, center.y));
                    dot.line_to(point(center.x, center.y + r));
                    dot.close();
                    if let Ok(built) = dot.build() {
                        window.paint_path(built, color);
                    }
                }
            },
        )
        .size_full();

        let item_down = item_id.clone();
        let item_move = item_id.clone();
        let item_up = item_id;

        let mut pane = div()
            .id(("popout-huddle-stage-pane", pane_ix))
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .bg(rgb(0x0B0D10))
            .child(
                img(img_frame)
                    .w_full()
                    .h_full()
                    .object_fit(ObjectFit::Contain),
            )
            .child(div().absolute().inset_0().child(draw_overlay));

        if self.huddle_draw_mode && pane_ix == 0 {
            pane = pane
                .cursor_crosshair()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                        let Some(pt) = this.normalize_stage_point(ev.position) else {
                            return;
                        };
                        this.huddle_drawing_active = true;
                        this.huddle_draw_last_pt = Some(pt);
                        let hue_msg = HuddleDrawMessage::ChangeHue {
                            item_id: item_down.clone(),
                            hue: this.huddle_draw_hue,
                        };
                        (this.send)(Command::SendHuddleDraw(hue_msg.clone()));
                        this.apply_local_draw(hue_msg, cx);
                    }),
                )
                .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                    let Some(pt) = this.normalize_stage_point(ev.position) else {
                        return;
                    };
                    if this.huddle_drawing_active {
                        if let Some(prev) = this.huddle_draw_last_pt {
                            let dx = pt.0 - prev.0;
                            let dy = pt.1 - prev.1;
                            if dx * dx + dy * dy > 0.00004 {
                                let msg = HuddleDrawMessage::New {
                                    item_id: item_move.clone(),
                                    segment: make_linear_segment(prev, pt),
                                };
                                this.huddle_draw_last_pt = Some(pt);
                                (this.send)(Command::SendHuddleDraw(msg.clone()));
                                this.apply_local_draw(msg, cx);
                            }
                        }
                    } else {
                        let pos_msg = HuddleDrawMessage::Position {
                            item_id: item_move.clone(),
                            x: pt.0,
                            y: pt.1,
                        };
                        (this.send)(Command::SendHuddleDraw(pos_msg.clone()));
                        this.apply_local_draw(pos_msg, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, _ev: &MouseUpEvent, _, cx| {
                        if this.huddle_drawing_active {
                            this.huddle_drawing_active = false;
                            this.huddle_draw_last_pt = None;
                            let end_msg = HuddleDrawMessage::End {
                                item_id: item_up.clone(),
                            };
                            (this.send)(Command::SendHuddleDraw(end_msg.clone()));
                            this.apply_local_draw(end_msg, cx);
                        }
                    }),
                );
        }
        pane
    }

    fn normalize_stage_point(&self, pos: Point<Pixels>) -> Option<(f64, f64)> {
        let bounds = *self.huddle_stage_bounds.lock().ok()?;
        let w: f32 = bounds.size.width.into();
        let h: f32 = bounds.size.height.into();
        if w <= 1.0 || h <= 1.0 {
            return None;
        }
        let x: f32 = (pos.x - bounds.origin.x).into();
        let y: f32 = (pos.y - bounds.origin.y).into();
        Some((
            (x / w).clamp(0.0, 1.0) as f64,
            (y / h).clamp(0.0, 1.0) as f64,
        ))
    }

    fn render_huddle_tiles(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        people: &[slackless_core::types::UserId],
        tile_bg: Rgba,
        _cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (tile_w, tile_h, avatar_px, grid_max_w) = match people.len() {
            0..=2 => (px(360.), px(244.), 88.0, px(820.)),
            3 => (px(268.), px(196.), 76.0, px(860.)),
            4 => (px(304.), px(192.), 74.0, px(660.)),
            _ => (px(236.), px(154.), 62.0, px(840.)),
        };
        h_flex()
            .max_w(grid_max_w)
            .flex_wrap()
            .items_center()
            .justify_center()
            .gap_3p5()
            .children(people.iter().map(|uid| {
                self.render_huddle_participant_tile(state, uid, tile_bg, tile_w, tile_h, avatar_px)
            }))
    }

    fn render_huddle_filmstrip(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        people: &[slackless_core::types::UserId],
        tile_bg: Rgba,
        _cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        if self.huddle_stage_expanded {
            return div();
        }
        h_flex()
            .flex_none()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2p5()
            .children(people.iter().take(6).map(|uid| {
                self.render_huddle_participant_tile(state, uid, tile_bg, px(148.), px(92.), 42.0)
            }))
    }

    fn render_huddle_participant_tile(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
        uid: &slackless_core::types::UserId,
        tile_bg: Rgba,
        tile_w: Pixels,
        tile_h: Pixels,
        avatar_px: f32,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use gpui_kit::gpui::{ObjectFit, StyledImage as _, img};
        use slackless_core::huddle::VideoTileKey;

        let white = rgb(0xFFFFFF);
        let emerald = rgb(0x2BAC76);
        let crimson = rgb(0xE01E5A);
        let snap = &state.session;
        let is_self = *uid == self.fixtures.self_user;
        let speaking = if is_self {
            !snap.muted && state.levels.local_rms > 0.03
        } else {
            state
                .levels
                .active_speakers
                .iter()
                .any(|(id, lvl)| id == uid && *lvl > 0.05)
        };
        let is_muted = if is_self {
            snap.muted
        } else {
            state.levels.muted_peers.contains(uid)
        };
        let badge_glyph = state
            .sticky_emojis
            .get(uid)
            .map(|s| sticky_emoji_glyph(s).to_owned())
            .or_else(|| state.raised_hands.contains(uid).then(|| "✋".to_owned()));
        let video_frame = if is_self {
            state.video_frames.get(&VideoTileKey::LocalCamera).cloned()
        } else {
            state
                .video_frames
                .get(&VideoTileKey::RemoteCamera(uid.clone()))
                .cloned()
        };
        let name = self.fixtures.user_name(uid).to_owned();
        let display_name = if is_self {
            format!("{name} (You)")
        } else {
            name
        };

        div()
            .relative()
            .w(tile_w)
            .h(tile_h)
            .rounded_2xl()
            .overflow_hidden()
            .bg(tile_bg)
            .border_2()
            .border_color(if speaking { emerald } else { rgba(0xFFFFFF14) })
            .child(if let Some(frame) = video_frame {
                img(frame)
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            } else {
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .p(px(3.))
                            .rounded_2xl()
                            .border_2()
                            .border_color(if speaking {
                                emerald.opacity(0.65)
                            } else {
                                rgba(0x00000000)
                            })
                            .child(crate::avatars::user_chip(&self.fixtures, uid, avatar_px)),
                    )
                    .into_any_element()
            })
            // Top-left frosted sticky/hand-raise pill
            .when_some(badge_glyph, |el, glyph| {
                el.child(
                    div()
                        .absolute()
                        .left(px(10.))
                        .top(px(10.))
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .bg(rgba(0x0E1014D6))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1F))
                        .text_sm()
                        .child(glyph),
                )
            })
            // Bottom-left frosted dark name pill with inline mic-off / active-speaker indicator
            .child(
                h_flex()
                    .absolute()
                    .left(px(10.))
                    .bottom(px(10.))
                    .max_w(tile_w - px(20.))
                    .px_2()
                    .py_1()
                    .gap_1p5()
                    .items_center()
                    .rounded_lg()
                    .bg(rgba(0x0E1014C8))
                    .when(is_muted, |pill| {
                        pill.child(
                            Icon::new(IconName::MicOff)
                                .size(px(11.))
                                .text_color(crimson),
                        )
                    })
                    .when(!is_muted && speaking, |pill| {
                        pill.child(div().size(px(7.)).rounded_full().bg(emerald))
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(white)
                            .child(display_name),
                    ),
            )
    }

    fn render_huddle_floating_reactions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
    ) -> Option<gpui_kit::gpui::Div> {
        if state.reactions.is_empty() {
            return None;
        }
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let mut stacked: Vec<(String, usize, Vec<String>)> = Vec::new();
        for entry in &state.reactions {
            let who = self.fixtures.user_name(&entry.user).to_owned();
            if let Some(slot) = stacked.iter_mut().find(|(em, _, _)| *em == entry.emoji) {
                slot.1 += 1;
                if !slot.2.contains(&who) {
                    slot.2.push(who);
                }
            } else {
                stacked.push((entry.emoji.clone(), 1, vec![who]));
            }
        }
        Some(
            h_flex()
                .absolute()
                .left(px(18.))
                .bottom(px(14.))
                .flex_wrap()
                .gap_1p5()
                .children(stacked.into_iter().map(|(emoji, count, names)| {
                    let glyph = sticky_emoji_glyph(&emoji);
                    let label = if count > 1 {
                        format!("{glyph} ×{count}")
                    } else {
                        glyph.to_owned()
                    };
                    h_flex()
                        .px_2p5()
                        .py_1()
                        .gap_1p5()
                        .items_center()
                        .rounded_full()
                        .bg(rgba(0x14171CE0))
                        .border_1()
                        .border_color(rgba(0xFFFFFF1F))
                        .shadow_lg()
                        .text_xs()
                        .text_color(white)
                        .child(label)
                        .child(div().text_color(muted).child(names.join(", ")))
                })),
        )
    }

    fn render_huddle_floating_captions(
        &self,
        state: &crate::huddle_dock::HuddleViewState,
    ) -> Option<gpui_kit::gpui::Div> {
        let show_captions = state.session.captions_enabled || !state.captions.is_empty();
        if !show_captions {
            return None;
        }
        let white = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let mut cap_box = v_flex()
            .max_w(px(620.))
            .px_4()
            .py_2()
            .gap_0p5()
            .rounded_xl()
            .bg(rgba(0x0B0D10DC))
            .border_1()
            .border_color(rgba(0xFFFFFF18))
            .shadow_xl()
            .text_xs()
            .text_color(white);
        if state.captions.is_empty() {
            cap_box = cap_box.child(
                div()
                    .text_color(muted)
                    .child("CC · Listening for live speech transcripts…"),
            );
        } else {
            cap_box = cap_box.children(state.captions.iter().map(|(uid, text, is_partial)| {
                let who = self.fixtures.user_name(uid).to_owned();
                h_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(format!("{who}:")),
                    )
                    .child(
                        div()
                            .text_color(if *is_partial { muted } else { white })
                            .child(text.clone()),
                    )
            }));
        }
        Some(
            h_flex()
                .absolute()
                .bottom(px(14.))
                .left_0()
                .right_0()
                .justify_center()
                .px_4()
                .child(cap_box),
        )
    }

    fn render_huddle_active_popover(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        channel: &ChannelId,
        people: &[slackless_core::types::UserId],
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::gpui::Div> {
        if self.huddle_share_open {
            return Some(self.render_huddle_share_panel(snap.share_source_id.as_deref(), cx));
        }
        if self.huddle_reactions_open {
            return Some(self.render_huddle_reactions_panel(snap, cx));
        }
        if self.huddle_theme_open {
            return Some(self.render_huddle_theme_panel(snap.background_id.as_deref(), cx));
        }
        if self.huddle_invite_open {
            return Some(self.render_huddle_invite_panel(channel, people, cx));
        }
        if self.huddle_devices_open {
            return Some(self.render_huddle_devices_panel(
                snap.input_device.clone(),
                snap.output_device.clone(),
                cx,
            ));
        }
        None
    }

    fn render_huddle_share_panel(
        &self,
        active_source: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white, emerald, crimson) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
            rgb(0xE01E5A),
        );
        let send = self.send.clone();
        let loopback = slackless_core::huddle::system_audio_source_label();
        v_flex()
            .max_w(px(560.))
            .p_3()
            .gap_2p5()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(0xFFFFFF0A))
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .text_color(if loopback.is_some() { emerald } else { muted })
                    .child(if let Some(dev) = loopback {
                        format!("System audio capture ready via {dev}")
                    } else {
                        "System audio: install BlackHole 2ch (or virtual loopback) to share app audio"
                            .to_owned()
                    }),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("Share a screen or window"),
                    )
                    .when(active_source.is_some(), |row| {
                        let send_stop = send.clone();
                        row.child(
                            div()
                                .id("popout-huddle-share-stop")
                                .px_2p5()
                                .py_1()
                                .rounded_full()
                                .bg(crimson)
                                .cursor_pointer()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(white)
                                .child("Stop sharing")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    (send_stop)(Command::SetHuddleScreenShare { source_id: None });
                                    if this.fixtures.sample {
                                        if let Some(dock) = this.huddle_dock_entity() {
                                            dock.update(cx, |d, cx| d.set_sharing_local(None, cx));
                                        }
                                    }
                                    this.huddle_share_open = false;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(if self.huddle_screens.is_empty() {
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Scanning screens & windows… (grant Screen Recording permission in macOS System Settings if none appear)")
                    .into_any_element()
            } else {
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(self.huddle_screens.iter().enumerate().map(|(ix, src)| {
                        let selected = active_source == Some(src.id.as_str());
                        let src_id = src.id.clone();
                        let send_pick = send.clone();
                        let prefix = if src.is_monitor { "🖥 " } else { "🪟 " };
                        div()
                            .id(("popout-huddle-src", ix))
                            .px_2p5()
                            .py_1()
                            .rounded_lg()
                            .cursor_pointer()
                            .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                            .hover(|el| el.bg(if selected { emerald } else { rgba(0xFFFFFF22) }))
                            .text_xs()
                            .text_color(white)
                            .child(if selected {
                                format!("✓ {prefix}{}", src.name)
                            } else {
                                format!("{prefix}{}", src.name)
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_pick)(Command::SetHuddleScreenShare {
                                    source_id: Some(src_id.clone()),
                                });
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        dock.update(cx, |d, cx| {
                                            d.set_sharing_local(Some(src_id.clone()), cx);
                                        });
                                    }
                                }
                                this.huddle_share_open = false;
                                cx.notify();
                            }))
                    }))
                    .into_any_element()
            })
    }

    fn render_huddle_reactions_panel(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use slackless_core::huddle::{HuddleDataEvent, HuddleReactionKind};

        const QUICK: [(&str, &str); 8] = [
            ("👍", "+1"),
            ("🎉", "tada"),
            ("❤️", "heart"),
            ("😂", "joy"),
            ("👏", "clap"),
            ("🔥", "fire"),
            ("👀", "eyes"),
            ("🙌", "raised_hands"),
        ];
        const EFFECTS: [(&str, &str, &str); 6] = [
            ("🎉", "Confetti", "confetti"),
            ("💖", "Hearts", "hearts"),
            ("👏", "Applause", "clap"),
            ("✨", "Sparkles", "sparkles"),
            ("🚀", "Rocket", "rocket"),
            ("🏆", "Trophy", "trophy"),
        ];
        const STICKY: [(&str, &str, &str); 4] = [
            ("✋", "Hand", "raised_hand"),
            ("☕", "Coffee", "coffee"),
            ("🎧", "BRB", "headphones"),
            ("✅", "Ready", "white_check_mark"),
        ];

        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
        );
        let send = self.send.clone();
        let active_sticky = snap.sticky_emoji.clone();

        v_flex()
            .p_3()
            .gap_2p5()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(div().w(px(52.)).text_xs().text_color(muted).child("React"))
                    .children(QUICK.iter().enumerate().map(|(ix, &(glyph, code))| {
                        let em = code.to_owned();
                        let send_em = send.clone();
                        div()
                            .id(("popout-huddle-react", ix))
                            .size(px(32.))
                            .rounded_lg()
                            .cursor_pointer()
                            .hover(|el| el.bg(rgba(0xFFFFFF1C)))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_base()
                            .child(glyph)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                (send_em)(Command::SendHuddleReaction {
                                    emoji: em.clone(),
                                    kind: HuddleReactionKind::Standard,
                                    emoji_url: None,
                                });
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        let me = this.fixtures.self_user.clone();
                                        dock.update(cx, |d, cx| {
                                            d.apply_data_event(
                                                &HuddleDataEvent::Reaction {
                                                    user: me,
                                                    emoji: em.clone(),
                                                    kind: HuddleReactionKind::Standard,
                                                    emoji_url: None,
                                                    at_ms: 0,
                                                },
                                                cx,
                                            );
                                        });
                                    }
                                }
                                cx.notify();
                            }))
                    })),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .w(px(52.))
                            .text_xs()
                            .text_color(muted)
                            .child("Effects"),
                    )
                    .children(
                        EFFECTS
                            .iter()
                            .enumerate()
                            .map(|(ix, &(glyph, label, code))| {
                                let em = code.to_owned();
                                let send_ef = send.clone();
                                div()
                                    .id(("popout-huddle-effect", ix))
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(rgba(0xFFFFFF12))
                                    .hover(|el| el.bg(rgba(0xFFFFFF22)))
                                    .text_xs()
                                    .text_color(white)
                                    .child(format!("{glyph} {label}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        (send_ef)(Command::SendHuddleReaction {
                                            emoji: em.clone(),
                                            kind: HuddleReactionKind::Effect,
                                            emoji_url: None,
                                        });
                                        if this.fixtures.sample {
                                            if let Some(dock) = this.huddle_dock_entity() {
                                                let me = this.fixtures.self_user.clone();
                                                dock.update(cx, |d, cx| {
                                                    d.apply_data_event(
                                                        &HuddleDataEvent::Reaction {
                                                            user: me,
                                                            emoji: em.clone(),
                                                            kind: HuddleReactionKind::Effect,
                                                            emoji_url: None,
                                                            at_ms: 0,
                                                        },
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                        this.huddle_reactions_open = false;
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .items_center()
                    .child(div().w(px(52.)).text_xs().text_color(muted).child("Sticky"))
                    .children(
                        STICKY
                            .iter()
                            .enumerate()
                            .map(|(ix, &(glyph, label, code))| {
                                let selected = active_sticky.as_deref() == Some(code);
                                let next = (!selected).then(|| code.to_owned());
                                let send_st = send.clone();
                                div()
                                    .id(("popout-huddle-sticky", ix))
                                    .px_2p5()
                                    .py_1()
                                    .rounded_full()
                                    .cursor_pointer()
                                    .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                    .hover(|el| {
                                        el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                    })
                                    .text_xs()
                                    .text_color(white)
                                    .child(format!("{glyph} {label}"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        (send_st)(Command::SetHuddleStickyEmoji(next.clone()));
                                        if this.fixtures.sample {
                                            if let Some(dock) = this.huddle_dock_entity() {
                                                let me = this.fixtures.self_user.clone();
                                                dock.update(cx, |d, cx| {
                                                    d.apply_data_event(
                                                        &HuddleDataEvent::StickyEmoji {
                                                            user: me,
                                                            emoji: next.clone(),
                                                        },
                                                        cx,
                                                    );
                                                });
                                            }
                                        }
                                        cx.notify();
                                    }))
                            }),
                    ),
            )
    }

    fn render_huddle_theme_panel(
        &self,
        active_bg: Option<&str>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        const THEMES: [(&str, &str, u32); 6] = [
            ("default", "Obsidian", 0x1A1D21),
            ("midnight", "Midnight", 0x10172A),
            ("sunset", "Sunset", 0x2B1818),
            ("forest", "Forest", 0x11221B),
            ("ocean", "Ocean", 0x0F212E),
            ("plum", "Aubergine", 0x24142B),
        ];
        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x2BAC76),
        );
        let send = self.send.clone();
        h_flex()
            .p_3()
            .flex_wrap()
            .gap_2()
            .items_center()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .mr_1()
                    .child("Theme"),
            )
            .children(THEMES.iter().enumerate().map(|(ix, &(id, label, hex))| {
                let selected = active_bg == Some(id) || (active_bg.is_none() && id == "default");
                let bg_id = id.to_owned();
                let send_bg = send.clone();
                h_flex()
                    .id(("popout-huddle-theme", ix))
                    .px_2p5()
                    .py_1()
                    .gap_1p5()
                    .items_center()
                    .rounded_full()
                    .cursor_pointer()
                    .bg(if selected {
                        rgba(0xFFFFFF24)
                    } else {
                        rgba(0xFFFFFF10)
                    })
                    .border_1()
                    .border_color(if selected { emerald } else { rgba(0xFFFFFF18) })
                    .hover(|el| el.bg(rgba(0xFFFFFF20)))
                    .text_xs()
                    .text_color(white)
                    .child(
                        div()
                            .size(px(12.))
                            .rounded_full()
                            .bg(rgb(hex))
                            .border_1()
                            .border_color(rgba(0xFFFFFF44)),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        (send_bg)(Command::SetHuddleBackground(bg_id.clone()));
                        if let Some(dock) = this.huddle_dock_entity() {
                            dock.update(cx, |d, cx| d.set_background_local(bg_id.clone(), cx));
                        }
                        this.huddle_theme_open = false;
                        cx.notify();
                    }))
            }))
    }

    fn render_huddle_invite_panel(
        &self,
        channel: &ChannelId,
        people: &[slackless_core::types::UserId],
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
        );
        let candidates: Vec<_> = self
            .fixtures
            .users
            .iter()
            .filter(|u| !u.is_bot && !u.deleted && !people.contains(&u.id))
            .take(12)
            .cloned()
            .collect();
        let inv_ch = channel.clone();
        let send_inv = self.send.clone();
        v_flex()
            .max_w(px(520.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("Invite people to this huddle"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .children(candidates.into_iter().enumerate().map(|(ix, u)| {
                        let uid = u.id.clone();
                        let ch = inv_ch.clone();
                        let send = send_inv.clone();
                        let label = self.fixtures.user_name(&uid).to_owned();
                        h_flex()
                            .id(("popout-huddle-inv", ix))
                            .px_2p5()
                            .py_1()
                            .gap_1p5()
                            .items_center()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(rgba(0xFFFFFF12))
                            .hover(|el| el.bg(rgba(0xFFFFFF22)))
                            .text_xs()
                            .text_color(white)
                            .child(
                                gpui_kit::component::Icon::new(IconName::UserPlus)
                                    .size(px(12.))
                                    .text_color(muted),
                            )
                            .child(label)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                (send)(Command::InviteToHuddle {
                                    channel: ch.clone(),
                                    users: vec![uid.clone()],
                                });
                                actions::notify(
                                    "Invitation sent",
                                    "Notified member to join this huddle.",
                                    window,
                                    cx,
                                );
                                this.huddle_invite_open = false;
                                cx.notify();
                            }))
                    })),
            )
    }

    fn render_huddle_devices_panel(
        &self,
        cur_in: Option<String>,
        cur_out: Option<String>,
        _cx: &Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (card_bg, border, muted, white, emerald) = (
            rgb(0x222529),
            rgba(0xFFFFFF22),
            rgb(0xABABAD),
            rgb(0xFFFFFF),
            rgb(0x007A5A),
        );
        let send_in = self.send.clone();
        let send_out = self.send.clone();
        v_flex()
            .max_w(px(540.))
            .p_3()
            .gap_2()
            .rounded_2xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .shadow_2xl()
            .text_color(white)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("Microphone"),
            )
            .child(
                h_flex().flex_wrap().gap_1p5().children(
                    self.huddle_devices
                        .inputs
                        .iter()
                        .enumerate()
                        .map(|(ix, d)| {
                            let selected = cur_in.as_deref() == Some(d.name.as_str())
                                || (cur_in.is_none() && d.is_default);
                            let name = d.name.clone();
                            let out = cur_out.clone();
                            let send = send_in.clone();
                            div()
                                .id(("popout-in-dev", ix))
                                .px_2p5()
                                .py_1()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                .hover(|el| {
                                    el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                })
                                .text_xs()
                                .text_color(white)
                                .child(if selected {
                                    format!("✓ {}", d.name)
                                } else {
                                    d.name.clone()
                                })
                                .on_click(move |_, _, _| {
                                    (send)(Command::SetHuddleAudioDevices {
                                        input: Some(name.clone()),
                                        output: out.clone(),
                                    });
                                })
                        }),
                ),
            )
            .child(
                div()
                    .mt_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("Speaker"),
            )
            .child(
                h_flex().flex_wrap().gap_1p5().children(
                    self.huddle_devices
                        .outputs
                        .iter()
                        .enumerate()
                        .map(|(ix, d)| {
                            let selected = cur_out.as_deref() == Some(d.name.as_str())
                                || (cur_out.is_none() && d.is_default);
                            let name = d.name.clone();
                            let inp = cur_in.clone();
                            let send = send_out.clone();
                            div()
                                .id(("popout-out-dev", ix))
                                .px_2p5()
                                .py_1()
                                .rounded_lg()
                                .cursor_pointer()
                                .bg(if selected { emerald } else { rgba(0xFFFFFF12) })
                                .hover(|el| {
                                    el.bg(if selected { emerald } else { rgba(0xFFFFFF22) })
                                })
                                .text_xs()
                                .text_color(white)
                                .child(if selected {
                                    format!("✓ {}", d.name)
                                } else {
                                    d.name.clone()
                                })
                                .on_click(move |_, _, _| {
                                    (send)(Command::SetHuddleAudioDevices {
                                        input: inp.clone(),
                                        output: Some(name.clone()),
                                    });
                                })
                        }),
                ),
            )
    }

    fn render_huddle_media_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::list_screen_sources;

        let (muted_now, video_now, blur_now, sharing_now) =
            (snap.muted, snap.video, snap.blur_background, snap.sharing);
        let (dark_btn, dark_hover, white, obsidian, crimson, emerald) = (
            rgb(0x2E3136),
            rgb(0x3B3F46),
            rgb(0xFFFFFF),
            rgb(0x1A1D21),
            rgb(0xE01E5A),
            rgb(0x007A5A),
        );

        let send_mute = self.send.clone();
        let next_muted = !muted_now;
        let mic_btn = div()
            .id("popout-huddle-mic")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if muted_now { crimson } else { dark_btn })
            .hover(|el| el.bg(if muted_now { rgb(0xC91A50) } else { dark_hover }))
            .child(
                Icon::new(if muted_now {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .size(px(17.))
                .text_color(white),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_mute)(Command::SetHuddleMuted(next_muted));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_muted_local(next_muted, cx));
                    }
                }
            }));

        let send_cam = self.send.clone();
        let next_cam = !video_now;
        let cam_btn = div()
            .id("popout-huddle-video")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if video_now { white } else { dark_btn })
            .hover(|el| el.bg(if video_now { rgb(0xE8E8E8) } else { dark_hover }))
            .child(
                Icon::new(if video_now {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(17.))
                .text_color(if video_now { obsidian } else { white }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_cam)(Command::SetHuddleCamera {
                    enabled: next_cam,
                    device_index: None,
                    blur: blur_now,
                });
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_camera_local(next_cam, cx));
                    }
                }
            }));

        let send_blur = self.send.clone();
        let next_blur = !blur_now;
        let blur_btn = div()
            .id("popout-huddle-blur")
            .h(px(40.))
            .px_3()
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if blur_now { white } else { dark_btn })
            .hover(|el| el.bg(if blur_now { rgb(0xE8E8E8) } else { dark_hover }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if blur_now { obsidian } else { white })
            .child(if blur_now { "Blur on" } else { "Blur" })
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_blur)(Command::SetHuddleBlurBackground(next_blur));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_blur_local(next_blur, cx));
                    }
                }
            }));

        let share_btn = div()
            .id("popout-huddle-share")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if sharing_now { emerald } else { dark_btn })
            .hover(|el| {
                el.bg(if sharing_now {
                    rgb(0x2BAC76)
                } else {
                    dark_hover
                })
            })
            .child(
                Icon::new(IconName::ScreenShare)
                    .size(px(17.))
                    .text_color(white),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let next = !this.huddle_share_open;
                this.close_huddle_popovers();
                this.huddle_share_open = next;
                if this.huddle_share_open {
                    cx.spawn(async move |win, cx| {
                        let screens = cx
                            .background_executor()
                            .spawn(async move { list_screen_sources() })
                            .await;
                        let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                            win.huddle_screens = screens;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }));

        h_flex()
            .gap_2()
            .items_center()
            .child(mic_btn)
            .child(cam_btn)
            .when(video_now, |bar| bar.child(blur_btn))
            .child(share_btn)
    }

    fn render_huddle_collab_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let (dark_btn, dark_hover, white, obsidian, amber) = (
            rgb(0x2E3136),
            rgb(0x3B3F46),
            rgb(0xFFFFFF),
            rgb(0x1A1D21),
            rgb(0xECB22E),
        );
        let hand_now = snap.hand_raised;
        let captions_now = snap.captions_enabled;
        let music_on = snap.jukebox_song.is_some();

        let send_hand = self.send.clone();
        let next_hand = !hand_now;
        let hand_btn = div()
            .id("popout-huddle-hand")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if hand_now { amber } else { dark_btn })
            .hover(|el| el.bg(if hand_now { amber } else { dark_hover }))
            .child(
                gpui_kit::component::Icon::new(IconName::Hand)
                    .size(px(17.))
                    .text_color(if hand_now { obsidian } else { white }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_hand)(Command::SetHuddleHandRaised(next_hand));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        let me = this.fixtures.self_user.clone();
                        dock.update(cx, |d, cx| {
                            d.apply_data_event(
                                &slackless_core::huddle::HuddleDataEvent::HandRaise {
                                    user: me,
                                    raised: next_hand,
                                },
                                cx,
                            );
                        });
                    }
                }
            }));

        let react_btn = div()
            .id("popout-huddle-react-btn")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.huddle_reactions_open {
                dark_hover
            } else {
                dark_btn
            })
            .hover(|el| el.bg(dark_hover))
            .text_base()
            .child("😊")
            .on_click(cx.listener(|this, _, _, cx| {
                let next = !this.huddle_reactions_open;
                this.close_huddle_popovers();
                this.huddle_reactions_open = next;
                cx.notify();
            }));

        let send_cc = self.send.clone();
        let next_cc = !captions_now;
        let cc_btn = div()
            .id("popout-huddle-cc")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if captions_now { white } else { dark_btn })
            .hover(|el| {
                el.bg(if captions_now {
                    rgb(0xE8E8E8)
                } else {
                    dark_hover
                })
            })
            .child(
                gpui_kit::component::Icon::new(IconName::Captions)
                    .size(px(17.))
                    .text_color(if captions_now { obsidian } else { white }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_cc)(Command::SetHuddleCaptions(next_cc));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_captions_local(next_cc, cx));
                    }
                }
            }));

        let send_music = self.send.clone();
        let next_song = (!music_on).then(|| "lofi".to_owned());
        let music_btn = div()
            .id("popout-huddle-music")
            .w(px(44.))
            .h(px(40.))
            .rounded_full()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if music_on { rgb(0x611F69) } else { dark_btn })
            .hover(|el| el.bg(if music_on { rgb(0x7A2A84) } else { dark_hover }))
            .child(
                gpui_kit::component::Icon::new(IconName::Music)
                    .size(px(16.))
                    .text_color(white),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                (send_music)(Command::SetHuddleJukebox(next_song.clone()));
                if this.fixtures.sample {
                    if let Some(dock) = this.huddle_dock_entity() {
                        dock.update(cx, |d, cx| d.set_jukebox_local(next_song.clone(), cx));
                    }
                }
            }));

        h_flex()
            .gap_2()
            .items_center()
            .child(react_btn)
            .child(hand_btn)
            .child(cc_btn)
            .child(music_btn)
            .child(self.render_huddle_utility_controls(cx))
    }

    fn render_huddle_utility_controls(&self, cx: &mut Context<Self>) -> gpui_kit::gpui::Div {
        use gpui_kit::component::Icon;
        use slackless_core::huddle::list_audio_devices;

        let (dark_btn, dark_hover, white, crimson) =
            (rgb(0x2E3136), rgb(0x3B3F46), rgb(0xFFFFFF), rgb(0xE01E5A));
        let send_leave = self.send.clone();
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .id("popout-huddle-theme-btn")
                    .w(px(44.))
                    .h(px(40.))
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if self.huddle_theme_open {
                        dark_hover
                    } else {
                        dark_btn
                    })
                    .hover(|el| el.bg(dark_hover))
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size(px(16.))
                            .text_color(white),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = !this.huddle_theme_open;
                        this.close_huddle_popovers();
                        this.huddle_theme_open = next;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("popout-huddle-devices-btn")
                    .w(px(44.))
                    .h(px(40.))
                    .rounded_full()
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if self.huddle_devices_open {
                        dark_hover
                    } else {
                        dark_btn
                    })
                    .hover(|el| el.bg(dark_hover))
                    .child(
                        Icon::new(IconName::SlidersHorizontal)
                            .size(px(16.))
                            .text_color(white),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        let next = !this.huddle_devices_open;
                        this.close_huddle_popovers();
                        this.huddle_devices_open = next;
                        if this.huddle_devices_open {
                            cx.spawn(async move |win, cx| {
                                let devices = cx
                                    .background_executor()
                                    .spawn(async move { list_audio_devices() })
                                    .await;
                                let _ = win.update(cx, |win: &mut PopoutWindow, cx| {
                                    win.huddle_devices = devices;
                                    cx.notify();
                                });
                            })
                            .detach();
                        }
                        cx.notify();
                    })),
            )
            .child(div().w(px(1.)).h(px(24.)).mx_1().bg(rgba(0xFFFFFF1C)))
            .child(
                h_flex()
                    .id("popout-huddle-leave")
                    .h(px(40.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(crimson)
                    .hover(|el| el.bg(rgb(0xC91A50)))
                    .cursor_pointer()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(white)
                    .child(
                        Icon::new(IconName::PhoneOff)
                            .size(px(15.))
                            .text_color(white),
                    )
                    .child("Leave")
                    .on_click(move |_, window, _| {
                        (send_leave)(Command::LeaveHuddle);
                        window.remove_window();
                    }),
            )
    }

    fn render_huddle_controls(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        bar_bg: Rgba,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        h_flex()
            .flex_none()
            .h(px(72.))
            .px_4()
            .gap_2()
            .items_center()
            .justify_center()
            .bg(bar_bg)
            .border_t_1()
            .border_color(rgba(0xFFFFFF10))
            .child(self.render_huddle_media_controls(snap, cx))
            .child(self.render_huddle_collab_controls(snap, cx))
    }
}

fn huddle_theme_surface(bg_id: Option<&str>) -> (Rgba, Rgba) {
    match bg_id {
        Some("midnight") => (rgb(0x10172A), rgb(0x1B2440)),
        Some("sunset") => (rgb(0x2B1818), rgb(0x3B2222)),
        Some("forest") => (rgb(0x11221B), rgb(0x1B3328)),
        Some("ocean") => (rgb(0x0F212E), rgb(0x183244)),
        Some("plum") => (rgb(0x24142B), rgb(0x351F3E)),
        _ => (rgb(0x1A1D21), rgb(0x23262B)),
    }
}

fn make_linear_segment(
    p0: (f64, f64),
    p1: (f64, f64),
) -> slackless_core::huddle::HuddleDrawSegment {
    let lerp = |t: f64| [p0.0 + (p1.0 - p0.0) * t, p0.1 + (p1.1 - p0.1) * t];
    slackless_core::huddle::HuddleDrawSegment {
        a: [p0.0, p0.1],
        b: lerp(1.0 / 3.0),
        c: lerp(2.0 / 3.0),
        d: [p1.0, p1.1],
    }
}

fn sticky_emoji_glyph(code: &str) -> &str {
    match code {
        "raised_hand" => "✋",
        "coffee" => "☕",
        "headphones" => "🎧",
        "white_check_mark" => "✅",
        "+1" | "thumbsup" => "👍",
        "tada" => "🎉",
        "heart" => "❤️",
        "joy" => "😂",
        "clap" => "👏",
        "fire" => "🔥",
        "eyes" => "👀",
        "raised_hands" => "🙌",
        "confetti" => "🎉",
        "hearts" => "💖",
        "sparkles" => "✨",
        "rocket" => "🚀",
        "mic_drop" => "🎤",
        "trophy" => "🏆",
        "unicorn" => "🦄",
        other => other,
    }
}

fn effect_label(code: &str) -> (&'static str, &'static str) {
    match code {
        "confetti" => ("🎉", "Confetti"),
        "hearts" => ("💖", "Hearts"),
        "clap" => ("👏", "Applause"),
        "sparkles" => ("✨", "Sparkles"),
        "rocket" => ("🚀", "Rocket"),
        "mic_drop" => ("🎤", "Mic Drop"),
        "trophy" => ("🏆", "Trophy"),
        "unicorn" => ("🦄", "Unicorn"),
        _ => ("✨", "Effect"),
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
            | PopoutKind::Huddle { .. }
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
                | PopoutKind::Files { .. }
                | PopoutKind::Huddle { .. } => Ok(Vec::new()),
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
        | PopoutKind::Files { .. }
        | PopoutKind::Huddle { .. } => Vec::new(),
    }
}

impl Render for PopoutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let PopoutKind::Huddle {
            on_jump,
            on_open_canvas,
            ..
        } = &self.kind
        {
            return self
                .render_huddle_window(on_jump.clone(), on_open_canvas.clone(), cx)
                .into_any_element();
        }
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
            PopoutKind::Huddle { .. } => "Huddle · Live",
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
                        in_thread: !is_conversation
                            && matches!(&self.kind, PopoutKind::Thread { thread_ts, .. } if msg.ts != *thread_ts),
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
            PopoutKind::Huddle { .. } => v_flex().id("popout-huddle-noop").flex_1(),
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
        .into_any_element()
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
            let opts0 = popout_window_options("# general", false, Some(second_display), 0, cx);
            let opts1 = popout_window_options("# general", false, Some(second_display), 1, cx);
            assert_eq!(opts0.display_id, Some(second_display));
            assert_eq!(opts1.display_id, Some(second_display));
            let b0 = opts0.window_bounds.unwrap().get_bounds();
            let b1 = opts1.window_bounds.unwrap().get_bounds();
            assert_eq!(b1.origin.x - b0.origin.x, px(28.));
            assert_eq!(b1.origin.y - b0.origin.y, px(28.));

            let default_opts = popout_window_options("# general", false, None, 0, cx);
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
