//! Syntax-highlighted code blocks (with bounded LRU cache) and hover Copy button.

use std::cell::RefCell;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    StyledText, TextRun, Window, deferred, div, px,
};
use slackless_core::types::rich_text::{RichTextElement, RichTextPreformatted};

use super::render::{DEFAULT_FLAGS, RunFlags, RunTheme, paragraph_id};
use super::selectable::SelectableParagraph;

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

fn highlight_hash(code: &str, lang: crate::syntax::Lang) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ (lang as u64);
    for &b in code.as_bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn cached_highlight(
    code: &str,
    lang: crate::syntax::Lang,
) -> Vec<(usize, crate::syntax::TokenKind)> {
    struct Entry {
        hash: u64,
        lang: crate::syntax::Lang,
        code: String,
        tokens: Vec<(usize, crate::syntax::TokenKind)>,
    }
    thread_local! {
        static CACHE: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
    }
    const MAX_ENTRIES: usize = 64;
    let hash = highlight_hash(code, lang);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(pos) = cache
            .iter()
            .position(|e| e.hash == hash && e.lang == lang && e.code == code)
        {
            let entry = cache.remove(pos);
            let tokens = entry.tokens.clone();
            cache.push(entry);
            return tokens;
        }
        let tokens = crate::syntax::highlight(lang, code);
        if code.len() <= 32 * 1024 {
            if cache.len() >= MAX_ENTRIES {
                cache.remove(0);
            }
            cache.push(Entry {
                hash,
                lang,
                code: code.to_owned(),
                tokens: tokens.clone(),
            });
        }
        tokens
    })
}

fn highlighted_code_runs(code: &str, lang: crate::syntax::Lang, theme: &RunTheme) -> Vec<TextRun> {
    if code.is_empty() {
        return Vec::new();
    }
    cached_highlight(code, lang)
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

/// Plain selectable prose paragraph (used by clip/video transcripts in chat cards and the
/// image viewer) participating in the window's `TextSelectionLayer` with drag, double/triple-click,
/// and `Cmd+C`.
pub(crate) fn selectable_text(
    key: &str,
    paragraph_ix: usize,
    text: String,
    color: Option<gpui_kit::gpui::Hsla>,
    document_order: u64,
    cx: &App,
) -> AnyElement {
    let theme = RunTheme::body(cx, SharedString::from(key.to_owned()), document_order, None);
    let runs = if text.is_empty() {
        Vec::new()
    } else {
        vec![theme.run(DEFAULT_FLAGS, color, None, text.len())]
    };
    SelectableParagraph::new(
        paragraph_id(&theme.key, paragraph_ix, 0, &text),
        text,
        runs,
        Vec::new(),
        document_order,
        &theme,
    )
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui_kit::gpui::{Font, Hsla, SharedString};

    use super::{cached_highlight, highlighted_code_runs};
    use crate::messages::render::RunTheme;
    use crate::syntax::Lang;

    fn dummy_theme() -> RunTheme {
        let fg = Hsla::default();
        RunTheme {
            base_font: Font::default(),
            mono_font: Font::default(),
            code_font: Font::default(),
            foreground: fg,
            muted: fg,
            border: fg,
            link: fg,
            mention_bg: fg,
            link_wash: fg,
            code_bg: fg,
            code_fg: fg,
            selection: fg,
            order: None,
            links: None,
            underline_links: false,
            emoji_text_mode: false,
            dark: true,
            verbatim: false,
            paragraphs: Cell::new(0),
            key: SharedString::from("test"),
        }
    }

    #[test]
    fn highlighted_code_runs_byte_sum_equals_code_len_across_languages_and_utf8() {
        let theme = dummy_theme();
        assert!(highlighted_code_runs("", Lang::Rust, &theme).is_empty());

        let samples = [
            (
                Lang::Rust,
                "fn main() {\n    let s = \"héllo 🚀 世界\";\n    // comment 日本語\n    println!(\"{s}\");\n}",
            ),
            (
                Lang::Json,
                "{\n  \"key\": \"value — café\",\n  \"nums\": [1, 2.5, true, null]\n}",
            ),
            (
                Lang::Python,
                "def greet(name: str) -> str:\n    # Ångström\n    return f\"Hi {name} 👋\"\n",
            ),
            (
                Lang::Sql,
                "SELECT id, name FROM users WHERE bio LIKE '%über%' ORDER BY id DESC;",
            ),
            (
                Lang::Plain,
                "unknown language\nwith multi-byte UTF-8: åäö € 🦀\nline 3",
            ),
        ];

        for (lang, code) in samples {
            let runs = highlighted_code_runs(code, lang, &theme);
            let total: usize = runs.iter().map(|r| r.len).sum();
            assert_eq!(
                total,
                code.len(),
                "run byte lengths must sum exactly to code.len() for {lang:?}"
            );
            // Every run boundary must lie on a UTF-8 char boundary.
            let mut offset = 0;
            for run in &runs {
                offset += run.len;
                assert!(
                    code.is_char_boundary(offset),
                    "run ended mid-UTF-8 char at {offset} for {lang:?}"
                );
            }
        }
    }

    #[test]
    fn cached_highlight_hits_evicts_and_skips_oversized_inputs() {
        let snippet = "let answer = 42; // cached";
        let first = cached_highlight(snippet, Lang::Rust);
        let second = cached_highlight(snippet, Lang::Rust);
        assert_eq!(first, second);

        // Exceed MAX_ENTRIES (64) to exercise LRU eviction cleanly.
        for i in 0..70 {
            let code = format!("let x_{i} = {i};");
            let tokens = cached_highlight(&code, Lang::Rust);
            assert_eq!(tokens.iter().map(|(l, _)| *l).sum::<usize>(), code.len());
        }

        // Oversized input (> 32 KiB) still highlights accurately without entering cache.
        let huge = "let x = 1;\n".repeat(4_000);
        assert!(huge.len() > 32 * 1024);
        let tokens = cached_highlight(&huge, Lang::Rust);
        assert_eq!(tokens.iter().map(|(l, _)| *l).sum::<usize>(), huge.len());
    }
}
