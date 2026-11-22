//! The profile pane (right-hand panel, in place of the thread panel): large avatar, name and
//! pronouns, title, custom status, presence, local time, contact information and the
//! workspace's custom "About me" fields. Opened from the conversation header's peer avatar /
//! name, and for the signed-in user from the title-bar menu.
//!
//! `ProfilePanel::new(Arc<Fixtures>, &mut App) -> Entity<ProfilePanel>`
//! `ProfilePanel::open(&mut self, UserId, back: bool, &mut Context<Self>)`
//! `ProfilePanel::set_self_state(&mut self, SelfState, &mut Context<Self>)`
//! emits `ProfileEvent::Close` / `ProfileEvent::Back` / `ProfileEvent::Message(UserId)` /
//! `ProfileEvent::EditStatus`

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::types::{Presence, UserId};

use crate::emoji::glyph as emoji_glyph;
use crate::fixtures::Fixtures;
use crate::messages::{clock, local_offset_now, now_secs};
use crate::status::SelfState;

pub enum ProfileEvent {
    Close,
    /// Return to the pane the profile was opened over (the thread).
    Back,
    /// Open the DM with this user.
    Message(UserId),
    /// The signed-in user's "Set a status".
    EditStatus,
    /// The signed-in user's "Edit profile".
    EditProfile,
}

struct LoadedProfile {
    pronouns: Option<String>,
    fields: Vec<(String, String)>,
}

pub struct ProfilePanel {
    fixtures: Arc<Fixtures>,
    user: Option<UserId>,
    /// Opened over another pane: the header leads with Slack's back chevron.
    back: bool,
    self_state: SelfState,
    /// Live-hydrated pronouns & custom fields from `users.profile.get` + `team.profile.get`.
    loaded_profiles: HashMap<UserId, LoadedProfile>,
}

impl EventEmitter<ProfileEvent> for ProfilePanel {}

impl ProfilePanel {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<ProfilePanel> {
        cx.new(|_| ProfilePanel {
            fixtures,
            user: None,
            back: false,
            self_state: SelfState::default(),
            loaded_profiles: HashMap::new(),
        })
    }

    /// Read live-hydrated pronouns & custom fields for `user`, if fetched.
    pub fn loaded_profile(&self, user: &UserId) -> (Option<String>, Vec<(String, String)>) {
        self.loaded_profiles
            .get(user)
            .map(|p| (p.pronouns.clone(), p.fields.clone()))
            .unwrap_or_default()
    }

    /// Store live custom fields & pronouns hydrated from `users.profile.get`.
    pub fn set_user_profile(
        &mut self,
        user: UserId,
        pronouns: Option<String>,
        fields: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.loaded_profiles
            .insert(user, LoadedProfile { pronouns, fields });
        cx.notify();
    }

    /// Live refresh: the shown profile stays open, drawn from `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    /// Show `user`'s profile; `back` when it opens over another pane that its back
    /// chevron returns to.
    pub fn open(&mut self, user: UserId, back: bool, cx: &mut Context<Self>) {
        self.user = Some(user);
        self.back = back;
        cx.notify();
    }

    pub fn set_self_state(&mut self, state: SelfState, cx: &mut Context<Self>) {
        self.self_state = state;
        cx.notify();
    }
}

/// `4:12 PM local time`, plus how far it is from this machine's clock.
fn local_time(offset: i64) -> (String, Option<String>) {
    let now = now_secs();
    let time = format!("{} local time", clock(now, Some(offset)));
    let diff = (offset - local_offset_now()) / 3_600;
    let relative = match diff {
        0 => None,
        1 => Some("1 hour ahead".to_owned()),
        -1 => Some("1 hour behind".to_owned()),
        d if d > 0 => Some(format!("{d} hours ahead")),
        d => Some(format!("{} hours behind", -d)),
    };
    (time, relative)
}

/// A labeled detail line with a leading icon.
fn detail(icon: IconName, text: String, color: Hsla, muted: Hsla) -> Div {
    h_flex()
        .items_center()
        .gap_2()
        .text_sm()
        .text_color(color)
        .child(Icon::new(icon).text_color(muted).size(px(16.)))
        .child(text)
}

/// An outlined action button under the name.
fn action(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    border: Hsla,
    hover: Hsla,
) -> Stateful<Div> {
    h_flex()
        .id(id)
        .items_center()
        .gap_1p5()
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .hover(|el| el.bg(hover))
        .child(Icon::new(icon).size(px(14.)))
        .child(label)
}

impl Render for ProfilePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, foreground, muted, border, hover, link, success) = (
            theme.background,
            theme.foreground,
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.link,
            theme.success,
        );

        let header = h_flex()
            .h(px(46.))
            .flex_none()
            .px_4()
            .items_center()
            .border_b_1()
            .border_color(border)
            .when(self.back, |header| {
                header.child(
                    div()
                        .id("profile-back")
                        .size(px(28.))
                        .mr_3()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|el| el.bg(hover))
                        .child(
                            Icon::new(IconName::ChevronLeft)
                                .text_color(muted)
                                .size(px(16.)),
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Back))),
                )
            })
            .child(
                div()
                    .flex_1()
                    .font_weight(FontWeight::BOLD)
                    .child("Profile"),
            )
            .child(
                div()
                    .id("profile-close")
                    .size(px(28.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(Icon::new(IconName::X).text_color(muted).size(px(16.)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Close))),
            );

        let Some(user) = self
            .user
            .as_ref()
            .and_then(|id| self.fixtures.user(id))
            .cloned()
        else {
            return v_flex().size_full().bg(background).child(header);
        };
        let is_self = user.id == self.fixtures.self_user;
        let extras = self.fixtures.profile_extras.get(&user.id);
        let loaded = self.loaded_profiles.get(&user.id);
        let pronouns = loaded
            .and_then(|p| p.pronouns.clone())
            .or_else(|| {
                user.profile
                    .pronouns
                    .clone()
                    .filter(|p| !p.trim().is_empty())
            })
            .or_else(|| extras.and_then(|e| e.pronouns.clone()));
        let custom_fields: Vec<(String, String)> = match loaded {
            Some(p) if !p.fields.is_empty() => p.fields.clone(),
            _ => extras.map(|e| e.fields.clone()).unwrap_or_default(),
        };
        // The name messages show the person by (full or display name, per the workspace).
        let name = self.fixtures.name_of(&user).to_owned();
        let real_name = user
            .profile
            .real_name
            .clone()
            .or(user.real_name.clone())
            .unwrap_or_default();

        // Slack's big profile photo (an initials tile for someone without one).
        let photo = crate::avatars::user_chip(&self.fixtures, &user.id, 240.);

        let status = if is_self {
            self.self_state
                .status
                .as_ref()
                .map(|s| (s.glyph().to_owned(), s.text.clone()))
        } else {
            user.profile
                .status_text
                .clone()
                .filter(|t| !t.is_empty())
                .map(|text| {
                    let code = user.profile.status_emoji.clone().unwrap_or_default();
                    let glyph = emoji_glyph(code.trim_matches(':'))
                        .map(str::to_owned)
                        .unwrap_or(code);
                    (glyph, text)
                })
        };
        let away = if is_self {
            self.self_state.away
        } else {
            matches!(
                self.fixtures.presence(&user.id),
                Some(Presence::Away) | None
            )
        };
        let presence = h_flex()
            .items_center()
            .gap_2()
            .text_sm()
            .child(
                div()
                    .size(px(10.))
                    .rounded_full()
                    .when(!user.deleted && !away, |dot| dot.bg(success))
                    .when(!user.deleted && away, |dot| {
                        dot.border_1().border_color(muted)
                    }),
            )
            // Slack marks a deactivated person instead of showing a presence.
            .child(if user.deleted {
                "Deactivated account"
            } else if away {
                "Away"
            } else {
                "Active"
            })
            .when(is_self, |row| {
                row.children(self.self_state.pause_label().map(|label| {
                    h_flex()
                        .items_center()
                        .gap_1()
                        .text_color(muted)
                        .child(
                            Icon::new(IconName::MoonStar)
                                .text_color(muted)
                                .size(px(12.)),
                        )
                        .child(label)
                }))
            });

        let time = user.tz_offset.map(|offset| {
            let (time, relative) = local_time(offset);
            let text = match relative {
                Some(relative) => format!("{time} \u{b7} {relative}"),
                None => time,
            };
            detail(IconName::Clock, text, foreground, muted)
        });

        let target = user.id.clone();
        let actions = h_flex()
            .gap_2()
            .when(is_self, |row| {
                row.child(
                    action(
                        "profile-status",
                        IconName::CircleDot,
                        "Set a status",
                        border,
                        hover,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::EditStatus))),
                )
                .child(
                    action(
                        "profile-edit",
                        IconName::Pencil,
                        "Edit profile",
                        border,
                        hover,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::EditProfile))),
                )
            })
            // A deactivated person cannot be messaged.
            .when(!is_self && !user.deleted, |row| {
                row.child(
                    action(
                        "profile-message",
                        IconName::MessageSquare,
                        "Message",
                        border,
                        hover,
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ProfileEvent::Message(target.clone()))
                    })),
                )
            });

        let section = |title: &'static str| {
            div()
                .pt_4()
                .pb_1()
                .font_weight(FontWeight::BOLD)
                .border_t_1()
                .border_color(border)
                .child(title)
        };
        let contact = v_flex()
            .gap_2()
            .child(section("Contact information"))
            .children(user.profile.email.clone().map(|email| {
                v_flex()
                    .child(div().text_xs().text_color(muted).child("Email address"))
                    .child(div().text_sm().text_color(link).child(email))
            }))
            .children(
                user.profile
                    .phone
                    .clone()
                    .filter(|p| !p.is_empty())
                    .map(|phone| {
                        v_flex()
                            .child(div().text_xs().text_color(muted).child("Phone"))
                            .child(div().text_sm().text_color(link).child(phone))
                    }),
            );
        let about = v_flex()
            .gap_2()
            // Like Slack, the section only shows when it has something in it.
            .when(
                user.tz_label.is_some() || !custom_fields.is_empty(),
                |about| about.child(section("About me")),
            )
            .children(user.tz_label.clone().map(|zone| {
                v_flex()
                    .child(div().text_xs().text_color(muted).child("Time zone"))
                    .child(div().text_sm().child(zone))
            }))
            .children(custom_fields.into_iter().map(|(label, value)| {
                v_flex()
                    .child(div().text_xs().text_color(muted).child(label))
                    .child(div().text_sm().child(value))
            }));

        let body = v_flex()
            .id("profile-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(h_flex().justify_center().child(photo))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .text_xl()
                                    .font_weight(FontWeight::BOLD)
                                    .child(name.clone()),
                            )
                            .children(pronouns.map(|p| div().text_sm().text_color(muted).child(p))),
                    )
                    .when(!real_name.is_empty() && real_name != name, |col| {
                        col.child(div().text_sm().text_color(muted).child(real_name.clone()))
                    })
                    .children(
                        user.profile
                            .title
                            .clone()
                            .map(|title| div().text_sm().child(title)),
                    ),
            )
            .children(status.map(|(glyph, text)| {
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(hover)
                    .text_sm()
                    .child(glyph)
                    .child(text)
            }))
            .child(presence)
            .children(time)
            .child(actions)
            .child(contact)
            .child(about);

        v_flex()
            .size_full()
            .bg(background)
            .child(header)
            .child(body)
    }
}
