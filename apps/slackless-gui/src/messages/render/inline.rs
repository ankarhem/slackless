//! Inline text themes and the elements built from a paragraph's segments
//! ([`super::model`]); shortcode and color helpers.

use std::cell::Cell;
use std::hash::{Hash, Hasher};

use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::gpui::{
    AnyElement, App, Div, ElementId, Font, FontStyle, FontWeight, Hsla, InteractiveElement as _,
    InteractiveText, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, TextRun,
    UnderlineStyle, div, px, relative, rgb,
};
use slackless_core::domain::mrkdwn::{
    Directory, mrkdwn_to_plain_with, parse_mrkdwn_spans, replace_shortcodes_with,
};
use slackless_core::types::TextStyle;

use super::super::{INLINE_EMOJI, LinkHandler, LinkTarget, route_link};
use super::SelectableParagraph;
use super::model::{RunStyle, Segment, TextOpts, TextPiece, Tone, line_segments};
use crate::emoji;
use crate::fixtures::Fixtures;

pub(in crate::messages) const DEFAULT_FLAGS: RunFlags = RunFlags {
    bold: false,
    semibold: false,
    italic: false,
    strike: false,
    underline: false,
    code: false,
};

/// Mention chip style: semibold link color on an accent wash (guide section 10).
pub(in crate::messages) fn mention_flags() -> RunFlags {
    RunFlags {
        semibold: true,
        ..DEFAULT_FLAGS
    }
}

/// Inline style flags one text run carries.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(in crate::messages) struct RunFlags {
    pub(in crate::messages) bold: bool,
    pub(in crate::messages) semibold: bool,
    pub(in crate::messages) italic: bool,
    pub(in crate::messages) strike: bool,
    pub(in crate::messages) underline: bool,
    pub(in crate::messages) code: bool,
}

/// Theme-resolved fonts and colors shared by every run of one message render.
pub(in crate::messages) struct RunTheme {
    pub(in crate::messages) base_font: Font,
    /// Code blocks' face, sized by the block.
    pub(in crate::messages) mono_font: Font,
    /// Inline code's face: smaller glyphs at the body size (see
    /// [`crate::theme::INLINE_CODE_FAMILY`]).
    pub(in crate::messages) code_font: Font,
    pub(in crate::messages) foreground: Hsla,
    pub(in crate::messages) muted: Hsla,
    pub(in crate::messages) border: Hsla,
    pub(in crate::messages) link: Hsla,
    pub(in crate::messages) mention_bg: Hsla,
    /// Slack's tint behind chip links: address-labelled links and app chips.
    pub(in crate::messages) link_wash: Hsla,
    pub(in crate::messages) code_bg: Hsla,
    /// Inline `code` text (Slack's orange / red, see [`crate::theme::code_foreground`]).
    pub(in crate::messages) code_fg: Hsla,
    /// The window-selection highlight of selectable paragraphs.
    pub(in crate::messages) selection: Hsla,
    /// Selection document order of this theme's first paragraph; `None` renders plain,
    /// unselectable text (previews).
    pub(in crate::messages) order: Option<u64>,
    /// Routing for this text's links; `None` falls back to [`open_link_default`].
    pub(in crate::messages) links: Option<LinkHandler>,
    /// Always underline website links (Preferences > Accessibility > Underline links).
    pub(in crate::messages) underline_links: bool,
    /// Active dark/light appearance mode (used for syntax-highlighted code runs).
    pub(in crate::messages) dark: bool,
    /// Paragraphs rendered with this theme so far: keeps each paragraph's element id unique
    /// within its message (GPUI keys interaction and accessibility state by the id path)
    /// and counts its selection document order.
    pub(in crate::messages) paragraphs: Cell<usize>,
    /// The message this render belongs to; text ids are also unique across messages.
    pub(in crate::messages) key: SharedString,
}

impl RunTheme {
    fn from_app(
        cx: &App,
        key: SharedString,
        order: Option<u64>,
        links: Option<LinkHandler>,
    ) -> Self {
        let theme = cx.theme();
        let base_font = Font {
            family: theme.font_family.clone(),
            ..Default::default()
        };
        Self {
            mono_font: Font {
                family: theme.mono_font_family.clone(),
                ..base_font.clone()
            },
            code_font: Font {
                family: crate::theme::inline_code_family(cx),
                ..base_font.clone()
            },
            foreground: theme.foreground,
            muted: theme.muted_foreground,
            border: theme.border,
            link: theme.link,
            mention_bg: theme.accent.opacity(0.25),
            link_wash: theme.link.opacity(0.1),
            code_bg: theme.secondary,
            code_fg: crate::theme::code_foreground(cx),
            selection: gpui_kit::base::Theme::global(cx).tokens.colors.selection,
            order,
            links,
            underline_links: crate::preferences::display_prefs(cx).underline_links,
            dark: theme.mode.is_dark(),
            base_font,
            paragraphs: Cell::new(0),
            key,
        }
    }

    /// The theme of selectable message-body text: paragraphs participate in the window
    /// selection from document order `order` on, links route through `links`.
    pub(in crate::messages) fn body(
        cx: &App,
        key: SharedString,
        order: u64,
        links: Option<LinkHandler>,
    ) -> Self {
        Self::from_app(cx, key, Some(order), links)
    }

    /// The theme of preview text: not selectable, links open as URLs.
    fn plain(cx: &App, key: SharedString) -> Self {
        Self::from_app(cx, key, None, None)
    }

    pub(in crate::messages) fn run(
        &self,
        run_flags: RunFlags,
        color: Option<Hsla>,
        bg: Option<Hsla>,
        len: usize,
    ) -> TextRun {
        let mut font = if run_flags.code {
            self.code_font.clone()
        } else {
            self.base_font.clone()
        };
        if run_flags.bold {
            font.weight = FontWeight::BOLD;
        } else if run_flags.semibold {
            font.weight = FontWeight::SEMIBOLD;
        }
        if run_flags.italic {
            font.style = FontStyle::Italic;
        }
        TextRun {
            len,
            font,
            color: color.unwrap_or(if run_flags.code {
                self.code_fg
            } else {
                self.foreground
            }),
            background_color: bg,
            underline: run_flags.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: None,
                wavy: false,
            }),
            strikethrough: run_flags.strike.then(|| StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
        }
    }

    /// A model run in this theme: its tone picks the colors (inline code its wash, links the
    /// link color and the underline preference), its flags the font.
    pub(super) fn styled(&self, style: RunStyle, len: usize) -> TextRun {
        let mut flags = style.flags;
        let (color, bg) = match style.tone {
            Tone::Text => (
                flags.code.then_some(self.code_fg),
                flags.code.then_some(self.code_bg),
            ),
            Tone::Link => {
                flags.underline |= self.underline_links;
                (Some(self.link), None)
            }
            Tone::Reference => (Some(self.link), None),
            Tone::Mention => (Some(self.link), Some(self.mention_bg)),
            Tone::App => (Some(self.link), Some(self.link_wash)),
            Tone::Muted => (Some(self.muted), None),
        };
        self.run(flags, color, bg, len)
    }
}

pub(in crate::messages) fn flags(style: Option<&TextStyle>) -> RunFlags {
    let Some(style) = style else {
        return RunFlags::default();
    };
    RunFlags {
        bold: style.bold,
        semibold: false,
        italic: style.italic,
        strike: style.strike,
        underline: style.underline,
        code: style.code,
    }
}

/// One text piece of a paragraph: selectable in the transcript when the theme has an order,
/// plain (links still clickable) otherwise.
pub(in crate::messages) fn text_element(
    piece: &TextPiece,
    n: usize,
    theme: &RunTheme,
    routed: &Option<LinkHandler>,
) -> AnyElement {
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    let runs = piece
        .runs
        .iter()
        .map(|&(len, style)| theme.styled(style, len))
        .collect();
    let id = paragraph_id(&theme.key, paragraph, n, piece.hash);
    if let Some(base) = theme.order {
        return SelectableParagraph::new(
            id,
            piece.text.clone(),
            runs,
            piece.links.clone(),
            base + paragraph as u64,
            theme,
        )
        .into_any_element();
    }
    if piece.links.is_empty() {
        return StyledText::new(piece.text.clone())
            .with_runs(runs)
            .into_any_element();
    }
    let (ranges, targets): (Vec<_>, Vec<_>) = piece.links.iter().cloned().unzip();
    let styled = StyledText::new(piece.text.clone()).with_runs(runs);
    let routed = routed.clone();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| {
            route_link(routed.as_ref(), targets[ix].clone(), window, cx)
        })
        .into_any_element()
}

/// A paragraph: its segments in a wrap row. Message text participates in the window selection
/// (one participant per paragraph, ordered with the transcript's reading order); preview text
/// stays plain. Text wraps only inside a width-constrained block: a lone text piece (most
/// paragraphs) renders in a full-width block; pieces mixed with inline images or chips shrink
/// to wrap in the row, the pieces of one text separated by Slack's 8px `c-mrkdwn__br` block.
pub(in crate::messages) fn render_segments(segments: &[Segment], theme: &RunTheme) -> Div {
    let routed = theme.links.clone();
    let single = segments.len() == 1;
    let mut row = h_flex().min_w_0().flex_wrap();
    for (n, segment) in segments.iter().enumerate() {
        match segment {
            Segment::Text {
                pieces,
                lead,
                trail,
            } => {
                if let [piece] = pieces.as_slice()
                    && single
                {
                    return div()
                        .w_full()
                        .min_w_0()
                        .child(text_element(piece, n, theme, &routed));
                }
                if *lead {
                    row = row.child(div().w_full().h_0());
                }
                for (b, piece) in pieces.iter().enumerate() {
                    if b > 0 {
                        row = row.child(div().w_full().h(px(8.)));
                    }
                    if !piece.text.is_empty() {
                        row = row.child(
                            div()
                                .min_w_0()
                                .child(text_element(piece, n, theme, &routed)),
                        );
                    }
                }
                if *trail {
                    row = row.child(div().w_full().h_0());
                }
            }
            Segment::Image(image) => {
                // A Picture: animated custom emoji play, as in Slack.
                row = row.child(
                    div()
                        .flex_none()
                        .size(px(INLINE_EMOJI))
                        .child(crate::media::Picture::new(image.clone()).radius(px(2.))),
                );
            }
            Segment::Chip {
                url,
                label,
                icon,
                icon_size,
            } => {
                let id = format!("chip-{}-{}-{n}", theme.key, theme.paragraphs.get());
                let target = LinkTarget::Url(url.clone());
                let routed = routed.clone();
                row = row.child(
                    // Slack's pill is 1.3em tall, inside the line.
                    h_flex()
                        .id(SharedString::from(id))
                        .min_w_0()
                        .items_center()
                        .line_height(relative(1.3))
                        .gap_0p5()
                        .px_0p5()
                        .rounded_sm()
                        .bg(theme.link_wash)
                        .text_color(theme.link)
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            route_link(routed.as_ref(), target.clone(), window, cx)
                        })
                        .child(
                            Icon::new(*icon)
                                .flex_none()
                                .size(px(*icon_size))
                                .text_color(theme.link),
                        )
                        .child(label.clone()),
                );
            }
            Segment::ColorSwatch { hex, color } => {
                // Slack renders `#RRGGBB` inline with a crisp rounded color square beside it.
                row = row.child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded_sm()
                        .bg(theme.code_bg)
                        .child(div().text_color(theme.foreground).child(hex.clone()))
                        .child(
                            div()
                                .flex_none()
                                .size(px(11.))
                                .rounded(px(2.))
                                .bg(*color)
                                .border_1()
                                .border_color(theme.foreground.opacity(0.22)),
                        ),
                );
            }
        }
    }
    row
}

/// The stable element id of one rendered paragraph: its position in the message plus a hash
/// of its text, unique within the message and across messages.
pub(in crate::messages) fn paragraph_id(
    key: &SharedString,
    paragraph: usize,
    n: usize,
    hash: u64,
) -> ElementId {
    ElementId::Name(SharedString::from(format!(
        "text-{key}-{paragraph}-{n}-{hash:x}"
    )))
}

/// The hash a paragraph's element id carries.
pub(in crate::messages) fn text_hash(text: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// `text` as the plain text the clipboard, notifications and list previews show: `@mentions`,
/// `#channels`, links and `<!date>` tokens resolved through `names` as the transcript shows
/// them, formatting markers dropped, and known `:shortcode:` emoji as their glyphs (custom
/// emoji stay `:name:`); line breaks kept.
pub(crate) fn plain_text(text: &str, names: &dyn Directory) -> String {
    replace_shortcodes_with(&mrkdwn_to_plain_with(text, names), &emoji::display_glyph)
}

/// A one-paragraph mrkdwn preview (the DM list, the Activity feed, headers): formatting, links
/// and emoji as the transcript renders them, plain runs in `color`, line breaks flattened.
/// Preview text is not selectable; links open as URLs.
pub(crate) fn mrkdwn_preview(
    text: &str,
    fixtures: &Fixtures,
    color: Hsla,
    key: String,
    cx: &App,
) -> Div {
    let emoji_text = crate::preferences::display_prefs(cx).emoji_text_mode;
    render_preview(&preview_model(text, fixtures, emoji_text), color, key, cx)
}

/// A preview's model: one paragraph with its line breaks flattened, to render every frame
/// through [`render_preview`] without parsing again.
pub(crate) fn preview_model(text: &str, fixtures: &Fixtures, emoji_text: bool) -> Vec<Segment> {
    line_segments(
        &parse_mrkdwn_spans(&text.replace('\n', " ")),
        fixtures,
        TextOpts {
            emoji_text,
            verbatim: false,
        },
        false,
    )
}

pub(crate) fn render_preview(segments: &[Segment], color: Hsla, key: String, cx: &App) -> Div {
    let mut theme = RunTheme::plain(cx, key.into());
    theme.foreground = color;
    render_segments(segments, &theme)
}

/// Slack packs emoji code points as hex joined by `-` (`1f389`, skin tones appended).
pub(in crate::messages) fn unicode_glyph(hex: &str) -> Option<String> {
    let points = hex
        .split('-')
        .map(|point| u32::from_str_radix(point, 16).ok())
        .collect::<Option<Vec<u32>>>()?;
    Some(
        points
            .iter()
            .filter_map(|&point| char::from_u32(point))
            .collect(),
    )
}

pub(crate) fn hex_color(value: &str) -> Option<Hsla> {
    let hex = value.trim_start_matches('#');
    if hex.len() == 6 {
        u32::from_str_radix(hex, 16)
            .ok()
            .map(|color| Hsla::from(rgb(color)))
    } else {
        None
    }
}

/// If `text` consists exclusively of 1..=5 Slack `:emoji:` shortcodes (whitespace ignored),
/// return large 32px jumbomoji elements.
pub(in crate::messages) fn jumbomoji_elements(
    text: &str,
    fixtures: &Fixtures,
) -> Option<Vec<AnyElement>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    let mut rest = trimmed;
    while !rest.is_empty() {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        // One shortcode, skin tone suffix included (`:+1::skin-tone-2:` is one emoji).
        let (len, code) = emoji::parse_shortcode(rest)?;
        let art = emoji::EmojiArt::of(&code, fixtures);
        if matches!(art, emoji::EmojiArt::Text(_)) {
            return None;
        }
        out.push(art.element(px(32.)).into_any_element());
        rest = &rest[len..];
        if out.len() > 5 {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}
