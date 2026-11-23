//! Obsidian-style Live Preview projection onto gpui-kit's textarea:
//! - Semantic atoms (`@mentions`, `#channels`, `@here`, `:emoji:`) always render as inline
//!   tokens.
//! - Block prefixes (`# `, `## `, `### `, `- `, `1. `, `> `, `---`) reveal as muted editable
//!   markdown while the caret is on that line, and collapse into crisp visual indicators
//!   (`H1`/`H2`/`H3` badges, bullet discs, numbered markers, quote bars, horizontal rules)
//!   when the caret moves away.
//! - Inline delimiters (`*`, `**`, `_`, `~`, `~~`, `` ` ``, `<u>`/`</u>`, `[` / `](url)`)
//!   reveal in muted color when the caret touches the construct, and collapse to 0px tokens
//!   when the caret moves away so the styled text renders cleanly.

use std::ops::Range;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{InlineToken, InlineTokenContext, InputContent, TextDecoration};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, FontStyle, FontWeight, HighlightStyle, Hsla, IntoElement as _,
    ParentElement as _, SharedString, StrikethroughStyle, Styled as _, UnderlineStyle, div, px,
};
use slackless_core::types::{ChannelId, UserId, UsergroupId};

use crate::editor::{
    Atom, Attrs, Directory, Doc, LineKind, Style, delimiter_ranges, empty_pair_at,
};
use crate::emoji;
use crate::fixtures::Fixtures;

const MENTION: &str = "mention";
const EMOJI: &str = "emoji:";
const HIDE: &str = "hide";
const H1: &str = "h1";
const H2: &str = "h2";
const H3: &str = "h3";
const BULLET: &str = "bullet";
const ORDERED: &str = "ordered";
const QUOTE: &str = "quote";
const SOFT: &str = "soft";
const DIVIDER: &str = "divider";
const CODE_OPEN: &str = "code-open:";
const CODE_CLOSE: &str = "code-close";

impl Directory for Fixtures {
    fn mention(&self, id: &UserId) -> String {
        self.mention_name(id).to_owned()
    }

    fn channel_name(&self, id: &ChannelId) -> String {
        self.conversation(id)
            .and_then(|conversation| conversation.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn usergroup_handle(&self, id: &UsergroupId) -> String {
        self.usergroup_handle(id)
            .map(str::to_owned)
            .unwrap_or_else(|| id.to_string())
    }

    fn emoji_text(&self, shortcode: &str) -> Option<String> {
        emoji::display_glyph(shortcode).or_else(|| {
            let name = shortcode.split("::").next()?;
            self.custom_emoji(name).map(|_| format!(":{shortcode}:"))
        })
    }
}

/// The token id for a semantic atom.
fn atom_token_id(atom: &Atom) -> SharedString {
    match atom {
        Atom::User(_) | Atom::Channel(_) | Atom::Usergroup(_) | Atom::Broadcast(_) => {
            MENTION.into()
        }
        Atom::Emoji(shortcode) => format!("{EMOJI}{shortcode}").into(),
    }
}

/// Fenced code block geometry for painting a full-width rounded backdrop behind the
/// textarea in [`super::view`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodeBlockSpan {
    /// Byte range of the opening fence line (` ``` ` or ` ```lang `).
    pub open_line: Range<usize>,
    /// True when the opening fence is collapsed and has no language label (so its line is
    /// hidden and the backdrop starts 8px above the code body instead of at the top of the
    /// hidden fence row).
    pub open_collapsed_empty: bool,
    /// Byte ranges (`line.start..line.end`) of every code body line inside the block.
    pub body_lines: Vec<Range<usize>>,
    /// Byte range of the closing fence line (` ``` `), when the block is closed.
    pub close_line: Option<Range<usize>>,
    /// True when the closing fence is collapsed (so the backdrop ends 8px below the code
    /// body instead of at the bottom of the hidden fence row).
    pub close_collapsed: bool,
}

/// Collect all fenced code blocks in `doc` with their fence-collapse state for the current
/// `selection` and `focused` state.
pub(super) fn code_blocks(
    doc: &Doc,
    selection: &Range<usize>,
    focused: bool,
) -> Vec<CodeBlockSpan> {
    let text = doc.text();
    let parsed = doc.parse();
    let touches = |range: &Range<usize>| {
        focused && selection.end >= range.start && selection.start <= range.end
    };
    let splits_selection = |range: &Range<usize>| {
        (range.start < selection.start && selection.start < range.end)
            || (range.start < selection.end && selection.end < range.end)
    };

    let mut blocks = Vec::new();
    let mut current: Option<CodeBlockSpan> = None;

    for line in &parsed.lines {
        let line_range = line.start..line.end;
        let line_active = touches(&line_range);

        if line.kind == Some(LineKind::Fence) {
            if let Some(mut open) = current.take() {
                open.close_collapsed = !(line_active || splits_selection(&line.prefix));
                open.close_line = Some(line_range);
                blocks.push(open);
            } else {
                let lang = text[line.prefix.clone()].trim_start_matches('`').trim();
                let collapsed = !(line_active || splits_selection(&line.prefix));
                current = Some(CodeBlockSpan {
                    open_line: line_range,
                    open_collapsed_empty: collapsed && lang.is_empty(),
                    body_lines: Vec::new(),
                    close_line: None,
                    close_collapsed: false,
                });
            }
        } else if let Some(open) = current.as_mut() {
            open.body_lines.push(line_range);
        }
    }

    if let Some(open) = current {
        blocks.push(open);
    }
    blocks
}

/// Visual byte ranges of inline `` `code` `` spans in `doc` for painting crisp rounded bordered
/// pills in the composer backdrop (matching sent `.c-mrkdwn__code` pills). Uses `outer` when
/// the caret reveals the backticks and `inner` when the backticks are collapsed.
pub(super) fn inline_code_spans(
    doc: &Doc,
    selection: &Range<usize>,
    focused: bool,
) -> Vec<Range<usize>> {
    let text = doc.text();
    let parsed = doc.parse();
    let touches = |range: &Range<usize>| {
        if !focused {
            return false;
        }
        if selection.is_empty() {
            selection.start >= range.start && selection.start <= range.end
        } else {
            selection.start < range.end && selection.end > range.start
        }
    };
    let splits_selection = |range: &Range<usize>| {
        (selection.start > range.start && selection.start < range.end)
            || (selection.end > range.start && selection.end < range.end)
    };
    let mut spans = Vec::new();
    for line in &parsed.lines {
        if matches!(line.kind, Some(LineKind::Code | LineKind::Fence)) {
            continue;
        }
        for c in &line.inlines {
            if c.kind != crate::editor::InlineKind::Style(Style::CODE) {
                continue;
            }
            let revealed =
                touches(&c.outer) || splits_selection(&c.open) || splits_selection(&c.close);
            let range = if revealed {
                c.outer.clone()
            } else {
                c.inner.clone()
            };
            if range.start < range.end && range.end <= text.len() {
                spans.push(range);
            }
        }
    }
    spans
}

/// All inline code constructs (`outer`, `inner`) outside fenced code blocks, used for
/// bounded double/triple-click selection inside the composer.
pub(super) fn inline_code_constructs(doc: &Doc) -> Vec<(Range<usize>, Range<usize>)> {
    let parsed = doc.parse();
    let mut out = Vec::new();
    for line in &parsed.lines {
        if matches!(line.kind, Some(LineKind::Code | LineKind::Fence)) {
            continue;
        }
        for c in &line.inlines {
            if matches!(c.kind, crate::editor::InlineKind::Style(Style::CODE))
                && !c.inner.is_empty()
            {
                out.push((c.outer.clone(), c.inner.clone()));
            }
        }
    }
    out
}

/// If `offset` lies on a body line (`LineKind::Code`) of a fenced code block, return the
/// full byte range of that block's contiguous body lines (excluding the ```` ``` ```` fences).
pub(super) fn fenced_code_block_body_at(doc: &Doc, offset: usize) -> Option<Range<usize>> {
    let parsed = doc.parse();
    let ix = parsed
        .lines
        .partition_point(|line| line.end < offset)
        .min(parsed.lines.len().saturating_sub(1));
    if parsed.lines.get(ix)?.kind != Some(LineKind::Code) {
        return None;
    }
    let mut first = ix;
    while first > 0 && parsed.lines[first - 1].kind == Some(LineKind::Code) {
        first -= 1;
    }
    let mut last = ix;
    while last + 1 < parsed.lines.len() && parsed.lines[last + 1].kind == Some(LineKind::Code) {
        last += 1;
    }
    let start = parsed.lines[first].start;
    let end = parsed.lines[last].end;
    (start < end).then_some(start..end)
}

/// If `selection` lies within an inline code construct (including its backticks), return
/// that construct's `inner` byte range so `Cmd+A` can scope to the inline code pill first.
pub(super) fn inline_code_inner_at(doc: &Doc, selection: &Range<usize>) -> Option<Range<usize>> {
    inline_code_constructs(doc)
        .into_iter()
        .find(|(outer, _)| selection.start >= outer.start && selection.end <= outer.end)
        .map(|(_, inner)| inner)
}

/// Clamp a double-click selection strictly inside an inline code span's `inner` bounds when
/// it overlaps `inner` and spilled across its delimiters.
pub(super) fn clamp_selection_to_inner(
    selection: Range<usize>,
    inner: &Range<usize>,
) -> Option<Range<usize>> {
    if selection.start < inner.end && inner.start < selection.end {
        let clamped = selection.start.max(inner.start)..selection.end.min(inner.end);
        if clamped != selection && !clamped.is_empty() {
            return Some(clamped);
        }
    }
    None
}

/// The theme colors Live Preview draws with.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Palette {
    link: Hsla,
    code: Hsla,
    code_block_fg: Hsla,
    syntax: Hsla,
    danger: Hsla,
}

impl Palette {
    pub fn of(cx: &App) -> Self {
        Palette {
            link: cx.theme().link,
            code: crate::theme::code_foreground(cx),
            code_block_fg: cx.theme().foreground,
            syntax: cx.theme().muted_foreground.opacity(0.6),
            danger: cx.theme().danger,
        }
    }

    fn misspelling_style(self) -> HighlightStyle {
        HighlightStyle {
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(self.danger),
                wavy: true,
            }),
            ..HighlightStyle::default()
        }
    }

    fn syntax_style(self) -> HighlightStyle {
        HighlightStyle {
            color: Some(self.syntax),
            ..HighlightStyle::default()
        }
    }

    /// Fenced code blocks (` ``` `) keep neutral body foreground (never inline code's orange).
    /// Their full-width rounded container is painted behind the textarea in `view.rs` so blank
    /// lines and ragged line endings never break the box.
    fn code_block_style(self) -> HighlightStyle {
        HighlightStyle {
            color: Some(self.code_block_fg),
            ..HighlightStyle::default()
        }
    }

    fn highlight(self, attrs: &Attrs, heading_weight: Option<FontWeight>) -> HighlightStyle {
        let has = |style| attrs.style.contains(style);
        let is_link = attrs.link.is_some();
        let color = if is_link {
            Some(self.link)
        } else {
            has(Style::CODE).then_some(self.code)
        };
        let font_weight = if has(Style::BOLD) {
            Some(heading_weight.unwrap_or(FontWeight::BOLD))
        } else {
            heading_weight
        };
        HighlightStyle {
            color,
            font_weight,
            font_style: has(Style::ITALIC).then_some(FontStyle::Italic),
            background_color: None,
            underline: if is_link {
                Some(UnderlineStyle {
                    thickness: px(1.),
                    color: Some(self.link),
                    wavy: false,
                })
            } else {
                has(Style::UNDERLINE).then_some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                })
            },
            strikethrough: has(Style::STRIKE).then_some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..HighlightStyle::default()
        }
    }
}

/// Project `doc` into Obsidian Live Preview tokens and text decorations for the given
/// `selection`, `focused` state, and optional spellcheck misspelling spans (`F-SHELL-098`).
pub(super) fn project(
    doc: &Doc,
    selection: &Range<usize>,
    focused: bool,
    palette: Palette,
    misspellings: &[Range<usize>],
) -> (InputContent, Vec<TextDecoration>) {
    let text = doc.text();
    let parsed = doc.parse();
    let mut raw_tokens: Vec<(Range<usize>, SharedString)> = Vec::new();
    let mut decorations: Vec<TextDecoration> = Vec::new();

    // 1. Semantic atoms (@mentions, #channels, @here, :emoji:) always stay atomic tokens.
    for placed in doc.atoms() {
        raw_tokens.push((placed.range.clone(), atom_token_id(&placed.atom)));
    }

    let touches = |range: &Range<usize>| {
        focused && selection.end >= range.start && selection.start <= range.end
    };
    // Never place a token whose open interior contains a selection endpoint, even when
    // blurred: gpui-base atomically rejects any `set_tokens` that splits a selection.
    let splits_selection = |range: &Range<usize>| {
        (range.start < selection.start && selection.start < range.end)
            || (range.start < selection.end && selection.end < range.end)
    };

    // 2. Block prefixes and inline constructs: reveal muted markdown when touched by the
    //    caret while focused, otherwise collapse into Live Preview tokens.
    let mut in_code_fence = false;
    for line in &parsed.lines {
        let line_range = line.start..line.end;
        let line_active = touches(&line_range);

        if line.kind == Some(LineKind::Fence) {
            let opening = !in_code_fence;
            in_code_fence = !in_code_fence;
            if line_active || splits_selection(&line.prefix) {
                decorations.push(TextDecoration::new(
                    line.prefix.clone(),
                    palette.syntax_style(),
                ));
            } else if opening {
                let lang = text[line.prefix.clone()].trim_start_matches('`').trim();
                raw_tokens.push((line.prefix.clone(), format!("{CODE_OPEN}{lang}").into()));
            } else {
                raw_tokens.push((line.prefix.clone(), CODE_CLOSE.into()));
            }
            continue;
        }

        if !line.prefix.is_empty() {
            let block_token = match line.kind {
                Some(LineKind::Heading(1)) => Some(H1),
                Some(LineKind::Heading(2)) => Some(H2),
                Some(LineKind::Heading(_)) => Some(H3),
                Some(LineKind::Bullet) => Some(BULLET),
                Some(LineKind::Ordered) => Some(ORDERED),
                Some(LineKind::Quote) => Some(QUOTE),
                Some(LineKind::Soft) => Some(SOFT),
                Some(LineKind::Divider) => Some(DIVIDER),
                Some(LineKind::Fence | LineKind::Code) | None => None,
            };
            if line_active || block_token.is_none() || splits_selection(&line.prefix) {
                decorations.push(TextDecoration::new(
                    line.prefix.clone(),
                    palette.syntax_style(),
                ));
            } else if let Some(token_id) = block_token {
                raw_tokens.push((line.prefix.clone(), token_id.into()));
            }
        }

        if line.kind == Some(LineKind::Code) {
            if !line.content.is_empty() {
                decorations.push(TextDecoration::new(
                    line.content.clone(),
                    palette.code_block_style(),
                ));
            }
            continue;
        }
        if matches!(line.kind, Some(LineKind::Divider)) {
            continue;
        }

        let heading_weight = match line.kind {
            Some(LineKind::Heading(1 | 2)) => Some(FontWeight::BLACK),
            Some(LineKind::Heading(_)) => Some(FontWeight::BOLD),
            _ => None,
        };

        // Inline constructs: hide delimiters with 0px tokens when inactive (and not splitting
        // selection), mute them when active.
        for c in &line.inlines {
            if touches(&c.outer) || splits_selection(&c.open) || splits_selection(&c.close) {
                decorations.push(TextDecoration::new(c.open.clone(), palette.syntax_style()));
                decorations.push(TextDecoration::new(c.close.clone(), palette.syntax_style()));
            } else {
                raw_tokens.push((c.open.clone(), HIDE.into()));
                raw_tokens.push((c.close.clone(), HIDE.into()));
            }
        }

        // Mute an armed empty delimiter pair directly around the caret (`*|*`, `<u>|</u>`),
        // ignoring delimiters that already belong to parsed constructs.
        let armed_pair = (line_active && selection.is_empty())
            .then(|| empty_pair_at(text, line, selection.start).map(|(r, _)| r))
            .flatten();
        if let Some(pair_range) = &armed_pair {
            decorations.push(TextDecoration::new(
                pair_range.clone(),
                palette.syntax_style(),
            ));
        }

        // Styled content runs (combined with heading weight on heading lines, skipping
        // delimiter bytes so revealed markdown syntax stays regular muted weight).
        let runs = parsed.styled_runs(line);
        if let Some(weight) = heading_weight {
            let mut skip = delimiter_ranges(&line.inlines);
            if let Some(pair_range) = &armed_pair {
                skip.push(pair_range.clone());
                skip.sort_by_key(|r| r.start);
            }
            let emit_plain = |decorations: &mut Vec<TextDecoration>, range: Range<usize>| {
                let mut at = range.start;
                for s in &skip {
                    if s.end <= at {
                        continue;
                    }
                    if s.start >= range.end {
                        break;
                    }
                    if s.start > at {
                        decorations.push(TextDecoration::new(
                            at..s.start,
                            palette.highlight(&Attrs::default(), Some(weight)),
                        ));
                    }
                    at = s.end.min(range.end);
                }
                if at < range.end {
                    decorations.push(TextDecoration::new(
                        at..range.end,
                        palette.highlight(&Attrs::default(), Some(weight)),
                    ));
                }
            };
            let mut cursor = line.content.start;
            for run in &runs {
                if run.range.start > cursor {
                    emit_plain(&mut decorations, cursor..run.range.start);
                }
                decorations.push(TextDecoration::new(
                    run.range.clone(),
                    palette.highlight(&run.attrs, Some(weight)),
                ));
                cursor = run.range.end;
            }
            if cursor < line.content.end {
                emit_plain(&mut decorations, cursor..line.content.end);
            }
        } else {
            for run in runs {
                decorations.push(TextDecoration::new(
                    run.range,
                    palette.highlight(&run.attrs, None),
                ));
            }
        }
    }

    for span in misspellings {
        if span.start < span.end && span.end <= text.len() {
            decorations.push(TextDecoration::new(
                span.clone(),
                palette.misspelling_style(),
            ));
        }
    }

    // Build validated, non-overlapping InputContent.
    let content = build_content(text, raw_tokens);
    decorations.sort_by_key(|decoration| decoration.range.start);
    (content, decorations)
}

/// Fallback token snapshot containing only semantic atoms (`@mentions`, `#channels`,
/// `@here`, `:emoji:`), used if a secondary multi-cursor causes `SplitsSelection` on Live
/// Preview syntax tokens.
pub(super) fn atoms_only(doc: &Doc) -> InputContent {
    let raw_tokens = doc
        .atoms()
        .iter()
        .map(|placed| (placed.range.clone(), atom_token_id(&placed.atom)))
        .collect();
    build_content(doc.text(), raw_tokens)
}

fn build_content(text: &str, mut raw_tokens: Vec<(Range<usize>, SharedString)>) -> InputContent {
    raw_tokens.sort_by_key(|(range, _)| range.start);
    let shared = SharedString::from(text.to_owned());
    let mut content = InputContent::new(shared);
    let mut last_end = 0usize;
    for (range, id) in raw_tokens {
        if range.is_empty() || range.start < last_end || range.end > text.len() {
            continue;
        }
        let slice = &text[range.clone()];
        if slice
            .chars()
            .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            continue;
        }
        let token = InlineToken::new(id, SharedString::from(slice.to_owned()));
        if let Ok(next) = content.clone().with_token(range.clone(), token) {
            content = next;
            last_end = range.end;
        }
    }
    content
}

/// Draw one Live Preview token in place of its markdown syntax or atom text.
pub(super) fn render(context: &InlineTokenContext, fixtures: &Fixtures, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let token = context.token();
    let height = context.line_height();
    let selected = context.is_selected();
    let slot = |width: f32| {
        div()
            .flex_none()
            .h(height)
            .w(px(width))
            .flex()
            .items_center()
            .when(selected, |el| el.bg(theme.selection))
    };
    let heading_badge = |label: &'static str| {
        div()
            .flex_none()
            .h(height)
            .pr_1p5()
            .flex()
            .items_center()
            .when(selected, |el| el.bg(theme.selection))
            .child(
                div()
                    .px_1()
                    .h(px(16.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme.muted_foreground)
                    .child(label),
            )
            .into_any_element()
    };
    match token.id().as_ref() {
        HIDE | CODE_CLOSE => slot(0.).into_any_element(),
        H1 => heading_badge("H1"),
        H2 => heading_badge("H2"),
        H3 => heading_badge("H3"),
        BULLET => slot(20.)
            .justify_center()
            .text_color(theme.muted_foreground)
            .child("\u{2022}")
            .into_any_element(),
        ORDERED => div()
            .flex_none()
            .h(height)
            .min_w(px(20.))
            .pr_1p5()
            .flex()
            .items_center()
            .justify_end()
            .when(selected, |el| el.bg(theme.selection))
            .text_color(theme.muted_foreground)
            .child(SharedString::from(token.text().trim_end().to_owned()))
            .into_any_element(),
        QUOTE => slot(14.)
            .child(div().w(px(4.)).h_full().rounded_full().bg(theme.border))
            .into_any_element(),
        SOFT => slot(20.).into_any_element(),
        DIVIDER => div()
            .flex_1()
            .w(px(240.))
            .h(height)
            .flex()
            .items_center()
            .when(selected, |el| el.bg(theme.selection))
            .child(div().w_full().h(px(1.)).bg(theme.border))
            .into_any_element(),
        id => {
            if let Some(lang) = id.strip_prefix(CODE_OPEN) {
                if lang.is_empty() {
                    return slot(0.).into_any_element();
                }
                return div()
                    .flex_none()
                    .h(height)
                    .flex()
                    .items_center()
                    .when(selected, |el| el.bg(theme.selection))
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground.opacity(0.75))
                    .child(SharedString::from(lang.to_owned()))
                    .into_any_element();
            }
            match id.strip_prefix(EMOJI) {
                Some(shortcode) => div()
                    .flex_none()
                    .h(height)
                    .flex()
                    .items_center()
                    .when(selected, |el| el.bg(theme.selection))
                    .child(emoji::emoji_element(shortcode, fixtures, height * 0.72))
                    .into_any_element(),
                None => {
                    let background = if selected {
                        theme.selection
                    } else {
                        theme.accent.opacity(0.25)
                    };
                    div()
                        .flex_none()
                        .h(height)
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .px(px(2.))
                                .rounded_sm()
                                .bg(background)
                                .text_color(theme.link)
                                .child(token.text().clone()),
                        )
                        .into_any_element()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::hsla;

    fn test_palette() -> Palette {
        Palette {
            link: hsla(0.6, 0.8, 0.5, 1.0),
            code: hsla(0.0, 0.7, 0.5, 1.0),
            code_block_fg: hsla(0.0, 0.0, 0.85, 1.0),
            syntax: hsla(0.0, 0.0, 0.5, 0.6),
            danger: hsla(0.0, 0.8, 0.5, 1.0),
        }
    }

    fn token_ids(content: &InputContent) -> Vec<(String, String)> {
        content
            .tokens()
            .iter()
            .map(|span| {
                (
                    span.token().id().to_string(),
                    span.token().text().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn live_preview_shows_heading_badges_when_inactive_and_reveals_hashes_on_active_line() {
        let doc = Doc::from("# Heading 1\n## Heading 2\n### heading 3\nparagraph");
        let palette = test_palette();
        let end = doc.text().len();

        // Caret on `paragraph`: all 3 headings show H1/H2/H3 badges.
        let (content, _) = project(&doc, &(end..end), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![
                ("h1".into(), "# ".into()),
                ("h2".into(), "## ".into()),
                ("h3".into(), "### ".into()),
            ]
        );

        // Caret moves onto `# Heading 1`: `# ` reveals as editable muted markdown; H2/H3 stay badges.
        let (content, decorations) = project(&doc, &(4..4), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![("h2".into(), "## ".into()), ("h3".into(), "### ".into())]
        );
        assert!(
            decorations
                .iter()
                .any(|d| d.range == (0..2) && d.style.color == Some(palette.syntax)),
            "active heading prefix `# ` receives muted syntax color"
        );

        // Unfocused: all 3 headings collapse back to H1/H2/H3 badges even if caret is on line 1.
        let (content, _) = project(&doc, &(4..4), false, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![
                ("h1".into(), "# ".into()),
                ("h2".into(), "## ".into()),
                ("h3".into(), "### ".into()),
            ]
        );
    }

    #[test]
    fn live_preview_hides_inactive_inline_delimiters_and_reveals_only_the_active_span() {
        let doc = Doc::from("Say *bold* and [docs](https://x.dev) here");
        let palette = test_palette();
        let end = doc.text().len();

        // Caret at end (`here`): both *bold* and [docs](url) collapse their syntax markers.
        let (content, _) = project(&doc, &(end..end), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![
                ("hide".into(), "*".into()),
                ("hide".into(), "*".into()),
                ("hide".into(), "[".into()),
                ("hide".into(), "](https://x.dev)".into()),
            ]
        );

        // Caret inside `*bold*`: `*` markers reveal in muted syntax color; link stays collapsed.
        let (content, decorations) = project(&doc, &(6..6), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![
                ("hide".into(), "[".into()),
                ("hide".into(), "](https://x.dev)".into()),
            ]
        );
        assert!(
            decorations
                .iter()
                .any(|d| d.range == (4..5) && d.style.color == Some(palette.syntax))
        );
        assert!(
            decorations
                .iter()
                .any(|d| d.range == (9..10) && d.style.color == Some(palette.syntax))
        );

        // Caret inside `[docs](https://x.dev)`: link syntax reveals; `*bold*` stays collapsed.
        let link_pos = doc.text().find("docs").unwrap() + 1;
        let (content, _) = project(&doc, &(link_pos..link_pos), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![("hide".into(), "*".into()), ("hide".into(), "*".into())]
        );
    }

    #[test]
    fn live_preview_never_splits_selection_on_blur_and_heading_weight_skips_delimiters() {
        let palette = test_palette();
        // Caret inside multi-byte `# ` prefix when blurred: must NOT tokenize `0..2`.
        let heading = Doc::from("# Hello *world*");
        let (content, _) = project(&heading, &(1..1), false, palette, &[]);
        assert!(
            !content
                .tokens()
                .iter()
                .any(|s| s.range().start < 1 && 1 < s.range().end),
            "blurred token must not split selection"
        );

        // When active on `# Hello *world*`, heading weight must not paint over `*` delimiters.
        let (_, decorations) = project(&heading, &(10..10), true, palette, &[]);
        assert!(
            !decorations
                .iter()
                .any(|d| d.range.start <= 8 && d.range.end > 8 && d.style.font_weight.is_some()),
            "revealed `*` delimiter inside heading keeps regular syntax weight"
        );
    }

    #[test]
    fn live_preview_renders_code_blocks_with_neutral_text_and_collapsed_fences() {
        let doc = Doc::from("```\nwrong\n```");
        let palette = test_palette();

        // Caret inside code body (`wrong` is bytes 4..9): fences collapse, and code body uses
        // neutral `code_block_fg` with no per-glyph background (backdrop is painted full-width).
        let (content, decorations) = project(&doc, &(6..6), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![
                ("code-open:".into(), "```".into()),
                ("code-close".into(), "```".into()),
            ]
        );
        let body = decorations
            .iter()
            .find(|d| d.range == (4..9))
            .expect("code block line should be decorated");
        assert_eq!(body.style.color, Some(palette.code_block_fg));
        assert_ne!(body.style.color, Some(palette.code));
        assert_eq!(body.style.background_color, None);

        let blocks = code_blocks(&doc, &(6..6), true);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].open_collapsed_empty);
        assert!(blocks[0].close_collapsed);
        assert_eq!(blocks[0].body_lines, vec![4..9]);

        // Caret on opening fence line (0..3): opening fence reveals in syntax color; closing stays collapsed.
        let (content, decorations) = project(&doc, &(2..2), true, palette, &[]);
        assert_eq!(
            token_ids(&content),
            vec![("code-close".into(), "```".into())]
        );
        assert!(
            decorations
                .iter()
                .any(|d| d.range == (0..3) && d.style.color == Some(palette.syntax))
        );
        let blocks = code_blocks(&doc, &(2..2), true);
        assert!(!blocks[0].open_collapsed_empty);
        assert!(blocks[0].close_collapsed);

        // Language-tagged fence keeps its top row inside the backdrop even when caret is in body.
        let tagged = Doc::from("```rust\nlet x = 1;\n```");
        let blocks = code_blocks(&tagged, &(10..10), true);
        assert_eq!(blocks.len(), 1);
        assert!(!blocks[0].open_collapsed_empty);
        assert!(blocks[0].close_collapsed);
    }

    #[test]
    fn inline_code_spans_tracks_revealed_and_collapsed_ranges() {
        let doc = Doc::from("see `foo` ok");
        // Caret outside (`0..0`): backticks hidden, backdrop wraps `foo` (5..8).
        assert_eq!(inline_code_spans(&doc, &(0..0), true), vec![5..8]);
        // Caret inside (`6..6`): backticks revealed, backdrop wraps `` `foo` `` (4..9).
        assert_eq!(inline_code_spans(&doc, &(6..6), true), vec![4..9]);
        // Blurred with caret inside: backticks hidden, wraps `foo` (5..8).
        assert_eq!(inline_code_spans(&doc, &(6..6), false), vec![5..8]);
    }

    #[test]
    fn bounded_code_helpers_cover_fenced_blocks_and_inline_spans() {
        let doc =
            Doc::from("before `order.customerType: \"Org\"` after\n```\nline 1\nline 2\n```\ntail");
        // Inline code constructs yield exact (outer, inner) byte ranges.
        let constructs = inline_code_constructs(&doc);
        assert_eq!(constructs.len(), 1);
        let (outer, inner) = &constructs[0];
        assert_eq!(&doc.text()[outer.clone()], "`order.customerType: \"Org\"`");
        assert_eq!(&doc.text()[inner.clone()], "order.customerType: \"Org\"");

        // Clamping a word selection that spilled across a backtick bounds it to `inner`.
        assert_eq!(
            clamp_selection_to_inner((inner.start - 1)..(inner.start + 5), inner),
            Some(inner.start..(inner.start + 5))
        );
        assert_eq!(
            clamp_selection_to_inner(inner.start..(inner.start + 5), inner),
            None
        );

        // Fenced code block body spans all contiguous code lines, excluding fence lines.
        let l1 = doc.text().find("line 1").unwrap();
        let l2 = doc.text().find("line 2").unwrap();
        let expected = l1..(l2 + "line 2".len());
        assert_eq!(
            fenced_code_block_body_at(&doc, l1 + 2),
            Some(expected.clone())
        );
        assert_eq!(fenced_code_block_body_at(&doc, l2 + 2), Some(expected));
        // Prose lines and fence lines return None.
        assert_eq!(fenced_code_block_body_at(&doc, 0), None);
        assert_eq!(
            fenced_code_block_body_at(&doc, doc.text().find("```").unwrap()),
            None
        );
        assert_eq!(
            fenced_code_block_body_at(&doc, doc.text().find("tail").unwrap()),
            None
        );
    }

    #[test]
    fn misspellings_receive_wavy_danger_underlines() {
        let doc = Doc::from("hello wrld");
        let palette = test_palette();
        let span = 6..10;
        let (_, decorations) = project(&doc, &(0..0), true, palette, std::slice::from_ref(&span));
        assert!(decorations.iter().any(|d| {
            d.range == (6..10)
                && d.style
                    .underline
                    .is_some_and(|u| u.wavy && u.color == Some(palette.danger))
        }));
    }
}
