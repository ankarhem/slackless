//! Popped-out secondary GPUI windows for a conversation, thread, Activity feed, or Saved/Later
//! (`F-CORE-010`, `F-CORE-026`, `F-CORE-109`, `F-SHELL-149`): lightweight native window that
//! observes the main window's live entities for real-time updates and jumps back to the main
//! window when an Activity or Saved item is clicked.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_kit::open_window;
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::activity::ActivityFeed;
use crate::composer::{Composer, ComposerEvent};
use crate::fixtures::Fixtures;
use crate::later::{LaterItem, LaterView};
use crate::messages::{LinkHandler, LinkTarget, MessageList, message_row_linked};

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
    },
    Thread {
        channel: ChannelId,
        thread_ts: Ts,
        source: WeakEntity<MessageList>,
    },
    Activity {
        source: WeakEntity<ActivityFeed>,
        on_jump: JumpHandler,
    },
    Later {
        source: WeakEntity<LaterView>,
        on_jump: JumpHandler,
    },
}

/// Callback that forwards commands (`SendMessage`, etc.) to the workspace's live `CoreBridge`.
pub type CommandForward = Arc<dyn Fn(Command) + Send + Sync>;

/// Open a new native GPUI window showing `kind`.
pub fn open_popout(
    kind: PopoutKind,
    title: String,
    fixtures: Arc<Fixtures>,
    store: Option<SharedStore>,
    team: Option<TeamId>,
    send: CommandForward,
    cx: &mut App,
) {
    let bounds = Bounds::centered(None, size(px(720.), px(640.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from(title.clone())),
            traffic_light_position: Some(point(px(8.), px(12.))),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    };
    let _ = open_window(options, cx, move |window, cx| {
        window.set_window_title(&format!("{title} — slackless"));
        cx.new(|cx| PopoutWindow::new(kind, title, fixtures, store, team, send, window, cx))
    });
}

pub struct PopoutWindow {
    kind: PopoutKind,
    title: String,
    fixtures: Arc<Fixtures>,
    store: Option<SharedStore>,
    team: Option<TeamId>,
    send: CommandForward,
    messages: Vec<Message>,
    activity_rows: Vec<PopoutActivityRow>,
    later_items: Vec<LaterItem>,
    composer: Entity<Composer>,
    scroll: ScrollHandle,
    _subscriptions: Vec<gpui_kit::gpui::Subscription>,
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder = match &kind {
            PopoutKind::Conversation { .. } => format!("Message {title}"),
            PopoutKind::Thread { .. } => "Reply in thread…".to_owned(),
            PopoutKind::Activity { .. } | PopoutKind::Later { .. } => String::new(),
        };
        let has_composer = matches!(
            &kind,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. }
        );
        let composer = Composer::new(placeholder.into(), fixtures.clone(), window, cx);
        let initial = load_messages(&kind, &fixtures, store.as_ref(), team.as_ref());
        let mut subscriptions = vec![cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event: &ComposerEvent, _, cx| {
                if let ComposerEvent::Submit(content) = event {
                    this.send_content(content.clone(), cx);
                }
            },
        )];
        let mut activity_rows = Vec::new();
        let mut later_items = Vec::new();
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
            messages: initial,
            activity_rows,
            later_items,
            composer,
            scroll,
            _subscriptions: subscriptions,
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
            PopoutKind::Activity { .. } | PopoutKind::Later { .. } => return,
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
    if matches!(kind, PopoutKind::Activity { .. } | PopoutKind::Later { .. }) {
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
                PopoutKind::Activity { .. } | PopoutKind::Later { .. } => Ok(Vec::new()),
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
        PopoutKind::Activity { .. } | PopoutKind::Later { .. } => Vec::new(),
    }
}

impl Render for PopoutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted, fg, accent) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.accent,
        );
        let links: LinkHandler = Rc::new(|target, _, cx| {
            if let LinkTarget::Url(url) = target {
                cx.open_url(url.as_ref());
            }
        });
        let kind_label = match &self.kind {
            PopoutKind::Conversation { .. } => "Conversation",
            PopoutKind::Thread { .. } => "Thread",
            PopoutKind::Activity { .. } => "Activity · Live",
            PopoutKind::Later { .. } => "Later · Live",
        };
        let has_composer = matches!(
            &self.kind,
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. }
        );
        let header = h_flex()
            .flex_none()
            .h(px(40.))
            .pl(px(76.))
            .pr_3()
            .items_center()
            .justify_between()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child(self.title.clone()),
                    )
                    .child(div().text_xs().text_color(muted).child(kind_label)),
            )
            .child(
                Button::new("popout-refresh")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.reload_from_store(cx))),
            );
        let body = match &self.kind {
            PopoutKind::Conversation { .. } | PopoutKind::Thread { .. } => {
                let rows: Vec<_> = self
                    .messages
                    .iter()
                    .map(|msg| {
                        message_row_linked(&self.fixtures, msg, None, Some(&links), None, cx)
                    })
                    .collect();
                v_flex()
                    .id("popout-messages")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .py_3()
                    .gap_2()
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
        };

        let mut root = v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(header)
            .child(body);
        if has_composer {
            root = root.child(div().flex_none().p_2().child(self.composer.clone()));
        }
        root
    }
}
