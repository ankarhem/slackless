//! Floating incoming DM huddle ring banner (`F-COLLAB-031`, `huddle_invite` / `huddle_invite_cancel`).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, Window,
    div, px, rgb, rgba,
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
        let _ = cx.theme();
        let card_bg = rgb(0x1E2228);
        let fg = rgb(0xFFFFFF);
        let muted = rgb(0xABABAD);
        let emerald = rgb(0x2BAC76);
        let join_green = rgb(0x007A5A);

        let inviter_name = self.fixtures.user_name(&self.invite.inviter).to_owned();
        let channel = self.invite.channel.clone();

        h_flex()
            .w(px(356.))
            .p_3()
            .gap_3()
            .items_center()
            .rounded_2xl()
            .bg(card_bg)
            .border_1()
            .border_color(emerald.opacity(0.75))
            .shadow_2xl()
            .text_color(fg)
            .child(
                div()
                    .p(px(2.5))
                    .rounded_xl()
                    .border_2()
                    .border_color(emerald)
                    .child(user_chip(&self.fixtures, &self.invite.inviter, 40.)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(emerald),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
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
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .id("huddle-invite-decline")
                            .size(px(32.))
                            .rounded_full()
                            .bg(rgba(0xFFFFFF16))
                            .hover(|el| el.bg(rgba(0xFFFFFF26)))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::X).size(px(14.)).text_color(fg))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(HuddleInviteEvent::Decline);
                            })),
                    )
                    .child(
                        h_flex()
                            .id("huddle-invite-accept")
                            .h(px(32.))
                            .px_3()
                            .gap_1p5()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .bg(join_green)
                            .hover(|el| el.bg(emerald))
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(Icon::new(IconName::Headphones).size(px(13.)).text_color(fg))
                            .child("Join")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(HuddleInviteEvent::Accept(channel.clone()));
                            })),
                    ),
            )
    }
}
