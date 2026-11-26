//! Slack's member profile hover card (`p-member_profile_hover_card`), shown on hovering an
//! @mention pill or a message author's name: avatar, name and presence, pronouns, title,
//! "View full profile", custom status, local time, and the Message / Set a status action.
//!
//! The card renders from [`card`]; the data it shows from [`derive`]. Triggers (mention
//! pills in [`crate::messages::SelectableParagraph`] and author-name chips) report hover
//! through the window-wide [`Router`] global, so every surface that renders message text
//! (transcript, thread panel, search results, pins) drives the card without its view
//! threading new events upward. The root `App` owns the dwell / grace lifecycle over
//! [`Lifecycle`] and mounts the card in its overlay layer.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::Button;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, Bounds, Context, Div, Entity, FontWeight, Global, InteractiveElement as _,
    IntoElement as _, MouseDownEvent, ParentElement as _, Pixels, Point, SharedString, Size,
    Stateful, StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, div, point,
    px,
};
use slackless_core::types::{Team, UserId};

use crate::app::App as Root;
use crate::fixtures::Fixtures;
use crate::profile::{DerivedStatus, derive_user_profile};
use crate::status::SelfState;

/// How long the pointer must rest on a trigger before the card shows, so sweeping the mouse
/// across a transcript never flashes popovers.
pub(crate) const DWELL_MS: u64 = 300;
/// How long the card lingers after the pointer leaves its trigger or the card itself, so the
/// pointer can cross the gap into the card to click "Message" or "View full profile" and the
/// card promptly disappears once neither is hovered.
pub(crate) const GRACE_MS: u64 = 250;

// ---------------------------------------------------------------------------
// Triggers and routing
// ---------------------------------------------------------------------------

/// One hover trigger under the pointer: who it points at, where the card anchors (window
/// coordinates) and the reporting element's key. The key lets a leave from an element the
/// pointer has already moved off be told apart from a newer trigger's enter.
#[derive(Clone, PartialEq)]
pub(crate) struct Trigger {
    pub key: SharedString,
    pub user: UserId,
    pub anchor: Bounds<Pixels>,
}

/// Window-wide routing of trigger hover reports into the root (installed by [`init`]); the
/// reports carry no window state, so a missing root (tests, dialogs) makes them no-ops.
struct Router {
    app: WeakEntity<Root>,
}

impl Global for Router {}

/// Route hover reports to `app`'s lifecycle. Called once from `App::new`.
pub(crate) fn init(app: &Entity<Root>, cx: &mut App) {
    cx.set_global(Router {
        app: app.downgrade(),
    });
}

/// The pointer entered a profile hover trigger (an @mention pill or an author name).
pub(crate) fn entered(trigger: Trigger, cx: &mut App) {
    update(cx, |app, cx| app.profile_hover_entered(trigger, cx));
}

/// The pointer left the trigger registered under `key`.
pub(crate) fn left(key: &SharedString, cx: &mut App) {
    update(cx, |app, cx| app.profile_hover_left(key, cx));
}

fn update(cx: &mut App, route: impl FnOnce(&mut Root, &mut Context<Root>)) {
    let Some(app) = cx
        .try_global::<Router>()
        .and_then(|router| router.app.upgrade())
    else {
        return;
    };
    app.update(cx, route);
}

/// The root's member-hover-card lifecycle state: the trigger under the pointer, whether the
/// pointer is inside the open card, the card shown, and the pending dwell / grace timers
/// (dropping a timer cancels it).
#[derive(Default)]
pub(crate) struct Lifecycle {
    /// The trigger under the pointer (`None` once it reports a leave).
    pub trigger: Option<Trigger>,
    /// The pointer is inside the open card (the card's own hover).
    pub inside: bool,
    /// The card shown.
    pub card: Option<Trigger>,
    /// Pending dwell (enter -> show) and grace (leave -> close).
    pub dwell: Option<Task<()>>,
    pub grace: Option<Task<()>>,
}

/// Where the card anchors for a trigger: above one in the lower half of the viewport (the
/// card's bottom edge at the trigger), below one in the upper half, with Slack's small gap.
pub(crate) fn anchor_for(
    trigger: &Bounds<Pixels>,
    viewport: Size<Pixels>,
) -> (Anchor, Point<Pixels>) {
    /// The gap between trigger and card.
    const GAP: f32 = 4.;
    let middle = trigger.origin.y + trigger.size.height / 2.;
    if middle > viewport.height / 2. {
        (
            Anchor::BottomLeft,
            point(trigger.origin.x, trigger.origin.y - px(GAP)),
        )
    } else {
        (
            Anchor::TopLeft,
            point(trigger.origin.x, trigger.bottom() + px(GAP)),
        )
    }
}

// ---------------------------------------------------------------------------
// Card data
// ---------------------------------------------------------------------------

/// What the card shows for one user (see [`derive`]).
pub(crate) struct Profile {
    /// The headline name (the raw id while an external user is still resolving).
    pub name: String,
    pub pronouns: Option<String>,
    /// The job title line, when the profile carries one.
    pub title: Option<String>,
    /// Presence dot: `Some(active)` renders green / hollow; `None` while the record is
    /// missing (resolving) or the account is deactivated.
    pub active: Option<bool>,
    pub status: Option<Status>,
    /// `4:12 PM local time` on the user's clock, when their zone is known.
    pub local_time: Option<String>,
    /// The signed-in user ("Set a status" instead of "Message").
    pub is_self: bool,
    /// Another, undeleted member with a resolved record: the "Message" button.
    pub can_message: bool,
    /// True when the member belongs to an external Slack Connect organization (`F-COLLAB-800`).
    pub is_external: bool,
    /// Resolved external organization record when available.
    pub external_team: Option<Team>,
}

/// A custom status line: emoji shortcode (colons trimmed) and text.
pub(crate) type Status = DerivedStatus;

/// Derive the card for `user_id` via the shared [`derive_user_profile`] helper: the signed-in
/// user reads their status and presence from `self_state`; anyone else from their profile
/// (`status_expiration` respected, like the author line's emoji) and the workspace's presence map.
pub(crate) fn derive(fixtures: &Fixtures, self_state: &SelfState, user_id: &UserId) -> Profile {
    let derived = derive_user_profile(fixtures, self_state, user_id, None);
    Profile {
        name: derived.name,
        pronouns: derived.pronouns,
        title: derived.title,
        active: derived.active,
        status: derived.status,
        local_time: derived.local_time.map(|(time, _)| time),
        is_self: derived.is_self,
        can_message: derived.can_message,
        is_external: derived.is_external,
        external_team: derived.external_team,
    }
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

/// What the card does (the root wires each to its own methods).
pub(crate) struct CardActions {
    /// The name or "View full profile" was clicked: open the full profile panel.
    pub open_profile: CardAction,
    /// "Message" was clicked: open the DM.
    pub message: CardAction,
    /// "Set a status" was clicked (the signed-in user).
    pub set_status: CardAction,
    /// The pointer entered / left the card (the grace's "still open" leg).
    pub hovered: CardHoverAction,
    /// A press landed outside the card.
    pub dismiss: CardAction,
}

/// One card action, wired by the root to one of its methods.
type CardAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// The card's own hover report (enter / leave).
type CardHoverAction = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// The hover card (`p-member_profile_hover_card`): 340px wide, 56px avatar with the
/// external badge, name + pronouns + presence dot, title, "View full profile", custom
/// status and local time, and the full-width action (Message / Set a status).
pub(crate) fn card(
    fixtures: &Fixtures,
    self_state: &SelfState,
    user_id: &UserId,
    actions: &CardActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let (popover, border, muted, link, success) = (
        theme.popover,
        theme.border,
        theme.muted_foreground,
        theme.link,
        theme.success,
    );
    let profile = derive(fixtures, self_state, user_id);
    let avatar = crate::avatars::with_external_badge(
        fixtures,
        Some(user_id),
        crate::avatars::user_chip(fixtures, user_id, 56.),
        56.,
        popover,
    );
    let presence = if profile.is_external {
        Some(
            div()
                .flex_none()
                .size(px(10.))
                .rounded(px(2.5))
                .border_1()
                .border_color(muted),
        )
    } else {
        profile.active.map(|active| {
            div()
                .flex_none()
                .size(px(10.))
                .rounded_full()
                .when(active, |dot| dot.bg(success))
                .when(!active, |dot| dot.border_1().border_color(muted))
        })
    };
    let open_profile = actions.open_profile.clone();
    let name_row = h_flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .child(
            div()
                .id("member-hover-name")
                .min_w_0()
                .truncate()
                .cursor_pointer()
                .text_size(px(16.))
                .font_weight(FontWeight::BOLD)
                .child(profile.name)
                .on_click(move |_, window, cx| open_profile(window, cx)),
        )
        .children(profile.pronouns.map(|pronouns| {
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(muted)
                .child(pronouns)
        }))
        .children(presence);
    let open_profile = actions.open_profile.clone();
    let external_badge = profile.is_external.then(|| {
        let label = profile
            .external_team
            .as_ref()
            .map(|t| format!("External · {}", t.name))
            .unwrap_or_else(|| "External organization".to_owned());
        h_flex()
            .items_center()
            .gap_1()
            .mt_0p5()
            .px_1p5()
            .py(px(1.))
            .rounded_sm()
            .border_1()
            .border_color(border)
            .bg(theme.secondary.opacity(0.6))
            .child(if let Some(ref team) = profile.external_team {
                crate::channel_details::org_chip(team, 13.).into_any_element()
            } else {
                Icon::new(IconName::Building2)
                    .size(px(12.))
                    .text_color(muted)
                    .into_any_element()
            })
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child(label),
            )
            .when(
                profile
                    .external_team
                    .as_ref()
                    .is_some_and(|t| t.is_verified),
                |chip| chip.child(Icon::new(IconName::Check).size(px(10.)).text_color(link)),
            )
    });
    let identity = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(name_row)
        .children(external_badge)
        .children(profile.title.map(|title| {
            div()
                .min_w_0()
                .truncate()
                .text_size(px(13.))
                .text_color(muted)
                .child(title)
        }))
        .child(
            div()
                .id("member-hover-profile-link")
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(link)
                .hover(|link| link.underline())
                .child("View full profile")
                .on_click(move |_, window, cx| open_profile(window, cx)),
        );
    let details = (profile.status.is_some() || profile.local_time.is_some() || profile.is_external)
        .then(|| {
            v_flex()
                .gap_1p5()
                .mt_3()
                .when(profile.is_external, |col| {
                    col.child(
                        div().text_xs().text_color(muted).child(
                            "External person · Profile details managed by their organization",
                        ),
                    )
                })
                .children(profile.status.map(|status| {
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(crate::emoji::emoji_element(
                            &status.emoji,
                            fixtures,
                            px(20.),
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .text_size(px(13.))
                                .child(status.text),
                        )
                }))
                .children(profile.local_time.map(|time| {
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(Icon::new(IconName::Clock).text_color(muted).size(px(14.)))
                        .child(div().text_size(px(13.)).child(time))
                }))
        });
    let action = if profile.is_self {
        let set_status = actions.set_status.clone();
        Some(
            Button::new("member-hover-status")
                .outline()
                .label("Set a status")
                .icon(IconName::FaceGrinning)
                .w_full()
                .on_click(move |_, window, cx| set_status(window, cx)),
        )
    } else if profile.can_message {
        let message = actions.message.clone();
        Some(
            Button::new("member-hover-message")
                .outline()
                .label("Message")
                .icon(IconName::MessageSquare)
                .w_full()
                .on_click(move |_, window, cx| message(window, cx)),
        )
    } else {
        None
    };
    let (hovered, dismiss) = (actions.hovered.clone(), actions.dismiss.clone());
    div()
        .id("member-hover-card")
        .block_mouse_except_scroll()
        .w(px(340.))
        .rounded(px(8.))
        .border_1()
        .border_color(border)
        .bg(popover)
        .shadow_lg()
        .p_4()
        .on_hover(move |inside: &bool, window, cx| hovered(*inside, window, cx))
        .on_mouse_down_out(move |_: &MouseDownEvent, window, cx| dismiss(window, cx))
        .child(h_flex().items_start().gap_3().child(avatar).child(identity))
        .children(details)
        .children(action.map(|button| div().mt_3().child(button)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::UserStatus;
    use gpui_kit::gpui::size;
    use slackless_core::types::{Profile as UserProfile, User};

    #[test]
    fn self_and_other_users_derive_their_actions() {
        let fixtures = crate::fixtures::load();
        let self_state = SelfState {
            away: true,
            ..SelfState::default()
        };
        let mine = derive(&fixtures, &self_state, &fixtures.self_user);
        assert!(mine.is_self);
        assert!(!mine.can_message, "the signed-in user gets Set a status");
        assert_eq!(
            mine.active,
            Some(false),
            "self away reads as the hollow dot"
        );
        let peer = fixtures
            .dms
            .iter()
            .find_map(|dm| dm.user.clone())
            .expect("the sample workspace has a DM peer");
        let theirs = derive(&fixtures, &self_state, &peer);
        assert!(!theirs.is_self);
        assert!(theirs.can_message);
    }

    #[test]
    fn status_follows_self_state_and_expires_for_others() {
        let mut fixtures = crate::fixtures::load();
        let self_state = SelfState {
            status: Some(UserStatus {
                emoji: "coffee".to_owned(),
                text: "Brewing".to_owned(),
                clear_after: 0,
                pause: false,
                expiration: None,
            }),
            ..SelfState::default()
        };
        let mine = derive(&fixtures, &self_state, &fixtures.self_user);
        assert_eq!(
            mine.status
                .as_ref()
                .map(|status| (status.emoji.as_str(), status.text.as_str())),
            Some(("coffee", "Brewing"))
        );
        let expired = UserProfile {
            status_text: Some("Expired".to_owned()),
            status_emoji: Some(":books:".to_owned()),
            status_expiration: Some(1),
            ..Default::default()
        };
        let current = UserProfile {
            status_text: Some("Reading".to_owned()),
            status_emoji: Some(":books:".to_owned()),
            ..Default::default()
        };
        fixtures.users.push(User {
            id: UserId::from("U0EXPIRED"),
            profile: expired,
            ..Default::default()
        });
        fixtures.users.push(User {
            id: UserId::from("U0CURRENT"),
            profile: current,
            ..Default::default()
        });
        assert_eq!(
            derive(&fixtures, &SelfState::default(), &UserId::from("U0EXPIRED")).status,
            None,
            "an expired status drops"
        );
        let live = derive(&fixtures, &SelfState::default(), &UserId::from("U0CURRENT"));
        assert_eq!(
            live.status.map(|status| (status.emoji, status.text)),
            Some(("books".to_owned(), "Reading".to_owned())),
            "the shortcode trims its colons"
        );
    }

    #[test]
    fn local_time_reads_the_users_zone() {
        let mut fixtures = crate::fixtures::load();
        fixtures.users.push(User {
            id: UserId::from("U0TZ"),
            tz_offset: Some(3_600),
            ..Default::default()
        });
        let card = derive(&fixtures, &SelfState::default(), &UserId::from("U0TZ"));
        let expected = format!(
            "{} local time",
            crate::messages::clock(crate::messages::now_secs(), Some(3_600))
        );
        assert_eq!(card.local_time.as_deref(), Some(expected.as_str()));
        let zoneless = User {
            id: UserId::from("U0NOTZ"),
            ..Default::default()
        };
        fixtures.users.push(zoneless);
        assert_eq!(
            derive(&fixtures, &SelfState::default(), &UserId::from("U0NOTZ")).local_time,
            None
        );
    }

    #[test]
    fn an_unresolved_user_shows_a_pending_card() {
        let fixtures = crate::fixtures::load();
        let card = derive(
            &fixtures,
            &SelfState::default(),
            &UserId::from("U0EXTERNAL"),
        );
        assert_eq!(card.name, "U0EXTERNAL", "the id stands in while resolving");
        assert_eq!(card.active, None);
        assert_eq!(card.status, None);
        assert_eq!(card.local_time, None);
        assert!(!card.can_message, "no Message until the record resolves");
    }

    #[test]
    fn the_card_opens_away_from_the_viewports_busy_half() {
        let viewport = size(px(1000.), px(800.));
        let low = Bounds::new(point(px(100.), px(700.)), size(px(40.), px(20.)));
        let (anchor, position) = anchor_for(&low, viewport);
        assert_eq!(anchor, Anchor::BottomLeft);
        assert_eq!(position, point(px(100.), px(700. - 4.)));
        let high = Bounds::new(point(px(100.), px(100.)), size(px(40.), px(20.)));
        let (anchor, position) = anchor_for(&high, viewport);
        assert_eq!(anchor, Anchor::TopLeft);
        assert_eq!(position, point(px(100.), px(100. + 20. + 4.)));
    }

    #[test]
    fn external_user_profile_derives_external_flag_and_organization() {
        let fixtures = crate::fixtures::load();
        let ext = derive(&fixtures, &SelfState::default(), &UserId::from("U0KLA1"));
        assert!(ext.is_external);
        assert_eq!(
            ext.external_team.as_ref().map(|t| t.name.as_str()),
            Some("Klarna")
        );
        assert!(ext.external_team.as_ref().is_some_and(|t| t.is_verified));

        let own = derive(&fixtures, &SelfState::default(), &fixtures.self_user);
        assert!(!own.is_external);
        assert!(own.external_team.is_none());
    }
}
