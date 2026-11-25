//! Inline text runs, paragraph building, mrkdwn tokenization, and shortcode/color helpers.

use std::cell::Cell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyElement, App, Div, ElementId, Font, FontStyle, FontWeight, Hsla, ImageSource,
    InteractiveElement as _, InteractiveText, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, TextRun,
    UnderlineStyle, div, px, relative, rgb,
};
use slackless_core::types::{ChannelId, TextStyle, UserId};

use super::super::{APP_ICON_SLOT, INLINE_EMOJI, LinkHandler, LinkTarget, route_link};
use super::SelectableParagraph;
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
#[derive(Clone, Copy, Default)]
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
    /// Render emoji as plain `:shortcode:` text (Preferences > Messages & media).
    pub(in crate::messages) emoji_text_mode: bool,
    /// Active dark/light appearance mode (used for syntax-highlighted code runs).
    pub(in crate::messages) dark: bool,
    /// When `true` (Block Kit `mrkdwn` `verbatim: true`, `F-COLLAB-758`), skip auto-linking
    /// raw un-bracketed URLs.
    pub(in crate::messages) verbatim: bool,
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
            emoji_text_mode: crate::preferences::display_prefs(cx).emoji_text_mode,
            dark: theme.mode.is_dark(),
            verbatim: false,
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

/// One inline chunk of a paragraph: styled text (with its clickable link ranges), a custom
/// emoji image, or a link chip.
pub(in crate::messages) enum Segment {
    Text {
        text: String,
        runs: Vec<TextRun>,
        links: Vec<(Range<usize>, LinkTarget)>,
    },
    Image(ImageSource),
    /// Slack's chip for a link labelled with its own address (a link icon) or for a link to a
    /// message (a speech bubble): the icon and the label on a tinted pill.
    Chip {
        url: SharedString,
        label: String,
        icon: IconName,
        icon_size: f32,
    },
    /// Slack's inline hex color swatch (`#RRGGBB` + rounded color square, `F-SHELL-087`).
    ColorSwatch {
        hex: String,
        color: Hsla,
    },
}

/// Accumulates a paragraph's styled text and link ranges, split only at inline images.
pub(in crate::messages) struct ParagraphBuilder {
    pub(in crate::messages) text: String,
    pub(in crate::messages) runs: Vec<TextRun>,
    pub(in crate::messages) links: Vec<(Range<usize>, LinkTarget)>,
    pub(in crate::messages) segments: Vec<Segment>,
}

impl ParagraphBuilder {
    pub(in crate::messages) fn new() -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            links: Vec::new(),
            segments: Vec::new(),
        }
    }

    pub(super) fn push_text(
        &mut self,
        text: &str,
        theme: &RunTheme,
        run_flags: RunFlags,
        color: Option<Hsla>,
        bg: Option<Hsla>,
    ) {
        if text.is_empty() {
            return;
        }
        self.runs.push(theme.run(run_flags, color, bg, text.len()));
        self.text.push_str(text);
    }

    /// Slack's edit marker: a muted run at the paragraph's end that flows and wraps with the
    /// text. (A per-run font size is not expressible: one `StyledText` carries a single text
    /// size from its element, so the marker is muted at body size.)
    pub(super) fn push_edited(&mut self, theme: &RunTheme) {
        self.push_text(
            " (edited)",
            theme,
            RunFlags::default(),
            Some(theme.muted),
            None,
        );
    }

    /// A link inside the paragraph's text: a link-colored run, clickable through its range.
    /// Like Slack, a URL posted bare shows as itself, scheme and all, while a link labelled
    /// with its own address minus the scheme (`github.com/…`) gets the tinted chip look.
    /// `run` is the label's styling (Slack bolds `<url|*Error*>` and links inside `*…*`).
    /// Expands `:shortcode:` unicode emojis inside link labels (`<url|:bar_chart: Panel>`) so
    /// Slack alert/bot links render crisp glyphs instead of raw `:shortcode:` text.
    pub(super) fn push_link(
        &mut self,
        url: String,
        label: String,
        run: RunFlags,
        theme: &RunTheme,
    ) {
        let address = |s: &str| {
            s.split_once("://")
                .map_or(s, |(_, rest)| rest)
                .trim_end_matches('/')
                .to_owned()
        };
        if label != url && address(&label) == address(&url) {
            self.push_chip(url.into(), label, IconName::Link, 12.);
            return;
        }
        let expanded = if theme.emoji_text_mode {
            label
        } else {
            expand_shortcodes_in_text(&label)
        };
        let target = LinkTarget::Url(url.into());
        let start = self.text.len();
        let mut link_run = run;
        if theme.underline_links {
            link_run.underline = true;
        }
        self.push_text(&expanded, theme, link_run, Some(theme.link), None);
        self.links.push((start..self.text.len(), target));
    }

    /// An in-app mention (an @user or #channel chip): the mention look, clickable through
    /// its range like a link.
    pub(super) fn push_mention(
        &mut self,
        label: &str,
        target: LinkTarget,
        theme: &RunTheme,
        bg: Option<Hsla>,
    ) {
        let start = self.text.len();
        self.push_text(label, theme, mention_flags(), Some(theme.link), bg);
        self.links.push((start..self.text.len(), target));
    }

    /// An app's inline link chip (`attachment_mention`, e.g. a Jira issue): its title on the
    /// link wash after a slot the paragraph paints the app icon into, one clickable range.
    pub(super) fn push_attachment(&mut self, url: String, label: &str, theme: &RunTheme) {
        let start = self.text.len();
        for text in [APP_ICON_SLOT, label] {
            self.push_text(
                text,
                theme,
                RunFlags::default(),
                Some(theme.link),
                Some(theme.link_wash),
            );
        }
        self.links
            .push((start..self.text.len(), LinkTarget::Url(url.into())));
    }

    pub(super) fn push_image(&mut self, image: ImageSource) {
        self.flush();
        self.segments.push(Segment::Image(image));
    }

    pub(super) fn push_chip(
        &mut self,
        url: SharedString,
        label: String,
        icon: IconName,
        icon_size: f32,
    ) {
        self.flush();
        self.segments.push(Segment::Chip {
            url,
            label,
            icon,
            icon_size,
        });
    }

    pub(super) fn push_color_swatch(&mut self, hex: String, color: Hsla) {
        self.flush();
        self.segments.push(Segment::ColorSwatch { hex, color });
    }

    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        let runs = std::mem::take(&mut self.runs);
        let links = std::mem::take(&mut self.links);
        self.segments.push(Segment::Text { text, runs, links });
    }

    /// Drop the line breaks the paragraph ends with: Slack starts what follows (a code
    /// block, a quote, a list or the message's end) on its own line without them.
    pub(super) fn trim_trailing_breaks(&mut self) {
        while self.text.ends_with('\n') {
            self.text.pop();
            if let Some(last) = self.runs.last_mut() {
                last.len -= 1;
                if last.len == 0 {
                    self.runs.pop();
                }
            }
        }
    }

    pub(super) fn finish(mut self) -> Vec<Segment> {
        self.flush();
        self.segments
    }
}

/// One block of a paragraph after splitting at blank-line runs: its text, runs and links
/// (the same shape [`Segment::Text`] carries).
type ParagraphBlock = (String, Vec<TextRun>, Vec<(Range<usize>, LinkTarget)>);

/// Slack renders a run of one or more blank lines inside message text as a single short
/// paragraph break, not an empty text line: the web client emits an 8px block
/// (`.c-mrkdwn__br { height: 8px; display: block }`) between the pieces. Splits one
/// paragraph's text (with its byte-offset runs and link ranges) at those runs so the
/// pieces can stack with the break between them; text without blank lines comes back as
/// the single block it always was.
pub(in crate::messages) fn paragraph_break_split(
    text: &str,
    runs: &[TextRun],
    links: &[(Range<usize>, LinkTarget)],
) -> Vec<ParagraphBlock> {
    let slice_block = |range: Range<usize>| {
        let block_text = text[range.clone()].to_owned();
        let mut block_runs = Vec::new();
        let mut at = 0;
        for run in runs {
            let run_range = at..at + run.len;
            at = run_range.end;
            let start = run_range.start.max(range.start);
            let end = run_range.end.min(range.end);
            if start < end {
                block_runs.push(TextRun {
                    len: end - start,
                    ..run.clone()
                });
            }
        }
        let block_links = links
            .iter()
            .filter_map(|(link, target)| {
                let start = link.start.max(range.start);
                let end = link.end.min(range.end);
                (start < end).then(|| ((start - range.start)..(end - range.start), target.clone()))
            })
            .collect();
        (block_text, block_runs, block_links)
    };

    let bytes = text.as_bytes();
    let mut blocks = Vec::new();
    let mut start = 0;
    let mut ix = 0;
    while ix < bytes.len() {
        if bytes[ix] != b'\n' {
            ix += 1;
            continue;
        }
        let mut end = ix;
        while end < bytes.len() && bytes[end] == b'\n' {
            end += 1;
        }
        if end - ix >= 2 {
            blocks.push(slice_block(start..ix));
            start = end;
        }
        ix = end;
    }
    blocks.push(slice_block(start..text.len()));
    blocks
}

/// One text piece of a paragraph: selectable in the transcript when `order_base` is set,
/// plain (links still clickable) otherwise. Takes its own slot of the theme's paragraph
/// counter, so its id and document order stay unique however many pieces a paragraph
/// splits into.
pub(in crate::messages) fn text_element(
    text: String,
    runs: Vec<TextRun>,
    links: Vec<(Range<usize>, LinkTarget)>,
    n: usize,
    theme: &RunTheme,
    order_base: Option<u64>,
    routed: &Option<LinkHandler>,
) -> AnyElement {
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    if let Some(base) = order_base {
        return SelectableParagraph::new(
            paragraph_id(&theme.key, paragraph, n, &text),
            text,
            runs,
            links,
            base + paragraph as u64,
            theme,
        )
        .into_any_element();
    }
    if links.is_empty() {
        return StyledText::new(text).with_runs(runs).into_any_element();
    }
    let id = paragraph_id(&theme.key, paragraph, n, &text);
    let (ranges, targets): (Vec<_>, Vec<_>) = links.into_iter().unzip();
    let styled = StyledText::new(text).with_runs(runs);
    let routed = routed.clone();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, window, cx| {
            route_link(routed.as_ref(), targets[ix].clone(), window, cx)
        })
        .into_any_element()
}

pub(in crate::messages) fn render_segments(segments: Vec<Segment>, theme: &RunTheme) -> Div {
    // Message text participates in the window selection (one participant per paragraph,
    // ordered with the transcript's reading order); preview text stays plain.
    let order_base = theme.order;
    let routed = theme.links.clone();
    // Text wraps only inside a width-constrained block: a lone text run (most paragraphs)
    // renders in a full-width block; runs mixed with inline images shrink to wrap in the row.
    let single = segments.len() == 1;
    let mut row = h_flex().min_w_0().flex_wrap();
    for (n, segment) in segments.into_iter().enumerate() {
        match segment {
            Segment::Text {
                mut text,
                mut runs,
                mut links,
            } => {
                // In a row mixed with chips or images, a line break at a text's edge must end
                // the row: inside the text it would only add an empty line to its own box.
                let (lead, trail) = if single {
                    (false, false)
                } else {
                    trim_breaks(&mut text, &mut runs, &mut links)
                };
                let mut blocks = paragraph_break_split(&text, &runs, &links);
                if single
                    && blocks.len() == 1
                    && let Some((text, runs, links)) = blocks.pop()
                {
                    return div().w_full().min_w_0().child(text_element(
                        text, runs, links, n, theme, order_base, &routed,
                    ));
                }
                if lead {
                    row = row.child(div().w_full().h_0());
                }
                for (b, (text, runs, links)) in blocks.into_iter().enumerate() {
                    // The break between the pieces: Slack's 8px `c-mrkdwn__br` block, which
                    // the wrap row stacks between the two texts.
                    if b > 0 {
                        row = row.child(div().w_full().h(px(8.)));
                    }
                    if !text.is_empty() {
                        let element =
                            text_element(text, runs, links, n, theme, order_base, &routed);
                        row = row.child(div().min_w_0().child(element));
                    }
                }
                if trail {
                    row = row.child(div().w_full().h_0());
                }
            }
            Segment::Image(image) => {
                // A Picture: animated custom emoji play, as in Slack.
                row = row.child(
                    div()
                        .flex_none()
                        .size(px(INLINE_EMOJI))
                        .child(crate::media::Picture::new(image).radius(px(2.))),
                );
                continue;
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
                            Icon::new(icon)
                                .flex_none()
                                .size(px(icon_size))
                                .text_color(theme.link),
                        )
                        .child(label),
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
                        .child(div().text_color(theme.foreground).child(hex))
                        .child(
                            div()
                                .flex_none()
                                .size(px(11.))
                                .rounded(px(2.))
                                .bg(color)
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
    text: &str,
) -> ElementId {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    ElementId::Name(SharedString::from(format!(
        "text-{key}-{paragraph}-{n}-{:x}",
        hasher.finish()
    )))
}

/// Strip one line break from each edge of a text segment (keeping its runs and link ranges in
/// step) and report which edges had one.
pub(in crate::messages) fn trim_breaks(
    text: &mut String,
    runs: &mut Vec<TextRun>,
    links: &mut [(Range<usize>, LinkTarget)],
) -> (bool, bool) {
    let trail = text.ends_with('\n');
    if trail {
        text.pop();
        if let Some(last) = runs.last_mut() {
            last.len -= 1;
        }
    }
    let lead = text.starts_with('\n');
    if lead {
        text.remove(0);
        if let Some(first) = runs.first_mut() {
            first.len -= 1;
        }
        for (range, _) in links.iter_mut() {
            *range = range.start.saturating_sub(1)..range.end.saturating_sub(1);
        }
    }
    runs.retain(|run| run.len > 0);
    (lead, trail)
}

/// mrkdwn split at its ``` fences into `(fenced, text)` pieces: Slack renders fenced text
/// as a pre block, raw, dropping the line break just inside and outside each fence. An
/// unclosed fence stays literal; empty text pieces are left out.
pub(in crate::messages) fn mrkdwn_fences(text: &str) -> Vec<(bool, &str)> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("```") {
        let inner = &rest[open + 3..];
        let Some(close) = inner.find("```") else {
            break;
        };
        let before = &rest[..open];
        pieces.push((false, before.strip_suffix('\n').unwrap_or(before)));
        let code = &inner[..close];
        let code = code.strip_prefix('\n').unwrap_or(code);
        pieces.push((true, code.strip_suffix('\n').unwrap_or(code)));
        let after = &inner[close + 3..];
        rest = after.strip_prefix('\n').unwrap_or(after);
    }
    pieces.push((false, rest));
    pieces.retain(|(fenced, piece)| *fenced || !piece.is_empty());
    pieces
}

/// Strip a Slack mrkdwn blockquote prefix (`>` or `&gt;`) and one optional following space.
pub(in crate::messages) fn strip_quote_prefix(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("&gt;")
        .or_else(|| line.strip_prefix('>'))?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Lines of mrkdwn text into `column`, grouping consecutive `>` / `&gt;` blockquote lines
/// behind a Slack-style left quote bar; with `edited`, the edit marker rides inline at the
/// end of the last line that renders text.
pub(in crate::messages) fn mrkdwn_lines(
    mut column: Div,
    text: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Div {
    let lines: Vec<&str> = text.split('\n').collect();
    let last_text_line = lines.iter().rposition(|line| !line.is_empty());
    let mut blank = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        // A run of blank lines is one 8px break, like the rich-text path's `c-mrkdwn__br`.
        if line.is_empty() {
            if !blank {
                column = column.child(div().h(px(8.)));
            }
            blank = true;
            i += 1;
            continue;
        }
        blank = false;
        if strip_quote_prefix(line).is_some() {
            let mut quote_col = v_flex().flex_1().min_w_0();
            while i < lines.len() {
                let Some(inner) = strip_quote_prefix(lines[i]) else {
                    break;
                };
                if inner.is_empty() {
                    quote_col = quote_col.child(div().h(px(6.)));
                } else if let Some(row) =
                    mrkdwn_line(inner, fixtures, theme, edited && Some(i) == last_text_line)
                {
                    quote_col = quote_col.child(row);
                }
                i += 1;
            }
            column = column.child(
                h_flex()
                    .my_0p5()
                    .gap_2p5()
                    .items_stretch()
                    .min_w_0()
                    .child(div().flex_none().w(px(4.)).rounded_full().bg(theme.border))
                    .child(quote_col),
            );
            continue;
        }
        if let Some(row) = mrkdwn_line(line, fixtures, theme, edited && Some(i) == last_text_line) {
            column = column.child(row);
        }
        i += 1;
    }
    column
}

/// A one-paragraph mrkdwn preview (the DM list): formatting, links and emoji as the
/// transcript renders them, plain runs in `color`, line breaks flattened. Preview text is
/// not selectable; links open as URLs.
pub(crate) fn mrkdwn_preview(
    text: &str,
    fixtures: &Fixtures,
    color: Hsla,
    key: String,
    cx: &App,
) -> Div {
    let mut theme = RunTheme::plain(cx, key.into());
    theme.foreground = color;
    mrkdwn_line(&text.replace('\n', " "), fixtures, &theme, false).unwrap_or_else(div)
}

pub(in crate::messages) fn mrkdwn_line(
    line: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Option<Div> {
    let mut builder = ParagraphBuilder::new();
    let mut run = RunFlags::default();
    let mut rest = line;
    let mut prev: Option<char> = None;
    while let Some(start) = rest.find(['<', ':', '*', '_', '~', '`']) {
        let (before, tail) = rest.split_at(start);
        push_mrkdwn_text(&mut builder, before, theme, run);
        prev = before.chars().last().or(prev);
        rest = tail;
        let marker = rest.as_bytes()[0] as char;
        let handled = if matches!(marker, '*' | '_' | '~' | '`') {
            toggle_marker(&mut run, marker, prev, &rest[1..]).then_some(1)
        } else if run.code {
            None
        } else if marker == '<' {
            angle_token(&mut builder, rest, fixtures, theme, run)
        } else {
            emoji_token(&mut builder, rest, fixtures, theme, run)
        };
        let len = handled.unwrap_or_else(|| {
            push_mrkdwn_text(&mut builder, &rest[..1], theme, run);
            1
        });
        prev = rest[..len].chars().last();
        rest = &rest[len..];
    }
    push_mrkdwn_text(&mut builder, rest, theme, run);
    if edited {
        builder.push_edited(theme);
    }
    let segments = builder.finish();
    if segments.is_empty() {
        return None;
    }
    Some(render_segments(segments, theme))
}

/// `text` with Slack's three mrkdwn HTML escapes undone (zero-copy when no `&` is present).
pub(crate) fn unescape_cow(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('&') {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// `text` with Slack's three mrkdwn HTML escapes undone.
pub(crate) fn unescape(text: &str) -> String {
    unescape_cow(text).into_owned()
}

/// Plain mrkdwn text with Slack's HTML escapes undone; inline code gets its wash.
/// Unless `theme.verbatim` is set (Block Kit `verbatim: true`, `F-COLLAB-758`) or inside inline
/// code, bare `https://` / `http://` URLs are auto-linked.
pub(in crate::messages) fn push_mrkdwn_text(
    builder: &mut ParagraphBuilder,
    text: &str,
    theme: &RunTheme,
    run: RunFlags,
) {
    let unescaped = unescape(text);
    if run.code || theme.verbatim {
        let bg = run.code.then_some(theme.code_bg);
        builder.push_text(&unescaped, theme, run, None, bg);
        return;
    }
    let mut rest = unescaped.as_str();
    while let Some(pos) = rest.find("https://").or_else(|| rest.find("http://")) {
        let (before, tail) = rest.split_at(pos);
        if !before.is_empty() {
            builder.push_text(before, theme, run, None, None);
        }
        let raw_end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
            .unwrap_or(tail.len());
        let url = tail[..raw_end].trim_end_matches(['.', ',', ';', ':', ')', ']']);
        if url.len() > "https://".len() {
            builder.push_link(url.to_owned(), url.to_owned(), run, theme);
            rest = &tail[url.len()..];
        } else {
            builder.push_text(&tail[..raw_end.max(1)], theme, run, None, None);
            rest = &tail[raw_end.max(1)..];
        }
    }
    if !rest.is_empty() {
        builder.push_text(rest, theme, run, None, None);
    }
}

/// Open or close the style `marker` stands for. A marker opens at a word start when a closing
/// marker follows, and closes at a word end; anything else is literal.
pub(in crate::messages) fn toggle_marker(
    run: &mut RunFlags,
    marker: char,
    prev: Option<char>,
    after: &str,
) -> bool {
    if run.code && marker != '`' {
        return false;
    }
    let flag = match marker {
        '*' => &mut run.bold,
        '_' => &mut run.italic,
        '~' => &mut run.strike,
        _ => &mut run.code,
    };
    if *flag {
        let ends_word = after.chars().next().is_none_or(|c| !c.is_alphanumeric());
        if ends_word && prev.is_some_and(|c| !c.is_whitespace()) {
            *flag = false;
            return true;
        }
        return false;
    }
    let starts_word = prev.is_none_or(|c| !c.is_alphanumeric());
    let opens = after.chars().next().is_some_and(|c| !c.is_whitespace()) && after.contains(marker);
    if starts_word && opens {
        *flag = true;
    }
    starts_word && opens
}

/// A `<...>` token at the start of `rest`: user / channel / special mentions and links. A
/// link takes the surrounding styling `run` plus its label's own.
pub(in crate::messages) fn angle_token(
    builder: &mut ParagraphBuilder,
    rest: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    run: RunFlags,
) -> Option<usize> {
    let close = rest.find('>')?;
    let inner = &rest[1..close];
    let (target, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(label)),
        None => (inner, None),
    };
    if let Some(user) = target.strip_prefix('@') {
        let name = format!("@{}", fixtures.mention_name(&UserId::from(user)));
        builder.push_mention(
            &name,
            LinkTarget::Profile(UserId::from(user)),
            theme,
            Some(theme.mention_bg),
        );
    } else if let Some(channel) = target.strip_prefix('#') {
        let name = label
            .map(|label| format!("#{label}"))
            .or_else(|| {
                fixtures
                    .conversation(&ChannelId::from(channel))
                    .map(|conversation| fixtures.conversation_label(conversation))
            })
            .unwrap_or_else(|| format!("#{channel}"));
        builder.push_mention(
            &name,
            LinkTarget::Channel(ChannelId::from(channel)),
            theme,
            None,
        );
    } else if let Some(special) = target.strip_prefix('!') {
        if special.starts_with("date") {
            builder.push_text(
                label.unwrap_or(special),
                theme,
                RunFlags::default(),
                None,
                None,
            );
        } else {
            let name = label
                .map(str::to_owned)
                .unwrap_or_else(|| format!("@{}", special.split('^').next().unwrap_or(special)));
            builder.push_text(
                &name,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
    } else if target.contains("://") || target.starts_with("mailto:") {
        let (label, run) = styled_label(label.unwrap_or(target), run);
        builder.push_link(unescape(target), unescape(label), run, theme);
    } else {
        return None;
    }
    Some(close + 1)
}

/// A link label's own styling: Slack renders `<url|*TypeError*>` as a bold link. Markers
/// wrapping the whole label add their style to `run`; any others stay literal.
pub(in crate::messages) fn styled_label(mut label: &str, mut run: RunFlags) -> (&str, RunFlags) {
    while let Some(marker) = label
        .chars()
        .next()
        .filter(|c| matches!(c, '*' | '_' | '~'))
    {
        if label.len() < 3 || !label.ends_with(marker) {
            break;
        }
        match marker {
            '*' => run.bold = true,
            '_' => run.italic = true,
            _ => run.strike = true,
        }
        label = &label[1..label.len() - 1];
    }
    (label, run)
}

/// Expand standard unicode `:shortcode:` tokens inside text (`<url|:bar_chart: Panel>`, plain-text previews).
pub(crate) fn expand_shortcodes_in_text(text: &str) -> String {
    if !text.contains(':') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        if rest.starts_with(':')
            && let Some((len, code)) = emoji::parse_shortcode(rest)
            && let Some(glyph) = emoji::display_glyph(&code)
        {
            out.push_str(&glyph);
            rest = &rest[len..];
            continue;
        }
        let next_colon = rest[1..].find(':').map(|i| i + 1).unwrap_or(rest.len());
        out.push_str(&rest[..next_colon]);
        rest = &rest[next_colon..];
    }
    out
}

/// A `:shortcode:` at the start of `rest`: a glyph run, or a custom emoji image.
pub(in crate::messages) fn emoji_token(
    builder: &mut ParagraphBuilder,
    rest: &str,
    fixtures: &Fixtures,
    theme: &RunTheme,
    run: RunFlags,
) -> Option<usize> {
    if theme.emoji_text_mode {
        return None;
    }
    let (len, code) = emoji::parse_shortcode(rest)?;
    match emoji::display_glyph(&code) {
        Some(glyph) => builder.push_text(&glyph, theme, run, None, None),
        None => builder.push_image(fixtures.custom_emoji(&code)?.source()),
    }
    Some(len)
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
        let after_open = rest.strip_prefix(':')?;
        let close = after_open.find(':')?;
        let code = &after_open[..close];
        if code.is_empty()
            || !code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'+'))
        {
            return None;
        }
        if crate::emoji::glyph(code).is_none() && !fixtures.custom_emoji.contains_key(code) {
            return None;
        }
        out.push(crate::emoji::emoji_element(code, fixtures, px(32.)).into_any_element());
        rest = &after_open[close + 1..];
        if out.len() > 5 {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}
