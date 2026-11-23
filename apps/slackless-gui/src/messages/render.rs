//! Message body, rich-text / mrkdwn layout, selectable paragraphs, code blocks, and time/date
//! formatting for the transcript.

use std::cell::{Cell, RefCell};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::base::{
    GlobalState, TextSelectionHandle, TextSelectionRegistration, TextSelectionRun,
};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, BorderStyle, Bounds, Corners, CursorStyle, DispatchPhase, Div, Edges, Element,
    ElementId, Font, FontStyle, FontWeight, GlobalElementId, Hitbox, HitboxBehavior, Hsla,
    ImageSource, InspectorElementId, InteractiveElement as _, InteractiveText, IntoElement,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    ParentElement as _, Pixels, Point, RenderOnce, ScrollEvent, SharedString,
    StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, Subscription,
    TextLayout, TextRun, TransformationMatrix, UnderlineStyle, Window, deferred, div, px, relative,
    rgb, size, transparent_black,
};
use slackless_core::types::rich_text::{
    ListStyle, RichTextContainer, RichTextElement, RichTextList, RichTextPreformatted,
};
use slackless_core::types::{Block, ChannelId, Message, TextStyle, Ts, UserId};

use super::{APP_ICON, APP_ICON_SLOT, INLINE_EMOJI, LinkHandler, LinkTarget, MONTHS, route_link};
use crate::blockkit::{self, KitCx};
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::hover_card::{self, Trigger};

pub(super) const DEFAULT_FLAGS: RunFlags = RunFlags {
    bold: false,
    semibold: false,
    italic: false,
    strike: false,
    underline: false,
    code: false,
};

/// Mention chip style: semibold link color on an accent wash (guide section 10).
pub(super) fn mention_flags() -> RunFlags {
    RunFlags {
        semibold: true,
        ..DEFAULT_FLAGS
    }
}

/// Inline style flags one text run carries.
#[derive(Clone, Copy, Default)]
pub(super) struct RunFlags {
    pub(super) bold: bool,
    pub(super) semibold: bool,
    pub(super) italic: bool,
    pub(super) strike: bool,
    pub(super) underline: bool,
    pub(super) code: bool,
}

/// Theme-resolved fonts and colors shared by every run of one message render.
pub(super) struct RunTheme {
    pub(super) base_font: Font,
    /// Code blocks' face, sized by the block.
    pub(super) mono_font: Font,
    /// Inline code's face: smaller glyphs at the body size (see
    /// [`crate::theme::INLINE_CODE_FAMILY`]).
    pub(super) code_font: Font,
    pub(super) foreground: Hsla,
    pub(super) muted: Hsla,
    pub(super) border: Hsla,
    pub(super) link: Hsla,
    pub(super) mention_bg: Hsla,
    /// Slack's tint behind chip links: address-labelled links and app chips.
    pub(super) link_wash: Hsla,
    pub(super) code_bg: Hsla,
    /// Inline `code` text (Slack's orange / red, see [`crate::theme::code_foreground`]).
    pub(super) code_fg: Hsla,
    /// The window-selection highlight of selectable paragraphs.
    pub(super) selection: Hsla,
    /// Selection document order of this theme's first paragraph; `None` renders plain,
    /// unselectable text (previews).
    pub(super) order: Option<u64>,
    /// Routing for this text's links; `None` falls back to [`open_link_default`].
    pub(super) links: Option<LinkHandler>,
    /// Always underline website links (Preferences > Accessibility > Underline links).
    pub(super) underline_links: bool,
    /// Render emoji as plain `:shortcode:` text (Preferences > Messages & media).
    pub(super) emoji_text_mode: bool,
    /// Active dark/light appearance mode (used for syntax-highlighted code runs).
    pub(super) dark: bool,
    /// When `true` (Block Kit `mrkdwn` `verbatim: true`, `F-COLLAB-758`), skip auto-linking
    /// raw un-bracketed URLs.
    pub(super) verbatim: bool,
    /// Paragraphs rendered with this theme so far: keeps each paragraph's element id unique
    /// within its message (GPUI keys interaction and accessibility state by the id path)
    /// and counts its selection document order.
    pub(super) paragraphs: Cell<usize>,
    /// The message this render belongs to; text ids are also unique across messages.
    pub(super) key: SharedString,
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
    fn body(cx: &App, key: SharedString, order: u64, links: Option<LinkHandler>) -> Self {
        Self::from_app(cx, key, Some(order), links)
    }

    /// The theme of preview text: not selectable, links open as URLs.
    fn plain(cx: &App, key: SharedString) -> Self {
        Self::from_app(cx, key, None, None)
    }

    fn run(
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

pub(super) fn flags(style: Option<&TextStyle>) -> RunFlags {
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
pub(super) enum Segment {
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
pub(super) struct ParagraphBuilder {
    pub(super) text: String,
    pub(super) runs: Vec<TextRun>,
    pub(super) links: Vec<(Range<usize>, LinkTarget)>,
    pub(super) segments: Vec<Segment>,
}

impl ParagraphBuilder {
    pub(super) fn new() -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            links: Vec::new(),
            segments: Vec::new(),
        }
    }

    fn push_text(
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
    fn push_edited(&mut self, theme: &RunTheme) {
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
    fn push_link(&mut self, url: String, label: String, run: RunFlags, theme: &RunTheme) {
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
        let target = LinkTarget::Url(url.into());
        let start = self.text.len();
        let mut link_run = run;
        if theme.underline_links {
            link_run.underline = true;
        }
        self.push_text(&label, theme, link_run, Some(theme.link), None);
        self.links.push((start..self.text.len(), target));
    }

    /// An in-app mention (an @user or #channel chip): the mention look, clickable through
    /// its range like a link.
    fn push_mention(
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
    fn push_attachment(&mut self, url: String, label: &str, theme: &RunTheme) {
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

    fn push_image(&mut self, image: ImageSource) {
        self.flush();
        self.segments.push(Segment::Image(image));
    }

    fn push_chip(&mut self, url: SharedString, label: String, icon: IconName, icon_size: f32) {
        self.flush();
        self.segments.push(Segment::Chip {
            url,
            label,
            icon,
            icon_size,
        });
    }

    fn push_color_swatch(&mut self, hex: String, color: Hsla) {
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
    fn trim_trailing_breaks(&mut self) {
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

    fn finish(mut self) -> Vec<Segment> {
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
pub(super) fn paragraph_break_split(
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
pub(super) fn text_element(
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

pub(super) fn render_segments(segments: Vec<Segment>, theme: &RunTheme) -> Div {
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
                let blocks = paragraph_break_split(&text, &runs, &links);
                if single && blocks.len() == 1 {
                    let (text, runs, links) = blocks.into_iter().next().unwrap();
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
pub(super) fn paragraph_id(
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
pub(super) fn trim_breaks(
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

// ---------------------------------------------------------------------------
// Selectable message text (gpui-base window selection): one paragraph is one participant
// with its own retained handle, registered in document order so drags span paragraphs and
// messages of the transcript; copy joins the selected paragraphs with newlines. Link
// ranges stay clickable (a press without a drag activates one) and underlines on hover.
// ---------------------------------------------------------------------------

/// One paragraph of message-body text that participates in the window text selection: a
/// [`StyledText`] with styled runs whose link ranges stay clickable (see
/// gpui-base's `SelectableText` for the plain-run original, and its `PlainSelectableText`
/// test element for this registration pattern).
pub(super) struct SelectableParagraph {
    pub(super) id: ElementId,
    pub(super) text: SharedString,
    pub(super) styled: StyledText,
    pub(super) links: Vec<(Range<usize>, LinkTarget)>,
    pub(super) handler: Option<LinkHandler>,
    pub(super) document_order: u64,
    pub(super) selection: Hsla,
    pub(super) link: Hsla,
    /// Inline code spans and their fill color, rendered as a single rounded bordered pill
    /// matching Slack's `.c-mrkdwn__code` (kept out of `washes` so fill and border share exact
    /// geometry with no top/bottom overflow or side gap).
    pub(super) code: Vec<(Range<usize>, Hsla)>,
    /// True when this paragraph is the body of a fenced code block (```` ```...``` ````), so
    /// triple-clicking selects the entire code block across all lines.
    pub(super) is_code_block: bool,
    /// Non-code run backgrounds (mentions, app chips), painted under the selection: as
    /// run backgrounds GPUI paints them with the text, over the highlight.
    pub(super) washes: Vec<(Range<usize>, Hsla)>,
    pub(super) code_border: Hsla,
    /// App chips' icon slots ([`APP_ICON_SLOT`]): text offsets the icons are painted at.
    pub(super) icons: Vec<usize>,
}

/// What one paragraph keeps between frames: its selection handle (and the subscription
/// that redraws the window when a drag moves its selection), any bounded local selection
/// (multi-click inside inline code or a fenced code block), the cached projected text for
/// window drags, the link it hovers, the link a press started in, and its key for
/// member-hover-card reports.
pub(super) struct ParagraphState {
    pub(super) handle: TextSelectionHandle,
    pub(super) local_range: Rc<RefCell<Option<Range<usize>>>>,
    pub(super) projected: Rc<RefCell<String>>,
    pub(super) hovered_link: Rc<Cell<Option<usize>>>,
    pub(super) mouse_down: Rc<Cell<Option<usize>>>,
    /// This paragraph's key in hover-card trigger reports (stable across frames).
    pub(super) hover_key: SharedString,
    pub(super) _refresh: Subscription,
}

impl SelectableParagraph {
    pub(super) fn new(
        id: ElementId,
        text: String,
        mut runs: Vec<TextRun>,
        links: Vec<(Range<usize>, LinkTarget)>,
        document_order: u64,
        theme: &RunTheme,
    ) -> Self {
        let text: SharedString = text.into();
        // Inline code is the inline-code run with a wash; code blocks carry no run
        // background. An app chip's icon slot is its own run on the link wash.
        let mut code: Vec<(Range<usize>, Hsla)> = Vec::new();
        let mut washes = Vec::new();
        let mut icons = Vec::new();
        let mut at = 0;
        for run in &mut runs {
            let span = at..at + run.len;
            at = span.end;
            let Some(wash) = run.background_color.take() else {
                continue;
            };
            if wash == theme.link_wash && text.get(span.clone()) == Some(APP_ICON_SLOT) {
                icons.push(span.start);
            }
            if run.font.family == theme.code_font.family {
                match code.last_mut() {
                    Some((last, last_wash)) if last.end == span.start && *last_wash == wash => {
                        last.end = span.end;
                    }
                    _ => code.push((span, wash)),
                }
                continue;
            }
            washes.push((span, wash));
        }
        Self {
            code,
            is_code_block: false,
            washes,
            icons,
            code_border: theme.foreground.opacity(0.13),
            styled: StyledText::new(text.clone()).with_runs(runs),
            id,
            text,
            links,
            handler: theme.links.clone(),
            document_order,
            selection: theme.selection,
            link: theme.link,
        }
    }

    pub(super) fn with_code_block(mut self) -> Self {
        self.is_code_block = true;
        self
    }

    /// The line-box quads a range covers: one per wrapped line between its ends (gpui-base's
    /// own selection painting, replicated because it is private there).
    fn span_quads(
        start: Point<Pixels>,
        end: Point<Pixels>,
        bounds: Bounds<Pixels>,
        line_height: Pixels,
    ) -> Vec<Bounds<Pixels>> {
        if start.y == end.y {
            return vec![Bounds::from_corners(
                start,
                Point::new(end.x, end.y + line_height),
            )];
        }
        let mut quads = vec![Bounds::from_corners(
            start,
            Point::new(bounds.right(), start.y + line_height),
        )];
        if end.y > start.y + line_height {
            quads.push(Bounds::from_corners(
                Point::new(bounds.left(), start.y + line_height),
                Point::new(bounds.right(), end.y),
            ));
        }
        quads.push(Bounds::from_corners(
            Point::new(bounds.left(), end.y),
            Point::new(end.x, end.y + line_height),
        ));
        quads
    }

    /// Slack's inline-code pill: a single rounded quad carrying both the fill and the 1px 13%
    /// foreground ring with 3px corners, slightly wider than the glyphs and inset 1px from the
    /// line box so the fill never pokes outside the border.
    fn paint_code_box(
        &self,
        layout: &TextLayout,
        range: Range<usize>,
        wash: Hsla,
        window: &mut Window,
    ) {
        for bounds in self.range_quads(layout, range) {
            let pill = Bounds::from_corners(
                Point::new(bounds.left() - px(2.), bounds.top() + px(1.)),
                Point::new(bounds.right() + px(2.), bounds.bottom() - px(1.)),
            );
            window.paint_quad(PaintQuad {
                bounds: pill,
                background: wash.into(),
                corner_radii: Corners::all(px(3.)),
                border_widths: Edges::all(px(1.)),
                border_color: self.code_border,
                border_style: BorderStyle::default(),
            });
        }
    }

    /// An app chip's icon in its slot at `ix`, centered on the line in the link color. Slack
    /// draws the app's own glyph (Jira's mark); the bundled link glyph stands in for it.
    fn paint_app_icon(&self, layout: &TextLayout, ix: usize, window: &mut Window, cx: &App) {
        let Some(at) = layout.position_for_index(ix) else {
            return;
        };
        let side = px(APP_ICON);
        let bounds = Bounds::new(
            Point::new(at.x + px(1.), at.y + (layout.line_height() - side) / 2.),
            gpui_kit::gpui::Size::new(side, side),
        );
        let icon = IconName::Link.path();
        if let Err(e) = window.paint_svg(
            bounds,
            icon.clone(),
            None,
            TransformationMatrix::unit(),
            self.link,
            cx,
        ) {
            eprintln!("slackless: could not paint the {icon} icon ({e})");
        }
    }

    /// A run wash under the text, laid out as GPUI lays out a run background.
    fn paint_wash(
        &self,
        layout: &TextLayout,
        range: Range<usize>,
        wash: Hsla,
        window: &mut Window,
    ) {
        for bounds in self.range_quads(layout, range) {
            window.paint_quad(PaintQuad {
                bounds,
                background: wash.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The boxes a text range covers, one per wrapped line, each ending where that line's
    /// text ends (a selection instead runs on to the paragraph's edge).
    fn range_quads(&self, layout: &TextLayout, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return Vec::new();
        };
        let line_height = layout.line_height();
        let quad = |y: Pixels, left: Pixels, right: Pixels| {
            Bounds::from_corners(Point::new(left, y), Point::new(right, y + line_height))
        };
        if start.y == end.y {
            return vec![quad(start.y, start.x, end.x)];
        }
        // A wrapped span. The layout places a wrap boundary at the end of the line it closes,
        // so the farthest position found on a line is where that line's text ends; a line the
        // span only touches at such a boundary gets no box.
        let left = layout.bounds().left();
        let mut quads = Vec::new();
        let (mut y, mut from, mut to) = (start.y, start.x, start.x);
        for ix in (range.start..=range.end).filter(|ix| self.text.is_char_boundary(*ix)) {
            let Some(at) = layout.position_for_index(ix) else {
                continue;
            };
            if at.y == y {
                to = to.max(at.x);
                continue;
            }
            if to - from > px(1.) {
                quads.push(quad(y, from, to));
            }
            (y, from, to) = (at.y, left, at.x);
        }
        if to - from > px(1.) {
            quads.push(quad(y, from, to));
        }
        quads
    }

    /// The selection highlight behind the text.
    fn paint_selection(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        for bounds in Self::span_quads(start, end, layout.bounds(), layout.line_height()) {
            window.paint_quad(PaintQuad {
                bounds,
                background: self.selection.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// Slack's hover affordance on URL links: the hovered link underlines (mentions keep
    /// their highlight). Painted over the text, so it needs no re-layout.
    fn paint_hover_underline(&self, layout: &TextLayout, range: Range<usize>, window: &mut Window) {
        let (Some(start), Some(end)) = (
            layout.position_for_index(range.start),
            layout.position_for_index(range.end),
        ) else {
            return;
        };
        let line_height = layout.line_height();
        for bounds in Self::span_quads(start, end, layout.bounds(), line_height) {
            let underline = Bounds::new(
                Point::new(bounds.origin.x, bounds.bottom() - px(4.)),
                gpui_kit::gpui::Size::new(bounds.size.width, px(1.)),
            );
            window.paint_quad(PaintQuad {
                bounds: underline,
                background: self.link.into(),
                corner_radii: Corners::default(),
                border_widths: Edges::default(),
                border_color: transparent_black(),
                border_style: BorderStyle::default(),
            });
        }
    }

    /// The paragraph's link interactions: the pointing cursor over a link, the hover
    /// underline (with a redraw when the hovered link changes) and the click — a press and
    /// a release inside one link range, so a drag that selects never activates the link.
    #[allow(clippy::too_many_arguments)]
    fn paint_links(
        &self,
        global_id: Option<&GlobalElementId>,
        layout: &TextLayout,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let hovered_now = layout
            .index_for_position(window.mouse_position())
            .ok()
            .and_then(|ix| self.links.iter().position(|(range, _)| range.contains(&ix)));
        if hovered_now.is_some() {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        if let Some(ix) = hovered_now {
            if let Some((range, LinkTarget::Url(_))) = self.links.get(ix) {
                self.paint_hover_underline(layout, range.clone(), window);
            }
        }
        let (mouse_down, hovered_link, hover_key) = window.with_element_state::<ParagraphState, _>(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |state, _| {
                let state = state.expect("request_layout retained the paragraph state");
                let cells = (
                    state.mouse_down.clone(),
                    state.hovered_link.clone(),
                    state.hover_key.clone(),
                );
                (cells, state)
            },
        );
        let down = {
            let layout = layout.clone();
            let mouse_down = mouse_down.clone();
            let hitbox = hitbox.clone();
            move |event: &MouseDownEvent, phase, window: &mut Window, _: &mut App| {
                if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    if let Ok(ix) = layout.index_for_position(event.position) {
                        mouse_down.set(Some(ix));
                    }
                }
            }
        };
        window.on_mouse_event(down);
        let up = {
            let (layout, links, handler) =
                (layout.clone(), self.links.clone(), self.handler.clone());
            let hitbox = hitbox.clone();
            move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                    return;
                }
                let Some(down) = mouse_down.take() else {
                    return;
                };
                let Ok(release) = layout.index_for_position(event.position) else {
                    return;
                };
                if let Some((_, target)) = links
                    .iter()
                    .find(|(range, _)| range.contains(&down) && range.contains(&release))
                {
                    route_link(handler.as_ref(), target.clone(), window, cx);
                }
            }
        };
        window.on_mouse_event(up);
        // The wheel handler below takes its own copies: `r#move` moves the cells in.
        let wheel_hover = (hover_key.clone(), hovered_link.clone());
        let r#move = {
            let (layout, links) = (layout.clone(), self.links.clone());
            let (hitbox, hover_key) = (hitbox.clone(), hover_key.clone());
            move |event: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                // Off the paragraph counts as no link, so the cell (and a profile-link hover
                // card trigger) clears when the pointer leaves the hitbox too.
                let now = if hitbox.is_hovered(window) {
                    layout
                        .index_for_position(event.position)
                        .ok()
                        .and_then(|ix| links.iter().position(|(range, _)| range.contains(&ix)))
                } else {
                    None
                };
                let was = hovered_link.replace(now);
                if was == now {
                    return;
                }
                // Profile links (@mention pills) drive the member hover card: the anchor is
                // the pill's first glyph in window coordinates, pill-wide and one line tall.
                let trigger = |ix: usize| {
                    links.get(ix).and_then(|(range, target)| {
                        let LinkTarget::Profile(user) = target else {
                            return None;
                        };
                        let origin = layout.position_for_index(range.start)?;
                        Some((
                            user.clone(),
                            Bounds::new(origin, size(px(40.), layout.line_height())),
                        ))
                    })
                };
                if let Some((user, anchor)) = now.and_then(trigger) {
                    hover_card::entered(
                        Trigger {
                            key: hover_key.clone(),
                            user,
                            anchor,
                        },
                        cx,
                    );
                } else if was.and_then(trigger).is_some() {
                    hover_card::left(&hover_key, cx);
                }
                window.refresh();
            }
        };
        window.on_mouse_event(r#move);
        // A scroll can move a trigger out from under a still pointer: the card closes, as
        // Slack's does. Only the paragraph owning the trigger reports (its cell is set).
        let wheel = {
            let (hover_key, hovered_link) = wheel_hover;
            move |_: &ScrollEvent, phase, _window: &mut Window, cx: &mut App| {
                if phase == DispatchPhase::Bubble && hovered_link.replace(None).is_some() {
                    hover_card::left(&hover_key, cx);
                }
            }
        };
        window.on_mouse_event(wheel);
    }

    /// Bounded multi-click selection inside inline code pills (`.c-mrkdwn__code`) and fenced
    /// code blocks (`.c-mrkdwn__pre`), matching Slack:
    /// - Double-click inside inline code selects the word strictly clamped inside that pill.
    /// - Triple-click (or 4+ clicks) inside inline code selects only that inline code span,
    ///   never the surrounding paragraph.
    /// - Triple-click (or 4+ clicks) inside a fenced code block selects the entire code block
    ///   across all lines.
    fn paint_code_clicks(
        &self,
        handle: &TextSelectionHandle,
        local_range: Rc<RefCell<Option<Range<usize>>>>,
        layout: &TextLayout,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        let code_pills: Vec<(Range<usize>, Vec<Bounds<Pixels>>)> = self
            .code
            .iter()
            .map(|(range, _)| {
                let quads = self
                    .range_quads(layout, range.clone())
                    .into_iter()
                    .map(|b| {
                        Bounds::from_corners(
                            Point::new(b.left() - px(2.), b.top() + px(1.)),
                            Point::new(b.right() + px(2.), b.bottom() - px(1.)),
                        )
                    })
                    .collect();
                (range.clone(), quads)
            })
            .collect();
        let is_code_block = self.is_code_block;
        let text = self.text.clone();
        let layout = layout.clone();
        let hitbox = hitbox.clone();
        let handle = handle.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || event.click_count < 2
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let selected = if is_code_block {
                if event.click_count >= 3 && !text.is_empty() {
                    Some(0..text.len())
                } else {
                    None
                }
            } else {
                let hit = match layout.index_for_position(event.position) {
                    Ok(ix) => code_pills
                        .iter()
                        .find(|(span, _)| span.contains(&ix))
                        .map(|(span, _)| (span.clone(), ix))
                        .or_else(|| {
                            code_pills
                                .iter()
                                .find(|(_, quads)| {
                                    quads.iter().any(|q| q.contains(&event.position))
                                })
                                .map(|(span, _)| {
                                    let clamped = ix.clamp(
                                        span.start,
                                        span.end.saturating_sub(1).max(span.start),
                                    );
                                    (span.clone(), clamped)
                                })
                        }),
                    Err(nearest) => code_pills
                        .iter()
                        .find(|(_, quads)| quads.iter().any(|q| q.contains(&event.position)))
                        .map(|(span, _)| {
                            let clamped = nearest
                                .clamp(span.start, span.end.saturating_sub(1).max(span.start));
                            (span.clone(), clamped)
                        }),
                };
                hit.and_then(|(span, ix)| {
                    if event.click_count >= 3 {
                        (!span.is_empty()).then_some(span)
                    } else {
                        word_range_bounded(&text, span, ix)
                    }
                })
            };
            let Some(range) = selected else {
                return;
            };
            GlobalState::suppress_text_selection(cx);
            *local_range.borrow_mut() = Some(range);
            handle.set_local_selection(true, cx);
            window.refresh();
        });
    }
}

/// Word range around `offset`, strictly clamped to `bounds` so double-clicking inside an
/// inline code span never bleeds into adjacent prose.
fn word_range_bounded(text: &str, bounds: Range<usize>, offset: usize) -> Option<Range<usize>> {
    if bounds.is_empty() || bounds.end > text.len() {
        return None;
    }
    let slice = text.get(bounds.clone())?;
    let mut rel = offset.saturating_sub(bounds.start).min(slice.len());
    if rel == slice.len() {
        rel = slice.char_indices().next_back().map_or(0, |(ix, _)| ix);
    } else {
        while rel > 0 && !slice.is_char_boundary(rel) {
            rel -= 1;
        }
    }
    let ch = slice[rel..].chars().next()?;
    let classify = |c: char| -> u8 {
        if c == '_'
            || c.is_ascii_alphanumeric()
            || matches!(
                c,
                '\u{00C0}'..='\u{024F}'
                    | '\u{0400}'..='\u{04FF}'
                    | '\u{1E00}'..='\u{1EFF}'
                    | '\u{0300}'..='\u{036F}'
            )
        {
            1 // Word
        } else if matches!(c, '\n' | '\r') {
            2 // Newline
        } else if c.is_whitespace() {
            3 // Whitespace
        } else {
            4 // Other
        }
    };
    let kind = classify(ch);
    let connects = |next: char| matches!((kind, classify(next)), (1, 1) | (3, 3));
    let mut start = rel;
    for prev in slice[..rel].chars().rev().take(128) {
        if connects(prev) {
            start -= prev.len_utf8();
        } else {
            break;
        }
    }
    let mut end = rel + ch.len_utf8();
    for next in slice[end..].chars().take(128) {
        if connects(next) {
            end += next.len_utf8();
        } else {
            break;
        }
    }
    Some((bounds.start + start)..(bounds.start + end))
}

impl gpui_kit::gpui::IntoElement for SelectableParagraph {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectableParagraph {
    type RequestLayoutState = TextSelectionHandle;
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let handle = window.with_element_state(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |retained: Option<ParagraphState>, window| {
                let retained = retained.unwrap_or_else(|| {
                    let handle = TextSelectionHandle::new(self.text.clone(), cx);
                    let local_range: Rc<RefCell<Option<Range<usize>>>> =
                        Rc::new(RefCell::new(None));
                    let projected: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
                    {
                        let local_for_copy = local_range.clone();
                        let projected_for_copy = projected.clone();
                        let text_for_copy = self.text.clone();
                        handle.copy_with(
                            move |_| {
                                if let Some(range) = local_for_copy.borrow().as_ref() {
                                    return text_for_copy
                                        .get(range.clone())
                                        .unwrap_or_default()
                                        .to_owned();
                                }
                                projected_for_copy.borrow().clone()
                            },
                            cx,
                        );
                    }
                    {
                        let local_for_clear = local_range.clone();
                        handle.clear_with(
                            move |_| {
                                *local_for_clear.borrow_mut() = None;
                            },
                            cx,
                        );
                    }
                    let refresh = handle.refresh_window_on_change(window, cx);
                    ParagraphState {
                        handle,
                        local_range,
                        projected,
                        hovered_link: Rc::new(Cell::new(None)),
                        mouse_down: Rc::new(Cell::new(None)),
                        hover_key: format!("{:?}", self.id).into(),
                        _refresh: refresh,
                    }
                });
                (retained.handle.clone(), retained)
            },
        );
        let (layout_id, ()) = self
            .styled
            .request_layout(global_id, inspector_id, window, cx);
        (layout_id, handle)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.styled
            .prepaint(global_id, inspector_id, bounds, &mut (), window, cx);
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let registration = TextSelectionRegistration::new(hitbox.clone(), bounds)
            .with_document_order(self.document_order)
            .with_text_bounds(vec![bounds])
            .with_rendered_element(handle, window, cx);
        handle.register(registration, window, cx);
        hitbox
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        handle: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let layout = self.styled.layout().clone();
        #[cfg(test)]
        super::tests::record_painted_paragraph(&self.text, &layout);
        let projection = handle.update_runs(
            &[
                TextSelectionRun::new(self.text.clone(), layout.clone(), bounds)
                    .with_document_order(self.document_order),
            ],
            cx,
        );
        let (local_range, projected) = window.with_element_state::<ParagraphState, _>(
            global_id.expect("SelectableParagraph must have a stable element id"),
            |state, _| {
                let state = state.expect("request_layout retained the paragraph state");
                let cells = (state.local_range.clone(), state.projected.clone());
                (cells, state)
            },
        );
        {
            let mut out = projected.borrow_mut();
            out.clear();
            for range in projection.ranges().iter().flatten() {
                if let Some(slice) = self.text.get(range.clone()) {
                    out.push_str(slice);
                }
            }
        }
        for (range, wash) in &self.washes {
            self.paint_wash(&layout, range.clone(), *wash, window);
        }
        for (range, wash) in &self.code {
            self.paint_code_box(&layout, range.clone(), *wash, window);
        }
        if let Some(range) = local_range.borrow().clone() {
            self.paint_selection(&layout, range, window);
        } else {
            for range in projection.ranges().iter().flatten().cloned() {
                self.paint_selection(&layout, range, window);
            }
        }
        self.styled.paint(
            global_id,
            inspector_id,
            bounds,
            &mut (),
            &mut (),
            window,
            cx,
        );
        for &ix in &self.icons {
            self.paint_app_icon(&layout, ix, window, cx);
        }
        // Selectable text takes the text cursor, as in Slack; links switch it to a hand.
        let over_link = layout
            .index_for_position(window.mouse_position())
            .ok()
            .is_some_and(|ix| self.links.iter().any(|(range, _)| range.contains(&ix)));
        if !over_link {
            window.set_cursor_style(CursorStyle::IBeam, hitbox);
        }
        if self.is_code_block || !self.code.is_empty() {
            self.paint_code_clicks(handle, local_range, &layout, hitbox, window);
        }
        if !self.links.is_empty() {
            self.paint_links(global_id, &layout, hitbox, window);
        }
    }
}

pub(super) fn paragraph(
    elements: &[RichTextElement],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Option<Div> {
    let mut builder = ParagraphBuilder::new();
    for element in elements {
        push_element(&mut builder, element, fixtures, theme);
    }
    builder.trim_trailing_breaks();
    if edited {
        builder.push_edited(theme);
    }
    let segments = builder.finish();
    if segments.is_empty() {
        return None;
    }
    Some(render_segments(segments, theme))
}

pub(super) fn push_element(
    builder: &mut ParagraphBuilder,
    element: &RichTextElement,
    fixtures: &Fixtures,
    theme: &RunTheme,
) {
    match element {
        RichTextElement::Text(el) => {
            // Inline code gets the same wash as mrkdwn's backticks.
            let run = flags(el.style.as_ref());
            let bg = run.code.then_some(theme.code_bg);
            builder.push_text(&el.text, theme, run, None, bg);
        }
        RichTextElement::Link(el) => {
            let label = el.text.clone().unwrap_or_else(|| el.url.clone());
            // Slack draws a link inside backticks as plain code, not a link.
            let run = flags(el.style.as_ref());
            if run.code {
                builder.push_text(&label, theme, run, None, Some(theme.code_bg));
            } else {
                builder.push_link(el.url.clone(), label, run, theme);
            }
        }
        RichTextElement::User(el) => {
            let label = format!("@{}", fixtures.mention_name(&el.user_id));
            builder.push_mention(
                &label,
                LinkTarget::Profile(el.user_id.clone()),
                theme,
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Channel(el) => {
            let label = fixtures
                .conversation(&el.channel_id)
                .map(|conversation| fixtures.conversation_label(conversation))
                .unwrap_or_else(|| format!("#{}", el.channel_id));
            builder.push_mention(
                &label,
                LinkTarget::Channel(el.channel_id.clone()),
                theme,
                None,
            );
        }
        RichTextElement::Usergroup(el) => {
            let label = fixtures
                .usergroup_handle(&el.usergroup_id)
                .map(|handle| format!("@{handle}"))
                .unwrap_or_else(|| "@group".to_owned());
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Team(_) => {
            builder.push_text(
                "@workspace",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Broadcast(el) => {
            let label = format!("@{}", el.range.as_str());
            builder.push_text(
                &label,
                theme,
                mention_flags(),
                Some(theme.link),
                Some(theme.mention_bg),
            );
        }
        RichTextElement::Emoji(el) => {
            if theme.emoji_text_mode {
                let literal = format!(":{}:", el.name);
                builder.push_text(&literal, theme, flags(el.style.as_ref()), None, None);
            } else {
                let glyph = el
                    .unicode
                    .as_deref()
                    .and_then(unicode_glyph)
                    .or_else(|| emoji::display_glyph(&el.name));
                match (glyph, fixtures.custom_emoji(&el.name)) {
                    (Some(glyph), _) => {
                        builder.push_text(&glyph, theme, flags(el.style.as_ref()), None, None);
                    }
                    (None, Some(image)) => builder.push_image(image.source()),
                    (None, None) => {
                        let literal = format!(":{}:", el.name);
                        builder.push_text(&literal, theme, flags(el.style.as_ref()), None, None);
                    }
                }
            }
        }
        RichTextElement::Date(el) => {
            let text = el.fallback.clone().unwrap_or_else(|| {
                if el.timestamp > 0 {
                    date_label_absolute(&CivilTime::from_unix(el.timestamp))
                } else {
                    el.format.clone()
                }
            });
            builder.push_text(&text, theme, RunFlags::default(), None, None);
        }
        RichTextElement::Color(el) => {
            if let Some(color) = hex_color(&el.value) {
                builder.push_color_swatch(el.value.clone(), color);
            } else {
                builder.push_text(&el.value, theme, RunFlags::default(), None, None);
            }
        }
        RichTextElement::MessageMention(el) => {
            // Slack's chip for a link to a message: a speech bubble and "Author in #channel",
            // opening the message (its thread, for a reply).
            let author = el
                .author_id
                .as_ref()
                .and_then(|user| fixtures.user(user))
                .map(|user| fixtures.name_of(user).to_owned());
            let place = fixtures
                .conversation(&el.channel_id)
                .and_then(|conversation| conversation.name.as_ref())
                .map(|name| format!("#{name}"));
            let label = match (author, place) {
                (Some(author), Some(place)) => format!("{author} in {place}"),
                (Some(label), None) | (None, Some(label)) => label,
                (None, None) => el.text.clone().unwrap_or_else(|| "a message".to_owned()),
            };
            match &el.url {
                Some(url) => {
                    builder.push_chip(url.clone().into(), label, IconName::MessageCircle, 15.)
                }
                None => {
                    builder.push_text(&label, theme, RunFlags::default(), Some(theme.link), None)
                }
            }
        }
        RichTextElement::AttachmentMention(el) => {
            builder.push_attachment(el.url.clone(), el.text.as_deref().unwrap_or(&el.url), theme);
        }
        RichTextElement::Canvas(_) => {
            builder.push_text(
                "[canvas]",
                theme,
                RunFlags::default(),
                Some(theme.muted),
                None,
            );
        }
        RichTextElement::Unknown { .. } => {}
    }
}

/// Block-level rich text containers: paragraphs, lists, quotes and code blocks. With
/// `edited`, the final container's plain text (a section, or a list's last item) carries the
/// edit marker inline at its end; boxed text (quotes) and code blocks leave it to the body's
/// own trailing line.
pub(super) fn render_containers(
    containers: &[RichTextContainer],
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Vec<AnyElement> {
    containers
        .iter()
        .enumerate()
        .filter_map(|(n, container)| {
            let edited = edited && n + 1 == containers.len();
            match container {
                RichTextContainer::Section(section) => {
                    paragraph(&section.elements, fixtures, theme, edited)
                        .map(IntoElement::into_any_element)
                }
                RichTextContainer::List(list) if !list.elements.is_empty() => {
                    Some(render_list(list, fixtures, theme, edited).into_any_element())
                }
                RichTextContainer::Quote(quote) => {
                    paragraph(&quote.elements, fixtures, theme, false).map(|paragraph| {
                        div()
                            .border_l_2()
                            .border_color(theme.border)
                            .pl_3()
                            .child(paragraph)
                            .into_any_element()
                    })
                }
                RichTextContainer::Preformatted(preformatted) => {
                    Some(render_code(preformatted, theme).into_any_element())
                }
                RichTextContainer::Header(header) => {
                    paragraph(&header.elements, fixtures, theme, false).map(|paragraph| {
                        // Slack's `.p-rich_text_header`: 18px Black for Big, 15px Black for
                        // Medium and 15px Bold for Small, each on a 22.5px line, with 1em
                        // above (none as the first container) and 4px below.
                        let (size, weight) = match header.level.unwrap_or(2).clamp(1, 3) {
                            1 => (18., FontWeight::BLACK),
                            2 => (15., FontWeight::BLACK),
                            _ => (15., FontWeight::BOLD),
                        };
                        div()
                            .when(n > 0, |d| d.mt(px(15.)))
                            .mb(px(4.))
                            .text_size(px(size))
                            .line_height(px(22.5))
                            .font_weight(weight)
                            .child(paragraph)
                            .into_any_element()
                    })
                }
                RichTextContainer::Divider(_) => {
                    // Slack's `.p-rich_text_divider`: a full-width rule, 8px clear above and
                    // below.
                    Some(
                        div()
                            .w_full()
                            .my_2()
                            .h(px(1.))
                            .bg(theme.border)
                            .into_any_element(),
                    )
                }
                _ => None,
            }
        })
        .collect()
}

pub(super) fn render_list(
    list: &RichTextList,
    fixtures: &Fixtures,
    theme: &RunTheme,
    edited: bool,
) -> Div {
    let depth = list
        .indent
        .unwrap_or(0)
        .saturating_add(list.offset.unwrap_or(0));
    // Slack's disc sits 10px in, its text 28px in, nesting 22px a level.
    let indent = px(10. + 22. * depth as f32);
    let mut column = v_flex().gap_1();
    for (n, item) in list.elements.iter().enumerate() {
        let elements: &[RichTextElement] = match item {
            RichTextContainer::Section(section) => &section.elements,
            _ => &[],
        };
        if elements.is_empty() {
            continue;
        }
        let ordered = matches!(list.style, ListStyle::Ordered);
        let marker = match &list.style {
            ListStyle::Ordered => format!("{}.", n + 1),
            _ => "•".to_owned(),
        };
        let last = n + 1 == list.elements.len();
        if let Some(item_row) = paragraph(elements, fixtures, theme, edited && last) {
            column = column.child(
                h_flex()
                    .items_start()
                    .pl(indent)
                    .child(
                        div()
                            .flex_none()
                            .w(px(18.))
                            .when(!ordered, |marker| {
                                marker.text_size(px(20.)).line_height(px(22.))
                            })
                            .text_color(theme.foreground)
                            .child(marker),
                    )
                    .child(item_row),
            );
        }
    }
    column
}

pub(super) fn render_code(block: &RichTextPreformatted, theme: &RunTheme) -> Div {
    let code = block
        .elements
        .iter()
        .filter_map(|element| match element {
            RichTextElement::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<String>();
    code_block(
        code.trim_end_matches('\n').to_owned(),
        block
            .language
            .clone()
            .filter(|language| !language.is_empty()),
        theme,
    )
}

fn highlighted_code_runs(code: &str, lang: crate::syntax::Lang, theme: &RunTheme) -> Vec<TextRun> {
    if code.is_empty() {
        return Vec::new();
    }
    crate::syntax::highlight(lang, code)
        .into_iter()
        .map(|(len, kind)| {
            let color = crate::syntax::token_color(kind, theme.dark, theme.foreground);
            let mut run = theme.run(
                RunFlags {
                    code: true,
                    ..DEFAULT_FLAGS
                },
                Some(color),
                None,
                len,
            );
            run.font = theme.mono_font.clone();
            run
        })
        .collect()
}

/// One selectable monospaced code line/block (used by [`crate::files::SnippetCard`]) that
/// participates in the window's [`TextSelectionLayer`] with bounded double/triple-click and `Cmd+C`.
pub(crate) fn selectable_code(
    key: &str,
    line_ix: usize,
    text: String,
    lang: crate::syntax::Lang,
    document_order: u64,
    cx: &App,
) -> AnyElement {
    let theme = RunTheme::body(cx, SharedString::from(key.to_owned()), document_order, None);
    let runs = highlighted_code_runs(&text, lang, &theme);
    SelectableParagraph::new(
        paragraph_id(&theme.key, line_ix, 0, &text),
        text,
        runs,
        Vec::new(),
        document_order,
        &theme,
    )
    .with_code_block()
    .into_any_element()
}

/// Slack's pre block: 12px code on an 18px line, 8px in from a 13% foreground ring with 4px
/// corners, 32px kept clear at the right for the copy button a hover shows there.
pub(super) fn code_block(code: String, language: Option<String>, theme: &RunTheme) -> Div {
    // Code blocks select like the paragraphs around them: one participant, its own order.
    let paragraph = theme.paragraphs.get();
    theme.paragraphs.set(paragraph + 1);
    let order = theme.order.map(|base| base + paragraph as u64);
    let lang = crate::syntax::Lang::detect(language.as_deref(), None, &code);
    let runs = highlighted_code_runs(&code, lang, theme);
    let group = SharedString::from(format!("code-{}-{paragraph}", theme.key));
    let copy = CopyCode {
        id: ElementId::Name(format!("copy-{}-{paragraph}", theme.key).into()),
        group: group.clone(),
        code: code.clone(),
    };
    let text = match order {
        Some(order) => SelectableParagraph::new(
            paragraph_id(&theme.key, paragraph, 0, &code),
            code,
            runs,
            Vec::new(),
            order,
            theme,
        )
        .with_code_block()
        .into_any_element(),
        None => StyledText::new(code).with_runs(runs).into_any_element(),
    };
    v_flex()
        .min_w_0()
        .gap_1()
        .when_some(language, |column, language| {
            column.child(div().text_xs().text_color(theme.muted).child(language))
        })
        .child(
            div()
                .group(group)
                .relative()
                .w_full()
                .min_w_0()
                .rounded(px(4.))
                .border_1()
                .border_color(theme.foreground.opacity(0.13))
                .bg(theme.code_bg)
                .p_2()
                .pr(px(32.))
                .text_size(px(12.))
                .line_height(px(18.))
                .child(text)
                .child(copy),
        )
}

/// Slack's copy button on a pre block: the block's hover reveals it with a "Copy" tooltip;
/// a click copies the code and shows a check in a focus ring under "Copied!" until the
/// pointer leaves.
#[derive(IntoElement)]
pub(super) struct CopyCode {
    pub(super) id: ElementId,
    /// The pre block's hover group, which reveals the button.
    pub(super) group: SharedString,
    pub(super) code: String,
}

#[derive(Clone, Copy, Default)]
pub(super) struct CopyCodeState {
    pub(super) hovered: bool,
    pub(super) copied: bool,
    /// Clicks so far: a click's timer clears `copied` only if no later click restarted it.
    pub(super) copies: u32,
}

/// How long "Copied!" stays up after a click, the pointer gone or not.
pub(super) const COPIED_FOR: Duration = Duration::from_secs(2);

impl RenderOnce for CopyCode {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| CopyCodeState::default());
        let CopyCodeState {
            hovered, copied, ..
        } = *state.read(cx);
        let theme = cx.theme();
        let (fg, muted, ring) = (theme.foreground, theme.muted_foreground, theme.ring);
        // The pre block's own fill (`RunTheme::code_bg`).
        let block_bg = theme.secondary;
        let hover_bg = if copied { block_bg } else { fg.opacity(0.08) };
        let tip = (hovered || copied).then(|| {
            deferred(code_tooltip(if copied { "Copied!" } else { "Copy" }, cx)).with_priority(1)
        });
        div()
            .id(self.id)
            .absolute()
            .top(px(3.))
            .right(px(3.))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .opacity(0.)
            .group_hover(self.group, |button| button.opacity(1.))
            .when(hovered || copied, |button| button.opacity(1.))
            .hover(move |button| button.bg(hover_bg))
            .when(copied, |button| {
                // Slack's focus ring: a 1px line in a translucent 4px halo. GPUI paints a
                // shadow under the whole box, so an opaque fill keeps the halo outside it.
                button
                    .bg(block_bg)
                    .border_1()
                    .border_color(ring)
                    .shadow(vec![gpui_kit::gpui::BoxShadow {
                        color: ring.opacity(0.3),
                        offset: gpui_kit::gpui::point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(4.),
                        inset: false,
                    }])
            })
            .on_hover({
                let state = state.clone();
                move |hovered, window, cx| {
                    state.update(cx, |state, _| state.hovered = *hovered);
                    // Notifying the state alone repaints the transcript one event late.
                    window.refresh();
                }
            })
            .on_click(move |_, window, cx| {
                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(self.code.clone()));
                let copy = state.update(cx, |state, _| {
                    state.copied = true;
                    state.copies += 1;
                    state.copies
                });
                window.refresh();
                let (state, window) = (state.clone(), window.window_handle());
                cx.spawn(async move |cx| {
                    cx.background_executor().timer(COPIED_FOR).await;
                    state.update(cx, |state, _| state.copied &= state.copies != copy);
                    // A closed window has no button left to update.
                    let _ = window.update(cx, |_, window, _| window.refresh());
                })
                .detach();
            })
            .child(
                Icon::new(if copied {
                    IconName::Check
                } else {
                    IconName::Copy
                })
                .size(px(14.))
                .text_color(if hovered || copied { fg } else { muted }),
            )
            .children(tip)
    }
}

/// The tooltip over a pre block's copy button: Slack's bubble, 13px bold in a 1px ring, its
/// arrow pointing down at the button 2px below.
pub(super) fn code_tooltip(text: &'static str, cx: &App) -> Div {
    let theme = cx.theme();
    let (fill, ink, edge) = (
        theme.popover,
        theme.popover_foreground,
        theme.foreground.opacity(0.2),
    );
    div()
        .absolute()
        .bottom(px(30.))
        .left(px(-46.))
        .w(px(120.))
        .flex()
        .justify_center()
        .child(
            v_flex()
                .items_center()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(edge)
                        .bg(fill)
                        .text_color(ink)
                        .text_size(px(13.))
                        .line_height(px(16.25))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_nowrap()
                        .child(text),
                )
                .child(
                    // The arrow keeps the ring: a fill triangle 1.5px above a ring-colored
                    // one leaves a 1px edge and covers the bubble's ring where they meet.
                    div()
                        .relative()
                        .mt(px(-1.))
                        .child(crate::sidebar::tooltip_arrow(edge, false))
                        .child(
                            div()
                                .absolute()
                                .top(px(-1.5))
                                .left_0()
                                .child(crate::sidebar::tooltip_arrow(fill, false)),
                        ),
                ),
        )
}

/// Whether the body's final rendered paragraph is plain text: a rich-text section or list
/// at the end of the blocks, or the mrkdwn fallback when no block renders anything. Slack's
/// edit marker rides inline at that paragraph's end; any other body (Block Kit block,
/// attachment, image) puts the marker on its own line after the body.
pub(super) fn body_ends_in_text(message: &Message) -> bool {
    if !message.attachments.is_empty() {
        return false;
    }
    let fallback_text = message
        .blocks
        .iter()
        .all(|block| matches!(block, Block::RichText(rich) if rich.elements.is_empty()))
        && !message.text.trim().is_empty();
    let ends_in_paragraph = matches!(
        message.blocks.last(),
        Some(Block::RichText(rich))
            if matches!(
                rich.elements.last(),
                // After a list Slack puts the edit marker on its own line.
                Some(RichTextContainer::Section(_))
            )
    );
    fallback_text || ends_in_paragraph
}

pub(super) fn message_body(message: &Message, kcx: &KitCx, cx: &App) -> Div {
    let has_kit = message
        .blocks
        .iter()
        .any(|block| matches!(block, Block::Unknown { .. }));
    let mut column = v_flex()
        .min_w_0()
        .gap_1()
        .text_sm()
        .when(has_kit || !message.attachments.is_empty(), |column| {
            column.gap_2()
        });
    let edited = message.edited.is_some();
    let inline_edited = edited && body_ends_in_text(message);
    if message.subtype == Some(slackless_core::types::MessageSubtype::HuddleThread) {
        if let Some(card) = render_huddle_thread_card(message, kcx.fixtures, cx) {
            return column.child(card);
        }
    }
    let attachments = blockkit::attachments(message, kcx, cx);
    // Jumbomoji: when enabled (and not in plain-text emoji mode) and the message is 1..=5 emoji only, render them large (32px).
    let display = crate::preferences::display_prefs(cx);
    if display.jumbomoji
        && !display.emoji_text_mode
        && !has_kit
        && message.attachments.is_empty()
        && message.files.is_empty()
    {
        if let Some(jumbos) = jumbomoji_elements(&message.text, kcx.fixtures) {
            column = column.child(h_flex().items_center().gap_1p5().children(jumbos));
            if edited {
                let muted = cx.theme().muted_foreground;
                column = column.child(div().text_xs().text_color(muted).child("(edited)"));
            }
            return column;
        }
    }
    let mut rendered = 0;
    let last_block = message.blocks.len().saturating_sub(1);
    for (ix, block) in message.blocks.iter().enumerate() {
        let elements = render_block(block, ix, kcx, inline_edited && ix == last_block, cx);
        rendered += elements.len();
        column = column.children(elements);
    }
    if rendered == 0 && !message.text.is_empty() {
        column = column.child(mrkdwn(&message.text, kcx, false, inline_edited, cx));
    }
    if edited && !inline_edited {
        let muted = cx.theme().muted_foreground;
        column = column.child(div().text_xs().text_color(muted).child("(edited)"));
    }
    column.children(attachments)
}

/// Format huddle duration cleanly (`"16s"`, `"12m 4s"`, `"1h 5m"`).
pub(crate) fn format_huddle_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

/// Render official Slack's rich `huddle_thread` card in the message transcript (`F-COLLAB-003`).
fn render_huddle_thread_card(message: &Message, fixtures: &Fixtures, cx: &App) -> Option<Div> {
    let room = message.extra.get("room")?;
    let theme = cx.theme();
    let has_ended = room
        .get("has_ended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let custom_name = room
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let title = custom_name.map(str::to_owned).unwrap_or_else(|| {
        if has_ended {
            "A huddle happened".to_owned()
        } else {
            "A huddle is happening".to_owned()
        }
    });
    let date_start = room.get("date_start").and_then(|v| v.as_u64());
    let date_end = room.get("date_end").and_then(|v| v.as_u64());
    let duration_label = match (date_start, date_end) {
        (Some(start), Some(end)) if end > start => Some(format_huddle_duration(end - start)),
        _ => None,
    };
    let participant_ids: Vec<UserId> = room
        .get(if has_ended {
            "participant_history"
        } else {
            "participants"
        })
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| u.as_str().map(UserId::from))
                .collect()
        })
        .unwrap_or_default();
    let joined_summary = match participant_ids.len() {
        0 => None,
        1 => Some(format!(
            "{} joined",
            fixtures.user_name(&participant_ids[0])
        )),
        n => Some(format!("{n} people joined")),
    };
    let accent = if has_ended { theme.link } else { theme.success };
    let subtitle = match (joined_summary, duration_label) {
        (Some(j), Some(d)) => format!("{j} · {d}"),
        (Some(j), None) => j,
        (None, Some(d)) => format!("Lasted {d}"),
        (None, None) => {
            if has_ended {
                "Ended".to_owned()
            } else {
                "Live now".to_owned()
            }
        }
    };
    Some(
        h_flex()
            .max_w(px(460.))
            .p_3()
            .gap_3()
            .items_center()
            .rounded_lg()
            .border_1()
            .border_color(if has_ended {
                theme.border
            } else {
                accent.opacity(0.45)
            })
            .bg(theme.secondary.opacity(0.42))
            .child(
                div()
                    .size(px(36.))
                    .flex_none()
                    .rounded_md()
                    .bg(accent.opacity(0.16))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Headphones)
                            .size(px(18.))
                            .text_color(accent),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(subtitle),
                    ),
            )
            .when(!participant_ids.is_empty(), |row| {
                row.child(
                    h_flex().items_center().gap_0p5().children(
                        participant_ids
                            .iter()
                            .take(4)
                            .map(|uid| crate::avatars::user_chip(fixtures, uid, 22.)),
                    ),
                )
            }),
    )
}

/// One block of a message (or attachment): `rich_text` here, everything else through
/// [`blockkit::block`].
pub(crate) fn render_block(
    block: &Block,
    ix: usize,
    kcx: &KitCx,
    edited: bool,
    cx: &App,
) -> Vec<AnyElement> {
    match block {
        Block::RichText(rich) => {
            let (key, order) = kcx.text_theme();
            render_containers(
                &rich.elements,
                kcx.fixtures,
                &RunTheme::body(cx, key.into(), order, kcx.links.clone()),
                edited,
            )
        }
        Block::Unknown { r#type, raw } => blockkit::block(r#type, raw, ix, kcx, cx)
            .into_iter()
            .collect(),
    }
}

/// Renders Slack mrkdwn (fallback text, Block Kit text objects, attachment text), one line
/// per paragraph: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<@user>` / `<#channel>` /
/// `<!here>` chips, `<url|label>` links and `:emoji:` (custom emoji as images). `muted`
/// renders plain runs in the muted color (Block Kit context blocks). Unknown markup stays
/// literal. With a link handler (see [`KitCx`]) the text is selectable and its links route
/// through it.
pub(crate) fn mrkdwn(text: &str, kcx: &KitCx, muted: bool, edited: bool, cx: &App) -> Div {
    mrkdwn_with_verbatim(text, kcx, muted, edited, false, cx)
}

/// Render Slack `mrkdwn`, honoring Block Kit's `verbatim` flag (`F-COLLAB-758`): when `verbatim`
/// is `true`, raw un-bracketed URLs stay literal instead of being auto-linked.
pub(crate) fn mrkdwn_with_verbatim(
    text: &str,
    kcx: &KitCx,
    muted: bool,
    edited: bool,
    verbatim: bool,
    cx: &App,
) -> Div {
    let (key, order) = kcx.text_theme();
    let mut theme = RunTheme::body(cx, key.into(), order, kcx.links.clone());
    theme.verbatim = verbatim;
    if muted {
        theme.foreground = theme.muted;
    }
    let pieces = mrkdwn_fences(text);
    let mut column = v_flex().min_w_0();
    for (n, (fenced, piece)) in pieces.iter().enumerate() {
        column = if *fenced {
            column.child(
                div()
                    .my_1()
                    .child(code_block(unescape(piece), None, &theme)),
            )
        } else {
            mrkdwn_lines(
                column,
                piece,
                kcx.fixtures,
                &theme,
                edited && n + 1 == pieces.len(),
            )
        };
    }
    // A body ending in a pre block puts the edit marker on its own line after it.
    if edited && pieces.last().is_some_and(|(fenced, _)| *fenced) {
        column = column.child(div().text_xs().text_color(theme.muted).child("(edited)"));
    }
    column
}

/// mrkdwn split at its ``` fences into `(fenced, text)` pieces: Slack renders fenced text
/// as a pre block, raw, dropping the line break just inside and outside each fence. An
/// unclosed fence stays literal; empty text pieces are left out.
pub(super) fn mrkdwn_fences(text: &str) -> Vec<(bool, &str)> {
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
pub(super) fn strip_quote_prefix(line: &str) -> Option<&str> {
    let rest = line
        .strip_prefix("&gt;")
        .or_else(|| line.strip_prefix('>'))?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// Lines of mrkdwn text into `column`, grouping consecutive `>` / `&gt;` blockquote lines
/// behind a Slack-style left quote bar; with `edited`, the edit marker rides inline at the
/// end of the last line that renders text.
pub(super) fn mrkdwn_lines(
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

pub(super) fn mrkdwn_line(
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

/// `text` with Slack's three mrkdwn HTML escapes undone.
pub(crate) fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Plain mrkdwn text with Slack's HTML escapes undone; inline code gets its wash.
/// Unless `theme.verbatim` is set (Block Kit `verbatim: true`, `F-COLLAB-758`) or inside inline
/// code, bare `https://` / `http://` URLs are auto-linked.
pub(super) fn push_mrkdwn_text(
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
pub(super) fn toggle_marker(
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
pub(super) fn angle_token(
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
pub(super) fn styled_label(mut label: &str, mut run: RunFlags) -> (&str, RunFlags) {
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

/// A `:shortcode:` at the start of `rest`: a glyph run, or a custom emoji image.
pub(super) fn emoji_token(
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
pub(super) fn unicode_glyph(hex: &str) -> Option<String> {
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

// ---------------------------------------------------------------------------
// Local-time formatting: no chrono/time dependency, so the platform `localtime`
// provides the UTC offset and the civil conversion is done by hand.
// ---------------------------------------------------------------------------

/// Local wall-clock fields of a Unix timestamp, used for clock and date-divider labels.
pub(super) struct CivilTime {
    pub(super) year: i32,
    pub(super) month: u32,
    pub(super) day: u32,
    /// Local days since the Unix epoch; date breaks compare this.
    pub(super) day_index: i64,
    pub(super) hour: u32,
    pub(super) minute: u32,
}

impl CivilTime {
    pub(super) fn from_unix(secs: i64) -> Self {
        let shifted = secs + local_utc_offset(secs);
        let day_index = shifted.div_euclid(86_400);
        let second_of_day = shifted.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(day_index);
        CivilTime {
            year,
            month,
            day,
            day_index,
            hour: u32::try_from(second_of_day / 3_600).unwrap_or(0),
            minute: u32::try_from((second_of_day % 3_600) / 60).unwrap_or(0),
        }
    }

    pub(super) fn now() -> Self {
        Self::from_unix(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs() as i64)
                .unwrap_or(0),
        )
    }
}

pub(super) fn day_index(ts: &Ts) -> Option<i64> {
    ts.secs()
        .map(|secs| (secs + local_utc_offset(secs)).div_euclid(86_400))
}

/// If `text` consists exclusively of 1..=5 Slack `:emoji:` shortcodes (whitespace ignored),
/// return large 32px jumbomoji elements.
pub(super) fn jumbomoji_elements(text: &str, fixtures: &Fixtures) -> Option<Vec<AnyElement>> {
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

pub(super) fn format_clock(ts: &Ts) -> String {
    format_clock_with(ts, false)
}

pub(super) fn format_clock_with(ts: &Ts, clock_24h: bool) -> String {
    match ts.secs() {
        Some(secs) => {
            let time = CivilTime::from_unix(secs);
            if clock_24h {
                format!("{:02}:{:02}", time.hour, time.minute)
            } else {
                let hour12 = match time.hour % 12 {
                    0 => 12,
                    hour => hour,
                };
                let meridiem = if time.hour < 12 { "AM" } else { "PM" };
                format!("{}:{:02} {}", hour12, time.minute, meridiem)
            }
        }
        None => ts.as_str().to_owned(),
    }
}

/// Current Unix seconds.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// `3:42 PM` for `secs` on this machine's clock, or in the zone `utc_offset` seconds from UTC.
pub(crate) fn clock(secs: i64, utc_offset: Option<i64>) -> String {
    let shifted = secs + utc_offset.unwrap_or_else(|| local_utc_offset(secs));
    let second_of_day = shifted.rem_euclid(86_400);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let hour12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    let meridiem = if hour < 12 { "AM" } else { "PM" };
    format!("{hour12}:{minute:02} {meridiem}")
}

/// This machine's offset from UTC right now, in seconds.
pub(crate) fn local_offset_now() -> i64 {
    local_utc_offset(now_secs())
}

/// Compact stamp for list rows (DM list, activity, search): the clock time for today's
/// messages, just the weekday within the past week ("Monday"), the divider's day label
/// otherwise.
pub(crate) fn list_stamp(ts: &Ts) -> String {
    let now = CivilTime::now();
    match ts.secs().map(CivilTime::from_unix) {
        Some(time) if time.day_index == now.day_index => format_clock(ts),
        // A week back still reads as the weekday ("Friday" on a Friday), as Slack shows it.
        Some(time) if now.day_index - time.day_index <= 7 => {
            let label = date_label(&time, &now);
            match label.split_once(',') {
                Some((weekday, _)) => weekday.to_owned(),
                None => label,
            }
        }
        Some(time) => date_label(&time, &now),
        None => ts.as_str().to_owned(),
    }
}

/// When a file was created, as the Files view words it after "Created": "today",
/// "yesterday", "on October 1st", or "on January 19th, 2025" outside the current year.
pub(crate) fn file_day(secs: i64) -> String {
    let now = CivilTime::now();
    let time = CivilTime::from_unix(secs);
    let label = date_label(&time, &now);
    if time.day_index == now.day_index || time.day_index + 1 == now.day_index {
        return label.to_lowercase();
    }
    // Within the current year the label leads with the weekday: "Monday, October 1st".
    match label.split_once(", ") {
        Some((_, day)) if time.year == now.year => format!("on {day}"),
        _ => format!("on {label}"),
    }
}

/// The stamp on a message card outside the day-divided transcript (threads, search, pins):
/// like Slack, the time for today's messages, "Yesterday at …", the weekday within the past
/// week ("Tuesday at 9:01 AM"), the full date before that.
pub(super) fn card_stamp(ts: &Ts, clock_24h: bool) -> String {
    let Some(secs) = ts.secs() else {
        return ts.as_str().to_owned();
    };
    let day = CivilTime::from_unix(secs).day_index;
    match CivilTime::now().day_index - day {
        0 => format_clock_with(ts, clock_24h),
        2..=6 => format!(
            "{} at {}",
            weekday_name(day),
            format_clock_with(ts, clock_24h)
        ),
        _ => day_and_time(ts),
    }
}

/// The weekday of a day counted from the Unix epoch, which began on a Thursday.
pub(super) fn weekday_name(day_index: i64) -> &'static str {
    const WEEKDAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    WEEKDAYS[(day_index + 4).rem_euclid(7) as usize]
}

/// The day header of a day-grouped list (the Activity page): the divider's day label
/// (`Today`, `Yesterday`, `Tuesday, November 10th`), or the raw ts when it does not parse.
pub(crate) fn day_label(ts: &Ts) -> String {
    ts.secs()
        .map(|secs| date_label(&CivilTime::from_unix(secs), &CivilTime::now()))
        .unwrap_or_else(|| ts.as_str().to_owned())
}

/// Slack's long stamp (attachment footers, thread replies from other days):
/// `Yesterday at 9:25 AM`, `Today at 3:01 PM`, `Tuesday, November 10th at 8:00 AM`.
pub(crate) fn day_and_time(ts: &Ts) -> String {
    match ts.secs() {
        Some(secs) => format!(
            "{} at {}",
            date_label(&CivilTime::from_unix(secs), &CivilTime::now()),
            format_clock(ts)
        ),
        None => ts.as_str().to_owned(),
    }
}

/// `2026-11-19` (local date) for Unix `secs`; Block Kit date pickers use this format.
pub(crate) fn iso_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    format!("{:04}-{:02}-{:02}", time.year, time.month, time.day)
}

/// Slack's divider label: `Today` / `Yesterday` / `Tuesday, November 10th`, plus `, 2025`
/// outside the current year.
pub(super) fn date_label(time: &CivilTime, now: &CivilTime) -> String {
    const MONTH_NAMES: [&str; 12] = [
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
    if time.day_index == now.day_index {
        return "Today".to_owned();
    }
    if time.day_index + 1 == now.day_index {
        return "Yesterday".to_owned();
    }
    let weekday = weekday_name(time.day_index);
    let month = MONTH_NAMES[(time.month - 1) as usize];
    let day = time.day;
    let suffix = ordinal_suffix(day);
    // Slack names the weekday only within the current year: "October 25th, 2022".
    if time.year == now.year {
        format!("{weekday}, {month} {day}{suffix}")
    } else {
        format!("{month} {day}{suffix}, {}", time.year)
    }
}

/// `st`, `nd`, `rd` or `th` after `day`.
pub(super) fn ordinal_suffix(day: u32) -> &'static str {
    match (day % 10, day % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

/// A shared message's day as Slack's card for it shows it: `Nov 18th`, plus `, 2025` outside
/// the current year.
pub(crate) fn short_date(secs: i64) -> String {
    let time = CivilTime::from_unix(secs);
    let day = format!(
        "{} {}{}",
        MONTHS[(time.month - 1) as usize],
        time.day,
        ordinal_suffix(time.day)
    );
    if time.year == CivilTime::now().year {
        day
    } else {
        format!("{day}, {}", time.year)
    }
}

pub(super) fn date_label_absolute(time: &CivilTime) -> String {
    format!("{} {}", MONTHS[(time.month - 1) as usize], time.day)
}

/// Manual timezone offset override in seconds (`i64::MIN` = follow OS timezone, `F-SHELL-100`).
static TIMEZONE_OVERRIDE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);

/// Override the display clock timezone (`None` restores automatic system timezone).
pub fn set_timezone_override(offset_secs: Option<i64>) {
    TIMEZONE_OVERRIDE.store(
        offset_secs.unwrap_or(i64::MIN),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Offset from UTC in seconds at `unix_secs`, honoring manual override or platform `localtime`.
#[cfg(unix)]
pub(super) fn local_utc_offset(unix_secs: i64) -> i64 {
    let manual = TIMEZONE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if manual != i64::MIN {
        return manual;
    }
    /// `struct tm` as laid out by glibc and Darwin: nine `int` fields then
    /// `tm_gmtoff`/`tm_zone`.
    #[repr(C)]
    struct Tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }
    extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };
    // SAFETY: `tm` is a valid, fully initialized `struct tm` owned by this call, and `unix_secs`
    // outlives it.
    let ok = unsafe { !localtime_r(&unix_secs, &mut tm).is_null() };
    if ok {
        tm.tm_gmtoff
    } else {
        // The rest of `struct tm` exists for the C ABI; reading it keeps every field live.
        let _ = (
            tm.tm_sec, tm.tm_min, tm.tm_hour, tm.tm_mday, tm.tm_mon, tm.tm_year,
        );
        let _ = (tm.tm_wday, tm.tm_yday, tm.tm_isdst, tm.tm_zone);
        0
    }
}

#[cfg(not(unix))]
pub(super) fn local_utc_offset(_unix_secs: i64) -> i64 {
    let manual = TIMEZONE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if manual != i64::MIN {
        return manual;
    }
    0
}

/// (year, month, day) from days since the Unix epoch (proleptic Gregorian).
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_shift = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_shift + 2) / 5 + 1) as u32;
    let month = if month_shift < 10 {
        month_shift + 3
    } else {
        month_shift - 9
    } as u32;
    ((year + i64::from(month <= 2)) as i32, month, day)
}
