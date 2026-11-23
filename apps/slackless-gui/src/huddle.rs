//! Huddle UI without media (docs/research/api-huddles.md): the bar shown under the conversation
//! header while a huddle is open - participants, mic / video / screen-share / leave controls and
//! a prominent "Open huddle in browser" fallback - plus the fixture that says which channel has
//! a live huddle (the sidebar marks that row with a headphones indicator).
//!
//! DEFERRED: actual audio/video. Slack huddles run on Amazon Chime/WebRTC, which slackless does
//! not implement, so the mic/video/screen-share controls only flip local UI state and no media
//! is captured or played. Joining for real goes through the browser fallback, which opens the
//! huddle in Slack's web client.
//!
//! `HuddleBar::new(Arc<Fixtures>, Huddle, &mut App) -> Entity<HuddleBar>`
//! emits `HuddleEvent::Leave`.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement, ParentElement as _,
    Render, Styled as _, Window, div, px,
};
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
}

pub struct HuddleBar {
    fixtures: Arc<Fixtures>,
    huddle: Huddle,
    // UI-only control state; no media device is touched (see module docs).
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
            muted: true,
            video: false,
            sharing: false,
        })
    }

    /// Slack's web-client huddle link for this channel.
    fn browser_url(&self) -> String {
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
}

impl Render for HuddleBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_bg = cx.theme().muted;
        let border = cx.theme().border;
        let success = cx.theme().success;
        let muted_foreground = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        let mut people: Vec<&UserId> = self.huddle.participants.iter().collect();
        people.push(&self.fixtures.self_user);
        let count = people.len();
        let channel = self
            .fixtures
            .conversation(&self.huddle.channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.huddle.channel.to_string());

        let participants = h_flex()
            .gap_3()
            .children(people.into_iter().enumerate().map(|(ix, user)| {
                // The first participant is shown as the active speaker.
                let speaking = ix == 0;
                v_flex()
                    .items_center()
                    .gap_1()
                    .w(px(64.))
                    .child(
                        div()
                            .p_0p5()
                            .rounded_md()
                            .border_2()
                            .border_color(if speaking { success } else { border })
                            .child(user_chip(&self.fixtures, user, 36.)),
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
                    "Mute / unmute (UI only)",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.muted = !this.muted;
                    cx.notify();
                })),
            )
            .child(
                toggle(
                    "huddle-video",
                    self.video,
                    IconName::Video,
                    IconName::VideoOff,
                    "Video (UI only)",
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
                    "Share screen (UI only)",
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
        let open_thread = Button::new("huddle-open-thread")
            .outline()
            .with_size(Size::Small)
            .icon(IconName::MessageSquare)
            .label("Huddle thread")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(HuddleEvent::OpenThread)));
        let open_in_browser = Button::new("huddle-open-browser")
            .primary()
            .icon(IconName::ExternalLink)
            .label("Open huddle in browser")
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
                    .child(div().flex_1())
                    .child(open_thread)
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
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .text_xs()
                    .text_color(warning)
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .text_color(warning)
                            .size(px(12.)),
                    )
                    .child(
                        "Audio, video and screen sharing are not available in slackless yet - \
                         controls are display-only. Use \"Open huddle in browser\" to talk.",
                    ),
            )
            .when(self.sharing || self.video, |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("Nothing is being captured: media is handled only in the browser."),
                )
            })
    }
}
