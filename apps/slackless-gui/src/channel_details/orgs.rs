//! Channel slug normalization, Slack Connect organization summaries, member filters, and date helpers.

use std::collections::HashMap;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, Div, FontWeight, Hsla, ParentElement as _, SharedString, Styled as _, div, img, px,
};
use slackless_core::types::{Conversation, Presence, Profile, Team, TeamId, User, UserId};

use crate::fixtures::Fixtures;

use super::MemberRow;

/// Normalize user input into a valid Slack channel slug (lowercase letters, digits, hyphens/underscores, max 80 chars).
pub fn normalize_channel_slug(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().trim_start_matches('#').chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch == '-' || ch.is_whitespace()) && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// A Slack details card: the bold label (with the card's action, Slack's "Edit", at its
/// right) over the content, on a rounded bordered surface.
pub(super) fn card(label: &str, action: Option<AnyElement>, border: Hsla) -> Div {
    v_flex()
        .flex_none()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label.to_owned()),
                )
                .children(action),
        )
}

/// One connected-organization row of the About tab's Organizations section: the org icon,
/// name, its member count here, and Slack's Channel owner / Private notes.
pub(super) fn org_row(
    team: &Team,
    members: u32,
    is_host: bool,
    conversation: &Conversation,
    muted: Hsla,
) -> Div {
    let mut row = h_flex()
        .flex_none()
        .items_center()
        .gap_2()
        .px_1()
        .py_1p5()
        .child(org_chip(team, 20.))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(team.name.clone()),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(format!("{members} members")),
        );
    if is_host {
        row = row.child(
            div()
                .px_1p5()
                .py_0p5()
                .rounded_sm()
                .border_1()
                .border_color(muted.opacity(0.35))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(muted)
                .child("Channel owner"),
        );
    }
    if conversation.is_private {
        row = row.child(div().text_xs().text_color(muted).child("Private"));
    }
    row
}

/// One organization's Slack Connect line: its member count and record.
pub(crate) struct OrgLine {
    pub team: Team,
    pub members: u32,
}

/// The signed-in workspace's own team id.
pub(crate) fn own_team(fixtures: &Fixtures) -> TeamId {
    fixtures
        .workspaces
        .first()
        .map(|team| team.id.clone())
        .unwrap_or_default()
}

/// Stored profiles by id, so member surfaces look each person up once instead of scanning
/// the directory per member.
pub(super) fn profiles_by_id(users: &[User]) -> HashMap<&UserId, &User> {
    users.iter().map(|user| (&user.id, user)).collect()
}

/// Whether Slack's member list shows `member`: everyone except the app/bot and deactivated
/// members their stored profiles identify (an unknown member is a person whose profile has
/// not landed yet), and never `USLACKBOT`.
pub(super) fn is_listed(member: &UserId, profiles: &HashMap<&UserId, &User>) -> bool {
    member.as_str() != "USLACKBOT"
        && profiles
            .get(member)
            .is_none_or(|user| !user.is_bot && !user.deleted)
}

/// `field` when it holds more than whitespace.
pub(super) fn non_empty(field: &Option<String>) -> Option<String> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The profile's status emoji while it is current, trimmed of its colons (Slack hides
/// statuses past their expiration; `now` is unix seconds).
pub(super) fn current_status_emoji(profile: &Profile, now: u64) -> Option<String> {
    if profile
        .status_expiration
        .is_some_and(|end| end != 0 && end <= now as i64)
    {
        return None;
    }
    non_empty(&profile.status_emoji).map(|code| code.trim_matches(':').to_owned())
}

/// Slack's presence glyph beside a member's name: a green dot when active, a hollow ring
/// when away, and a hollow rounded square for people from another organization.
pub(super) fn member_presence(
    external: bool,
    presence: Option<Presence>,
    muted: Hsla,
    success: Hsla,
) -> Div {
    let glyph = div().flex_none().size(px(9.));
    if external {
        return glyph.rounded(px(2.5)).border_1().border_color(muted);
    }
    match presence {
        Some(Presence::Active) => glyph.rounded_full().bg(success),
        _ => glyph.rounded_full().border_1().border_color(muted),
    }
}

/// The Members tab's filter, Slack's select beside "Find people".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum MemberFilter {
    #[default]
    All,
    /// Everyone from another organization (offered once two or more are present).
    External,
    Managers,
    /// Full members of the workspace: no channel manager, guest or external person.
    Members,
    Guests,
    /// "People from": the signed-in workspace (`None`) or one external organization.
    Team(Option<TeamId>),
}

impl MemberFilter {
    pub(super) fn keeps(&self, row: &MemberRow) -> bool {
        match self {
            Self::All => true,
            Self::External => row.team.is_some() && !row.guest,
            Self::Managers => row.manager,
            Self::Members => !row.manager && !row.guest && row.team.is_none(),
            Self::Guests => row.guest,
            Self::Team(team) => row.team == *team,
        }
    }
}

/// One choice of the member filter: its label and how many rows it keeps.
pub(super) struct FilterChoice {
    pub(super) label: SharedString,
    pub(super) filter: MemberFilter,
    pub(super) count: usize,
}

/// Slack's filter choices over `rows` (its account-type filter and "People from" group):
/// All; External organizations once two or more are present; Channel Managers, Members and
/// Guests; then, when external people are listed, the signed-in workspace and each
/// external organization in row order. Free-standing for the test.
pub(super) fn filter_choices(fixtures: &Fixtures, rows: &[MemberRow]) -> Vec<FilterChoice> {
    // The organization options list only non-guest externals: a guest appears under
    // Guests alone, never under their organization or "External organizations".
    let mut externals: Vec<&TeamId> = Vec::new();
    for team in rows
        .iter()
        .filter(|row| !row.guest)
        .filter_map(|row| row.team.as_ref())
    {
        if !externals.contains(&team) {
            externals.push(team);
        }
    }
    let mut filters = vec![("All".to_owned(), MemberFilter::All)];
    if externals.len() > 1 {
        filters.push(("External organizations".to_owned(), MemberFilter::External));
    }
    filters.extend([
        ("Channel Managers".to_owned(), MemberFilter::Managers),
        ("Members".to_owned(), MemberFilter::Members),
        ("Guests".to_owned(), MemberFilter::Guests),
    ]);
    if !externals.is_empty() {
        let workspace = fixtures
            .workspaces
            .first()
            .map_or_else(|| "Your workspace".to_owned(), |team| team.name.to_string());
        filters.push((workspace, MemberFilter::Team(None)));
        // Every row's team came from `external_team`, whose record is in `teams`.
        for team in externals {
            if let Some(record) = fixtures.teams.iter().find(|record| &record.id == team) {
                filters.push((
                    record.name.to_string(),
                    MemberFilter::Team(Some(team.clone())),
                ));
            }
        }
    }
    filters
        .into_iter()
        .map(|(label, filter)| FilterChoice {
            count: rows.iter().filter(|row| filter.keeps(row)).count(),
            label: label.into(),
            filter,
        })
        .collect()
}

/// The organizations present in `members`: every team a resolved profile belongs to, plus the
/// own organization carrying the members without one (own-workspace people need no lookup).
/// The own organization comes first, the connected ones after it by name; an organization
/// whose record has not been fetched yet is left out.
pub(crate) fn orgs(
    own: &TeamId,
    members: &[UserId],
    users: &[User],
    teams: &[Team],
) -> Vec<OrgLine> {
    let profiles = profiles_by_id(users);
    // A channel spans a handful of organizations: a short list beats a map here.
    let mut counts: Vec<(&TeamId, u32)> = Vec::new();
    for member in members.iter().filter(|member| is_listed(member, &profiles)) {
        let team = profiles
            .get(member)
            .and_then(|user| user.team_id.as_ref())
            .unwrap_or(own);
        match counts.iter_mut().find(|(id, _)| *id == team) {
            Some(entry) => entry.1 += 1,
            None => counts.push((team, 1)),
        }
    }
    let mut lines: Vec<OrgLine> = counts
        .into_iter()
        .filter_map(|(id, members)| {
            let team = teams.iter().find(|team| team.id == *id).cloned()?;
            Some(OrgLine { team, members })
        })
        .collect();
    // The own organization first, then the connected ones by name; unknown orgs sort by id.
    lines.sort_by(|a, b| {
        let own = |line: &OrgLine| line.team.id == *own;
        own(b)
            .cmp(&own(a))
            .then_with(|| a.team.name.cmp(&b.team.name))
    });
    lines
}

/// The Slack Connect banner's lines: [`orgs`] minus the own organization.
pub(crate) fn external_orgs(
    own: &TeamId,
    members: &[UserId],
    users: &[User],
    teams: &[Team],
) -> Vec<OrgLine> {
    orgs(own, members, users, teams)
        .into_iter()
        .filter(|line| line.team.id != *own)
        .collect()
}

/// The team hosting the conversation: the host id the boot payloads carry (a Slack Connect
/// channel another organization hosts), else `context_team_id`, which names the viewing
/// workspace rather than the host.
pub(crate) fn host_team(conversation: &Conversation) -> Option<TeamId> {
    conversation
        .extra
        .get("conversation_host_id")
        .and_then(|host| host.as_str())
        .map(TeamId::from)
        .or_else(|| conversation.context_team_id.clone())
}

/// The smallest icon URL of a team record (`icon.image_34`, then the larger sizes).
pub(crate) fn team_icon_url(team: &Team) -> Option<SharedString> {
    let icon = team.icon.as_ref()?;
    [
        "image_34",
        "image_44",
        "image_68",
        "image_88",
        "image_132",
        "image_230",
    ]
    .into_iter()
    .find_map(|key| icon.get(key).and_then(|url| url.as_str()))
    .filter(|url| !url.is_empty())
    .map(SharedString::from)
}

/// An organization's avatar: its icon, else an initials chip in its identity color.
pub(crate) fn org_chip(team: &Team, size: f32) -> Div {
    match team_icon_url(team) {
        Some(url) => div()
            .size(px(size))
            .flex_none()
            .rounded(px(crate::avatars::avatar_radius(size)))
            .bg(crate::theme::avatar_color(team.id.as_str()))
            // GPUI clips square: the image rounds its own corners.
            .child(
                img(url)
                    .size_full()
                    .rounded(px(crate::avatars::avatar_radius(size))),
            ),
        None => {
            let initial = team
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            div()
                .size(px(size))
                .flex_none()
                .rounded(px((size / 5.).max(3.)))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .bg(crate::theme::avatar_color(team.id.as_str()))
                .text_color(crate::theme::avatar_foreground())
                .child(initial)
        }
    }
}

/// `June 25, 2020` for a channel's `created` unix seconds: Slack's "Created by" date, in UTC
/// (messages.rs keeps the local-clock machinery for the transcript; this one only ever names
/// a day of a stored creation date).
pub(crate) fn created_on(secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let (year, month, day) = slackless_core::types::time::civil_from_days(secs.div_euclid(86_400));
    format!("{} {}, {}", MONTHS[(month - 1) as usize], day, year)
}
