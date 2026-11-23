//! Huddle bar shown under a conversation header while a huddle is open in the current channel
//! (and fixture huddle helper for sample mode).
//!
//! Live workspaces connect natively to Slack's Amazon Chime audio SFU (`slackless-huddle`) while
//! keeping a one-click "Open in browser" fallback for video/screen sharing (Phases 2–3) or
//! UDP-blocked corporate networks.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};
use slackless_core::huddle::{HuddleAudioLevels, HuddlePhase, HuddleSessionSnapshot};
use slackless_core::types::{ChannelId, UserId};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;

/// A live huddle: where it is and who is in it.
#[derive(Clone)]
pub struct Huddle {
    pub channel: ChannelId,
    pub participants: Vec<UserId>,
}

/// The fixture huddle: running in the first channel with up to three other members. Sample
/// workspace only.
pub fn fixture(fixtures: &Fixtures) -> Option<Huddle> {
    if !fixtures.sample {
        return None;
    }
    let channel = fixtures.channels.first()?.id.clone();
    let participants = fixtures
        .users
        .iter()
        .map(|user| user.id.clone())
        .filter(|id| *id != fixtures.self_user)
        .take(3)
        .collect();
    Some(Huddle {
        channel,
        participants,
    })
}

pub enum HuddleEvent {
    Leave,
    /// Open or toggle the active huddle's thread in the right-hand thread panel (`F-COLLAB-014`/`015`).
    OpenThread,
    /// Toggle local microphone mute (`Command::SetHuddleMuted`).
    ToggleMute,
    /// Open the full Pop-out Huddle Window (`PopoutKind::Huddle`).
    PopOut,
    /// Copy shareable huddle link (`Command::CopyHuddleLink`).
    CopyLink,
    /// Open the huddle's notes canvas if attached.
    OpenCanvas(String),
}

pub struct HuddleBar {
    fixtures: Arc<Fixtures>,
    huddle: Huddle,
    session: Option<HuddleSessionSnapshot>,
    levels: HuddleAudioLevels,
    muted: bool,
    video: bool,
    sharing: bool,
}

impl EventEmitter<HuddleEvent> for HuddleBar {}

impl HuddleBar {
    pub fn new(
        fixtures: Arc<Fixtures>,
        huddle: Huddle,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<HuddleBar> {
        cx.new(|_| HuddleBar {
            fixtures,
            huddle,
            session: None,
            levels: HuddleAudioLevels::default(),
            muted: false,
            video: false,
            sharing: false,
        })
    }

    pub fn set_participants(&mut self, participants: Vec<UserId>, cx: &mut Context<Self>) {
        if self.huddle.participants != participants {
            self.huddle.participants = participants;
            cx.notify();
        }
    }

    pub fn update_session(
        &mut self,
        session: Option<HuddleSessionSnapshot>,
        cx: &mut Context<Self>,
    ) {
        if let Some(snap) = &session {
            self.muted = snap.muted;
        }
        self.session = session;
        cx.notify();
    }

    pub fn update_levels(&mut self, levels: HuddleAudioLevels, cx: &mut Context<Self>) {
        self.levels = levels;
        cx.notify();
    }

    /// Slack's web-client huddle link for this channel.
    fn browser_url(&self) -> String {
        if let Some(link) = self.session.as_ref().and_then(|s| s.huddle_link.as_ref()) {
            return link.clone();
        }
        let team = self
            .fixtures
            .workspaces
            .first()
            .map(|team| team.id.to_string())
            .unwrap_or_default();
        format!(
            "https://app.slack.com/huddle/{team}/{}",
            self.huddle.channel
        )
    }

    fn is_speaking(&self, user: &UserId, ix: usize) -> bool {
        if self.session.is_some() {
            if *user == self.fixtures.self_user {
                !self.muted && self.levels.local_rms > 0.03
            } else {
                self.levels
                    .active_speakers
                    .iter()
                    .any(|(uid, lvl)| uid == user && *lvl > 0.05)
            }
        } else {
            ix == 0
        }
    }
}

impl Render for HuddleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_bg = cx.theme().muted;
        let border = cx.theme().border;
        let success = cx.theme().success;
        let muted_foreground = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let danger = cx.theme().danger;

        let mut people: Vec<&UserId> = self.huddle.participants.iter().collect();
        if !people.contains(&&self.fixtures.self_user) {
            people.push(&self.fixtures.self_user);
        }
        let count = people.len();
        let channel = self
            .fixtures
            .conversation(&self.huddle.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.huddle.channel.to_string());

        let phase = self.session.as_ref().map(|s| s.phase);
        let status_label = match phase {
            Some(HuddlePhase::Joining | HuddlePhase::Connecting) => Some(("Connecting…", warning)),
            Some(HuddlePhase::Connected) => Some(("Audio connected", success)),
            Some(HuddlePhase::Reconnecting) => Some(("Reconnecting…", warning)),
            Some(HuddlePhase::Failed) => Some(("Connection failed", danger)),
            _ => None,
        };

        let participants = h_flex()
            .gap_3()
            .children(people.into_iter().enumerate().map(|(ix, user)| {
                let speaking = self.is_speaking(user, ix);
                let user_muted = if *user == self.fixtures.self_user {
                    self.muted
                } else {
                    self.levels.muted_peers.contains(user)
                };
                v_flex()
                    .items_center()
                    .gap_1()
                    .w(px(64.))
                    .child(
                        div()
                            .relative()
                            .p_0p5()
                            .rounded_md()
                            .border_2()
                            .border_color(if speaking { success } else { border })
                            .child(user_chip(&self.fixtures, user, 36.))
                            .when(user_muted, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .right(px(-3.))
                                        .bottom(px(-3.))
                                        .size(px(14.))
                                        .rounded_full()
                                        .bg(muted_bg)
                                        .border_1()
                                        .border_color(border)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            Icon::new(IconName::MicOff)
                                                .size(px(9.))
                                                .text_color(danger),
                                        ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .truncate()
                            .text_xs()
                            .child(self.fixtures.user_name(user).to_owned()),
                    )
            }));

        let toggle = |id: &'static str,
                      on: bool,
                      on_icon: IconName,
                      off_icon: IconName,
                      tip: &'static str| {
            let button = Button::new(id)
                .with_size(Size::Small)
                .icon(if on { on_icon } else { off_icon })
                .tooltip(tip);
            if on { button.primary() } else { button.ghost() }
        };
        let controls = h_flex()
            .gap_1()
            .items_center()
            .child(
                toggle(
                    "huddle-mic",
                    !self.muted,
                    IconName::Mic,
                    IconName::MicOff,
                    if self.muted {
                        "Unmute microphone (⌘⇧Space)"
                    } else {
                        "Mute microphone (⌘⇧Space)"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.muted = !this.muted;
                    cx.emit(HuddleEvent::ToggleMute);
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "huddle-video",
                    self.video,
                    IconName::Video,
                    IconName::VideoOff,
                    "Video (Phase 2 — use Open in browser for video)",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.video = !this.video;
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "huddle-share",
                    self.sharing,
                    IconName::ScreenShare,
                    IconName::ScreenShare,
                    "Share screen (Phase 3 — use Open in browser for screen share)",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.sharing = !this.sharing;
                    cx.notify();
                })),
            )
            .child(
                Button::new("huddle-leave")
                    .danger()
                    .with_size(Size::Small)
                    .icon(IconName::PhoneOff)
                    .label("Leave")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleEvent::Leave))),
            );

        let url = self.browser_url();
        let canvas_id = self.session.as_ref().and_then(|s| s.canvas_file_id.clone());
        let open_thread = Button::new("huddle-open-thread")
            .outline()
            .with_size(Size::Small)
            .icon(IconName::MessageSquare)
            .label("Huddle thread")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleEvent::OpenThread)));
        let copy_link = Button::new("huddle-copy-link")
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::Link)
            .tooltip("Copy huddle link")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleEvent::CopyLink)));
        let pop_out = Button::new("huddle-popout")
            .outline()
            .with_size(Size::Small)
            .icon(IconName::ExternalLink)
            .label("Pop out")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleEvent::PopOut)));
        let open_in_browser = Button::new("huddle-open-browser")
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::ExternalLink)
            .label("Browser")
            .tooltip("Open huddle in Slack web client")
            .on_click(move |_, _, cx| cx.open_url(&url));

        v_flex()
            .flex_none()
            .mx_4()
            .my_2()
            .p_3()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(success.opacity(0.5))
            .bg(muted_bg)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Headphones)
                            .text_color(success)
                            .size(px(16.)),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("Huddle in {channel}")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child(format!("{count} participants")),
                    )
                    .when_some(status_label, |row, (label, color)| {
                        row.child(
                            div()
                                .px_1p5()
                                .py_0p5()
                                .rounded_sm()
                                .bg(color.opacity(0.15))
                                .text_xs()
                                .text_color(color)
                                .child(label),
                        )
                    })
                    .child(div().flex_1())
                    .child(copy_link)
                    .when_some(canvas_id, |row, fid| {
                        row.child(
                            Button::new("huddle-open-canvas")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::FileText)
                                .label("Notes")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(HuddleEvent::OpenCanvas(fid.clone()));
                                })),
                        )
                    })
                    .child(open_thread)
                    .child(pop_out)
                    .child(open_in_browser),
            )
            .child(
                h_flex()
                    .items_end()
                    .gap_4()
                    .child(participants)
                    .child(div().flex_1())
                    .child(controls),
            )
            .when_some(
                self.session.as_ref().and_then(|s| s.error.clone()),
                |bar, err| {
                    bar.child(
                        h_flex()
                            .gap_1p5()
                            .items_center()
                            .text_xs()
                            .text_color(danger)
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .text_color(danger)
                                    .size(px(12.)),
                            )
                            .child(err),
                    )
                },
            )
            .when(self.sharing || self.video, |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("Native audio is active; video and screen sharing are coming in Phases 2–3 (use Browser to share video/screen)."),
                )
            })
    }
}
