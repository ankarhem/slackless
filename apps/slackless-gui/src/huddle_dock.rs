//! Persistent sidebar mini-dock widget for an active native Slack Huddle (`F-COLLAB-001..059`).
//!
//! Thin presentation layer observing [`crate::huddle_state::HuddleState`] and pinned at the bottom
//! of the main window so the user can switch channels, threads, DMs, or Unreads without ever
//! losing audio controls, active-speaker visibility, or one-click pop-out.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};
use slackless_core::huddle::{AudioDevices, HuddlePhase, list_audio_devices};
use slackless_core::types::{ChannelId, ConversationKind, UserId};

use crate::avatars::user_chip;
use crate::huddle_state::HuddleState;
use crate::huddle_video::{
    format_elapsed, huddle_browser_url, is_audio_device_selected, is_user_speaking,
};

pub enum HuddleDockEvent {
    ToggleMute,
    ToggleCamera,
    ToggleScreenShare,
    Leave,
    PopOut,
    OpenFloatingPanel,
    JumpToChannel(ChannelId),
    OpenThread,
    OpenCanvas(String),
    CopyLink(ChannelId),
    OpenBrowser(String),
    RetryJoin(ChannelId),
    OpenAudioSettings,
    SwitchInputDevice(Option<String>),
    SwitchOutputDevice(Option<String>),
}

pub struct HuddleDock {
    state: Entity<HuddleState>,
    menu_open: bool,
    devices: AudioDevices,
    _state_sub: Subscription,
}

impl EventEmitter<HuddleDockEvent> for HuddleDock {}

impl HuddleDock {
    pub fn new(state: Entity<HuddleState>, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let sub = cx.observe(&state, |_, _, cx| cx.notify());
            Self {
                state,
                menu_open: false,
                devices: AudioDevices::default(),
                _state_sub: sub,
            }
        })
    }

    fn channel_label(&self, state: &HuddleState) -> String {
        let fixtures = state.fixtures();
        fixtures
            .conversation(&state.session().channel)
            .map(|c| fixtures.conversation_label(c))
            .unwrap_or_else(|| state.session().channel.to_string())
    }

    fn browser_url(&self, state: &HuddleState) -> String {
        huddle_browser_url(
            state.fixtures(),
            &state.session().channel,
            state.session().huddle_link.as_deref(),
        )
    }

    fn all_people(&self, state: &HuddleState) -> Vec<UserId> {
        let mut people = state.participants().to_vec();
        let self_uid = &state.fixtures().self_user;
        if !people.contains(self_uid) {
            people.push(self_uid.clone());
        }
        people
    }

    fn is_speaking(&self, state: &HuddleState, user: &UserId) -> bool {
        is_user_speaking(
            *user == state.fixtures().self_user,
            state.session().muted,
            state.levels(),
            user,
        )
    }

    fn is_muted_user(&self, state: &HuddleState, user: &UserId) -> bool {
        if *user == state.fixtures().self_user {
            state.session().muted
        } else {
            state.levels().muted_peers.contains(user)
        }
    }

    fn render_menu(
        &self,
        channel: &ChannelId,
        canvas_id: Option<String>,
        active_in: Option<String>,
        active_out: Option<String>,
        browser_url: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let theme = cx.theme();
        let (card_bg, border, muted_fg, fg, hover_bg) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.popover_foreground,
            theme.accent,
        );
        let copy_ch = channel.clone();

        v_flex()
            .w_full()
            .p_2()
            .gap_1()
            .rounded_xl()
            .bg(card_bg)
            .border_1()
            .border_color(border)
            .shadow_xl()
            .text_xs()
            .text_color(fg)
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Microphone"),
            )
            .children(self.devices.inputs.iter().enumerate().map(|(ix, dev)| {
                let selected = is_audio_device_selected(active_in.as_deref(), dev);
                let dev_name = dev.name.clone();
                div()
                    .id(("dock-in-dev", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .hover(move |el| el.bg(hover_bg))
                    .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchInputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_1()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted_fg)
                    .child("Speaker"),
            )
            .children(self.devices.outputs.iter().enumerate().map(|(ix, dev)| {
                let selected = is_audio_device_selected(active_out.as_deref(), dev);
                let dev_name = dev.name.clone();
                div()
                    .id(("dock-out-dev", ix))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .truncate()
                    .hover(move |el| el.bg(hover_bg))
                    .when(selected, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .child(if selected {
                        format!("✓ {}", dev.name)
                    } else {
                        dev.name.clone()
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::SwitchOutputDevice(Some(dev_name.clone())));
                        cx.notify();
                    }))
            }))
            .child(div().h(px(1.)).my_1().bg(border))
            .child(
                h_flex()
                    .id("dock-floating-controls")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(Icon::new(IconName::Pin).size(px(12.)).text_color(muted_fg))
                    .child("Floating controls panel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenFloatingPanel);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("dock-open-thread")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::MessageSquare)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Open huddle thread")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenThread);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("dock-copy-link")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(Icon::new(IconName::Link).size(px(12.)).text_color(muted_fg))
                    .child("Copy huddle link")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::CopyLink(copy_ch.clone()));
                        cx.notify();
                    })),
            )
            .when_some(canvas_id, |m, fid| {
                m.child(
                    h_flex()
                        .id("dock-open-canvas")
                        .px_2()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .rounded_md()
                        .cursor_pointer()
                        .hover(move |el| el.bg(hover_bg))
                        .child(
                            Icon::new(IconName::FileText)
                                .size(px(12.))
                                .text_color(muted_fg),
                        )
                        .child("Open huddle notes")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.menu_open = false;
                            cx.emit(HuddleDockEvent::OpenCanvas(fid.clone()));
                            cx.notify();
                        })),
                )
            })
            .child(
                h_flex()
                    .id("dock-audio-settings")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::Settings)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Audio & video preferences…")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenAudioSettings);
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .id("dock-open-browser")
                    .px_2()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover_bg))
                    .child(
                        Icon::new(IconName::ExternalLink)
                            .size(px(12.))
                            .text_color(muted_fg),
                    )
                    .child("Open in browser")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.menu_open = false;
                        cx.emit(HuddleDockEvent::OpenBrowser(browser_url.clone()));
                        cx.notify();
                    })),
            )
    }

    fn render_action_bar(
        &self,
        muted: bool,
        video: bool,
        sharing: bool,
        pal: &DockPalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let btn_bg = pal.btn_bg;
        let btn_hover = pal.btn_hover;
        let btn_fg = pal.card_fg;
        let active_bg = pal.active_btn_bg;
        let active_fg = pal.active_btn_fg;
        let danger = pal.danger;
        let danger_hover = pal.danger_hover;
        let danger_fg = pal.danger_fg;
        let primary = pal.primary;
        let primary_hover = pal.primary_hover;
        let primary_fg = pal.primary_fg;

        let mic_btn = div()
            .id("huddle-dock-mic")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if muted { danger } else { active_bg })
            .hover(move |el| el.bg(if muted { danger_hover } else { btn_hover }))
            .child(
                Icon::new(if muted {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .size(px(15.))
                .text_color(if muted { danger_fg } else { active_fg }),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleMute)));

        let video_btn = div()
            .id("huddle-dock-video")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if video { active_bg } else { btn_bg })
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(if video {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(15.))
                .text_color(if video { active_fg } else { btn_fg }),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleCamera)));

        let share_btn = div()
            .id("huddle-dock-share")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if sharing { primary } else { btn_bg })
            .hover(move |el| el.bg(if sharing { primary_hover } else { btn_hover }))
            .child(
                Icon::new(IconName::ScreenShare)
                    .size(px(15.))
                    .text_color(if sharing { primary_fg } else { btn_fg }),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::ToggleScreenShare)));

        let more_btn = div()
            .id("huddle-dock-more")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.menu_open { btn_hover } else { btn_bg })
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(IconName::Ellipsis)
                    .size(px(15.))
                    .text_color(btn_fg),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.menu_open = !this.menu_open;
                if this.menu_open {
                    cx.spawn(async move |dock, cx| {
                        let devices = cx
                            .background_executor()
                            .spawn(async move { list_audio_devices() })
                            .await;
                        let _ = dock.update(cx, |dock: &mut HuddleDock, cx| {
                            dock.devices = devices;
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            }));

        let popout_btn = div()
            .id("huddle-dock-popout")
            .size(px(32.))
            .rounded_lg()
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(btn_bg)
            .hover(move |el| el.bg(btn_hover))
            .child(
                Icon::new(IconName::ExternalLink)
                    .size(px(15.))
                    .text_color(btn_fg),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::PopOut)));

        let leave_btn = h_flex()
            .id("huddle-dock-leave")
            .h(px(32.))
            .px_4()
            .rounded_lg()
            .cursor_pointer()
            .items_center()
            .justify_center()
            .bg(danger)
            .hover(move |el| el.bg(danger_hover))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(danger_fg)
            .child("Leave")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleDockEvent::Leave)));

        h_flex()
            .flex_none()
            .items_center()
            .gap_1p5()
            .child(mic_btn)
            .child(video_btn)
            .child(share_btn)
            .child(more_btn)
            .child(popout_btn)
            .child(leave_btn)
    }

    fn channel_heading(&self, state: &HuddleState) -> (Option<IconName>, String) {
        let fixtures = state.fixtures();
        let Some(conv) = fixtures.conversation(&state.session().channel) else {
            return (None, self.channel_label(state));
        };
        let label = fixtures.conversation_label(conv);
        match conv.kind() {
            ConversationKind::PrivateChannel => (
                Some(IconName::Lock),
                label.trim_start_matches("# ").to_owned(),
            ),
            ConversationKind::PublicChannel => {
                (None, format!("#{}", label.trim_start_matches("# ")))
            }
            ConversationKind::Im | ConversationKind::Mpim => (None, label),
        }
    }
}

struct DockPalette {
    card_bg: Hsla,
    card_fg: Hsla,
    muted_fg: Hsla,
    border: Hsla,
    btn_bg: Hsla,
    btn_hover: Hsla,
    active_btn_bg: Hsla,
    active_btn_fg: Hsla,
    primary: Hsla,
    primary_hover: Hsla,
    primary_fg: Hsla,
    danger: Hsla,
    danger_hover: Hsla,
    danger_fg: Hsla,
    success: Hsla,
    warning: Hsla,
}

fn dock_palette(cx: &App) -> DockPalette {
    let theme = cx.theme();
    DockPalette {
        card_bg: theme.background,
        card_fg: theme.foreground,
        muted_fg: theme.muted_foreground,
        border: theme.border,
        btn_bg: theme.secondary,
        btn_hover: theme.secondary_hover,
        active_btn_bg: theme.foreground,
        active_btn_fg: theme.background,
        primary: theme.primary,
        primary_hover: theme.primary_hover,
        primary_fg: theme.primary_foreground,
        danger: theme.danger,
        danger_hover: theme.danger_hover,
        danger_fg: theme.danger_foreground,
        success: theme.success,
        warning: theme.warning,
    }
}

impl Render for HuddleDock {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = dock_palette(cx);
        let state = self.state.read(cx);
        let session = state.session();

        let (status_color, status_text) = match session.phase {
            HuddlePhase::Joining | HuddlePhase::Connecting => {
                (pal.warning, "Connecting…".to_owned())
            }
            HuddlePhase::Connected => (pal.success, format_elapsed(state.elapsed_secs())),
            HuddlePhase::Reconnecting => (pal.warning, "Reconnecting…".to_owned()),
            HuddlePhase::Leaving => (pal.muted_fg, "Leaving…".to_owned()),
            HuddlePhase::Failed => (pal.danger, "Failed".to_owned()),
            HuddlePhase::Idle => (pal.muted_fg, "Ended".to_owned()),
        };

        let phase = session.phase;
        let muted = session.muted;
        let video = session.video;
        let sharing = session.sharing;
        let topic = session.topic.clone();
        let error = session.error.clone();
        let channel = session.channel.clone();
        let (lock_icon, channel_name) = self.channel_heading(state);
        let browser_url = self.browser_url(state);
        let fixtures = state.fixtures().clone();
        let people = self.all_people(state);
        let visible_people: Vec<(UserId, bool, bool)> = people
            .iter()
            .take(4)
            .map(|uid| {
                (
                    uid.clone(),
                    self.is_speaking(state, uid),
                    self.is_muted_user(state, uid),
                )
            })
            .collect();
        let overflow_count = people.len().saturating_sub(visible_people.len());
        let canvas_id = session.canvas_file_id.clone();
        let active_in = session.input_device.clone();
        let active_out = session.output_device.clone();
        let menu_el = self.menu_open.then(|| {
            self.render_menu(
                &channel,
                canvas_id,
                active_in,
                active_out,
                browser_url.clone(),
                cx,
            )
        });
        let jump_ch = channel.clone();
        let action_bar = self.render_action_bar(muted, video, sharing, &pal, cx);

        let mut bar = div().id("huddle-sidebar-dock").relative().w_full().child(
            h_flex()
                .w_full()
                .h(px(46.))
                .px_3p5()
                .gap_3()
                .items_center()
                .justify_between()
                .rounded_lg()
                .bg(pal.card_bg)
                .border_1()
                .border_color(pal.border)
                .shadow_sm()
                .text_color(pal.card_fg)
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .items_center()
                        .gap_2p5()
                        .child(
                            h_flex()
                                .id("huddle-dock-jump")
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .cursor_pointer()
                                .child(Icon::new(IconName::Headphones).size(px(16.)).text_color(
                                    if phase == HuddlePhase::Connected {
                                        pal.card_fg
                                    } else {
                                        status_color
                                    },
                                ))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(pal.card_fg)
                                        .child("Huddle in"),
                                )
                                .when_some(lock_icon, |row, ic| {
                                    row.child(Icon::new(ic).size(px(13.)).text_color(pal.card_fg))
                                })
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(pal.card_fg)
                                        .child(channel_name),
                                )
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(HuddleDockEvent::JumpToChannel(jump_ch.clone()));
                                })),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .items_center()
                                .gap_1()
                                .children(visible_people.into_iter().map(
                                    |(uid, speaking, user_muted)| {
                                        div()
                                            .relative()
                                            .p(px(1.5))
                                            .rounded_md()
                                            .border_2()
                                            .border_color(if speaking {
                                                pal.success
                                            } else {
                                                pal.border
                                            })
                                            .child(user_chip(&fixtures, &uid, 20.))
                                            .when(user_muted, |el| {
                                                el.child(
                                                    div()
                                                        .absolute()
                                                        .right(px(-3.))
                                                        .bottom(px(-3.))
                                                        .size(px(12.))
                                                        .rounded_full()
                                                        .bg(pal.card_bg)
                                                        .border_1()
                                                        .border_color(pal.border)
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .child(
                                                            Icon::new(IconName::MicOff)
                                                                .size(px(7.))
                                                                .text_color(pal.danger),
                                                        ),
                                                )
                                            })
                                    },
                                ))
                                .when(overflow_count > 0, |row| {
                                    row.child(
                                        div()
                                            .px_1()
                                            .py_0p5()
                                            .rounded_md()
                                            .bg(pal.btn_bg)
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(pal.card_fg)
                                            .child(format!("+{overflow_count}")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .py_0p5()
                                .rounded_full()
                                .bg(status_color.opacity(0.14))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(status_color)
                                .child(status_text),
                        )
                        .when_some(topic, |row, topic| {
                            row.child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(pal.muted_fg)
                                    .child(topic),
                            )
                        })
                        .when_some(error, |row, err_msg| {
                            let retry_ch = channel.clone();
                            let fallback_url = browser_url.clone();
                            row.child(
                                h_flex()
                                    .items_center()
                                    .gap_1p5()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(pal.danger.opacity(0.12))
                                    .border_1()
                                    .border_color(pal.danger.opacity(0.4))
                                    .child(
                                        Icon::new(IconName::TriangleAlert)
                                            .size(px(12.))
                                            .text_color(pal.danger),
                                    )
                                    .child(
                                        div()
                                            .max_w(px(200.))
                                            .truncate()
                                            .text_xs()
                                            .text_color(pal.card_fg)
                                            .child(err_msg),
                                    )
                                    .child(
                                        Button::new("huddle-dock-retry")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .label("Retry")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(HuddleDockEvent::RetryJoin(
                                                    retry_ch.clone(),
                                                ));
                                            })),
                                    )
                                    .child(
                                        Button::new("huddle-dock-browser-fallback")
                                            .primary()
                                            .with_size(Size::XSmall)
                                            .label("Browser")
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.emit(HuddleDockEvent::OpenBrowser(
                                                    fallback_url.clone(),
                                                ));
                                            })),
                                    ),
                            )
                        }),
                )
                .child(action_bar),
        );

        if let Some(menu) = menu_el {
            bar = bar.child(
                div()
                    .id("huddle-dock-menu-surface")
                    .absolute()
                    .right_2()
                    .bottom(px(52.))
                    .w(px(260.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        if this.menu_open {
                            this.menu_open = false;
                            cx.notify();
                        }
                    }))
                    .child(menu),
            );
        }

        bar
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{self, SlackTheme};
    use gpui_kit::base::Root;
    use gpui_kit::gpui::{self, TestAppContext};
    use slackless_core::huddle::{HuddleAudioLevels, HuddleSessionSnapshot};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;

    #[gpui::test]
    fn huddle_dock_renders_every_phase_and_theme_and_tracks_speakers_and_overflow(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(crate::zoom::Zoom::default());
            gpui_kit::init(cx);
            theme::init(cx);
        });
        let fx = Arc::new(crate::fixtures::load());
        let me = fx.self_user.clone();
        let peers: Vec<UserId> = fx
            .users
            .iter()
            .filter(|u| u.id != me)
            .take(5)
            .map(|u| u.id.clone())
            .collect();
        let channel = fx.channels[0].id.clone();

        let state = cx.update(|cx| {
            let snap = HuddleSessionSnapshot {
                phase: HuddlePhase::Connecting,
                channel: channel.clone(),
                call_id: "R0DOCK".into(),
                topic: Some("Standup".into()),
                huddle_link: Some("https://app.slack.com/huddle/T1/C1".into()),
                ..Default::default()
            };
            HuddleState::new(fx.clone(), snap, peers.clone(), cx)
        });
        let events: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let ev_log = events.clone();
        let dock_slot: Rc<RefCell<Option<Entity<HuddleDock>>>> = Rc::new(RefCell::new(None));
        let dock_slot_in = dock_slot.clone();
        let state_in = state.clone();

        let (root, vcx) = cx.add_window_view(move |window, cx| {
            let dock = HuddleDock::new(state_in.clone(), cx);
            cx.subscribe(&dock, move |_, _, ev: &HuddleDockEvent, _| {
                let tag = match ev {
                    HuddleDockEvent::ToggleMute => "mute",
                    HuddleDockEvent::ToggleCamera => "cam",
                    HuddleDockEvent::ToggleScreenShare => "share",
                    HuddleDockEvent::Leave => "leave",
                    HuddleDockEvent::PopOut => "popout",
                    HuddleDockEvent::OpenFloatingPanel => "float",
                    HuddleDockEvent::JumpToChannel(_) => "jump",
                    HuddleDockEvent::OpenThread => "thread",
                    HuddleDockEvent::OpenCanvas(_) => "canvas",
                    HuddleDockEvent::CopyLink(_) => "copy",
                    HuddleDockEvent::OpenBrowser(_) => "browser",
                    HuddleDockEvent::RetryJoin(_) => "retry",
                    HuddleDockEvent::OpenAudioSettings => "audio",
                    HuddleDockEvent::SwitchInputDevice(_) => "in",
                    HuddleDockEvent::SwitchOutputDevice(_) => "out",
                };
                ev_log.borrow_mut().push(tag);
            })
            .detach();
            *dock_slot_in.borrow_mut() = Some(dock.clone());
            Root::new(dock, window, cx)
        });
        let dock = dock_slot.borrow_mut().take().unwrap();

        // Exercise every HuddlePhase + error/UDP-blocked + Dark & Light themes + open menu.
        for (phase, err, udp_blocked, mode) in [
            (HuddlePhase::Joining, None, false, SlackTheme::Dark),
            (HuddlePhase::Connecting, None, false, SlackTheme::Dark),
            (HuddlePhase::Connected, None, false, SlackTheme::Dark),
            (HuddlePhase::Connected, None, false, SlackTheme::Light),
            (HuddlePhase::Reconnecting, None, false, SlackTheme::Dark),
            (
                HuddlePhase::Failed,
                Some("UDP media blocked".to_owned()),
                true,
                SlackTheme::Light,
            ),
            (HuddlePhase::Leaving, None, false, SlackTheme::Dark),
        ] {
            vcx.update(|window, cx| {
                theme::activate(mode, cx);
                state.update(cx, |s, cx| {
                    let mut snap = s.session().clone();
                    snap.phase = phase;
                    snap.error = err.clone();
                    snap.udp_blocked = udp_blocked;
                    snap.muted = matches!(phase, HuddlePhase::Reconnecting);
                    snap.sharing = matches!(phase, HuddlePhase::Connected);
                    snap.canvas_file_id = Some("F0CANVAS".into());
                    s.update_session(snap, cx);
                    s.update_levels(
                        HuddleAudioLevels {
                            local_rms: 0.4,
                            active_speakers: peers
                                .first()
                                .cloned()
                                .map(|u| vec![(u, 0.7)])
                                .unwrap_or_default(),
                            muted_peers: peers.get(1).cloned().into_iter().collect(),
                        },
                        cx,
                    );
                });
                dock.update(cx, |d, cx| {
                    d.menu_open = matches!(phase, HuddlePhase::Connected);
                    // Verify helper invariants on people, speaking, muted, label, browser_url.
                    let st = state.read(cx);
                    assert!(d.all_people(st).contains(&me));
                    assert!(d.all_people(st).len() >= 5);
                    if let Some(first_peer) = peers.first() {
                        assert!(d.is_speaking(st, first_peer));
                    }
                    if let Some(second_peer) = peers.get(1) {
                        assert!(d.is_muted_user(st, second_peer));
                    }
                    assert!(!d.channel_label(st).is_empty());
                    assert!(d.browser_url(st).contains("https://"));
                    cx.notify();
                });
                let _ = window.draw(cx);
            });
        }
        drop(root);
    }
}
