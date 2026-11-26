//! Local Catch-Up / Daily Briefing view: fast local-store summary of direct mentions, unread
//! channels with counts & top excerpts, active threads, and newly transcribed clips/huddles
//! across the active workspace or all signed-in workspaces.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::catchup::{
    CatchUpBriefing, CatchUpMention, CatchUpThread, CatchUpTranscript, CatchUpUnreadChannel,
    CatchUpWindow, MentionKind, TranscriptKind,
};
use slackless_core::types::{ChannelId, TeamId, Ts};

/// Events emitted upward from [`CatchUpView`] to the root [`crate::app::App`].
pub enum CatchUpEvent {
    Close,
    OpenChannel {
        workspace: TeamId,
        channel: ChannelId,
        ts: Option<Ts>,
    },
    OpenThread {
        workspace: TeamId,
        channel: ChannelId,
        thread_ts: Ts,
    },
    OpenHuddleTranscript {
        workspace: TeamId,
        call_id: String,
        channel: Option<ChannelId>,
    },
    WindowChanged(CatchUpWindow),
    ScopeChanged(bool),
    Refresh,
    MarkCaughtUp,
    Summarize,
}

/// Interactive Local Catch-Up / Daily Briefing main-column page.
pub struct CatchUpView {
    briefing: CatchUpBriefing,
    window: CatchUpWindow,
    all_workspaces: bool,
    multi_workspace_available: bool,
    loading: bool,
    error: Option<String>,
}

impl EventEmitter<CatchUpEvent> for CatchUpView {}

impl CatchUpView {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let now = slackless_core::types::time::now_epoch_secs();
        cx.new(|_| Self {
            briefing: CatchUpBriefing::empty(CatchUpWindow::SinceLastVisit, now),
            window: CatchUpWindow::SinceLastVisit,
            all_workspaces: false,
            multi_workspace_available: false,
            loading: false,
            error: None,
        })
    }

    pub fn window(&self) -> CatchUpWindow {
        self.window
    }

    pub fn all_workspaces(&self) -> bool {
        self.all_workspaces
    }

    pub fn set_multi_workspace_available(&mut self, available: bool, cx: &mut Context<Self>) {
        if self.multi_workspace_available != available {
            self.multi_workspace_available = available;
            if !available && self.all_workspaces {
                self.all_workspaces = false;
            }
            cx.notify();
        }
    }

    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.loading = loading;
        if loading {
            self.error = None;
        }
        cx.notify();
    }

    pub fn set_briefing(&mut self, briefing: CatchUpBriefing, cx: &mut Context<Self>) {
        self.briefing = briefing;
        self.loading = false;
        self.error = None;
        cx.notify();
    }

    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        self.loading = false;
        cx.notify();
    }

    pub fn select_window(&mut self, window: CatchUpWindow, cx: &mut Context<Self>) {
        if self.window != window {
            self.window = window;
            cx.emit(CatchUpEvent::WindowChanged(window));
            cx.notify();
        }
    }

    pub fn select_scope(&mut self, all_workspaces: bool, cx: &mut Context<Self>) {
        if self.all_workspaces != all_workspaces {
            self.all_workspaces = all_workspaces;
            cx.emit(CatchUpEvent::ScopeChanged(all_workspaces));
            cx.notify();
        }
    }

    pub fn current_briefing(&self) -> CatchUpBriefing {
        self.briefing.clone()
    }

    #[cfg(test)]
    pub fn briefing(&self) -> &CatchUpBriefing {
        &self.briefing
    }

    fn format_relative(ts_secs: i64, now_secs: i64) -> String {
        let delta = now_secs.saturating_sub(ts_secs).max(0);
        if delta < 60 {
            "just now".to_owned()
        } else if delta < 3_600 {
            format!("{}m ago", delta / 60)
        } else if delta < 86_400 {
            format!("{}h ago", delta / 3_600)
        } else {
            format!("{}d ago", delta / 86_400)
        }
    }

    fn render_section_header(icon: IconName, title: &'static str, count: usize, cx: &App) -> Div {
        let theme = cx.theme();
        h_flex().items_center().justify_between().pb_1().child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(icon)
                        .size(px(14.))
                        .text_color(theme.muted_foreground),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(title.to_uppercase()),
                )
                .child(
                    div()
                        .px_1p5()
                        .py(px(1.))
                        .rounded_full()
                        .bg(theme.secondary)
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground)
                        .child(count.to_string()),
                ),
        )
    }

    fn render_mention_row(
        &self,
        ix: usize,
        m: &CatchUpMention,
        show_workspace: bool,
        now_secs: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let hover_bg = theme.secondary.opacity(0.65);
        let badge_color = match m.kind {
            MentionKind::DirectMention => theme.danger,
            MentionKind::DirectMessage => theme.primary,
            MentionKind::Broadcast => theme.warning,
        };
        let ws = m.workspace.clone();
        let ch = m.channel.clone();
        let ts = m.ts.clone();
        let thread_ts = m.thread_ts.clone();
        let ws_label = m
            .workspace_name
            .clone()
            .unwrap_or_else(|| m.workspace.as_str().to_owned());

        v_flex()
            .id(SharedString::from(format!("catchup-mention-{ix}")))
            .w_full()
            .min_w_0()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .border_1()
            .border_color(border.opacity(0.7))
            .bg(theme.background)
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .on_click(cx.listener(move |_, _, _, cx| {
                if let Some(root_ts) = thread_ts.clone() {
                    cx.emit(CatchUpEvent::OpenThread {
                        workspace: ws.clone(),
                        channel: ch.clone(),
                        thread_ts: root_ts,
                    });
                } else {
                    cx.emit(CatchUpEvent::OpenChannel {
                        workspace: ws.clone(),
                        channel: ch.clone(),
                        ts: Some(ts.clone()),
                    });
                }
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .truncate()
                                    .child(m.author_name.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .truncate()
                                    .child(format!("in {}", m.channel_label)),
                            )
                            .when(show_workspace, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(theme.secondary)
                                        .text_xs()
                                        .text_color(muted)
                                        .child(ws_label),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(badge_color.opacity(0.16))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(badge_color)
                                    .child(m.kind.label()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(Self::format_relative(m.created_at, now_secs)),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(fg.opacity(0.9))
                    .truncate()
                    .child(m.excerpt.clone()),
            )
            .into_any_element()
    }

    fn render_unread_card(
        &self,
        ix: usize,
        u: &CatchUpUnreadChannel,
        show_workspace: bool,
        now_secs: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let hover_bg = theme.secondary.opacity(0.65);
        let ws = u.workspace.clone();
        let ch = u.channel.clone();
        let latest_ts = u.latest_ts.clone();
        let ws_label = u
            .workspace_name
            .clone()
            .unwrap_or_else(|| u.workspace.as_str().to_owned());

        v_flex()
            .id(SharedString::from(format!("catchup-unread-{ix}")))
            .w_full()
            .min_w_0()
            .gap_1p5()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .border_1()
            .border_color(border.opacity(0.7))
            .bg(theme.background)
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(CatchUpEvent::OpenChannel {
                    workspace: ws.clone(),
                    channel: ch.clone(),
                    ts: Some(latest_ts.clone()),
                });
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(if u.is_dm {
                                    IconName::User
                                } else {
                                    IconName::Hash
                                })
                                .flex_none()
                                .size(px(13.))
                                .text_color(muted),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .truncate()
                                    .child(u.channel_label.clone()),
                            )
                            .when(show_workspace, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(theme.secondary)
                                        .text_xs()
                                        .text_color(muted)
                                        .child(ws_label),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .when(u.mention_count > 0, |row| {
                                row.child(
                                    div()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded_full()
                                        .bg(theme.danger)
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.danger_foreground)
                                        .child(format!("@{}", u.mention_count)),
                                )
                            })
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(theme.secondary)
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(fg)
                                    .child(format!(
                                        "{} {}",
                                        u.message_count,
                                        if u.message_count == 1 { "msg" } else { "msgs" }
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(Self::format_relative(u.latest_at, now_secs)),
                            ),
                    ),
            )
            .children(u.excerpts.iter().map(|ex| {
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_baseline()
                    .gap_1p5()
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(format!("{}:", ex.author_name)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .text_color(fg.opacity(0.88))
                            .truncate()
                            .child(ex.text.clone()),
                    )
            }))
            .into_any_element()
    }

    fn render_thread_row(
        &self,
        ix: usize,
        t: &CatchUpThread,
        show_workspace: bool,
        now_secs: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let hover_bg = theme.secondary.opacity(0.65);
        let ws = t.workspace.clone();
        let ch = t.channel.clone();
        let thread_ts = t.thread_ts.clone();
        let ws_label = t
            .workspace_name
            .clone()
            .unwrap_or_else(|| t.workspace.as_str().to_owned());

        v_flex()
            .id(SharedString::from(format!("catchup-thread-{ix}")))
            .w_full()
            .min_w_0()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .border_1()
            .border_color(border.opacity(0.7))
            .bg(theme.background)
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(CatchUpEvent::OpenThread {
                    workspace: ws.clone(),
                    channel: ch.clone(),
                    thread_ts: thread_ts.clone(),
                });
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .truncate()
                                    .child(t.channel_label.clone()),
                            )
                            .when(show_workspace, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(theme.secondary)
                                        .text_xs()
                                        .text_color(muted)
                                        .child(ws_label),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(theme.primary.opacity(0.16))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.primary)
                                    .child(format!(
                                        "+{} new ({} total)",
                                        t.new_reply_count, t.total_reply_count
                                    )),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(Self::format_relative(t.latest_reply_at, now_secs)),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .text_color(muted)
                    .truncate()
                    .child(format!("{}: {}", t.root_author, t.root_excerpt)),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(fg.opacity(0.92))
                    .truncate()
                    .child(format!(
                        "Latest \u{2014} {}: {}",
                        t.latest_reply_author, t.latest_reply_excerpt
                    )),
            )
            .into_any_element()
    }

    fn render_transcript_row(
        &self,
        ix: usize,
        tr: &CatchUpTranscript,
        show_workspace: bool,
        now_secs: i64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let hover_bg = theme.secondary.opacity(0.65);
        let icon = match tr.kind {
            TranscriptKind::FileClip => IconName::Video,
            TranscriptKind::Huddle => IconName::Headphones,
        };
        let ws = tr.workspace.clone();
        let ch = tr.channel.clone();
        let ts = tr.message_ts.clone();
        let call_id = tr.call_id.clone();
        let kind = tr.kind;
        let ws_label = tr
            .workspace_name
            .clone()
            .unwrap_or_else(|| tr.workspace.as_str().to_owned());

        v_flex()
            .id(SharedString::from(format!("catchup-transcript-{ix}")))
            .w_full()
            .min_w_0()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(8.))
            .border_1()
            .border_color(border.opacity(0.7))
            .bg(theme.background)
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .on_click(cx.listener(move |_, _, _, cx| match kind {
                TranscriptKind::Huddle => {
                    if let Some(cid) = call_id.clone() {
                        cx.emit(CatchUpEvent::OpenHuddleTranscript {
                            workspace: ws.clone(),
                            call_id: cid,
                            channel: ch.clone(),
                        });
                    } else if let Some(channel) = ch.clone() {
                        cx.emit(CatchUpEvent::OpenChannel {
                            workspace: ws.clone(),
                            channel,
                            ts: ts.clone(),
                        });
                    }
                }
                TranscriptKind::FileClip => {
                    if let Some(channel) = ch.clone() {
                        cx.emit(CatchUpEvent::OpenChannel {
                            workspace: ws.clone(),
                            channel,
                            ts: ts.clone(),
                        });
                    }
                }
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(icon)
                                    .flex_none()
                                    .size(px(14.))
                                    .text_color(theme.primary),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .truncate()
                                    .child(tr.title.clone()),
                            )
                            .when_some(tr.channel_label.clone(), |row, label| {
                                row.child(
                                    div()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(format!("in {label}")),
                                )
                            })
                            .when(show_workspace, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(theme.secondary)
                                        .text_xs()
                                        .text_color(muted)
                                        .child(ws_label),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.))
                                    .rounded_full()
                                    .bg(theme.secondary)
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("{} \u{00B7} {}", tr.kind.label(), tr.source)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(Self::format_relative(tr.updated_at, now_secs)),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_sm()
                    .text_color(fg.opacity(0.9))
                    .truncate()
                    .child(tr.excerpt.clone()),
            )
            .into_any_element()
    }
}

impl Render for CatchUpView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.background;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let secondary = theme.secondary;

        let now_secs = self.briefing.generated_at_secs;
        let show_workspace = self.all_workspaces && self.briefing.workspaces.len() > 1;
        let ws_summary = if self.briefing.workspaces.is_empty() {
            "Local archive".to_owned()
        } else if self.briefing.workspaces.len() == 1 {
            self.briefing.workspaces[0]
                .name
                .clone()
                .unwrap_or_else(|| self.briefing.workspaces[0].team_id.as_str().to_owned())
        } else {
            format!("{} workspaces", self.briefing.workspaces.len())
        };
        let subtitle = format!(
            "{} \u{00B7} {} \u{00B7} {} mentions, {} unread channels, {} threads, {} transcripts",
            ws_summary,
            self.window.label(),
            self.briefing.mentions.len(),
            self.briefing.unread_channels.len(),
            self.briefing.active_threads.len(),
            self.briefing.transcripts.len(),
        );

        let window_buttons = [
            (
                CatchUpWindow::SinceLastVisit,
                "catchup-win-visit",
                "Since last visit",
            ),
            (CatchUpWindow::Last24h, "catchup-win-24h", "Last 24h"),
            (CatchUpWindow::Last7d, "catchup-win-7d", "Last 7d"),
        ];

        let active_window = self.window;
        let all_ws = self.all_workspaces;

        let header = v_flex()
            .flex_none()
            .w_full()
            .gap_2()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Sparkles)
                                    .flex_none()
                                    .size(px(18.))
                                    .text_color(theme.primary),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(fg)
                                            .child("Catch-Up"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .truncate()
                                            .child(subtitle),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("catchup-summarize")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::Sparkles)
                                    .label("Summarize")
                                    .tooltip("Summarize Catch-Up briefing")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(CatchUpEvent::Summarize);
                                    })),
                            )
                            .child(
                                Button::new("catchup-refresh")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::RefreshCw)
                                    .tooltip("Refresh briefing")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(CatchUpEvent::Refresh);
                                    })),
                            )
                            .child(
                                Button::new("catchup-mark-caught-up")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(IconName::CheckCheck)
                                    .label("Mark caught up")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(CatchUpEvent::MarkCaughtUp);
                                    })),
                            )
                            .child(
                                Button::new("catchup-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::X)
                                    .tooltip("Close Catch-Up")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(CatchUpEvent::Close);
                                    })),
                            ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .flex_wrap()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .p(px(2.))
                            .rounded(px(7.))
                            .bg(secondary.opacity(0.6))
                            .children(window_buttons.into_iter().map(|(win, id, label)| {
                                let selected = active_window == win;
                                let btn = Button::new(id)
                                    .with_size(Size::XSmall)
                                    .label(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_window(win, cx);
                                    }));
                                if selected { btn.primary() } else { btn.ghost() }
                            })),
                    )
                    .when(self.multi_workspace_available, |row| {
                        let btn_this = Button::new("catchup-scope-this")
                            .with_size(Size::XSmall)
                            .label("This workspace")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_scope(false, cx);
                            }));
                        let btn_all = Button::new("catchup-scope-all")
                            .with_size(Size::XSmall)
                            .label("All workspaces")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.select_scope(true, cx);
                            }));
                        row.child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .p(px(2.))
                                .rounded(px(7.))
                                .bg(secondary.opacity(0.6))
                                .child(if !all_ws {
                                    btn_this.primary()
                                } else {
                                    btn_this.ghost()
                                })
                                .child(if all_ws {
                                    btn_all.primary()
                                } else {
                                    btn_all.ghost()
                                }),
                        )
                    }),
            );

        let body = if self.loading {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(
                    Icon::new(IconName::RefreshCw)
                        .size(px(22.))
                        .text_color(muted),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("Building your Catch-Up briefing from local archive\u{2026}"),
                )
                .into_any_element()
        } else if let Some(err) = &self.error {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .size(px(22.))
                        .text_color(theme.danger),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(fg)
                        .child("Could not build Catch-Up"),
                )
                .child(div().text_xs().text_color(muted).child(err.clone()))
                .into_any_element()
        } else if self.briefing.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .p_8()
                .child(
                    Icon::new(IconName::CheckCheck)
                        .size(px(26.))
                        .text_color(theme.primary),
                )
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child("You\u{2019}re all caught up"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("No new mentions, unread channels, active threads, or transcripts in this window."),
                )
                .into_any_element()
        } else {
            let mut mentions = Vec::with_capacity(self.briefing.mentions.len());
            for (ix, m) in self.briefing.mentions.iter().enumerate() {
                mentions.push(self.render_mention_row(ix, m, show_workspace, now_secs, cx));
            }
            let mut unreads = Vec::with_capacity(self.briefing.unread_channels.len());
            for (ix, u) in self.briefing.unread_channels.iter().enumerate() {
                unreads.push(self.render_unread_card(ix, u, show_workspace, now_secs, cx));
            }
            let mut threads = Vec::with_capacity(self.briefing.active_threads.len());
            for (ix, t) in self.briefing.active_threads.iter().enumerate() {
                threads.push(self.render_thread_row(ix, t, show_workspace, now_secs, cx));
            }
            let mut transcripts = Vec::with_capacity(self.briefing.transcripts.len());
            for (ix, tr) in self.briefing.transcripts.iter().enumerate() {
                transcripts.push(self.render_transcript_row(ix, tr, show_workspace, now_secs, cx));
            }

            v_flex()
                .id("catchup-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_5()
                .py_4()
                .gap_5()
                .when(!mentions.is_empty(), |col| {
                    col.child(
                        v_flex()
                            .gap_2()
                            .child(Self::render_section_header(
                                IconName::Bell,
                                "Direct mentions & DMs",
                                mentions.len(),
                                cx,
                            ))
                            .children(mentions),
                    )
                })
                .when(!unreads.is_empty(), |col| {
                    col.child(
                        v_flex()
                            .gap_2()
                            .child(Self::render_section_header(
                                IconName::Inbox,
                                "Unread channels",
                                unreads.len(),
                                cx,
                            ))
                            .children(unreads),
                    )
                })
                .when(!threads.is_empty(), |col| {
                    col.child(
                        v_flex()
                            .gap_2()
                            .child(Self::render_section_header(
                                IconName::MessageSquareText,
                                "Active threads",
                                threads.len(),
                                cx,
                            ))
                            .children(threads),
                    )
                })
                .when(!transcripts.is_empty(), |col| {
                    col.child(
                        v_flex()
                            .gap_2()
                            .child(Self::render_section_header(
                                IconName::Headphones,
                                "New transcripts",
                                transcripts.len(),
                                cx,
                            ))
                            .children(transcripts),
                    )
                })
                .into_any_element()
        };

        v_flex()
            .id("catchup-view")
            .size_full()
            .min_w_0()
            .bg(bg)
            .child(header)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::gpui::TestAppContext;
    use slackless_core::catchup::{
        CatchUpExcerpt, CatchUpMention, CatchUpThread, CatchUpTranscript, CatchUpUnreadChannel,
        CatchUpWorkspaceSummary,
    };

    use super::*;

    #[gpui_kit::gpui::test]
    fn catchup_view_emits_window_and_scope_changes_and_renders_briefing(cx: &mut TestAppContext) {
        let view = cx.update(CatchUpView::new);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let events_sub = events.clone();
        cx.update(|cx| {
            let _sub = cx.subscribe(&view, move |_, event, _| match event {
                CatchUpEvent::WindowChanged(w) => {
                    events_sub
                        .borrow_mut()
                        .push(format!("window:{}", w.label()));
                }
                CatchUpEvent::ScopeChanged(all) => {
                    events_sub.borrow_mut().push(format!("scope:{all}"));
                }
                _ => {}
            });
            _sub.detach();
        });

        let now = 1_750_000_000;
        let briefing = CatchUpBriefing {
            generated_at_secs: now,
            window: CatchUpWindow::Last24h,
            cutoff_secs: now - 86_400,
            workspaces: vec![CatchUpWorkspaceSummary {
                team_id: TeamId::from("T1"),
                name: Some("Acme".into()),
                cutoff_secs: now - 86_400,
            }],
            mentions: vec![CatchUpMention {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                channel: ChannelId::from("C1"),
                channel_label: "#general".into(),
                ts: Ts::from("1749999000.000100"),
                thread_ts: None,
                author_id: None,
                author_name: "Alice".into(),
                excerpt: "Check the release notes".into(),
                created_at: now - 1000,
                kind: MentionKind::DirectMention,
            }],
            unread_channels: vec![CatchUpUnreadChannel {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                channel: ChannelId::from("C1"),
                channel_label: "#general".into(),
                is_dm: false,
                message_count: 3,
                mention_count: 1,
                latest_ts: Ts::from("1749999000.000100"),
                latest_at: now - 1000,
                excerpts: vec![CatchUpExcerpt {
                    ts: Ts::from("1749999000.000100"),
                    author_name: "Alice".into(),
                    text: "Check the release notes".into(),
                    created_at: now - 1000,
                }],
            }],
            active_threads: vec![CatchUpThread {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                channel: ChannelId::from("C1"),
                channel_label: "#general".into(),
                thread_ts: Ts::from("1749998000.000100"),
                root_author: "Bob".into(),
                root_excerpt: "Deploy plan".into(),
                new_reply_count: 2,
                total_reply_count: 5,
                latest_reply_ts: Ts::from("1749999500.000100"),
                latest_reply_author: "Alice".into(),
                latest_reply_excerpt: "Green on staging".into(),
                latest_reply_at: now - 500,
            }],
            transcripts: vec![CatchUpTranscript {
                workspace: TeamId::from("T1"),
                workspace_name: Some("Acme".into()),
                kind: TranscriptKind::Huddle,
                channel: Some(ChannelId::from("C1")),
                channel_label: Some("#general".into()),
                message_ts: None,
                file_id: None,
                call_id: Some("R1".into()),
                title: "Huddle in #general".into(),
                source: "local".into(),
                excerpt: "Agreed on Friday launch".into(),
                updated_at: now - 200,
            }],
        };

        view.update(cx, |v, cx| {
            v.set_multi_workspace_available(true, cx);
            v.select_window(CatchUpWindow::Last7d, cx);
            v.select_scope(true, cx);
            v.set_briefing(briefing, cx);
        });

        view.read_with(cx, |v, _| {
            assert_eq!(v.window(), CatchUpWindow::Last7d);
            assert!(v.all_workspaces());
            assert_eq!(v.briefing().mentions.len(), 1);
            assert_eq!(v.briefing().unread_channels.len(), 1);
            assert_eq!(v.briefing().active_threads.len(), 1);
            assert_eq!(v.briefing().transcripts.len(), 1);
        });
        assert_eq!(
            *events.borrow(),
            vec!["window:Last 7d".to_owned(), "scope:true".to_owned()]
        );
    }
}
