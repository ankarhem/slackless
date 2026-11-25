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
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, Size, h_flex, h_resizable, resizable_panel, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, Bounds, ClipboardItem, Context, DisplayId, Edges, Entity,
    Focusable as _, FontWeight, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement as _, PathBuilder, Pixels, Point, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, anchored, canvas, deferred,
    div, hsla, point, px, rgb, rgba, size,
};
use gpui_kit::open_window;
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin, MessageAction, MessageChange};
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

/// Parameters for opening a popped-out secondary GPUI window.
pub struct PopoutRequest {
    pub kind: PopoutKind,
    pub title: String,
    pub fixtures: Arc<Fixtures>,
    pub store: Option<SharedStore>,
    pub team: Option<TeamId>,
    pub send: CommandForward,
    pub later_sync: Option<(WeakEntity<LaterView>, usize)>,
    pub display_id: Option<DisplayId>,
}

/// Open a new native GPUI window showing `req.kind`, centered (with cascade offset) on
/// `req.display_id` (the main window's display) or the primary display if `None`.
pub fn open_popout(req: PopoutRequest, cx: &mut App) {
    let slot = CASCADE_STEP.fetch_add(1, Ordering::Relaxed);
    let is_huddle = matches!(req.kind, PopoutKind::Huddle { .. });
    let options = popout_window_options(&req.title, is_huddle, req.display_id, slot, cx);
    let _ = open_window(options, cx, move |window, cx| {
        window.set_window_title(&format!("{} — slackless", req.title));
        window.activate_window();
        cx.new(|cx| PopoutWindow::new(req, window, cx))
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

/// Active popover or modal inside the Pop-out Huddle Window (mutually exclusive).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HuddlePopover {
    Invite,
    Devices,
    Camera,
    Share,
    Reactions,
    Theme,
    More,
    Music,
}

/// Active tab inside the Screen Sharing picker modal (`Share entire screen` vs `Window`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum HuddleShareTab {
    Screen,
    Window,
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
    huddle_video: Option<WeakEntity<crate::huddle_dock::HuddleVideoStore>>,
    _huddle_popout_guard: Option<crate::huddle_dock::HuddlePopoutGuard>,
    last_store_revision: u64,
    reload_gen: u64,
    huddle_popover: Option<HuddlePopover>,
    huddle_share_tab: HuddleShareTab,
    huddle_share_selected: Option<String>,
    huddle_share_audio: bool,
    huddle_stage_expanded: bool,
    huddle_draw_mode: bool,
    huddle_draw_hue: i32,
    huddle_drawing_active: bool,
    huddle_draw_points: Vec<(f64, f64)>,
    huddle_draw_local_segments: Vec<slackless_core::huddle::HuddleDrawSegment>,
    huddle_draw_local_item: String,
    huddle_draw_last_cursor_ms: u64,
    huddle_stage_bounds: Arc<Mutex<Bounds<Pixels>>>,
    huddle_focused_screen: Option<usize>,
    huddle_topic_editing: Option<Entity<TextareaState>>,
    huddle_drawer: Option<HuddleDrawer>,
    huddle_thread_fetched: Option<Ts>,
    huddle_thread_root_cache: Option<Ts>,
    huddle_thread_store_msgs: Vec<Message>,
    huddle_thread_local: Vec<Message>,
    huddle_devices: slackless_core::huddle::AudioDevices,
    huddle_cameras: Vec<slackless_core::huddle::CameraDeviceInfo>,
    huddle_selected_camera: Option<usize>,
    huddle_screens: Vec<slackless_core::huddle::ScreenSource>,
    huddle_screen_thumbs: std::collections::HashMap<String, Arc<gpui_kit::gpui::RenderImage>>,
    /// Allocated only for interactive popouts (`Conversation`, `Thread`, `Huddle`).
    composer: Option<Entity<Composer>>,
    /// Allocated only for interactive popouts (`Conversation`, `Thread`, `Huddle`).
    emoji_picker: Option<Entity<EmojiPicker>>,
    picker: Option<(PopoutPickerTarget, Point<Pixels>)>,
    editing: Option<(ChannelId, Ts, Entity<TextareaState>)>,
    scroll: ScrollHandle,
    image_viewer: Option<Entity<ImageViewer>>,
    _viewer_sub: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl PopoutWindow {
    fn new(req: PopoutRequest, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let PopoutRequest {
            kind,
            title,
            fixtures,
            store,
            team,
            send,
            later_sync,
            display_id: _,
        } = req;
        let has_composer = matches!(
            &kind,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. }
        );
        let is_huddle = matches!(&kind, PopoutKind::Huddle { .. });
        let interactive = has_composer || is_huddle;
        let mut subscriptions = Vec::new();
        let (composer, emoji_picker) = if interactive {
            let placeholder = match &kind {
                PopoutKind::Conversation { .. } => format!("Message {title}"),
                PopoutKind::Thread { .. } => "Reply in thread…".to_owned(),
                PopoutKind::Huddle { .. } => "Reply…".to_owned(),
                _ => String::new(),
            };
            let composer = Composer::new(placeholder.into(), fixtures.clone(), window, cx);
            let emoji_picker = EmojiPicker::new(fixtures.clone(), window, cx);
            subscriptions.push(cx.subscribe_in(
                &composer,
                window,
                |this: &mut Self, _, event: &ComposerEvent, window, cx| {
                    match event {
                        ComposerEvent::Submit(content) => {
                            this.send_content_guarded(content.clone(), Some(window), cx);
                        }
                        ComposerEvent::SubmitWithFiles { content, files } => {
                            this.send_files(content.clone(), files.clone(), Some(window), cx);
                        }
                        ComposerEvent::OpenEmojiPicker(pos) => {
                            this.open_picker(PopoutPickerTarget::Composer, *pos, window, cx);
                        }
                        ComposerEvent::EditLast => {
                            if let Some(ch) = match &this.kind {
                                PopoutKind::Conversation { channel, .. }
                                | PopoutKind::Thread { channel, .. } => Some(channel.clone()),
                                _ => None,
                            } && let Some(last_own) = this
                                .messages
                                .iter()
                                .rev()
                                .find(|m| m.user.as_ref() == Some(&this.fixtures.self_user))
                                .cloned()
                            {
                                this.start_edit(ch, &last_own, window, cx);
                            }
                        }
                        _ => {}
                    }
                },
            ));
            subscriptions.push(cx.subscribe_in(
                &emoji_picker,
                window,
                |this: &mut Self, _, event: &EmojiPickerEvent, window, cx| {
                    if let EmojiPickerEvent::Pick(choice) = event {
                        this.pick_emoji(choice, window, cx);
                    }
                },
            ));
            (Some(composer), Some(emoji_picker))
        } else {
            (None, None)
        };
        // Fixture mode loads synchronously from in-memory fixtures; live mode loads asynchronously
        // off the UI thread below so opening a popout never takes `store.lock()` on the UI thread.
        let initial = if store.is_none() {
            load_fixture_messages(&kind, &fixtures)
        } else {
            Vec::new()
        };
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
        if let (Some(ch), Some((weak_later, workspace))) = (active_channel.clone(), &later_sync)
            && let Some(later) = weak_later.upgrade()
        {
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
        let mut activity_rows = Vec::new();
        let mut later_items = Vec::new();
        let mut files_items = Vec::new();
        let mut huddle_state = None;
        let mut huddle_video = None;
        let mut huddle_popout_guard = None;
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
                    let st = dock.read(cx).view_state();
                    let video_store = dock.read(cx).video_store();
                    let target =
                        composer::AlsoSendTarget::for_channel(&fixtures, &st.session.channel);
                    if let Some(composer) = &composer {
                        composer.update(cx, |c, cx| c.set_also_send(target, cx));
                    }
                    huddle_state = Some(st);
                    huddle_popout_guard = Some(video_store.read(cx).register_popout());
                    huddle_video = Some(video_store.downgrade());
                    subscriptions.push(cx.observe(&video_store, |_this, _vs, cx| {
                        cx.notify();
                    }));
                    subscriptions.push(cx.observe(&dock, |this, dock, cx| {
                        let st = dock.read(cx).view_state();
                        let prev_ch = this
                            .huddle_state
                            .as_ref()
                            .map(|s| s.session.channel.clone());
                        if prev_ch.as_ref() != Some(&st.session.channel) {
                            let target = composer::AlsoSendTarget::for_channel(
                                &this.fixtures,
                                &st.session.channel,
                            );
                            if let Some(composer) = &this.composer {
                                composer.update(cx, |c, cx| c.set_also_send(target, cx));
                            }
                            this.huddle_thread_fetched = None;
                            this.huddle_thread_root_cache = None;
                            this.huddle_thread_store_msgs.clear();
                            this.huddle_thread_local.clear();
                        }
                        this.huddle_state = Some(st);
                        if this.huddle_drawer == Some(HuddleDrawer::Thread) {
                            this.refresh_huddle_thread_async(cx);
                        }
                        this.maybe_fetch_huddle_thread();
                        cx.notify();
                    }));
                }
            }
            PopoutKind::Canvas { .. } => {}
        }
        // Lightweight fallback refresh from the shared SQLite store for secondary windows:
        // checks an O(1) atomic revision first so idle windows do zero SQLite queries and zero
        // repaints.
        if store.is_some() && (has_composer || is_huddle) {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let rev = slackless_core::store::Store::message_revision();
                    let keep = this
                        .update(cx, |win, cx| {
                            if rev == win.last_store_revision {
                                return;
                            }
                            if matches!(&win.kind, PopoutKind::Huddle { .. })
                                && win.huddle_drawer != Some(HuddleDrawer::Thread)
                            {
                                return;
                            }
                            win.last_store_revision = rev;
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
        let mut win = Self {
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
            huddle_video,
            _huddle_popout_guard: huddle_popout_guard,
            last_store_revision: slackless_core::store::Store::message_revision(),
            reload_gen: 0,
            huddle_popover: None,
            huddle_share_tab: HuddleShareTab::Screen,
            huddle_share_selected: None,
            huddle_share_audio: false,
            huddle_stage_expanded: false,
            huddle_draw_mode: false,
            huddle_draw_hue: 0,
            huddle_drawing_active: false,
            huddle_draw_points: Vec::new(),
            huddle_draw_local_segments: Vec::new(),
            huddle_draw_local_item: String::new(),
            huddle_draw_last_cursor_ms: 0,
            huddle_stage_bounds: Arc::new(Mutex::new(Bounds::default())),
            huddle_focused_screen: None,
            huddle_topic_editing: None,
            huddle_drawer: None,
            huddle_thread_fetched: None,
            huddle_thread_root_cache: None,
            huddle_thread_store_msgs: Vec::new(),
            huddle_thread_local: Vec::new(),
            huddle_devices: slackless_core::huddle::AudioDevices::default(),
            huddle_cameras: Vec::new(),
            huddle_selected_camera: None,
            huddle_screens: Vec::new(),
            huddle_screen_thumbs: std::collections::HashMap::new(),
            composer,
            emoji_picker,
            picker: None,
            editing: None,
            scroll,
            image_viewer: None,
            _viewer_sub: None,
            _subscriptions: subscriptions,
        };
        if win.store.is_some() && has_composer {
            win.reload_from_store(cx);
        }
        win
    }

    fn open_picker(
        &mut self,
        target: PopoutPickerTarget,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(emoji_picker) = &self.emoji_picker else {
            return;
        };
        emoji_picker.update(cx, |picker, cx| {
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
                if let Some(composer) = &self.composer {
                    composer.update(cx, |c, cx| c.insert_emoji(choice, window, cx));
                }
            }
            PopoutPickerTarget::Message { channel, ts } => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts)
                    && actions::add_reaction(msg, &self.fixtures.self_user, &choice.name)
                {
                    (self.send)(Command::AddReaction {
                        channel,
                        ts,
                        name: choice.name.clone(),
                    });
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
        // Seed with raw mrkdwn/rich_text source rather than plain_text so formatting & mentions
        // are never stripped during inline editing.
        let initial = crate::editor::from_rich_text(&message.blocks, &*self.fixtures)
            .map(|doc| doc.to_mrkdwn())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| message.text.clone());
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
        let Some((images, index)) =
            image_viewer::collect_viewer_images(&self.fixtures, &self.messages, ts, &origin)
        else {
            return;
        };
        let viewer = ImageViewer::new(self.fixtures.clone(), self.title.clone(), images, index, cx);
        self._viewer_sub = Some(cx.subscribe_in(
            &viewer,
            window,
            |this: &mut Self, _, event: &ViewerEvent, window, cx| {
                if matches!(event, ViewerEvent::Close) {
                    this.close_image_viewer(window, cx);
                }
            },
        ));
        self.image_viewer = Some(viewer);
        cx.notify();
    }

    fn close_image_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.image_viewer.take() {
            viewer.update(cx, |v, cx| v.evict_assets(window, cx));
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
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts)
                    && actions::add_reaction(msg, &self.fixtures.self_user, &name)
                {
                    (self.send)(Command::AddReaction { channel, ts, name });
                    cx.notify();
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
                actions::copy_message_text(&self.fixtures, &message, window, cx);
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
                actions::notify_saved_toggled(saved, String::new(), window, cx);
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
                let fixtures = self.fixtures.clone();
                let msg_clone = message.clone();
                actions::open_custom_remind_for_message(
                    &fixtures,
                    channel.clone(),
                    &message,
                    window,
                    cx,
                    move |request, when, window, cx| {
                        let _ = this.update(cx, |win, cx| {
                            let due = request.due;
                            win.saved.insert(request.ts.clone());
                            (win.send)(Command::SaveItem {
                                channel: request.channel.clone(),
                                ts: request.ts,
                                date_due: due,
                            });
                            win.sync_later_item(&request.channel, &msg_clone, true, due, cx);
                            actions::notify("Reminder set", when, window, cx);
                            cx.notify();
                        });
                    },
                );
            }
            MessageAction::TogglePin => {
                if let Some(msg) = self.messages.iter_mut().find(|m| m.ts == ts) {
                    let pinned = actions::toggle_pin(msg, &channel);
                    (self.send)(Command::SetPinned {
                        channel,
                        ts,
                        pinned,
                    });
                    actions::notify_pin_toggled(pinned, String::new(), window, cx);
                    cx.notify();
                }
            }
            MessageAction::MarkUnread => {
                let cursor = self
                    .messages
                    .iter()
                    .take_while(|m| m.ts < ts)
                    .last()
                    .map(|m| m.ts.clone())
                    .unwrap_or_else(|| crate::threads_cards::ts_just_before(&ts));
                match &self.kind {
                    PopoutKind::Thread { thread_ts, .. } => {
                        (self.send)(Command::MarkThreadRead {
                            channel,
                            thread_ts: thread_ts.clone(),
                            ts: cursor,
                        });
                    }
                    _ => {
                        (self.send)(Command::MarkRead {
                            channel,
                            ts: cursor,
                        });
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
                if let Some(MessageChange::BlockAction(dispatch)) = actions::dispatch_block_action(
                    !self.fixtures.sample,
                    &message,
                    &channel,
                    action,
                    attachment,
                    window,
                    cx,
                ) {
                    (self.send)(Command::BlockAction {
                        bot: dispatch.bot,
                        app_id: dispatch.app_id,
                        team: dispatch.team,
                        action: dispatch.action,
                        container: dispatch.container,
                    });
                }
            }
            MessageAction::DismissEphemeral => {
                self.messages.retain(|m| m.ts != ts);
                cx.notify();
            }
            MessageAction::AddToList => {}
            MessageAction::JoinHuddle => {}
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
                }
                if self.huddle_drawer == Some(HuddleDrawer::Thread) {
                    self.refresh_huddle_thread_async(cx);
                    self.maybe_fetch_huddle_thread();
                    cx.notify();
                }
                return;
            }
            PopoutKind::Canvas { .. } => return,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. } => {}
        }
        if let (Some(store), Some(team)) = (self.store.clone(), self.team.clone()) {
            let query = match &self.kind {
                PopoutKind::Conversation { channel, .. } => (channel.clone(), None),
                PopoutKind::Thread {
                    channel, thread_ts, ..
                } => (channel.clone(), Some(thread_ts.clone())),
                _ => return,
            };
            self.reload_gen = self.reload_gen.wrapping_add(1);
            let generation = self.reload_gen;
            cx.spawn(async move |this, cx| {
                let fresh = cx
                    .background_executor()
                    .spawn(async move {
                        let Ok(guard) = store.lock() else {
                            return None;
                        };
                        let rows = match query.1 {
                            Some(thread_ts) => guard.thread_replies(&team, &query.0, &thread_ts),
                            None => guard.channel_history(&team, &query.0, None, 200),
                        };
                        rows.ok().map(|list| {
                            list.into_iter()
                                .filter(|r| !r.deleted)
                                .map(|r| r.message)
                                .collect::<Vec<_>>()
                        })
                    })
                    .await;
                if let Some(fresh) = fresh {
                    let _ = this.update(cx, |win, cx| {
                        if win.reload_gen != generation {
                            return;
                        }
                        win.apply_refreshed_messages(fresh, cx);
                    });
                }
            })
            .detach();
            return;
        }
        let fresh = load_fixture_messages(&self.kind, &self.fixtures);
        self.apply_refreshed_messages(fresh, cx);
    }

    fn apply_refreshed_messages(&mut self, fresh: Vec<Message>, cx: &mut Context<Self>) {
        // Compare full message equality so remote edits, reactions, and pins always refresh.
        if fresh == self.messages {
            return;
        }
        let appended = fresh.len() > self.messages.len()
            || fresh.last().map(|m| &m.ts) != self.messages.last().map(|m| &m.ts);
        self.messages = fresh;
        if appended {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    fn maybe_fetch_huddle_thread(&mut self) {
        if self.huddle_drawer != Some(HuddleDrawer::Thread) {
            return;
        }
        let Some(state) = &self.huddle_state else {
            return;
        };
        let Some(root_ts) = self.resolve_huddle_thread_root(&state.session) else {
            return;
        };
        if self.huddle_thread_fetched.as_ref() == Some(&root_ts) {
            return;
        }
        self.huddle_thread_fetched = Some(root_ts.clone());
        (self.send)(Command::OpenThread {
            channel: state.session.channel.clone(),
            thread_ts: root_ts,
        });
    }

    fn send_content_guarded(
        &mut self,
        mut content: MessageContent,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        composer::apply_outgoing_transforms(&mut content, &self.fixtures, cx);
        if let Some(window) = window
            && composer::outgoing_guard_prefs(cx).warn_before_sending_secrets
        {
            let matches = slackless_core::detect_secrets(&content.text);
            if !matches.is_empty() {
                let this = cx.entity().downgrade();
                let pending = content;
                composer::confirm_secret_send_dialog(matches, window, cx, move |_, cx| {
                    let _ = this.update(cx, |win, cx| win.deliver_content(pending.clone(), cx));
                });
                return;
            }
        }
        self.deliver_content(content, cx);
    }

    #[cfg(test)]
    fn send_content(&mut self, mut content: MessageContent, cx: &mut Context<Self>) {
        composer::apply_outgoing_transforms(&mut content, &self.fixtures, cx);
        self.deliver_content(content, cx);
    }

    fn deliver_content(&mut self, content: MessageContent, cx: &mut Context<Self>) {
        if matches!(&self.kind, PopoutKind::Huddle { .. }) {
            let Some(state) = &self.huddle_state else {
                return;
            };
            let channel = state.session.channel.clone();
            let thread_ts = self.resolve_huddle_thread_root(&state.session);
            let broadcast = self
                .composer
                .as_ref()
                .is_some_and(|c| c.update(cx, |composer, cx| composer.take_also_send(cx)));
            let mut local =
                self.fixtures
                    .local_message(&channel, content.text.clone(), thread_ts.clone());
            local.blocks = content.blocks.clone();
            local.attachments = content.attachments.clone();
            self.huddle_thread_local.push(local);
            (self.send)(Command::SendMessage {
                channel,
                content,
                thread_ts,
                broadcast,
            });
            cx.notify();
            return;
        }
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

    fn send_files(
        &mut self,
        mut content: Option<MessageContent>,
        files: Vec<crate::composer::PendingAttachment>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let (channel, thread_ts) = match &self.kind {
            PopoutKind::Conversation { channel, .. } => (channel.clone(), None),
            PopoutKind::Thread {
                channel, thread_ts, ..
            } => (channel.clone(), Some(thread_ts.clone())),
            PopoutKind::Huddle { .. } => {
                let Some(state) = &self.huddle_state else {
                    return;
                };
                let channel = state.session.channel.clone();
                let thread_ts = self.resolve_huddle_thread_root(&state.session);
                (channel, thread_ts)
            }
            PopoutKind::Activity { .. }
            | PopoutKind::Later { .. }
            | PopoutKind::Canvas { .. }
            | PopoutKind::Files { .. } => return,
        };
        let uploads: Vec<slackless_core::api::UploadFile> = files
            .into_iter()
            .map(|attachment| slackless_core::api::UploadFile {
                filename: attachment.filename,
                bytes: attachment.bytes,
            })
            .collect();
        if uploads.is_empty() {
            if let Some(content) = content {
                self.send_content_guarded(content, window, cx);
            }
            return;
        }
        if let Some(ref mut c) = content {
            composer::apply_outgoing_transforms(c, &self.fixtures, cx);
        }
        (self.send)(Command::UploadFiles {
            channel,
            thread_ts,
            content,
            files: uploads,
        });
        cx.notify();
    }
}

fn load_fixture_messages(kind: &PopoutKind, fixtures: &Fixtures) -> Vec<Message> {
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

impl PopoutWindow {
    fn render_overlays(
        &self,
        root: gpui_kit::gpui::Stateful<gpui_kit::gpui::Div>,
        viewer_top: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let picker_overlay = self
            .picker
            .as_ref()
            .map(|(_, pos)| *pos)
            .zip(self.emoji_picker.clone());
        root.when_some(picker_overlay, |root, (pos, emoji_picker)| {
            let viewport = window.viewport_size();
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
                            .child(emoji_picker),
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
                    .top(viewer_top)
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(viewer),
            )
        })
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (bg, border, muted, fg) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        );
        let kind_label = match &self.kind {
            PopoutKind::Conversation { .. } => "Conversation",
            PopoutKind::Thread { .. } => "Thread",
            PopoutKind::Activity { .. } => "Activity · Live",
            PopoutKind::Later { .. } => "Later · Live",
            PopoutKind::Canvas { .. } => "Canvas",
            PopoutKind::Files { .. } => "Files · Live",
            PopoutKind::Huddle { .. } => "Huddle · Live",
        };
        TitleBar::new()
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
            )
    }

    fn render_messages_body(
        &self,
        channel: ChannelId,
        on_jump: JumpHandler,
        is_conversation: bool,
        links: &LinkHandler,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (bg, hover_bg, saved_bg) = (
            theme.background,
            theme.secondary.opacity(0.5),
            theme.warning.opacity(0.08),
        );
        let view = cx.entity();
        let mut rows: Vec<gpui_kit::gpui::AnyElement> = Vec::with_capacity(self.messages.len());
        let mut prev_msg: Option<&Message> = None;
        let editing = self.editing.clone();
        for (ix, msg) in self.messages.iter().enumerate() {
            if let Some((_, edit_ts, edit_input)) = &editing
                && *edit_ts == msg.ts
            {
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
            let chips =
                actions::reaction_chips(&key, &reactions, &self.fixtures, Some(&handler), cx)
                    .map(|c| c.pl(px(48.)));
            let row_body = message_row_linked(
                &self.fixtures,
                &display,
                Some(&handler),
                Some(links),
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
            let quick =
                crate::preferences::active_quick_reactions(&self.fixtures.quick_reactions, cx);
            let menu_handler = handler.clone();
            let row_key = key.clone();
            let keep_inside = ix == 0;
            let item = actions::HoverOverlay::new(SharedString::from(key), move |hover, _, cx| {
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
                            actions::hover_toolbar(&row_key, &quick, set, &handler, &hover, cx)
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

    fn render_activity_body(
        &self,
        on_jump: JumpHandler,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (bg, border, muted, accent) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.accent,
        );
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
            list = list.children(self.activity_rows.iter().enumerate().map(|(ix, item)| {
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

    fn render_later_body(
        &self,
        on_jump: JumpHandler,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.accent);
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
                            .child(div().text_xs().text_color(muted).child(item.label.clone())),
                    )
                    .child(div().text_sm().child(item.text.clone()))
            }));
        }
        list
    }

    fn render_canvas_body(
        &self,
        doc: &crate::canvas::CanvasDoc,
        file_id: &Option<String>,
        open_browser_url: &Option<String>,
        on_jump: &JumpHandler,
        cx: &App,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
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
        let on_load_comments = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(Command::LoadCanvasComments {
                    canvas_id: fid.clone(),
                });
            }) as crate::canvas::CanvasCommentsLoadFn
        });
        let on_post_comment = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |text, thread_ts, _: &mut Window, _: &mut App| {
                (send)(Command::PostCanvasComment {
                    canvas_id: fid.clone(),
                    text,
                    thread_ts,
                });
            }) as crate::canvas::CanvasCommentPostFn
        });
        let on_resolve_comment = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |thread_ts, resolved, _: &mut Window, _: &mut App| {
                (send)(Command::ResolveCanvasComment {
                    canvas_id: fid.clone(),
                    thread_ts,
                    resolved,
                });
            }) as crate::canvas::CanvasCommentResolveFn
        });
        let on_resolve_all_comments = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(Command::ResolveAllCanvasComments {
                    canvas_id: fid.clone(),
                });
            }) as crate::canvas::CanvasCommentResolveAllFn
        });
        let on_set_access = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(
                move |req: crate::canvas::ShareAccessRequest, _: &mut Window, _: &mut App| {
                    (send)(Command::SetCanvasAccess {
                        canvas_id: fid.clone(),
                        access_level: req.access_level,
                        channel_ids: req.channel_ids,
                        user_ids: req.user_ids,
                        post_permalink: req.post_permalink,
                    });
                },
            ) as crate::canvas::CanvasAccessSetFn
        });
        let on_delete_access = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |channel_ids, user_ids, _: &mut Window, _: &mut App| {
                (send)(Command::DeleteCanvasAccess {
                    canvas_id: fid.clone(),
                    channel_ids,
                    user_ids,
                });
            }) as crate::canvas::CanvasAccessDeleteFn
        });
        let on_delete_canvas = file_id.clone().map(|fid| {
            let send = self.send.clone();
            Rc::new(move |_: &mut Window, _: &mut App| {
                (send)(Command::DeleteCanvas {
                    canvas_id: fid.clone(),
                });
            }) as crate::canvas::CanvasDeleteFn
        });
        let channel_lookup = crate::canvas::channel_lookup_from_fixtures(self.fixtures.clone());
        let jump_ch = on_jump.clone();
        let on_open_channel: crate::canvas::CanvasOpenChannelFn =
            Rc::new(move |channel_id, _window, cx| {
                jump_ch(channel_id, None, false, cx);
            });
        let on_print_canvas: crate::canvas::CanvasExportFn =
            Rc::new(
                move |doc, window, cx| match crate::canvas::trigger_canvas_print(&doc) {
                    Ok(_) => actions::notify(
                        "Print canvas",
                        "Opened printable canvas preview.",
                        window,
                        cx,
                    ),
                    Err(err) => actions::notify("Print canvas", err, window, cx),
                },
            );
        let on_save_pdf: crate::canvas::CanvasExportFn = Rc::new(move |doc, window, cx| {
            let stem = crate::canvas::canvas_export_stem(&doc.title);
            let target = std::env::temp_dir().join(format!("{stem}.pdf"));
            match crate::canvas::save_canvas_pdf_to_path(&doc, &target) {
                Ok(saved) => actions::notify(
                    "Saved PDF",
                    format!("Exported to {}", saved.display()),
                    window,
                    cx,
                ),
                Err(err) => actions::notify("Save as PDF", err, window, cx),
            }
        });
        let on_upload_file: crate::canvas::CanvasUploadFileFn = {
            let send = self.send.clone();
            let upload_channel = self.fixtures.messages.keys().next().cloned();
            let is_live = file_id.is_some();
            Rc::new(move |on_done, window: &mut Window, cx: &mut App| {
                let rx = cx.prompt_for_paths(gpui_kit::gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Upload file to canvas".into()),
                });
                let send_u = send.clone();
                let ch_u = upload_channel.clone();
                window
                    .spawn(cx, async move |cx| {
                        let Ok(Ok(Some(paths))) = rx.await else {
                            return;
                        };
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let filename = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("attachment")
                            .to_owned();
                        let bytes = if is_live {
                            std::fs::read(&path).ok().filter(|b| !b.is_empty())
                        } else {
                            None
                        };
                        let url = format!("file://{}", path.display());
                        let _ = cx.update(|window, cx| {
                            if let (Some(ch), Some(bytes)) = (ch_u.clone(), bytes) {
                                (send_u)(Command::UploadFiles {
                                    channel: ch,
                                    thread_ts: None,
                                    content: None,
                                    files: vec![slackless_core::api::UploadFile {
                                        filename: filename.clone(),
                                        bytes,
                                    }],
                                });
                            }
                            on_done(
                                crate::canvas::UploadedCanvasFile {
                                    filename: filename.clone(),
                                    url,
                                },
                                window,
                                cx,
                            );
                            actions::notify(
                                "Inserted file",
                                format!("Added {filename} to canvas."),
                                window,
                                cx,
                            );
                        });
                    })
                    .detach();
            })
        };
        let user_lookup = crate::canvas::user_lookup_from_fixtures(self.fixtures.clone());
        let on_open_profile: crate::canvas::CanvasOpenProfileFn = {
            let fixtures = self.fixtures.clone();
            let send = self.send.clone();
            let is_live = file_id.is_some();
            Rc::new(move |user_id, window: &mut Window, cx: &mut App| {
                if is_live {
                    (send)(Command::FetchUserProfile(user_id.clone()));
                }
                if let Some(team) = fixtures.workspaces.first().filter(|t| !t.domain.is_empty()) {
                    cx.open_url(&format!(
                        "https://{}.slack.com/team/{}",
                        team.domain, user_id
                    ));
                } else {
                    let name = fixtures.user_name(&user_id).to_owned();
                    actions::notify("Profile", format!("User: {name}"), window, cx);
                }
            })
        };
        let on_open_embed: crate::canvas::CanvasOpenEmbedFn = {
            let fixtures = self.fixtures.clone();
            Rc::new(move |kind, fid, url, window: &mut Window, cx: &mut App| {
                if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                } else if let Some(team) =
                    fixtures.workspaces.first().filter(|t| !t.domain.is_empty())
                    && !fid.is_empty()
                {
                    let seg = match kind {
                        crate::canvas::CanvasEmbedKind::Canvas => "docs",
                        crate::canvas::CanvasEmbedKind::List => "lists",
                    };
                    cx.open_url(&format!(
                        "https://{}.slack.com/{seg}/{}/{fid}",
                        team.domain, team.id
                    ));
                } else {
                    actions::notify(
                        "Embedded link",
                        format!("Cannot open embedded item {fid}"),
                        window,
                        cx,
                    );
                }
            })
        };
        let on_open_media: crate::canvas::CanvasOpenMediaFn =
            Rc::new(move |url, _kind, window: &mut Window, cx: &mut App| {
                if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                } else {
                    actions::notify(
                        "Media link",
                        format!("Cannot open unsupported media URL: {url}"),
                        window,
                        cx,
                    );
                }
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
                on_upload_file: Some(on_upload_file),
                canvas_url: open_browser_url.clone(),
                on_open_media: Some(on_open_media),
                user_lookup: Some(user_lookup),
                channel_lookup: Some(channel_lookup),
                on_open_profile: Some(on_open_profile),
                on_open_channel: Some(on_open_channel),
                on_open_embed: Some(on_open_embed),
                on_load_comments,
                on_post_comment,
                on_resolve_comment,
                on_resolve_all_comments,
                on_set_access,
                on_delete_access,
                on_delete_canvas,
                on_print_canvas: Some(on_print_canvas),
                on_save_pdf: Some(on_save_pdf),
                ..Default::default()
            },
            file_id.is_none(),
            cx,
        )
    }

    fn render_files_body(
        &self,
        cx: &Context<Self>,
    ) -> gpui_kit::gpui::Stateful<gpui_kit::gpui::Div> {
        let theme = cx.theme();
        let (border, muted, accent) = (theme.border, theme.muted_foreground, theme.accent);
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
}

impl Render for PopoutWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let PopoutKind::Huddle {
            on_jump,
            on_open_canvas,
            ..
        } = &self.kind
        {
            // Immediately free replaced Metal sprite-atlas tiles on this owning window.
            if let Some(vs) = self.huddle_video.as_ref().and_then(WeakEntity::upgrade) {
                let stale = vs.update(cx, |store, _| store.drain_stale_images());
                for img in stale {
                    cx.drop_image(img, Some(window));
                }
            }
            let root = self
                .render_huddle_window(on_jump.clone(), on_open_canvas.clone(), cx)
                .on_action(|_: &crate::menu_bar::CloseWindow, window, _| {
                    window.remove_window();
                });
            return self
                .render_overlays(root, px(38.), window, cx)
                .into_any_element();
        }
        let theme = cx.theme();
        let (bg, fg) = (theme.background, theme.foreground);
        let jump_for_links = self.jump_handler();
        let links: LinkHandler = Rc::new(move |target, _, cx| match target {
            LinkTarget::Url(url) => cx.open_url(url.as_ref()),
            LinkTarget::Channel(channel) => jump_for_links(channel.clone(), None, false, cx),
            LinkTarget::Thread { channel, thread_ts } => {
                jump_for_links(channel.clone(), Some(thread_ts.clone()), true, cx)
            }
            LinkTarget::Profile(_) => {}
        });
        let header = self.render_header(cx);
        let body = match &self.kind {
            PopoutKind::Conversation {
                channel, on_jump, ..
            } => self.render_messages_body(channel.clone(), on_jump.clone(), true, &links, cx),
            PopoutKind::Thread {
                channel, on_jump, ..
            } => self.render_messages_body(channel.clone(), on_jump.clone(), false, &links, cx),
            PopoutKind::Activity { on_jump, .. } => self.render_activity_body(on_jump.clone(), cx),
            PopoutKind::Later { on_jump, .. } => self.render_later_body(on_jump.clone(), cx),
            PopoutKind::Canvas {
                doc,
                file_id,
                open_browser_url,
                on_jump,
            } => self.render_canvas_body(doc, file_id, open_browser_url, on_jump, cx),
            PopoutKind::Files { .. } => self.render_files_body(cx),
            PopoutKind::Huddle { .. } => v_flex().id("popout-huddle-noop").flex_1(),
        };

        let root = v_flex()
            .id("popout-window-root")
            .relative()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .on_action(|_: &crate::menu_bar::CloseWindow, window, _| {
                window.remove_window();
            })
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        if this.image_viewer.is_some() {
                            this.close_image_viewer(window, cx);
                        } else if this.picker.take().is_some() || this.editing.take().is_some() {
                            cx.notify();
                        }
                    }
                }),
            )
            .child(header)
            .child(body)
            .when_some(self.composer.clone(), |root, composer| {
                root.child(div().flex_none().p_2().child(composer))
            });
        self.render_overlays(root, px(40.), window, cx)
            .into_any_element()
    }
}

pub mod floating_controls;
mod huddle;
#[cfg(test)]
mod tests;

pub use floating_controls::open_huddle_floating_panel;
