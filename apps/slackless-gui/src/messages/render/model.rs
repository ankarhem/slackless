//! The theme-free render model of message text: core's mrkdwn spans (and Block Kit's rich
//! text elements, `super::body`) resolved against the workspace into [`Segment`]s, the
//! paragraphs built from them, and the blocks a mrkdwn body splits into. Elements are built
//! from the model at paint time (`super::inline`), so the model is parsed once per message
//! and cached (`super::body`).

use std::hash::{DefaultHasher, Hasher};
use std::ops::Range;

use gpui_kit::assets::IconName;
use gpui_kit::gpui::{Hsla, ImageSource, SharedString};
use linkify::{LinkFinder, LinkKind};
use slackless_core::domain::mrkdwn::{MrkdwnSpan, Style};

use super::super::{APP_ICON_SLOT, LinkTarget};
use super::inline::{DEFAULT_FLAGS, RunFlags, mention_flags};
use crate::emoji;
use crate::fixtures::Fixtures;

/// What a text run draws as beyond its flags: plain text (inline code in its wash), a website
/// link (underlined when the preference asks), a reference drawn like a link without the
/// underline (channel mentions), a mention chip on its wash, an app chip on the link wash, or
/// muted text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::messages) enum Tone {
    Text,
    Link,
    Reference,
    Mention,
    App,
    Muted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::messages) struct RunStyle {
    pub(in crate::messages) flags: RunFlags,
    pub(in crate::messages) tone: Tone,
}

impl RunStyle {
    pub(in crate::messages) const fn plain() -> Self {
        Self {
            flags: DEFAULT_FLAGS,
            tone: Tone::Text,
        }
    }

    pub(in crate::messages) const fn with_flags(self, flags: RunFlags) -> Self {
        Self { flags, ..self }
    }
}

/// One text piece of a paragraph: its text, its runs as byte lengths, the link ranges inside
/// it, and a hash of its text for its element id.
pub(in crate::messages) struct TextPiece {
    pub(in crate::messages) text: SharedString,
    pub(in crate::messages) runs: Vec<(usize, RunStyle)>,
    pub(in crate::messages) links: Vec<(Range<usize>, LinkTarget)>,
    pub(in crate::messages) hash: u64,
}

/// One inline chunk of a paragraph: text split at its blank-line runs, a custom emoji image,
/// a link chip, or a color swatch.
pub(in crate::messages) enum Segment {
    Text {
        pieces: Vec<TextPiece>,
        /// A line break at the paragraph's start or end was cut (in a row mixed with images
        /// or chips, it would only add an empty line to the text's own box).
        lead: bool,
        trail: bool,
    },
    Image(ImageSource),
    /// Slack's chip for a link labelled with its own address (a link icon) or for a link to a
    /// message (a speech bubble): the icon and the label on a tinted pill.
    Chip {
        url: SharedString,
        label: SharedString,
        icon: IconName,
        icon_size: f32,
    },
    /// Slack's inline hex color swatch (`#RRGGBB` + rounded color square, `F-SHELL-087`).
    ColorSwatch {
        hex: String,
        color: Hsla,
    },
}

/// The preferences that shape the model of a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::messages) struct TextOpts {
    /// Render emoji as plain `:shortcode:` text (Preferences > Messages & media).
    pub(in crate::messages) emoji_text: bool,
    /// Block Kit `mrkdwn` `verbatim: true` (`F-COLLAB-758`): skip auto-linking bare URLs.
    pub(in crate::messages) verbatim: bool,
}

/// Accumulates a paragraph's styled text and link ranges, split only at inline images.
#[derive(Default)]
pub(in crate::messages) struct ParagraphBuilder {
    text: String,
    runs: Vec<(usize, RunStyle)>,
    links: Vec<(Range<usize>, LinkTarget)>,
    segments: Vec<Segment>,
}

impl ParagraphBuilder {
    pub(in crate::messages) fn new() -> Self {
        Self::default()
    }

    pub(in crate::messages) fn push_text(&mut self, text: &str, style: RunStyle) {
        if text.is_empty() {
            return;
        }
        self.runs.push((text.len(), style));
        self.text.push_str(text);
    }

    /// A clickable range inside the paragraph's text: `target` over `text`, drawn in `style`.
    pub(in crate::messages) fn push_link(
        &mut self,
        text: &str,
        target: LinkTarget,
        style: RunStyle,
    ) {
        let start = self.text.len();
        self.push_text(text, style);
        self.links.push((start..self.text.len(), target));
    }

    /// Slack's edit marker: a muted run at the paragraph's end that flows and wraps with the
    /// text. (A per-run font size is not expressible: one `StyledText` carries a single text
    /// size from its element, so the marker is muted at body size.)
    pub(super) fn push_edited(&mut self) {
        self.push_text(
            " (edited)",
            RunStyle {
                tone: Tone::Muted,
                ..RunStyle::plain()
            },
        );
    }

    /// An app's inline link chip (`attachment_mention`, e.g. a Jira issue): its title on the
    /// link wash after a slot the paragraph paints the app icon into, one clickable range.
    pub(super) fn push_attachment(&mut self, url: String, label: &str) {
        let start = self.text.len();
        let style = RunStyle {
            tone: Tone::App,
            ..RunStyle::plain()
        };
        self.push_text(APP_ICON_SLOT, style);
        self.push_text(label, style);
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
            label: label.into(),
            icon,
            icon_size,
        });
    }

    pub(super) fn push_color_swatch(&mut self, hex: String, color: Hsla) {
        self.flush();
        self.segments.push(Segment::ColorSwatch { hex, color });
    }

    /// Drop the line breaks the paragraph ends with: Slack starts what follows (a code
    /// block, a quote, a list or the message's end) on its own line without them.
    pub(super) fn trim_trailing_breaks(&mut self) {
        while self.text.ends_with('\n') {
            self.text.pop();
            if let Some(last) = self.runs.last_mut() {
                last.0 -= 1;
                if last.0 == 0 {
                    self.runs.pop();
                }
            }
        }
    }

    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        let runs = std::mem::take(&mut self.runs);
        let links = std::mem::take(&mut self.links);
        let mut hasher = DefaultHasher::new();
        hasher.write(text.as_bytes());
        self.segments.push(Segment::Text {
            pieces: vec![TextPiece {
                text: text.into(),
                runs,
                links,
                hash: hasher.finish(),
            }],
            lead: false,
            trail: false,
        });
    }

    /// The paragraph's segments, each text split at its blank-line runs (Slack's 8px
    /// `c-mrkdwn__br` block goes between the pieces) and, in a row mixed with images or chips,
    /// its edge line breaks cut.
    pub(super) fn finish(mut self) -> Vec<Segment> {
        self.flush();
        let single = self.segments.len() == 1;
        for segment in &mut self.segments {
            let Segment::Text {
                pieces,
                lead,
                trail,
                ..
            } = segment
            else {
                continue;
            };
            let Some(first) = pieces.first_mut() else {
                continue;
            };
            let text: &str = &first.text;
            let (mut start, mut end) = (0, text.len());
            if !single {
                if text.ends_with('\n') {
                    end -= 1;
                    *trail = true;
                    if let Some(last) = first.runs.last_mut() {
                        last.0 -= 1;
                    }
                }
                if text[start..end].starts_with('\n') {
                    start += 1;
                    *lead = true;
                    if let Some(first_run) = first.runs.first_mut() {
                        first_run.0 -= 1;
                    }
                    for (range, _) in &mut first.links {
                        *range = range.start.saturating_sub(1)..range.end.saturating_sub(1);
                    }
                }
                first.runs.retain(|run| run.0 > 0);
            }
            *pieces = paragraph_break_split(&text[start..end], &first.runs, &first.links);
        }
        self.segments
    }
}

/// `text` as one piece when it has no blank-line run, else one piece per blank-line-free
/// block, with the runs and link ranges sliced to match.
fn paragraph_break_split(
    text: &str,
    runs: &[(usize, RunStyle)],
    links: &[(Range<usize>, LinkTarget)],
) -> Vec<TextPiece> {
    let slice = |range: Range<usize>| {
        let mut out = Vec::new();
        let mut at = 0;
        for &(len, style) in runs {
            let run_end = at + len;
            let (start, end) = (at.max(range.start), run_end.min(range.end));
            if start < end {
                out.push((end - start, style));
            }
            at = run_end;
        }
        let links = links
            .iter()
            .filter_map(|(link, target)| {
                let start = link.start.max(range.start);
                let end = link.end.min(range.end);
                (start < end).then(|| ((start - range.start)..(end - range.start), target.clone()))
            })
            .collect();
        let mut hasher = DefaultHasher::new();
        hasher.write(text[range.clone()].as_bytes());
        TextPiece {
            text: text[range].to_owned().into(),
            runs: out,
            links,
            hash: hasher.finish(),
        }
    };
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
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
            pieces.push(slice(start..ix));
            start = end;
        }
        ix = end;
    }
    pieces.push(slice(start..text.len()));
    pieces
}

/// One line of mrkdwn as paragraph segments: core's spans resolved against the workspace,
/// with `edited` adding Slack's edit marker at the end.
pub(in crate::messages) fn line_segments(
    spans: &[MrkdwnSpan],
    fixtures: &Fixtures,
    opts: TextOpts,
    edited: bool,
) -> Vec<Segment> {
    let mut builder = ParagraphBuilder::new();
    for span in spans {
        push_span(&mut builder, span, fixtures, opts);
    }
    if edited {
        builder.push_edited();
    }
    builder.finish()
}

fn push_span(
    builder: &mut ParagraphBuilder,
    span: &MrkdwnSpan,
    fixtures: &Fixtures,
    opts: TextOpts,
) {
    match span {
        MrkdwnSpan::Text { text, style } => push_words(builder, text, *style, opts),
        MrkdwnSpan::Link { url, label, style } => push_link(
            builder,
            url.clone(),
            label.clone().unwrap_or_else(|| url.clone()),
            flags_of(*style),
            opts,
        ),
        MrkdwnSpan::UserMention { id, label, .. } => {
            // Named as the transcript knows the user, else as the text names them.
            let name = match (fixtures.mention_name(id), label.as_deref()) {
                (known, Some(label)) if known == id.as_str() && !label.is_empty() => label,
                (known, _) => known,
            };
            builder.push_link(
                &format!("@{name}"),
                LinkTarget::Profile(id.clone()),
                mention_style(true),
            );
        }
        MrkdwnSpan::ChannelMention { id, name, .. } => {
            let label = name
                .as_deref()
                .map(|name| format!("#{name}"))
                .or_else(|| {
                    fixtures
                        .conversation(id)
                        .and_then(|conversation| conversation.name.as_deref())
                        .map(|name| format!("#{name}"))
                })
                .unwrap_or_else(|| format!("#{id}"));
            builder.push_link(
                &label,
                LinkTarget::Channel(id.clone()),
                mention_style(false),
            );
        }
        MrkdwnSpan::UsergroupMention { id, handle, .. } => {
            let name = handle
                .as_deref()
                .or_else(|| fixtures.usergroup_handle(id))
                .unwrap_or(id.as_str());
            builder.push_text(
                &format!("@{}", name.trim_start_matches('@')),
                mention_style(true),
            );
        }
        MrkdwnSpan::Broadcast { range, .. } => {
            builder.push_text(&format!("@{}", range.as_str()), mention_style(true));
        }
        MrkdwnSpan::Emoji { shortcode, style } => {
            push_emoji(builder, shortcode, fixtures, opts, flags_of(*style));
        }
        MrkdwnSpan::Date { fallback, .. } => builder.push_text(fallback, RunStyle::plain()),
    }
}

/// The look of an `@mention` / `#channel` chip: semibold link color, on the accent wash
/// (`washed`: user mentions, user groups and specials) or plain (channel references).
pub(super) fn mention_style(washed: bool) -> RunStyle {
    RunStyle {
        flags: mention_flags(),
        tone: if washed {
            Tone::Mention
        } else {
            Tone::Reference
        },
    }
}

fn flags_of(style: Style) -> RunFlags {
    RunFlags {
        bold: style.contains(Style::BOLD),
        italic: style.contains(Style::ITALIC),
        strike: style.contains(Style::STRIKE),
        code: style.contains(Style::CODE),
        underline: style.contains(Style::UNDERLINE),
        semibold: false,
    }
}

/// Plain words in `style`: inline code keeps its wash; otherwise bare `http(s)://` URLs are
/// auto-linked (Slack auto-links those only, so `www.` hosts, `ftp://` and emails stay text)
/// unless `opts.verbatim` (Block Kit `verbatim: true`).
fn push_words(builder: &mut ParagraphBuilder, text: &str, style: Style, opts: TextOpts) {
    let run = RunStyle::plain().with_flags(flags_of(style));
    if run.flags.code || opts.verbatim {
        builder.push_text(text, run);
        return;
    }
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    let mut plain_from = 0;
    for link in finder.links(text) {
        let url = link.as_str();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            continue;
        }
        builder.push_text(&text[plain_from..link.start()], run);
        push_link(builder, url.to_owned(), url.to_owned(), run.flags, opts);
        plain_from = link.end();
    }
    builder.push_text(&text[plain_from..], run);
}

/// A link inside the paragraph's text: a link-colored run, clickable through its range. Like
/// Slack, a URL posted bare shows as itself, scheme and all, while a link labelled with its
/// own address minus the scheme (`github.com/…`) gets the tinted chip look. `run` is the
/// label's styling (Slack bolds `<url|*Error*>` and links inside `*…*`). Expands `:shortcode:`
/// unicode emoji inside the label (`<url|:bar_chart: Panel>`) so bot links render crisp glyphs.
pub(super) fn push_link(
    builder: &mut ParagraphBuilder,
    url: String,
    label: String,
    run: RunFlags,
    opts: TextOpts,
) {
    fn address(s: &str) -> &str {
        s.split_once("://")
            .map_or(s, |(_, rest)| rest)
            .trim_end_matches('/')
    }
    if label != url && address(&label) == address(&url) {
        builder.push_chip(url.into(), label, IconName::Link, 12.);
        return;
    }
    let expanded = if opts.emoji_text {
        label
    } else {
        emoji::replace_shortcodes(&label)
    };
    builder.push_link(
        &expanded,
        LinkTarget::Url(url.into()),
        RunStyle {
            tone: Tone::Link,
            ..RunStyle::plain()
        }
        .with_flags(run),
    );
}

/// A `:shortcode:`: a glyph run, a custom emoji image, or its literal text (in emoji text
/// mode, or when nothing resolves it).
fn push_emoji(
    builder: &mut ParagraphBuilder,
    shortcode: &str,
    fixtures: &Fixtures,
    opts: TextOpts,
    run: RunFlags,
) {
    let style = RunStyle::plain().with_flags(run);
    if opts.emoji_text {
        builder.push_text(&format!(":{shortcode}:"), style);
    } else if let Some(glyph) = emoji::display_glyph(shortcode) {
        builder.push_text(&glyph, style);
    } else if let Some(image) = fixtures.custom_emoji(shortcode) {
        builder.push_image(image.source());
    } else {
        builder.push_text(&format!(":{shortcode}:"), style);
    }
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
