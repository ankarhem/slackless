//! Popped-out secondary GPUI windows for a single conversation or thread (`F-CORE-010`,
//! `F-CORE-026`, `F-CORE-109`): lightweight native window hosting a live message transcript
//! and composer (or thread replies + reply composer) backed by the workspace's SQLite store
//! and sending commands through the main window's live [`CoreBridge`].

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Bounds, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, point, px, size,
};
use gpui_kit::open_window;
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::composer::{Composer, ComposerEvent};
use crate::fixtures::Fixtures;
use crate::messages::{LinkHandler, LinkTarget, message_row_linked};

/// What this popped-out native window displays.
#[derive(Clone)]
pub enum PopoutKind {
    Conversation(ChannelId),
    Thread { channel: ChannelId, thread_ts: Ts },
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
            PopoutKind::Conversation(_) => format!("Message {title}"),
            PopoutKind::Thread { .. } => "Reply in thread…".to_owned(),
        };
        let composer = Composer::new(placeholder.into(), fixtures.clone(), window, cx);
        let initial = load_messages(&kind, &fixtures, store.as_ref(), team.as_ref());
        let sub = cx.subscribe_in(
            &composer,
            window,
            |this: &mut Self, _, event: &ComposerEvent, _, cx| {
                if let ComposerEvent::Submit(content) = event {
                    this.send_content(content.clone(), cx);
                }
            },
        );
        // Periodically refresh from the shared SQLite store so live messages appear in the popout.
        if store.is_some() {
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
            composer,
            scroll,
            _subscriptions: vec![sub],
        }
    }

    fn reload_from_store(&mut self, cx: &mut Context<Self>) {
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
            PopoutKind::Conversation(ch) => (ch.clone(), None),
            PopoutKind::Thread { channel, thread_ts } => (channel.clone(), Some(thread_ts.clone())),
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
    if let (Some(store), Some(team)) = (store, team) {
        if let Ok(guard) = store.lock() {
            let rows = match kind {
                PopoutKind::Conversation(ch) => guard.channel_history(team, ch, None, 200),
                PopoutKind::Thread { channel, thread_ts } => {
                    guard.thread_replies(team, channel, thread_ts)
                }
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
        PopoutKind::Conversation(ch) => fixtures.messages.get(ch).cloned().unwrap_or_default(),
        PopoutKind::Thread { channel, thread_ts } => fixtures
            .threads
            .get(&(channel.clone(), thread_ts.clone()))
            .cloned()
            .unwrap_or_default(),
    }
}

impl Render for PopoutWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border, muted, fg) = (
            theme.background,
            theme.border,
            theme.muted_foreground,
            theme.foreground,
        );
        let links: LinkHandler = std::rc::Rc::new(|target, _, cx| {
            if let LinkTarget::Url(url) = target {
                cx.open_url(url.as_ref());
            }
        });
        let kind_label = match &self.kind {
            PopoutKind::Conversation(_) => "Conversation",
            PopoutKind::Thread { .. } => "Thread",
        };
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
        let rows: Vec<_> = self
            .messages
            .iter()
            .map(|msg| message_row_linked(&self.fixtures, msg, None, Some(&links), None, cx))
            .collect();
        let body = v_flex()
            .id("popout-messages")
            .flex_1()
            .min_h_0()
            .px_4()
            .py_3()
            .gap_2()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(rows);

        v_flex()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .child(header)
            .child(body)
            .child(div().flex_none().p_2().child(self.composer.clone()))
    }
}
