//! How the document shows inside the input engine: atoms as inline tokens (mention chips,
//! emoji, list bullets, quote bars) and formatting as text decorations.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{InlineToken, InlineTokenContext, InputContent, TextDecoration};
use gpui_kit::gpui::{
    AnyElement, App, FontStyle, FontWeight, HighlightStyle, Hsla, IntoElement as _,
    ParentElement as _, SharedString, StrikethroughStyle, Styled as _, UnderlineStyle, div, px,
};
use slackless_core::types::{ChannelId, UserId};

use crate::editor::{Atom, Attrs, Directory, Doc, LineKind, Style};
use crate::emoji;
use crate::fixtures::Fixtures;

const MENTION: &str = "mention";
const EMOJI: &str = "emoji:";
const BULLET: &str = "bullet";
const ORDERED: &str = "ordered";
const QUOTE: &str = "quote";
const CODE: &str = "code";
const SOFT: &str = "soft";

impl Directory for Fixtures {
    fn mention(&self, id: &UserId) -> String {
        self.mention_name(id).to_owned()
    }

    fn channel_name(&self, id: &ChannelId) -> String {
        self.conversation(id)
            .and_then(|conversation| conversation.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn emoji_text(&self, shortcode: &str) -> Option<String> {
        emoji::display_glyph(shortcode).or_else(|| {
            let name = shortcode.split("::").next()?;
            self.custom_emoji(name).map(|_| format!(":{shortcode}:"))
        })
    }
}

/// The token id for `atom`: the kind of thing to draw, plus the shortcode of an emoji.
fn token_id(atom: &Atom) -> SharedString {
    match atom {
        Atom::User(_) | Atom::Channel(_) | Atom::Broadcast(_) => MENTION.into(),
        Atom::Emoji(shortcode) => format!("{EMOJI}{shortcode}").into(),
        Atom::Marker(LineKind::Bullet) => BULLET.into(),
        Atom::Marker(LineKind::Ordered) => ORDERED.into(),
        Atom::Marker(LineKind::Quote) => QUOTE.into(),
        Atom::Marker(LineKind::Code) => CODE.into(),
        Atom::Marker(LineKind::Soft) => SOFT.into(),
    }
}

/// The document as the engine takes it: its text with one token per atom. A token the engine
/// refuses (its text is not a whole grapheme run) leaves the draft as plain text.
pub(super) fn content(doc: &Doc) -> InputContent {
    let text = SharedString::from(doc.text().to_owned());
    let tokens = doc
        .atoms()
        .iter()
        .try_fold(InputContent::new(text.clone()), |content, placed| {
            let shown = SharedString::from(doc.text()[placed.range.clone()].to_owned());
            content.with_token(
                placed.range.clone(),
                InlineToken::new(token_id(&placed.atom), shown),
            )
        });
    tokens.unwrap_or_else(|error| {
        eprintln!("slackless: composer token rejected: {error}");
        InputContent::new(text)
    })
}

/// The theme colors formatting is drawn with.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Palette {
    link: Hsla,
    code: Hsla,
    code_background: Hsla,
}

impl Palette {
    pub fn of(cx: &App) -> Self {
        Palette {
            link: cx.theme().link,
            code: crate::theme::code_foreground(cx),
            code_background: cx.theme().secondary,
        }
    }

    fn highlight(self, attrs: &Attrs) -> HighlightStyle {
        let has = |style| attrs.style.contains(style);
        let is_link = attrs.link.is_some();
        let color = if is_link {
            Some(self.link)
        } else {
            has(Style::CODE).then_some(self.code)
        };
        HighlightStyle {
            color,
            font_weight: has(Style::BOLD).then_some(FontWeight::BOLD),
            font_style: has(Style::ITALIC).then_some(FontStyle::Italic),
            background_color: has(Style::CODE).then_some(self.code_background),
            underline: is_link.then_some(UnderlineStyle {
                thickness: px(1.),
                color: Some(self.link),
                wavy: false,
            }),
            strikethrough: has(Style::STRIKE).then_some(StrikethroughStyle {
                thickness: px(1.),
                color: None,
            }),
            ..HighlightStyle::default()
        }
    }
}

/// The document's formatting as engine decorations: one per span, and one washing the text
/// of each code-block line.
pub(super) fn decorations(doc: &Doc, palette: Palette) -> Vec<TextDecoration> {
    let spans = doc
        .spans()
        .iter()
        .map(|span| TextDecoration::new(span.range.clone(), palette.highlight(&span.attrs)));
    let code = Attrs {
        style: Style::CODE,
        link: None,
    };
    let code_lines = doc
        .atoms()
        .iter()
        .filter(|placed| placed.atom == Atom::Marker(LineKind::Code))
        .map(|placed| placed.range.end..doc.line_end(placed.range.end))
        .filter(|line| !line.is_empty())
        .map(|line| TextDecoration::new(line, palette.highlight(&code)));
    let mut decorations: Vec<_> = spans.chain(code_lines).collect();
    decorations.sort_by_key(|decoration| decoration.range.start);
    decorations
}

/// Draw one token in place of its text.
pub(super) fn render(context: &InlineTokenContext, fixtures: &Fixtures, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let token = context.token();
    let height = context.line_height();
    let slot = |width: f32| {
        div()
            .flex_none()
            .h(height)
            .w(px(width))
            .flex()
            .items_center()
    };
    match token.id().as_ref() {
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
            .text_color(theme.muted_foreground)
            .child(SharedString::from(token.text().trim_end().to_owned()))
            .into_any_element(),
        QUOTE => slot(14.)
            .child(div().w(px(4.)).h_full().rounded_full().bg(theme.border))
            .into_any_element(),
        CODE => slot(10.)
            .child(div().w(px(2.)).h_full().bg(theme.border))
            .into_any_element(),
        SOFT => slot(20.).into_any_element(),
        id => match id.strip_prefix(EMOJI) {
            Some(shortcode) => div()
                .flex_none()
                .h(height)
                .flex()
                .items_center()
                .child(emoji::emoji_element(shortcode, fixtures, height * 0.72))
                .into_any_element(),
            None => {
                let background = if context.is_selected() {
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
        },
    }
}
