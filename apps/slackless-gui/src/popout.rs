//! Popped-out secondary GPUI windows for a conversation, thread, Activity feed, or Saved/Later
//! (`F-CORE-010`, `F-CORE-026`, `F-CORE-109`, `F-SHELL-149`): lightweight native window that
//! observes the main window's live entities for real-time updates and jumps back to the main
//! window when an Activity or Saved item is clicked.

use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::TitleBar;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::ContextMenuExt as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Bounds, ClipboardItem, Context, DisplayId, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, div, point, px, size,
};
use gpui_kit::open_window;
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Command, SharedStore};

use crate::actions::{self, ActionHandler, ActionSet, MenuOrigin, MessageAction};
use crate::activity::ActivityFeed;
use crate::composer::{Composer, ComposerEvent};
use crate::fixtures::Fixtures;
use crate::image_viewer::{self, ImageOrigin, ImageViewer, ViewerEvent};
use crate::later::{LaterItem, LaterView};
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
}

/// Callback that forwards commands (`SendMessage`, etc.) to the workspace's live `CoreBridge`.
pub type CommandForward = Arc<dyn Fn(Command) + Send + Sync>;

fn popout_window_options(title: &str, display_id: Option<DisplayId>, cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(display_id, size(px(720.), px(640.)), cx);
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

/// Open a new native GPUI window showing `kind`, centered on `display_id` (the main window's
/// display) or the primary display if `None`.
#[allow(clippy::too_many_arguments)]
pub fn open_popout(
    kind: PopoutKind,
    title: String,
    fixtures: Arc<Fixtures>,
    store: Option<SharedStore>,
    team: Option<TeamId>,
    send: CommandForward,
    display_id: Option<DisplayId>,
    cx: &mut App,
) {
    let options = popout_window_options(&title, display_id, cx);
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
    saved: HashSet<Ts>,
    activity_rows: Vec<PopoutActivityRow>,
    later_items: Vec<LaterItem>,
    composer: Entity<Composer>,
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
            saved: HashSet::new(),
            activity_rows,
            later_items,
            composer,
            scroll,
            image_viewer: None,
            _viewer_sub: None,
            _subscriptions: subscriptions,
        }
    }

    fn jump_handler(&self) -> JumpHandler {
        match &self.kind {
            PopoutKind::Conversation { on_jump, .. }
            | PopoutKind::Thread { on_jump, .. }
            | PopoutKind::Activity { on_jump, .. }
            | PopoutKind::Later { on_jump, .. } => on_jump.clone(),
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
            MessageAction::PickReaction(_) => {
                actions::notify(
                    "Emoji picker",
                    "Use quick reactions here, or open the main window for the full picker.",
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
                        channel,
                        ts,
                        date_due: None,
                    });
                } else {
                    self.saved.remove(&ts);
                    (self.send)(Command::DeleteSavedItem { channel, ts });
                }
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
                self.saved.insert(ts.clone());
                (self.send)(Command::SaveItem {
                    channel,
                    ts,
                    date_due: Some(remind.at(crate::drafts::now_secs())),
                });
                actions::notify("Reminder set", remind.label().to_lowercase(), window, cx);
                cx.notify();
            }
            MessageAction::RemindCustom => {
                let this = cx.entity().downgrade();
                crate::later::open_custom_remind_dialog(window, cx, move |due, window, cx| {
                    let _ = this.update(cx, |win, cx| {
                        win.saved.insert(ts.clone());
                        (win.send)(Command::SaveItem {
                            channel: channel.clone(),
                            ts: ts.clone(),
                            date_due: Some(due),
                        });
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
                (self.send)(Command::MarkRead { channel, ts });
                actions::notify("Marked unread", String::new(), window, cx);
            }
            MessageAction::Edit | MessageAction::ViewHistory => {
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
                let mut rows = Vec::with_capacity(self.messages.len());
                let mut prev_msg: Option<&Message> = None;
                for (ix, msg) in self.messages.iter().enumerate() {
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
                    let toolbar = actions::hover_toolbar(
                        &key,
                        &self.fixtures.quick_reactions,
                        set,
                        &handler,
                        msg_group.clone(),
                        cx,
                    )
                    .when(ix == 0, |bar| bar.top(px(3.)));
                    let menu_handler = handler.clone();

                    let mut item = v_flex()
                        .id(SharedString::from(key))
                        .group(msg_group.clone())
                        .relative()
                        .px_4()
                        .py_0p5()
                        .when(is_saved, |row| {
                            row.bg(saved_bg).child(actions::saved_banner(cx))
                        })
                        .when(!is_saved, |row| row.hover(move |r| r.bg(hover_bg)))
                        .child(message_row_linked(
                            &self.fixtures,
                            &display,
                            Some(&handler),
                            Some(&links),
                            grouped,
                            cx,
                        ))
                        .children(chips)
                        .child(toolbar)
                        .context_menu(move |menu, window, cx| {
                            actions::actions_menu(
                                menu,
                                set,
                                &menu_handler,
                                MenuOrigin::Context,
                                window,
                                cx,
                            )
                        });
                    if is_conversation {
                        let jump = on_jump.clone();
                        let ch = channel.clone();
                        let ts = msg.ts.clone();
                        let reply_group = SharedString::from(format!("popout-reply-{ix}"));
                        if let Some(bar) = reply_bar(
                            &self.fixtures,
                            msg,
                            ("popout-thread-chip", ix),
                            msg_group,
                            reply_group,
                            move |_, cx| jump(ch.clone(), Some(ts.clone()), true, cx),
                            cx,
                        ) {
                            item = item.child(div().pl(px(48.)).child(bar));
                        }
                    }
                    rows.push(item);
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
        };

        let mut root = v_flex()
            .relative()
            .size_full()
            .bg(bg)
            .text_color(fg)
            .on_key_down(
                cx.listener(|this, event: &gpui_kit::gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" && this.image_viewer.is_some() {
                        this.close_image_viewer(cx);
                    }
                }),
            )
            .child(header)
            .child(body);
        if has_composer {
            root = root.child(div().flex_none().p_2().child(self.composer.clone()));
        }
        root.when_some(self.image_viewer.clone(), |root, viewer| {
            root.child(
                div()
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
            vec!["add:eyes", "rm:eyes", "save", "unsave", "pin:true"]
        );
        assert_eq!(*jumped.lock().unwrap(), vec![(channel, Some(ts), true)]);
    }

    #[gpui::test]
    fn open_popout_targets_the_requested_display(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let second_display = DisplayId::from(2u64);
        cx.simulate_display_added(second_display, None);

        cx.update(|cx| {
            let opts = popout_window_options("# general", Some(second_display), cx);
            assert_eq!(opts.display_id, Some(second_display));
            assert!(opts.window_bounds.is_some());
            assert!(opts.titlebar.is_some());

            let default_opts = popout_window_options("# general", None, cx);
            assert_eq!(default_opts.display_id, None);
        });
    }
}
