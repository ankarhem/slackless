//! Popped-out secondary GPUI windows for a conversation, thread, Activity feed, Saved/Later,
//! Canvas, Files, or native Huddle (`F-CORE-010`, `F-CORE-026`, `F-CORE-109`, `F-SHELL-149`):
//! lightweight native window that observes the main window's live entities for real-time updates
//! and jumps back to the main window when an Activity or Saved item is clicked.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::TitleBar;
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::{ActiveTheme as _, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Bounds, Context, DisplayId, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString, Styled as _,
    Subscription, TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div, point, px,
    size,
};
use gpui_kit::open_window;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::activity::ActivityFeed;
use crate::composer::{self, Composer, ComposerEvent};
use crate::emoji::{EmojiPicker, EmojiPickerEvent};
use crate::fixtures::Fixtures;
use crate::huddle_state::{HuddlePopoutGuard, HuddleState, HuddleVideoStore, HuddleViewState};
use crate::image_viewer::ImageViewer;
use crate::later::{LaterItem, LaterView};
use crate::messages::{LinkHandler, LinkTarget, MessageList};

mod actions;
pub mod floating_controls;
mod huddle;
mod render;
mod sync;
#[cfg(test)]
mod tests;

pub use floating_controls::open_huddle_floating_panel;

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
    /// Popped-out native Huddle window (`F-COLLAB-001..059`): observes [`HuddleState`] directly
    /// for live participant tiles, active speaker rings, audio controls, device switcher, invite
    /// picker, thread & notes canvas links.
    Huddle {
        source: WeakEntity<HuddleState>,
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

pub(super) enum PopoutPickerTarget {
    Composer,
    Message { channel: ChannelId, ts: Ts },
}

/// Side-by-side right-hand drawer inside the Pop-out Huddle Window (`PopoutKind::Huddle`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HuddleDrawer {
    Thread,
    Notes,
    Diagnostics,
}

/// Active popover or modal inside the Pop-out Huddle Window (mutually exclusive).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HuddlePopover {
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
pub(super) enum HuddleShareTab {
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
    huddle_state: Option<HuddleViewState>,
    huddle_video: Option<WeakEntity<HuddleVideoStore>>,
    _huddle_popout_guard: Option<HuddlePopoutGuard>,
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
            sync::load_fixture_messages(&kind, &fixtures)
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
                if let Some(state_entity) = source.upgrade() {
                    let st = state_entity.read(cx).view_state();
                    let video_store = state_entity.read(cx).video_store();
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
                    subscriptions.push(cx.observe(&state_entity, |this, state_entity, cx| {
                        let st = state_entity.read(cx).view_state();
                        let (channel_changed, root_changed) =
                            this.huddle_state.as_ref().map_or((true, true), |prev| {
                                (
                                    prev.session.channel != st.session.channel,
                                    prev.session.thread_root_ts != st.session.thread_root_ts,
                                )
                            });
                        if channel_changed {
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
                        // Audio levels alone notify ~8x/s: only a new channel or thread root
                        // changes the query (new replies arrive through the revision poll).
                        if (channel_changed || root_changed)
                            && this.huddle_drawer == Some(HuddleDrawer::Thread)
                        {
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
                    if let Some(viewer) = this.image_viewer.clone() {
                        match viewer
                            .update(cx, |v, cx| v.handle_keystroke(&event.keystroke, window, cx))
                        {
                            crate::image_viewer::ViewerKeyOutcome::Close => {
                                this.close_image_viewer(window, cx);
                                cx.stop_propagation();
                            }
                            crate::image_viewer::ViewerKeyOutcome::Handled => {
                                cx.stop_propagation();
                            }
                            crate::image_viewer::ViewerKeyOutcome::Ignored => {}
                        }
                    } else if event.keystroke.key == "escape"
                        && (this.picker.take().is_some() || this.editing.take().is_some())
                    {
                        cx.notify();
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
