//! Reaction pills, reaction tooltips, and local reaction mutations.

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    self, AnyView, App, Div, FontWeight, HighlightStyle, InteractiveElement as _,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledText, Window, div, px,
};
use slackless_core::types::{Message, Reaction, UserId};

use super::{ActionHandler, MessageAction};
use crate::emoji;
use crate::fixtures::Fixtures;

/// The space Slack keeps under a message's reaction pills (each pill's 4px bottom margin
/// plus its bar's), less the message card's own 2px bottom padding.
pub const REACTIONS_BOTTOM: Pixels = px(6.);

/// Reaction chips under a message. With a handler, a chip toggles the signed-in user's
/// reaction and a trailing `+` opens the picker; without one the chips are static.
pub fn reaction_chips(
    key: &str,
    reactions: &[Reaction],
    fixtures: &Fixtures,
    handler: Option<&ActionHandler>,
    cx: &App,
) -> Option<Div> {
    if reactions.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let (secondary, secondary_hover, foreground) =
        (theme.secondary, theme.secondary_hover, theme.foreground);
    let (reacted_bg, reacted_fg, reacted_ring) = crate::theme::reacted_pill(cx);
    let mut row = h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .pt_1()
        .pb(REACTIONS_BOTTOM);
    for reaction in reactions {
        let mine = reaction.users.contains(&fixtures.self_user);
        // Slack's `.c-reaction`: a 24px capsule, filled blue and bold when it includes you.
        let chip = h_flex()
            .id(SharedString::from(format!(
                "{key}-reaction-{}",
                reaction.name
            )))
            .h(px(24.))
            .items_center()
            .gap_1()
            .px_2()
            .rounded_full()
            .text_xs()
            .when(mine, |chip| {
                chip.bg(reacted_bg)
                    .font_weight(FontWeight::BOLD)
                    .when_some(reacted_ring, |chip, ring| {
                        chip.border_1().border_color(ring)
                    })
            })
            .when(!mine, |chip| {
                chip.bg(secondary).hover(|chip| chip.bg(secondary_hover))
            })
            // Slack's reaction emoji fills a 16px box. A glyph inks about a quarter past its
            // font size (20px of ink at 16px), so it is set at 13px; an image takes the box.
            .child(emoji::emoji_element(
                &reaction.name,
                fixtures,
                if emoji::glyph(&reaction.name).is_some() {
                    px(13.)
                } else {
                    px(16.)
                },
            ))
            .child(
                div()
                    .text_color(if mine { reacted_fg } else { foreground })
                    .child(reaction.count.to_string()),
            )
            .tooltip(reaction_tooltip(
                emoji::EmojiArt::of(&reaction.name, fixtures),
                reaction_tip(reaction, fixtures, handler.is_some()),
            ))
            .when_some(handler.cloned(), |chip, handler| {
                let name = reaction.name.clone();
                chip.cursor_pointer().on_click(move |_, window, cx| {
                    handler(MessageAction::ToggleReaction(name.clone()), window, cx);
                })
            });
        row = row.child(chip);
    }
    if let Some(handler) = handler.cloned() {
        // Slack's add-reaction control is a capsule the size of a reaction.
        row = row.child(
            h_flex()
                .id(SharedString::from(format!("{key}-reaction-add")))
                .h(px(24.))
                .px_2()
                .items_center()
                .rounded_full()
                .bg(secondary)
                .hover(|pill| pill.bg(secondary_hover))
                .cursor_pointer()
                .child(
                    Icon::new(IconName::FaceSlightlySmilingPlus)
                        .size(px(16.))
                        .text_color(foreground),
                )
                .tooltip(|window, cx| Tooltip::new("Add reaction").build(window, cx))
                .on_click(move |event, window, cx| {
                    handler(MessageAction::PickReaction(event.position()), window, cx);
                }),
        );
    }
    Some(row)
}

/// Slack's reaction tooltip text (`getTip`): who reacted, then "reacted with :name:". One
/// reactor reads "You (click to remove)" for yourself on a pill that toggles; several are
/// listed "A, B, and you", ending in "others" when Slack sent only some of the reactors.
/// External Slack Connect reactors carry their org name and unrecognized non-Unicode shortcodes
/// are annotated as external workspace emoji (`F-COLLAB-878`).
fn reaction_tip(
    reaction: &Reaction,
    fixtures: &Fixtures,
    removable: bool,
) -> (SharedString, SharedString) {
    let me = &fixtures.self_user;
    let display_reactor = |user: &UserId| -> String {
        let base = fixtures.user_name(user);
        match fixtures.external_team(user) {
            Some(team) if !team.name.is_empty() => format!("{base} ({})", team.name),
            _ => base.to_owned(),
        }
    };
    let who = match reaction.users.as_slice() {
        [only] if only == me && removable => "You (click to remove)".to_owned(),
        [only] if only == me => "You".to_owned(),
        [only] => display_reactor(only),
        users => {
            let mut names: Vec<String> = users
                .iter()
                .enumerate()
                .map(|(ix, user)| match user == me {
                    true if ix == 0 => "You".to_owned(),
                    true => "you".to_owned(),
                    false => display_reactor(user),
                })
                .collect();
            if reaction.count as usize > users.len() {
                names.push("others".to_owned());
            }
            match names.as_slice() {
                [one] => one.clone(),
                [one, other] => format!("{one} and {other}"),
                [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
                [] => String::new(),
            }
        }
    };
    let is_external_emoji =
        emoji::glyph(&reaction.name).is_none() && fixtures.custom_emoji(&reaction.name).is_none();
    let what = if is_external_emoji {
        format!(
            "reacted with :{}: · External workspace emoji",
            reaction.name
        )
    } else {
        format!("reacted with :{}:", reaction.name)
    };
    (who.into(), what.into())
}

/// Slack's reaction tooltip (`.c-reaction__tip`): the emoji large on a white 36px tile, then
/// who reacted in bold with "reacted with :name:" muted, centred in at most 200px.
fn reaction_tooltip(
    art: emoji::EmojiArt,
    (who, what): (SharedString, SharedString),
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let (art, who, what) = (art.clone(), who.clone(), what.clone());
        Tooltip::element(move |_, cx| {
            // A glyph inks about a quarter past its font size; an image fills the box.
            let size = match art {
                emoji::EmojiArt::Glyph(_) => px(26.),
                _ => px(32.),
            };
            let line = format!("{who} {what}");
            let subtitle = HighlightStyle {
                color: Some(cx.theme().muted_foreground),
                font_weight: Some(FontWeight::NORMAL),
                ..Default::default()
            };
            v_flex()
                .items_center()
                .max_w(px(200.))
                .py_1()
                .gap_1()
                .text_size(px(13.))
                .line_height(px(16.))
                .font_weight(FontWeight::BOLD)
                .child(
                    div()
                        .size(px(36.))
                        .p(px(2.))
                        .rounded(px(4.))
                        .bg(gpui::white())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(art.element(size)),
                )
                .child(
                    div().text_center().child(
                        StyledText::new(line).with_highlights([(
                            who.len() + 1..who.len() + 1 + what.len(),
                            subtitle,
                        )]),
                    ),
                )
        })
        .build(window, cx)
    }
}

/// Toggle `me`'s `name` reaction on `message`; an emptied aggregate is removed. Returns
/// whether `me` has the reaction afterwards.
pub fn toggle_reaction(message: &mut Message, me: &UserId, name: &str) -> bool {
    match message.reactions.iter().position(|r| r.name == name) {
        Some(index) => {
            let reaction = &mut message.reactions[index];
            let reacted = match reaction.users.iter().position(|user| user == me) {
                Some(position) => {
                    reaction.users.remove(position);
                    false
                }
                None => {
                    reaction.users.push(me.clone());
                    true
                }
            };
            reaction.count = reaction.users.len() as u32;
            if reaction.count == 0 {
                message.reactions.remove(index);
            }
            reacted
        }
        None => add_reaction(message, me, name),
    }
}

/// Add `me`'s `name` reaction unless it is already there; returns whether it was added.
pub fn add_reaction(message: &mut Message, me: &UserId, name: &str) -> bool {
    match message.reactions.iter_mut().find(|r| r.name == name) {
        Some(reaction) => {
            let added = !reaction.users.contains(me);
            if added {
                reaction.users.push(me.clone());
            }
            reaction.count = reaction.users.len() as u32;
            added
        }
        None => {
            message.reactions.push(Reaction {
                name: name.to_owned(),
                count: 1,
                users: vec![me.clone()],
            });
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use slackless_core::types::{Reaction, Team, TeamId, User, UserId};

    use super::reaction_tip;

    #[test]
    fn reaction_tip_annotates_external_reactors_and_external_workspace_emoji() {
        let mut fixtures = crate::fixtures::load();
        let ext_team_id = TeamId::from("T9PARTNER");
        fixtures.teams.push(Team {
            id: ext_team_id.clone(),
            name: "PartnerCo".to_owned(),
            domain: "partnerco".to_owned(),
            ..Default::default()
        });
        let ext_user_id = UserId::from("U9EXT");
        let ext_user = User {
            id: ext_user_id.clone(),
            name: "ext_pat".to_owned(),
            real_name: Some("Pat External".to_owned()),
            team_id: Some(ext_team_id),
            ..Default::default()
        };
        fixtures.users_mut().push(ext_user);

        // External reactor + unknown custom shortcode -> annotated as external workspace emoji.
        let reaction = Reaction {
            name: "partner_party".to_owned(),
            count: 1,
            users: vec![ext_user_id.clone()],
        };
        let (who, what) = reaction_tip(&reaction, &fixtures, true);
        assert_eq!(who.as_ref(), "Pat External (PartnerCo)");
        assert!(
            what.contains("External workspace emoji"),
            "unexpected tooltip subtitle: {what}"
        );

        // Standard Unicode emoji from external reactor should NOT be marked external emoji.
        let unicode_reaction = Reaction {
            name: "thumbsup".to_owned(),
            count: 1,
            users: vec![ext_user_id],
        };
        let (who2, what2) = reaction_tip(&unicode_reaction, &fixtures, true);
        assert_eq!(who2.as_ref(), "Pat External (PartnerCo)");
        assert_eq!(what2.as_ref(), "reacted with :thumbsup:");
    }
}
