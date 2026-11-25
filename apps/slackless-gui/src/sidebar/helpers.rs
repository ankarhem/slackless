//! Sidebar helper types, layout metrics, tooltips, count pills, and mention matching.

use super::*;

/// A "More unreads" scroll in flight: Slack eases the list to its target instead of jumping.
#[derive(Clone, Copy)]
pub(super) struct ScrollAnim {
    pub(super) from: Pixels,
    pub(super) to: Pixels,
    pub(super) start: Instant,
    /// The offset this animation set last; finding any other means the user scrolled.
    pub(super) last: Pixels,
}

/// The open "More conversations" popover: its section and the menu, until the menu dismisses.
pub(super) struct SectionSearch {
    pub(super) key: SharedString,
    pub(super) menu: Entity<PopupMenu>,
    pub(super) _dismiss: Subscription,
}

/// What the "Create a channel section" dialog builds when it confirms.
pub(super) enum SectionFormMode {
    /// A plain new section.
    Create,
    /// `Rename(section)` of an existing one.
    Rename(String),
    /// A new section that `CreateWithChannel(channel)` moves into.
    CreateWithChannel(ChannelId),
}

/// Where a channel dropped on a section lands: `Some(target)` accepts the drop, with
/// `target` the custom section's id or `None` for the default Channels bucket. The DMs
/// bucket and the server-managed buckets take no drops.
pub(super) fn drop_target(
    section: Option<&ChannelSection>,
    key: &SharedString,
) -> Option<Option<String>> {
    match section {
        Some(section) => match section.kind {
            SectionKind::Standard => Some(Some(section.id.clone())),
            SectionKind::Channels => Some(None),
            _ => None,
        },
        None => (key.as_ref() == "channels").then_some(None),
    }
}

/// Whether a section takes DM drops (custom sections do; the Channels bucket does not).
pub(super) fn accepts_dms(section: Option<&ChannelSection>, key: &SharedString) -> bool {
    match section {
        Some(section) => section.kind == SectionKind::Standard,
        None => key.as_ref() != "channels",
    }
}

pub(super) const SCROLL_ANIM: Duration = Duration::from_millis(300);

/// Slack's tooltip over a Slack Connect row, centered above the name with its arrow pointing
/// down at it: the other organizations' icons in framed tiles (three, then a "+N" tile) over
/// "A, B, C, and N other external organizations are in this channel", in Slack's inverse
/// tooltip colors and caption type. `None` while no organization record is known.
pub(super) fn org_tooltip(teams: &[&Team], cx: &gpui_kit::gpui::App) -> Option<Div> {
    let text = match teams {
        [] => return None,
        [a] => format!("{} is in this channel", a.name),
        [a, b] => format!("{} and {} are in this channel", a.name, b.name),
        [a, b, c] => format!("{}, {}, and {} are in this channel", a.name, b.name, c.name),
        [a, b, c, rest @ ..] => format!(
            "{}, {}, {}, and {} other external organization{} are in this channel",
            a.name,
            b.name,
            c.name,
            rest.len(),
            if rest.len() == 1 { "" } else { "s" }
        ),
    };
    let theme = cx.theme();
    let (surface, ink, plus) = (theme.foreground, theme.background, theme.muted_foreground);
    let frame = |tile: Div| div().p(px(2.)).rounded(px(10.)).bg(ink).child(tile);
    let more = teams.len().saturating_sub(3);
    let tiles = h_flex()
        .justify_center()
        .gap_1()
        .my_1()
        .children(
            teams
                .iter()
                .take(3)
                .map(|team| frame(crate::channel_details::org_chip(team, 36.))),
        )
        .when(more > 0, |tiles| {
            tiles.child(frame(
                div()
                    .size(px(36.))
                    .rounded(px(8.))
                    .bg(plus)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(20.))
                    .child(if more < 9 {
                        format!("+{more}")
                    } else {
                        "9+".to_owned()
                    }),
            ))
        });
    Some(
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(relative(1.))
            .pb(px(2.))
            .flex()
            .justify_center()
            .child(
                v_flex()
                    .items_center()
                    .child(
                        v_flex()
                            .max_w(px(200.))
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .bg(surface)
                            .text_color(ink)
                            .text_size(px(13.))
                            .line_height(px(16.25))
                            .font_weight(FontWeight::BOLD)
                            .text_center()
                            .child(tiles)
                            .child(text),
                    )
                    .child(tooltip_arrow(surface, false)),
            ),
    )
}

/// A Slack tooltip's arrow in the tooltip's color: a small triangle pointing down at the
/// trigger below the tooltip, or up at one above it.
pub(crate) fn tooltip_arrow(color: Hsla, up: bool) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (left, top) = (bounds.origin.x, bounds.origin.y);
            let (width, height) = (bounds.size.width, bounds.size.height);
            let (base, tip) = if up {
                (top + height, top)
            } else {
                (top, top + height)
            };
            let mut path = PathBuilder::fill();
            path.move_to(point(left, base));
            path.line_to(point(left + width, base));
            path.line_to(point(left + width / 2., tip));
            path.close();
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(14.))
    .h(px(7.))
}

/// Focus mode's subtle unread marker, in place of bolding and count pills.
pub(super) fn focus_dot(cx: &gpui_kit::gpui::App) -> Div {
    div()
        .flex_none()
        .size(px(6.))
        .rounded_full()
        .bg(cx.theme().sidebar_foreground.opacity(0.6))
}

/// Slack-style inline count pill: red for mentions (and DM messages), the sidebar's primary
/// emphasis colors otherwise.
pub(super) fn count_pill(count: u32, mentions: bool, cx: &gpui_kit::gpui::App) -> Div {
    let theme = cx.theme();
    let (bg, fg) = if mentions {
        crate::theme::mention_badge(cx)
    } else {
        (theme.sidebar_primary, theme.sidebar_primary_foreground)
    };
    // Slack's pill is wider than tall: 18px high, at least 25px wide.
    h_flex()
        .flex_none()
        .justify_center()
        .items_center()
        .gap_0p5()
        .h(px(18.))
        .min_w(px(25.))
        .px_2()
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .child(count.to_string())
}

/// One flattened child of the sidebar's scroll list: unread conversation rows are the ones
/// the "More unreads" pills track.
pub(super) enum ListChild {
    /// An unread conversation row; the flag says it carries mentions.
    Unread(bool),
    /// Headers, read rows and dividers.
    Plain,
}

/// Room kept between a revealed unread row and the list edge: the pill's height plus its inset.
pub(super) const PILL_CLEARANCE: f32 = 44.;

/// One edge of the list's hidden unreads: the unread row the pill scrolls to next, and
/// whether any unread row hidden past that edge carries mentions. `below` picks the pill's
/// arrow and the list edge it rides.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HiddenUnreads {
    pub(super) row: usize,
    /// The hidden unread row farthest past the edge.
    pub(super) far: usize,
    pub(super) mentions: bool,
    pub(super) below: bool,
}

/// The "More unreads" pills for `unread_rows` (child indices with mention flags, in list
/// order): the first unread row below the fold and the last one scrolled above it. Rows are
/// placed with the scroll handle's recorded child bounds; `None` on an edge whose unread
/// rows are in view (or before the list has laid out).
pub(super) fn hidden_unreads(
    scroll: &ScrollHandle,
    unread_rows: &[(usize, bool)],
) -> (Option<HiddenUnreads>, Option<HiddenUnreads>) {
    let viewport = scroll.bounds();
    // Child bounds live in the layout's unscrolled space; scrolling down drives the offset
    // negative (`ScrollHandle::set_offset`), so subtracting it maps the viewport's edges
    // into that space.
    let top = viewport.top() - scroll.offset().y;
    let bottom = viewport.bottom() - scroll.offset().y;
    let mut below = Option::<HiddenUnreads>::None;
    let mut above = Option::<HiddenUnreads>::None;
    for &(row, mentions) in unread_rows {
        if let Some(bounds) = scroll.bounds_for_item(row) {
            if bounds.bottom() > bottom {
                let pill = below.get_or_insert(HiddenUnreads {
                    row,
                    far: row,
                    mentions: false,
                    below: true,
                });
                pill.far = row;
                pill.mentions |= mentions;
            } else if bounds.top() < top {
                above = Some(HiddenUnreads {
                    row,
                    far: above.map_or(row, |pill| pill.far),
                    mentions: above.is_some_and(|pill| pill.mentions) || mentions,
                    below: false,
                });
            }
        }
    }
    (below, above)
}

/// A section's header: Slack's current label for a built-in bucket (the stored names lag it:
/// "Slack Connect", "Direct Messages"), else the user's name, led by the section emoji (when
/// it resolves) or the bucket's icon; `None` leaves the header's own caret square.
pub(super) fn section_title(
    section: &ChannelSection,
    fixtures: &Fixtures,
) -> (String, Option<AnyElement>) {
    let icon = |name: IconName| Some(Icon::new(name).size(px(15.)).into_any_element());
    let emoji = &section.emoji;
    let resolves = emoji::display_glyph(emoji).is_some() || fixtures.custom_emoji(emoji).is_some();
    let (builtin_name, leading) = match section.kind {
        SectionKind::Channels => (Some("Channels"), None),
        SectionKind::DirectMessages => (Some("Direct messages"), None),
        SectionKind::Stars => (Some("Starred"), icon(IconName::Star)),
        SectionKind::SlackConnect => (Some("External connections"), icon(IconName::Building2)),
        SectionKind::RecentApps => (Some("Apps"), icon(IconName::LayoutGrid)),
        SectionKind::Agents => (Some("Agents"), icon(IconName::Bot)),
        _ if !emoji.is_empty() && resolves => {
            (None, Some(emoji::emoji_element(emoji, fixtures, px(15.))))
        }
        _ => (None, None),
    };
    let name = builtin_name.map_or_else(|| section.name.clone(), str::to_owned);
    (name, leading)
}

/// The workspace icon Slack serves (`team.icon.image_88`, ...), unless it is the generated
/// default.
pub(super) fn team_icon_url(team: &Team) -> Option<String> {
    let icon = team.icon.as_ref()?;
    if icon.get("image_default").and_then(|value| value.as_bool()) == Some(true) {
        return None;
    }
    ["image_88", "image_68", "image_132", "image_44"]
        .iter()
        .find_map(|key| icon.get(*key)?.as_str())
        .map(str::to_owned)
}

/// Up to the first two initials of a workspace name, e.g. "Acme Corp" -> "AC".
pub(crate) fn team_initials(name: &str) -> String {
    name.split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// Whether `message` @-mentions `user`, in the mrkdwn fallback text or the rich-text blocks.
pub(crate) fn mentions_user(message: &Message, user: &UserId) -> bool {
    let needle = format!("<@{}>", user.as_str());
    message.text.contains(&needle)
        || message.blocks.iter().any(|block| match block {
            Block::RichText(rich) => rich
                .elements
                .iter()
                .any(|container| container_mentions_user(container, user)),
            Block::Unknown { .. } => false,
        })
}

pub(super) fn container_mentions_user(container: &RichTextContainer, user: &UserId) -> bool {
    match container {
        RichTextContainer::Section(section) => section
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::List(list) => list
            .elements
            .iter()
            .any(|nested| container_mentions_user(nested, user)),
        RichTextContainer::Quote(quote) => quote
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Preformatted(pre) => pre
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Header(header) => header
            .elements
            .iter()
            .any(|element| element_mentions_user(element, user)),
        RichTextContainer::Divider(_) => false,
        RichTextContainer::Unknown { .. } => false,
    }
}

pub(super) fn element_mentions_user(element: &RichTextElement, user: &UserId) -> bool {
    matches!(element, RichTextElement::User(mention) if mention.user_id == *user)
}
