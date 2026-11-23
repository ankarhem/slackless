//! Floating incoming DM huddle ring banner (`F-COLLAB-031`, `huddle_invite` / `huddle_invite_cancel`).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, px,
};
use slackless_core::types::{ChannelId, UserId};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;

/// Active incoming DM huddle invitation being rung.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingHuddleInvite {
    pub channel: ChannelId,
    pub inviter: UserId,
    pub call_id: String,
}

pub enum HuddleInviteEvent {
    Accept(ChannelId),
    Decline,
}

pub struct HuddleInviteBanner {
    fixtures: Arc<Fixtures>,
    invite: PendingHuddleInvite,
}

impl EventEmitter<HuddleInviteEvent> for HuddleInviteBanner {}

impl HuddleInviteBanner {
    pub fn new(fixtures: Arc<Fixtures>, invite: PendingHuddleInvite, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self { fixtures, invite })
    }

    pub fn invite(&self) -> &PendingHuddleInvite {
        &self.invite
    }
}

impl Render for HuddleInviteBanner {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let bg = theme.popover;
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let success = theme.success;

        let inviter_name = self.fixtures.user_name(&self.invite.inviter).to_owned();
        let channel = self.invite.channel.clone();

        h_flex()
            .w(px(340.))
            .p_3()
            .gap_3()
            .items_center()
            .rounded_xl()
            .bg(bg)
            .border_1()
            .border_color(success.opacity(0.65))
            .shadow_lg()
            .text_color(fg)
            .child(
                div()
                    .p(px(2.))
                    .rounded_lg()
                    .border_2()
                    .border_color(success)
                    .child(user_chip(&self.fixtures, &self.invite.inviter, 36.)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(success),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{inviter_name} is huddling")),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Invited you to join a huddle"),
                    ),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Button::new("huddle-invite-decline")
                            .ghost()
                            .with_size(Size::Small)
                            .icon(IconName::X)
                            .tooltip("Decline")
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(HuddleInviteEvent::Decline);
                            })),
                    )
                    .child(
                        Button::new("huddle-invite-accept")
                            .primary()
                            .with_size(Size::Small)
                            .icon(IconName::Headphones)
                            .label("Join")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HuddleInviteEvent::Accept(channel.clone()));
                            })),
                    ),
            )
    }
}
