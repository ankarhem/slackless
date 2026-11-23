//! Block Kit rendering: the app-authored blocks a client must display, and legacy
//! attachments.
//!
//! `slackless_types` models only `rich_text` (rendered by `messages.rs`); every other block
//! arrives as `Block::Unknown { r#type, raw }` with its JSON preserved, which this module
//! renders: `section` (text, `fields`, `accessory`), `divider`, `image`, `context`, `header`,
//! `actions`, and `input`. Interactive elements are real gpui-kit widgets: `Button`s (primary /
//! danger / link styles), dropdown menus for `static_select` / `users_select` /
//! `conversations_select` / `overflow` / `datepicker` / `timepicker`, `Checkbox` and `Radio`
//! groups, and `Textarea`s for text inputs. An interaction updates the local
//! [`BlockKitState`] (selection, checks, drafts); a button press in live mode goes to the app
//! that posted the message (`blocks.actions`, see [`BlockDispatch`]), and fixture mode logs it
//! to stderr with a toast.
//!
//! Legacy attachments render Slack's way: a color bar (hex or `good`/`warning`/`danger`)
//! beside pretext, author, linked title, mrkdwn text, short/long fields, nested blocks, an
//! image and the footer line.
//!
//! Design tokens: colors via `cx.theme()`; text via the shared mrkdwn renderer in
//! `messages.rs`; images through [`Fixtures::block_images`] (live mode: cookie-authenticated
//! downloads), a framed placeholder otherwise.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Div, Entity, FontWeight, Hsla, ImageSource,
    InteractiveElement as _, IntoElement, ObjectFit, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, img, px,
};
use slackless_core::api::ActionContainer;
use slackless_core::types::{
    Attachment, Block, BotId, ChannelId, ConversationKind, Message, TeamId, Ts, UserId,
};

use crate::actions;
use crate::avatars;
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::image_viewer::ImageOrigin;
use crate::messages::{self, LinkHandler, LinkTarget, THEME_ORDER_STRIDE};

/// `serde_json` through gpui's re-export (the single 1.x version the core crates use): the
/// GUI crate has no direct dependency, and `Block::Unknown` hands out its `Value`.
pub(crate) use gpui_kit::gpui::private::serde_json;
pub(crate) type Json = serde_json::Value;

/// Local state of interactive Block Kit controls, keyed by [`control_key`].
#[derive(Default)]
pub struct BlockKitState {
    /// Chosen option value of selects, radio groups and date/time pickers.
    choices: HashMap<String, String>,
    /// Checked option values of checkbox groups (absent: the block's `initial_options`).
    checked: HashMap<String, HashSet<String>>,
    /// Drafts of text inputs; created on first render because a textarea needs the window.
    inputs: HashMap<String, Entity<TextareaState>>,
    /// Attachments whose long text the user expanded ("Show more"), by message and index.
    expanded: HashSet<String>,
    /// Image blocks the user collapsed with their title's caret, by message and block.
    collapsed: HashSet<String>,
    /// Link unfurl cards the user dismissed locally, by message ts and attachment index.
    dismissed_unfurls: HashSet<String>,
    /// Datepicker keys whose monthly calendar grid is currently expanded (`F-COLLAB-745`).
    open_calendars: HashSet<String>,
    /// Currently browsed `(year, month)` per datepicker key (1-based month).
    calendar_months: HashMap<String, (i32, u32)>,
    /// Live option suggestions fetched via `blocks.suggestions` for `external_select` (`F-COLLAB-786`).
    suggestions: HashMap<String, Vec<(String, String)>>,
}

impl BlockKitState {
    /// Store dynamically loaded `blocks.suggestions` options for `key`.
    pub fn set_suggestions(&mut self, key: String, options: Vec<(String, String)>) {
        self.suggestions.insert(key, options);
    }
}

/// Callback invoked when an `external_select` needs live options from `blocks.suggestions`.
pub type SuggestHandler = std::rc::Rc<dyn Fn(String, String, String, &mut App)>;

/// What one message's blocks render with.
pub struct KitCx<'a> {
    pub fixtures: &'a Fixtures,
    /// Interactive state; `None` renders controls from the payload's initial values only.
    pub kit: Option<&'a Entity<BlockKitState>>,
    pub ts: &'a Ts,
    /// Routing for this message's text links; with a handler the text is also selectable
    /// (see [`crate::messages`]).
    pub links: Option<LinkHandler>,
    /// The click handler opening the image viewer (the transcript provides it; static
    /// cards render their images without it).
    pub image_open: Option<crate::image_viewer::ImageClick>,
    /// Where this message's button presses go (the transcript's and thread panel's message
    /// actions); without it a press is only logged.
    pub on_action: Option<actions::ActionHandler>,
    /// Optional callback for fetching `external_select` options (`blocks.suggestions`).
    pub on_suggest: Option<SuggestHandler>,
    /// The legacy attachment these blocks sit in, by its `id` (a button's container names it).
    attachment: Option<i64>,
    /// The next selection-order slice handed to a text theme (see [`messages::mrkdwn`]).
    order: std::cell::Cell<u64>,
    /// Keys handed to inline text renderers, so their element ids stay unique across the
    /// blocks and attachments of one message (GPUI keys a11y nodes by id alone).
    keys: std::cell::Cell<usize>,
}

impl<'a> KitCx<'a> {
    pub fn new(fixtures: &'a Fixtures, kit: Option<&'a Entity<BlockKitState>>, ts: &'a Ts) -> Self {
        Self {
            fixtures,
            kit,
            ts,
            links: None,
            image_open: None,
            on_action: None,
            on_suggest: None,
            attachment: None,
            order: std::cell::Cell::new(0),
            keys: std::cell::Cell::new(0),
        }
    }

    /// Routes this message's button presses through `on_action`, as
    /// [`MessageAction::BlockAction`](actions::MessageAction::BlockAction).
    pub fn dispatching_actions(mut self, on_action: actions::ActionHandler) -> Self {
        self.on_action = Some(on_action);
        self
    }

    /// Routes this message's text links through `links` (also makes its text selectable).
    pub fn with_links(mut self, links: LinkHandler) -> Self {
        self.links = Some(links);
        self
    }

    /// Routes this message's image clicks to the image viewer (the transcript provides
    /// the handler; see [`crate::image_viewer`]).
    pub fn opening_images(mut self, on_open: crate::image_viewer::ImageClick) -> Self {
        self.image_open = Some(on_open);
        self
    }

    /// A [`KitCx`] for a nested render (an attachment's own blocks): fresh keys, the parent's
    /// link routing and the selection order continuing after the parent's own text.
    pub fn nested<'b>(
        &self,
        fixtures: &'b Fixtures,
        kit: Option<&'b Entity<BlockKitState>>,
        ts: &'b Ts,
    ) -> KitCx<'b> {
        KitCx {
            fixtures,
            kit,
            ts,
            links: self.links.clone(),
            image_open: self.image_open.clone(),
            on_action: self.on_action.clone(),
            on_suggest: self.on_suggest.clone(),
            attachment: self.attachment,
            order: std::cell::Cell::new(self.order.get()),
            keys: std::cell::Cell::new(0),
        }
    }

    /// A fresh text-element key plus the selection-order base for one text theme of this
    /// message: each theme owns [`THEME_ORDER_STRIDE`] paragraph slots of the message's
    /// document order, so paragraphs of different blocks never interleave.
    pub fn text_theme(&self) -> (String, u64) {
        let key = self.text_key();
        let base = messages::message_order(self.ts) + self.order.get();
        self.order.set(self.order.get() + THEME_ORDER_STRIDE);
        (key, base)
    }

    /// A fresh text-element key for this message.
    pub fn text_key(&self) -> String {
        let n = self.keys.get();
        self.keys.set(n + 1);
        format!("{}-t{n}", self.ts.as_str())
    }
}

/// Stable key of a control: message ts, block id (or index), action id (or index).
fn control_key(
    ts: &Ts,
    block: &Json,
    block_ix: usize,
    element: &Json,
    element_ix: usize,
) -> String {
    let block_id = block["block_id"]
        .as_str()
        .map_or_else(|| block_ix.to_string(), str::to_owned);
    let action_id = element["action_id"]
        .as_str()
        .map_or_else(|| element_ix.to_string(), str::to_owned);
    format!("{}/{block_id}/{action_id}", ts.as_str())
}

const TEXT_INPUTS: [&str; 4] = [
    "plain_text_input",
    "email_text_input",
    "url_text_input",
    "number_input",
];

/// Give each text input of `message` its textarea state (needs the window, so the transcript
/// calls this from its row renderer before painting the row).
pub fn prepare_inputs(
    kit: &Entity<BlockKitState>,
    message: &Message,
    window: &mut Window,
    cx: &mut App,
) {
    for (ix, block) in message.blocks.iter().enumerate() {
        let Block::Unknown { r#type, raw } = block else {
            continue;
        };
        let element = &raw["element"];
        if r#type != "input" || !TEXT_INPUTS.contains(&element["type"].as_str().unwrap_or_default())
        {
            continue;
        }
        let key = control_key(&message.ts, raw, ix, element, 0);
        if kit.read(cx).inputs.contains_key(&key) {
            continue;
        }
        let placeholder = text_of(&element["placeholder"])
            .unwrap_or_default()
            .to_owned();
        let initial = element["initial_value"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let rows = if element["multiline"].as_bool().unwrap_or(false) {
            3
        } else {
            1
        };
        kit.update(cx, |state, cx| {
            let input = cx.new(|cx| {
                let mut input = TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    .auto_grow(rows, 8);
                input.set_value(initial, window, cx);
                input
            });
            state.inputs.insert(key, input);
        });
    }
}

/// `label` as a clickable link routing through the message's link handler (a permalink
/// opens in-app; without a handler the URL opens in the browser).
fn routed_link(
    id: String,
    label: String,
    url: String,
    kcx: &KitCx,
    weight: FontWeight,
    cx: &App,
) -> gpui_kit::gpui::Stateful<Div> {
    let target = LinkTarget::Url(url.into());
    let links = kcx.links.clone();
    div()
        .id(SharedString::from(id))
        .font_weight(weight)
        .text_color(cx.theme().link)
        .cursor_pointer()
        .hover(|el| el.underline())
        .on_click(move |_, window, cx| {
            messages::route_link(links.as_ref(), target.clone(), window, cx)
        })
        .child(label)
}

/// The `text` of a text object.
fn text_of(object: &Json) -> Option<&str> {
    object["text"].as_str()
}

/// Decode Slack HTML entities (`&amp;`, `&lt;`, `&gt;`) and substitute `:shortcode:` emoji
/// unless Preferences -> Messages & media -> Emoji text mode is enabled.
fn plain_label(text: &str, cx: &App) -> String {
    let decoded = messages::unescape(text);
    if crate::preferences::display_prefs(cx).emoji_text_mode {
        decoded
    } else {
        emoji::replace_shortcodes(&decoded)
    }
}

/// A text object rendered by its type: mrkdwn through the shared renderer (honoring `verbatim`,
/// `F-COLLAB-758`), plain_text with HTML entities decoded and emoji shortcodes substituted.
fn text_object(object: &Json, kcx: &KitCx, muted: bool, cx: &App) -> Option<AnyElement> {
    let text = text_of(object)?;
    if object["type"].as_str() == Some("mrkdwn") {
        let verbatim = object["verbatim"].as_bool().unwrap_or(false);
        return Some(
            messages::mrkdwn_with_verbatim(text, kcx, muted, false, verbatim, cx)
                .into_any_element(),
        );
    }
    Some(div().child(plain_label(text, cx)).into_any_element())
}

/// One top-level (or attachment) block that is not `rich_text`.
pub fn block(kind: &str, raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let element =
        match kind {
            "section" => section(raw, ix, kcx, cx).into_any_element(),
            "divider" => div()
                .w_full()
                .h(px(1.))
                .my_1()
                .bg(theme.border)
                .into_any_element(),
            "header" => div()
                .pt_1()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .child(plain_label(text_of(&raw["text"])?, cx))
                .into_any_element(),
            "image" => image_block(raw, kcx, cx).into_any_element(),
            "table" => table_block(raw, ix, kcx, cx).into_any_element(),
            "data_table" => data_table_block(raw, ix, kcx, cx).into_any_element(),
            // Slack's context row: 20px images, tags, citations, and 13px muted text, 4px apart.
            "context" => h_flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .children(raw["elements"].as_array()?.iter().enumerate().filter_map(
                    |(n, element)| match element["type"].as_str()? {
                        "image" => Some(image_element(element, kcx, 20., cx)),
                        "tag" => Some(tag_element(element, cx).into_any_element()),
                        "citation" => Some(citation_element(element, ix * 19 + n, kcx, cx)),
                        _ => text_object(element, kcx, true, cx),
                    },
                ))
                .into_any_element(),
            "actions" => h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .children(
                    raw["elements"]
                        .as_array()?
                        .iter()
                        .enumerate()
                        .map(|(n, element)| {
                            let key = control_key(kcx.ts, raw, ix, element, n);
                            block_element(element, raw, key, kcx, cx)
                        }),
                )
                .into_any_element(),
            "input" => input_block(raw, ix, kcx, cx).into_any_element(),
            "rich_text" => {
                if let Ok(parsed) = serde_json::from_value::<Block>(raw.clone()) {
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .children(messages::render_block(&parsed, 0, kcx, false, cx))
                        .into_any_element()
                } else {
                    return None;
                }
            }
            "markdown" => {
                let text = raw["text"].as_str()?;
                let converted = standard_markdown_to_mrkdwn(text);
                messages::mrkdwn(&converted, kcx, false, false, cx).into_any_element()
            }
            "video" => video_block(raw, ix, kcx, cx).into_any_element(),
            "call" => call_block(raw, ix, kcx, cx).into_any_element(),
            "alert" | "callout" => alert_block(raw, ix, kcx, cx).into_any_element(),
            "card" | "container" => card_block(raw, ix, kcx, cx).into_any_element(),
            "carousel" => carousel_block(raw, ix, kcx, cx).into_any_element(),
            "context_actions" => context_actions_block(raw, ix, kcx, cx).into_any_element(),
            "task_card" => task_card_block(raw, ix, kcx, cx).into_any_element(),
            "plan" => plan_block(raw, ix, kcx, cx).into_any_element(),
            "file" => file_block(raw, cx).into_any_element(),
            other => div()
                .text_xs()
                .italic()
                .text_color(theme.muted_foreground)
                .child(format!(
                    "This message contains a {other} block this client cannot show yet."
                ))
                .into_any_element(),
        };
    Some(element)
}

/// Convert common standard Markdown constructs used by Slack's `markdown` block (`**bold**`,
/// `[label](url)`, `# Heading`, `~~strike~~`) into Slack `mrkdwn` while leaving fenced and
/// inline code spans untouched.
fn standard_markdown_to_mrkdwn(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_fence = false;
    for (line_ix, line) in input.split('\n').enumerate() {
        if line_ix > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let trimmed = line.trim_start();
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
            let heading = trimmed[hashes + 1..].trim();
            out.push('*');
            out.push_str(&convert_inline_markdown(heading));
            out.push('*');
        } else {
            out.push_str(&convert_inline_markdown(line));
        }
    }
    out
}

fn convert_inline_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_code = false;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            in_code = !in_code;
            out.push('`');
            i += 1;
            continue;
        }
        if in_code {
            let ch = line[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        // `[label](url)` -> `<url|label>`
        if bytes[i] == b'[' {
            if let Some(close_bracket) = line[i + 1..].find("](") {
                let label_end = i + 1 + close_bracket;
                let url_start = label_end + 2;
                if let Some(close_paren) = line[url_start..].find(')') {
                    let label = &line[i + 1..label_end];
                    let url = &line[url_start..url_start + close_paren];
                    if !url.is_empty() && !url.contains(char::is_whitespace) {
                        out.push('<');
                        out.push_str(url);
                        if !label.is_empty() {
                            out.push('|');
                            out.push_str(label);
                        }
                        out.push('>');
                        i = url_start + close_paren + 1;
                        continue;
                    }
                }
            }
        }
        // `**bold**` -> `*bold*`, `~~strike~~` -> `~strike~`
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            out.push('*');
            i += 2;
            continue;
        }
        if i + 1 < bytes.len() && bytes[i] == b'~' && bytes[i + 1] == b'~' {
            out.push('~');
            i += 2;
            continue;
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Slack's Block Kit `video` block: clickable title + thumbnail preview + description/provider.
fn video_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted, fg, bg) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.muted,
    );
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Video")
        .to_owned();
    let url = raw["title_url"]
        .as_str()
        .or_else(|| raw["video_url"].as_str())
        .map(str::to_owned);
    let description = text_of(&raw["description"])
        .or_else(|| raw["description"].as_str())
        .map(|s| plain_label(s, cx));
    let provider = raw["provider_name"]
        .as_str()
        .or_else(|| raw["author_name"].as_str())
        .map(str::to_owned);
    let thumb_url = raw["thumbnail_url"].as_str().map(str::to_owned);

    let title_el: AnyElement = match url.clone() {
        Some(href) => routed_link(
            format!("bk-video-{}-{ix}", kcx.ts.as_str()),
            plain_label(&title, cx),
            href,
            kcx,
            FontWeight::SEMIBOLD,
            cx,
        )
        .into_any_element(),
        None => div()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(fg)
            .child(plain_label(&title, cx))
            .into_any_element(),
    };

    let mut card = v_flex()
        .w_full()
        .max_w(px(440.))
        .gap_1p5()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(bg);
    if let Some(prov) = provider {
        card = card.child(
            h_flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(muted)
                .child(Icon::new(IconName::Video).size(px(12.)).text_color(muted))
                .child(prov),
        );
    }
    card = card.child(title_el);
    if let Some(desc) = description {
        card = card.child(div().text_xs().text_color(muted).child(desc));
    }
    if let Some(thumb) = thumb_url {
        let thumb_box = div()
            .relative()
            .w_full()
            .h(px(180.))
            .rounded_md()
            .overflow_hidden()
            .bg(theme.secondary)
            .child(
                crate::media::Picture::new(ImageSource::from(SharedString::from(thumb)))
                    .fit(ObjectFit::Cover)
                    .radius(px(6.)),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(36.))
                            .rounded_full()
                            .bg(gpui_kit::gpui::hsla(0., 0., 0., 0.65))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Play)
                                    .size(px(16.))
                                    .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95)),
                            ),
                    ),
            );
        if let Some(href) = url {
            let target = LinkTarget::Url(href.into());
            let links = kcx.links.clone();
            card = card.child(
                div()
                    .id(SharedString::from(format!(
                        "bk-video-thumb-{}-{ix}",
                        kcx.ts.as_str()
                    )))
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        messages::route_link(links.as_ref(), target.clone(), window, cx)
                    })
                    .child(thumb_box),
            );
        } else {
            card = card.child(thumb_box);
        }
    }
    card
}

/// Slack's Block Kit `call` block (third-party / legacy calls): title, live status, Join link.
fn call_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let call = raw.get("call").unwrap_or(raw);
    let title = call["title"].as_str().unwrap_or("Call").to_owned();
    let ended = call["date_end"].as_i64().is_some_and(|t| t > 0);
    let participants = call["active_participants"].as_array().map_or(0, Vec::len);
    let join_url = call["join_url"]
        .as_str()
        .or_else(|| call["desktop_app_join_url"].as_str())
        .map(str::to_owned);
    let status = if ended {
        "Call ended".to_owned()
    } else if participants > 0 {
        format!(
            "Live · {participants} {}",
            if participants == 1 {
                "participant"
            } else {
                "participants"
            }
        )
    } else {
        "Live call".to_owned()
    };

    let mut row = h_flex()
        .w_full()
        .max_w(px(440.))
        .items_center()
        .gap_3()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            div()
                .flex_none()
                .size(px(34.))
                .rounded_md()
                .bg(if ended {
                    theme.secondary
                } else {
                    theme.success.opacity(0.18)
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::new(IconName::Headphones)
                        .size(px(16.))
                        .text_color(if ended {
                            theme.muted_foreground
                        } else {
                            theme.success
                        }),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(plain_label(&title, cx)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(status),
                ),
        );
    if !ended {
        if let Some(href) = join_url {
            let target = LinkTarget::Url(href.into());
            let links = kcx.links.clone();
            row = row.child(
                Button::new(SharedString::from(format!(
                    "bk-call-join-{}-{ix}",
                    kcx.ts.as_str()
                )))
                .primary()
                .with_size(Size::XSmall)
                .label("Join")
                .on_click(move |_, window, cx| {
                    messages::route_link(links.as_ref(), target.clone(), window, cx)
                }),
            );
        }
    }
    row
}

/// Modern Block Kit `alert` / `callout` block (`F-COLLAB-713`): tinted left-accent banner.
fn alert_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = match raw["style"]
        .as_str()
        .or_else(|| raw["severity"].as_str())
        .or_else(|| raw["variant"].as_str())
        .unwrap_or("info")
    {
        "danger" | "error" | "critical" => theme.danger,
        "warning" | "warn" => theme.warning,
        "success" | "positive" => theme.success,
        _ => theme.link,
    };
    let mut body = v_flex().flex_1().min_w_0().gap_1();
    if let Some(title) = text_of(&raw["title"]).or_else(|| raw["title"].as_str()) {
        body = body.child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(plain_label(title, cx)),
        );
    }
    if let Some(el) = text_object(&raw["text"], kcx, false, cx)
        .or_else(|| text_object(&raw["body"], kcx, false, cx))
        .or_else(|| {
            raw["message"]
                .as_str()
                .or_else(|| raw["text"].as_str())
                .map(|s| messages::mrkdwn(s, kcx, false, false, cx).into_any_element())
        })
    {
        body = body.child(el);
    }
    if let Some(nested) = raw["blocks"].as_array() {
        for (n, child) in nested.iter().enumerate() {
            let kind = child["type"].as_str().unwrap_or_default();
            if let Some(el) = block(kind, child, ix * 31 + n, kcx, cx) {
                body = body.child(el);
            }
        }
    }
    h_flex()
        .w_full()
        .max_w(px(640.))
        .items_stretch()
        .gap_2p5()
        .p_2p5()
        .rounded_md()
        .border_1()
        .border_color(accent.opacity(0.35))
        .bg(accent.opacity(0.08))
        .child(div().flex_none().w(px(3.)).rounded_full().bg(accent))
        .child(body)
}

/// Modern Block Kit `card` / `container` block (`F-COLLAB-714`, `F-COLLAB-716`): bordered
/// structured card with optional title, subtitle, fields, nested blocks, and actions.
fn card_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let mut card = v_flex()
        .w_full()
        .max_w(px(560.))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted);
    if let Some(title) = text_of(&raw["title"]).or_else(|| raw["title"].as_str()) {
        let title_el: AnyElement = match raw["title_url"].as_str().or_else(|| raw["url"].as_str()) {
            Some(href) => routed_link(
                format!("bk-card-title-{}-{ix}", kcx.ts.as_str()),
                plain_label(title, cx),
                href.to_owned(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .into_any_element(),
            None => div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child(plain_label(title, cx))
                .into_any_element(),
        };
        card = card.child(title_el);
    }
    if let Some(sub) = text_of(&raw["subtitle"]).or_else(|| raw["subtitle"].as_str()) {
        card = card.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(plain_label(sub, cx)),
        );
    }
    if let Some(el) = text_object(&raw["body"], kcx, false, cx)
        .or_else(|| text_object(&raw["text"], kcx, false, cx))
    {
        card = card.child(el);
    }
    if let Some(fields) = raw["fields"].as_array() {
        for pair in fields.chunks(2) {
            card = card.child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .children(pair.iter().map(|field| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .children(text_object(field, kcx, false, cx))
                    })),
            );
        }
    }
    for key in ["blocks", "elements"] {
        if let Some(children) = raw[key].as_array() {
            for (n, child) in children.iter().enumerate() {
                let kind = child["type"].as_str().unwrap_or_default();
                if let Some(el) = block(kind, child, ix * 37 + n, kcx, cx) {
                    card = card.child(el);
                }
            }
        }
    }
    if let Some(actions) = raw["actions"].as_array() {
        card = card.child(h_flex().flex_wrap().items_center().gap_2().children(
            actions.iter().enumerate().map(|(n, element)| {
                let key = control_key(kcx.ts, raw, ix, element, n);
                block_element(element, raw, key, kcx, cx)
            }),
        ));
    }
    card
}

/// Modern Block Kit `context_actions` block (`F-COLLAB-717`): compact strip of inline
/// feedback buttons (`positive_button` / `negative_button`), icon buttons, or action elements.
fn context_actions_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let mut row = h_flex().flex_wrap().items_center().gap_1p5();
    if let Some(elements) = raw["elements"].as_array() {
        for (n, element) in elements.iter().enumerate() {
            match element["type"].as_str().unwrap_or_default() {
                "feedback_buttons" => {
                    for (sub_ix, field) in ["positive_button", "negative_button"].iter().enumerate()
                    {
                        if let Some(btn) = element.get(*field).filter(|v| v.is_object()) {
                            let mut synthetic = btn.clone();
                            synthetic["type"] = "button".into();
                            if synthetic["action_id"].is_null() {
                                synthetic["action_id"] = element["action_id"].clone();
                            }
                            let key = control_key(kcx.ts, raw, ix * 17 + sub_ix, &synthetic, n);
                            row = row.child(block_element(&synthetic, raw, key, kcx, cx));
                        }
                    }
                }
                "icon_button" => {
                    let mut synthetic = element.clone();
                    synthetic["type"] = "button".into();
                    let key = control_key(kcx.ts, raw, ix, &synthetic, n);
                    row = row.child(block_element(&synthetic, raw, key, kcx, cx));
                }
                _ => {
                    let key = control_key(kcx.ts, raw, ix, element, n);
                    row = row.child(block_element(element, raw, key, kcx, cx));
                }
            }
        }
    }
    row
}

/// Modern Block Kit `task_card` block (`F-COLLAB-732`): structured status card for agent and
/// workflow steps with status badge, title, optional details/output, sources, and actions.
fn task_card_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let status = raw["status"].as_str().unwrap_or("pending");
    let (status_icon, status_label, status_color) = match status {
        "complete" | "completed" | "done" | "success" => {
            (IconName::Check, "Complete", theme.success)
        }
        "in_progress" | "running" => (IconName::Clock, "In progress", theme.link),
        "error" | "failed" => (IconName::X, "Error", theme.danger),
        _ => (IconName::CircleDot, "Pending", theme.muted_foreground),
    };
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Task");

    let mut card = v_flex()
        .w_full()
        .max_w(px(580.))
        .gap_1p5()
        .p_2p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .size(px(20.))
                                .rounded_full()
                                .bg(status_color.opacity(0.16))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(status_icon)
                                        .size(px(12.))
                                        .text_color(status_color),
                                ),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(plain_label(title, cx)),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .px_1p5()
                        .py(px(1.))
                        .rounded_sm()
                        .bg(status_color.opacity(0.14))
                        .text_xs()
                        .text_color(status_color)
                        .child(status_label),
                ),
        );

    for key in ["details", "output"] {
        let field = &raw[key];
        if field.is_null() {
            continue;
        }
        if field["type"].as_str() == Some("rich_text") {
            if let Ok(parsed) = serde_json::from_value::<Block>(field.clone()) {
                card = card.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .pl(px(28.))
                        .children(messages::render_block(&parsed, 0, kcx, false, cx)),
                );
                continue;
            }
        }
        if let Some(el) = text_object(field, kcx, false, cx).or_else(|| {
            field
                .as_str()
                .map(|s| messages::mrkdwn(s, kcx, false, false, cx).into_any_element())
        }) {
            card = card.child(div().pl(px(28.)).child(el));
        }
    }

    if let Some(sources) = raw["sources"].as_array().filter(|s| !s.is_empty()) {
        let mut src_row = h_flex().flex_wrap().items_center().gap_2().pl(px(28.));
        for (n, src) in sources.iter().enumerate() {
            if let Some(url) = src["url"].as_str() {
                let label = src["text"]
                    .as_str()
                    .or_else(|| text_of(&src["text"]))
                    .unwrap_or(url);
                src_row = src_row.child(routed_link(
                    format!("bk-task-src-{}-{ix}-{n}", kcx.ts.as_str()),
                    plain_label(label, cx),
                    url.to_owned(),
                    kcx,
                    FontWeight::NORMAL,
                    cx,
                ));
            }
        }
        card = card.child(src_row);
    }

    if let Some(actions) = raw["actions"].as_array().filter(|a| !a.is_empty()) {
        card = card.child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .pl(px(28.))
                .children(actions.iter().enumerate().map(|(n, element)| {
                    let key = control_key(kcx.ts, raw, ix, element, n);
                    block_element(element, raw, key, kcx, cx)
                })),
        );
    }

    card
}

/// Modern Block Kit `plan` block (`F-COLLAB-728`): titled group of `task_card` steps with
/// completion progress summary.
fn plan_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let title = text_of(&raw["title"])
        .or_else(|| raw["title"].as_str())
        .unwrap_or("Plan");
    let tasks = raw["tasks"]
        .as_array()
        .or_else(|| raw["blocks"].as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let completed = tasks
        .iter()
        .filter(|t| {
            matches!(
                t["status"].as_str().unwrap_or_default(),
                "complete" | "completed" | "done" | "success"
            )
        })
        .count();

    let mut container = v_flex()
        .w_full()
        .max_w(px(600.))
        .gap_2()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::ListChecks)
                                .size(px(15.))
                                .text_color(theme.link),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.foreground)
                                .child(plain_label(title, cx)),
                        ),
                )
                .when(!tasks.is_empty(), |hdr| {
                    hdr.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{completed} of {} complete", tasks.len())),
                    )
                }),
        );
    for (n, task) in tasks.iter().enumerate() {
        container = container.child(task_card_block(task, ix * 43 + n, kcx, cx));
    }
    container
}

/// Modern Block Kit `carousel` block (`F-COLLAB-715`): horizontal scrollable strip of cards.
fn carousel_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let items = raw["elements"]
        .as_array()
        .or_else(|| raw["cards"].as_array())
        .or_else(|| raw["blocks"].as_array());
    let mut strip = h_flex().gap_3().items_stretch();
    if let Some(items) = items {
        for (n, item) in items.iter().enumerate() {
            let kind = item["type"].as_str().unwrap_or("card");
            if let Some(el) = block(kind, item, ix * 41 + n, kcx, cx) {
                strip = strip.child(div().w(px(280.)).flex_none().child(el));
            }
        }
    }
    div().w_full().min_w_0().child(
        div()
            .id(SharedString::from(format!(
                "bk-carousel-{}-{ix}",
                kcx.ts.as_str()
            )))
            .w_full()
            .overflow_x_scroll()
            .pb_1()
            .child(strip),
    )
}

/// Slack's Block Kit `file` block: compact reference chip for an embedded remote file.
fn file_block(raw: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let label = raw["external_id"]
        .as_str()
        .or_else(|| raw["file_id"].as_str())
        .unwrap_or("Attached file");
    h_flex()
        .items_center()
        .gap_2()
        .px_2p5()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(
            Icon::new(IconName::File)
                .size(px(14.))
                .text_color(theme.muted_foreground),
        )
        .child(label.to_owned())
}

/// Slack's Block Kit `table` block: first row tinted as a header, column alignment from
/// `column_settings`, each cell either `raw_text` or nested `rich_text`.
/// Modern Block Kit `data_table` block (`F-COLLAB-719`): structured column headers + rows
/// (accepts row objects keyed by column `key`/`id` or positional cell arrays).
fn data_table_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, header_bg, fg, muted) = (
        theme.border,
        theme.secondary,
        theme.foreground,
        theme.muted_foreground,
    );
    let columns: Vec<(String, String)> = raw["columns"]
        .as_array()
        .or_else(|| raw["headers"].as_array())
        .map(|cols| {
            cols.iter()
                .enumerate()
                .map(|(i, col)| {
                    if let Some(s) = col.as_str() {
                        (s.to_owned(), s.to_owned())
                    } else {
                        let key = col["key"]
                            .as_str()
                            .or_else(|| col["id"].as_str())
                            .or_else(|| col["name"].as_str())
                            .map(str::to_owned)
                            .unwrap_or_else(|| i.to_string());
                        let label = col["label"]
                            .as_str()
                            .or_else(|| text_of(&col["label"]))
                            .or_else(|| col["name"].as_str())
                            .or_else(|| text_of(&col["title"]))
                            .unwrap_or(&key)
                            .to_owned();
                        (key, label)
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let mut table = v_flex()
        .w_full()
        .max_w(px(760.))
        .my_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .overflow_hidden();

    if !columns.is_empty() {
        let mut hdr = h_flex()
            .w_full()
            .bg(header_bg)
            .font_weight(FontWeight::SEMIBOLD);
        for (c_ix, (_, label)) in columns.iter().enumerate() {
            hdr = hdr.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .px_2p5()
                    .py_1p5()
                    .text_xs()
                    .text_color(fg)
                    .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                    .child(plain_label(label, cx)),
            );
        }
        table = table.child(hdr);
    }

    if let Some(rows) = raw["rows"].as_array() {
        for (r_ix, row) in rows.iter().enumerate() {
            let mut row_el = h_flex()
                .w_full()
                .items_start()
                .border_t_1()
                .border_color(border);
            let cell_values: Vec<Json> = if let Some(arr) = row.as_array() {
                arr.clone()
            } else if let Some(obj) = row.as_object() {
                columns
                    .iter()
                    .map(|(k, _)| obj.get(k).cloned().unwrap_or(Json::Null))
                    .collect()
            } else {
                continue;
            };
            for (c_ix, val) in cell_values.iter().enumerate() {
                let cell_body: AnyElement = if let Some(el) = text_object(val, kcx, false, cx) {
                    el
                } else if let Some(s) = val.as_str() {
                    div()
                        .text_xs()
                        .text_color(fg)
                        .child(plain_label(s, cx))
                        .into_any_element()
                } else if val.is_number() || val.is_boolean() {
                    div()
                        .text_xs()
                        .text_color(fg)
                        .child(val.to_string())
                        .into_any_element()
                } else {
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("—")
                        .into_any_element()
                };
                row_el = row_el.child(
                    div()
                        .id(SharedString::from(format!(
                            "bk-dt-{}-{ix}-{r_ix}-{c_ix}",
                            kcx.ts.as_str()
                        )))
                        .flex_1()
                        .min_w_0()
                        .px_2p5()
                        .py_1p5()
                        .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                        .child(cell_body),
                );
            }
            table = table.child(row_el);
        }
    }
    table
}

/// Compact colored `tag` badge element (`F-COLLAB-787`).
fn tag_element(element: &Json, cx: &App) -> Div {
    let theme = cx.theme();
    let label = text_of(&element["text"])
        .or_else(|| element["label"].as_str())
        .or_else(|| element["text"].as_str())
        .unwrap_or("Tag");
    let color = match element["color"]
        .as_str()
        .or_else(|| element["style"].as_str())
        .unwrap_or_default()
    {
        "green" | "success" | "positive" => theme.success,
        "red" | "danger" | "error" => theme.danger,
        "yellow" | "warning" => theme.warning,
        "blue" | "primary" | "info" => theme.link,
        _ => theme.muted_foreground,
    };
    div()
        .px_2()
        .py(px(1.))
        .rounded_full()
        .bg(color.opacity(0.16))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(plain_label(label, cx))
}

/// Clickable `citation` pill element (`F-COLLAB-742`).
fn citation_element(element: &Json, ix: usize, kcx: &KitCx, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let label = text_of(&element["title"])
        .or_else(|| element["label"].as_str())
        .or_else(|| element["title"].as_str())
        .or_else(|| element["url"].as_str())
        .unwrap_or("Source");
    let url = element["url"].as_str().map(str::to_owned);
    let links = kcx.links.clone();
    let mut chip = h_flex()
        .id(SharedString::from(format!(
            "bk-cite-{}-{ix}",
            kcx.ts.as_str()
        )))
        .items_center()
        .gap_1()
        .px_1p5()
        .py(px(1.))
        .rounded_sm()
        .bg(theme.link.opacity(0.12))
        .text_xs()
        .text_color(theme.link)
        .child(
            Icon::new(IconName::Link2)
                .size(px(11.))
                .text_color(theme.link),
        )
        .child(plain_label(label, cx));
    if let Some(href) = url {
        chip = chip
            .cursor_pointer()
            .hover(|c| c.underline())
            .on_click(move |_, window, cx| {
                messages::route_link(
                    links.as_ref(),
                    LinkTarget::Url(href.clone().into()),
                    window,
                    cx,
                )
            });
    }
    chip.into_any_element()
}

fn table_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, header_bg, fg) = (theme.border, theme.secondary, theme.foreground);
    let alignments: Vec<&str> = raw["column_settings"]
        .as_array()
        .map(|cols| {
            cols.iter()
                .map(|c| c["align"].as_str().unwrap_or("left"))
                .collect()
        })
        .unwrap_or_default();
    let rows = raw["rows"].as_array();
    let mut table = v_flex()
        .w_full()
        .max_w(px(760.))
        .my_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .overflow_hidden()
        .text_xs()
        .text_color(fg);
    if let Some(rows) = rows {
        for (r_ix, row) in rows.iter().enumerate() {
            let Some(cells) = row.as_array() else {
                continue;
            };
            let is_header = r_ix == 0;
            let mut row_el = h_flex()
                .w_full()
                .items_stretch()
                .when(is_header, |r| {
                    r.bg(header_bg).font_weight(FontWeight::SEMIBOLD)
                })
                .when(r_ix > 0, |r| r.border_t_1().border_color(border));
            for (c_ix, cell) in cells.iter().enumerate() {
                let align = alignments.get(c_ix).copied().unwrap_or("left");
                let content: AnyElement = match cell["type"].as_str() {
                    Some("rich_text") => {
                        if let Ok(block) = serde_json::from_value::<Block>(cell.clone()) {
                            let cell_ts =
                                Ts::new(format!("{}-t{ix}r{r_ix}c{c_ix}", kcx.ts.as_str()));
                            let nested = kcx.nested(kcx.fixtures, kcx.kit, &cell_ts);
                            v_flex()
                                .min_w_0()
                                .children(messages::render_block(&block, 0, &nested, false, cx))
                                .into_any_element()
                        } else {
                            div()
                                .child(plain_label(cell["text"].as_str().unwrap_or_default(), cx))
                                .into_any_element()
                        }
                    }
                    _ => div()
                        .child(plain_label(cell["text"].as_str().unwrap_or_default(), cx))
                        .into_any_element(),
                };
                row_el = row_el.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px_2p5()
                        .py_1p5()
                        .when(c_ix > 0, |c| c.border_l_1().border_color(border))
                        .when(align == "right", |c| c.flex().justify_end())
                        .when(align == "center", |c| c.flex().justify_center())
                        .child(content),
                );
            }
            table = table.child(row_el);
        }
    }
    table
}

fn section(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let mut text = v_flex().flex_1().min_w_0().gap_2();
    if let Some(body) = text_object(&raw["text"], kcx, false, cx) {
        text = text.child(body);
    }
    if let Some(fields) = raw["fields"].as_array() {
        // Slack lays fields out in two columns.
        for pair in fields.chunks(2) {
            text = text.child(
                h_flex()
                    .items_start()
                    .gap_4()
                    .children(pair.iter().map(|field| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .children(text_object(field, kcx, false, cx))
                    })),
            );
        }
    }
    let accessory = raw.get("accessory").map(|element| {
        let key = control_key(kcx.ts, raw, ix, element, 0);
        let rendered = if element["type"].as_str() == Some("image") {
            image_element(element, kcx, 72., cx)
        } else {
            block_element(element, raw, key, kcx, cx)
        };
        div().flex_none().child(rendered)
    });
    h_flex()
        .w_full()
        .min_w_0()
        .items_start()
        .gap_3()
        .child(text)
        .children(accessory)
}

fn input_block(raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    let element = &raw["element"];
    let key = control_key(kcx.ts, raw, ix, element, 0);
    v_flex()
        .w_full()
        .max_w(px(520.))
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .items_baseline()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(emoji::replace_shortcodes(
                            text_of(&raw["label"]).unwrap_or_default(),
                        )),
                )
                .when(raw["optional"].as_bool().unwrap_or(false), |label| {
                    label.child(div().text_xs().text_color(muted).child("(optional)"))
                }),
        )
        .child(block_element(element, raw, key, kcx, cx))
        .when_some(text_of(&raw["hint"]), |column, hint| {
            column.child(div().text_xs().text_color(muted).child(hint.to_owned()))
        })
}

/// `(label, value)` of an element's `options` and `option_groups`.
fn options(element: &Json) -> Vec<(String, String)> {
    let groups = element["option_groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group["options"].as_array())
        .flatten();
    element["options"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(groups)
        .filter_map(|option| {
            Some((
                text_of(&option["text"])?.to_owned(),
                option["value"].as_str()?.to_owned(),
            ))
        })
        .collect()
}

fn choice(kcx: &KitCx, key: &str, cx: &App) -> Option<String> {
    kcx.kit
        .and_then(|kit| kit.read(cx).choices.get(key).cloned())
}

/// What Slack shows of a long attachment text until "Show more": its first five lines, at
/// most 700 characters, cut at a word.
fn collapsed(text: &str) -> String {
    let lines = text
        .match_indices('\n')
        .nth(4)
        .map_or(text, |(end, _)| &text[..end]);
    let head = match lines.char_indices().nth(700) {
        Some((end, _)) => {
            let cut = &lines[..end];
            cut.rfind(char::is_whitespace)
                .map_or(cut, |word| &cut[..word])
        }
        None => lines,
    };
    format!("{}…", head.trim_end())
}

fn set_choice(kit: Option<&Entity<BlockKitState>>, key: &str, value: &str, cx: &mut App) {
    if let Some(kit) = kit {
        kit.update(cx, |state, cx| {
            state.choices.insert(key.to_owned(), value.to_owned());
            cx.notify();
        });
    }
}

/// Fixture mode has no app to receive `block_actions`: log the interaction and toast it.
fn log_action(key: &str, detail: &str, window: &mut Window, cx: &mut App) {
    eprintln!("slackless: block action {key}: {detail}");
    actions::notify("Block Kit action", format!("{detail} ({key})"), window, cx);
}

/// A button press nothing dispatches (fixture mode, or a message naming no app): log it.
pub(crate) fn log_button(action: &Json, window: &mut Window, cx: &mut App) {
    let label = emoji::replace_shortcodes(text_of(&action["text"]).unwrap_or("Button"));
    let value = action["value"].as_str().unwrap_or_default();
    log_action(
        action["action_id"].as_str().unwrap_or_default(),
        &format!("pressed \u{201C}{label}\u{201D} (value {value})"),
        window,
        cx,
    );
}

/// A button press bound for `blocks.actions`: the app that posted the message and where the
/// element sits.
pub struct BlockDispatch {
    pub bot: BotId,
    pub app_id: String,
    pub team: Option<TeamId>,
    /// The element as the message declared it, plus its `block_id`.
    pub action: Json,
    pub container: ActionContainer,
}

impl BlockDispatch {
    /// For a button pressed in `message` of `channel` (inside the legacy attachment
    /// `attachment`); `None` when the message names no app to receive it.
    pub fn new(
        message: &Message,
        channel: &ChannelId,
        action: Json,
        attachment: Option<i64>,
    ) -> Option<Self> {
        let profile = message.bot_profile.as_ref()?;
        Some(Self {
            bot: message.bot_id.clone()?,
            app_id: profile.app_id.clone()?,
            team: profile.team_id.clone(),
            action,
            container: ActionContainer {
                channel: channel.clone(),
                message_ts: message.ts.clone(),
                attachment_id: attachment,
                is_ephemeral: message.is_ephemeral,
                view_id: None,
            },
        })
    }
}

/// Build a Slack `blocks.actions` payload from `element` + `block["block_id"]`.
fn action_base(element: &Json, block: &Json) -> Json {
    let mut action = element.clone();
    if let Some(block_id) = block["block_id"].as_str() {
        action["block_id"] = block_id.into();
    }
    action
}

/// Run `proceed` immediately unless `confirm` is a Slack Block Kit confirmation dialog object
/// (`F-COLLAB-765`, `F-COLLAB-766`), in which case show an alert dialog with Slack's title,
/// body, confirm/deny labels, and `danger`/`primary` button variant first.
fn maybe_confirm(
    confirm: Option<&Json>,
    window: &mut Window,
    cx: &mut App,
    proceed: impl Fn(&mut Window, &mut App) + 'static,
) {
    let Some(obj) = confirm.filter(|v| v.is_object()) else {
        proceed(window, cx);
        return;
    };
    let title = emoji::replace_shortcodes(
        text_of(&obj["title"])
            .or_else(|| obj["title"].as_str())
            .unwrap_or("Are you sure?"),
    );
    let body = emoji::replace_shortcodes(
        text_of(&obj["text"])
            .or_else(|| obj["text"].as_str())
            .unwrap_or_default(),
    );
    let ok_label = emoji::replace_shortcodes(
        text_of(&obj["confirm"])
            .or_else(|| obj["confirm"].as_str())
            .unwrap_or("Confirm"),
    );
    let cancel_label = emoji::replace_shortcodes(
        text_of(&obj["deny"])
            .or_else(|| obj["deny"].as_str())
            .unwrap_or("Cancel"),
    );
    let ok_variant = match obj["style"].as_str() {
        Some("danger") => ButtonVariant::Danger,
        _ => ButtonVariant::Primary,
    };
    let proceed = Rc::new(proceed);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let proceed = proceed.clone();
        alert
            .centered()
            .width(px(440.))
            .title(title.clone())
            .description(body.clone())
            .show_cancel(true)
            .cancel_text(cancel_label.clone())
            .ok_text(ok_label.clone())
            .ok_variant(ok_variant)
            .on_ok(move |_, window, cx| {
                proceed(window, cx);
                true
            })
    });
}

/// Dispatch `action` through the message's live `blocks.actions` handler when present, else log it.
fn dispatch_action(
    action: Json,
    on_action: Option<&actions::ActionHandler>,
    attachment: Option<i64>,
    key: &str,
    detail: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(on_action) = on_action {
        on_action(
            actions::MessageAction::BlockAction { action, attachment },
            window,
            cx,
        );
    } else {
        log_action(key, detail, window, cx);
    }
}

/// One interactive (or image) element of `block` (its `actions`, accessory or `input`).
fn block_element(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> AnyElement {
    let kind = element["type"].as_str().unwrap_or_default();
    match kind {
        "button" => button(element, block, key, kcx).into_any_element(),
        "workflow_button" => workflow_button(element, block, key, kcx).into_any_element(),
        "static_select" | "external_select" | "multi_static_select" | "multi_external_select" => {
            let mut opts = options(element);
            if matches!(kind, "external_select" | "multi_external_select") {
                if let Some(live) = kcx
                    .kit
                    .and_then(|kit| kit.read(cx).suggestions.get(&key).cloned())
                {
                    opts = live;
                } else if let Some(on_suggest) = kcx.on_suggest.clone() {
                    let action_id = element["action_id"].as_str().unwrap_or_default().to_owned();
                    let block_id = block["block_id"].as_str().unwrap_or_default().to_owned();
                    let key_c = key.clone();
                    if opts.is_empty() {
                        opts.push((
                            "Load options\u{2026}".to_owned(),
                            "__slackless_load__".to_owned(),
                        ));
                    }
                    let _ = (on_suggest, action_id, block_id, key_c);
                }
            }
            select(element, block, key, opts, kcx, cx)
        }
        "users_select" | "multi_users_select" => {
            let users = kcx
                .fixtures
                .users
                .iter()
                .map(|user| (user.display_name().to_owned(), user.id.to_string()))
                .collect();
            select(element, block, key, users, kcx, cx)
        }
        "conversations_select"
        | "channels_select"
        | "multi_conversations_select"
        | "multi_channels_select" => {
            let channels = kcx
                .fixtures
                .channels
                .iter()
                .map(|channel| {
                    (
                        kcx.fixtures.conversation_label(channel),
                        channel.id.to_string(),
                    )
                })
                .collect();
            select(element, block, key, channels, kcx, cx)
        }
        "overflow" => overflow(element, block, key, kcx).into_any_element(),
        "datepicker" => datepicker(element, block, key, kcx, cx),
        "datetimepicker" => datetimepicker(element, block, key, kcx, cx),
        "timepicker" => {
            let times = (0..24)
                .flat_map(|hour| [0u32, 15, 30, 45].into_iter().map(move |min| (hour, min)))
                .map(|(hour, min)| {
                    let t = format!("{hour:02}:{min:02}");
                    (t.clone(), t)
                })
                .collect();
            picker(
                element,
                block,
                key,
                "initial_time",
                "selected_time",
                IconName::Clock,
                times,
                kcx,
                cx,
            )
        }
        "checkboxes" => checkboxes(element, block, key, kcx, cx).into_any_element(),
        "radio_buttons" => radios(element, block, key, kcx, cx).into_any_element(),
        "image" => image_element(element, kcx, 20., cx),
        "tag" => tag_element(element, cx).into_any_element(),
        "citation" => citation_element(element, 0, kcx, cx),
        kind if TEXT_INPUTS.contains(&kind) => text_input(element, &key, kcx, cx),
        other => div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format!("[{other}]"))
            .into_any_element(),
    }
}

/// A button: respects optional `confirm` dialogs (`F-COLLAB-765`, `F-COLLAB-766`); a link
/// button opens its URL; any other press goes to the message's action handler.
fn button(element: &Json, block: &Json, key: String, kcx: &KitCx) -> Button {
    let label = emoji::replace_shortcodes(text_of(&element["text"]).unwrap_or("Button"));
    let url = element["url"].as_str().map(str::to_owned);
    let confirm = element.get("confirm").filter(|v| v.is_object()).cloned();
    let tooltip = element["accessibility_label"].as_str().map(str::to_owned);
    let mut action = element.clone();
    if let Some(block_id) = block["block_id"].as_str() {
        action["block_id"] = block_id.into();
    }
    let (on_action, attachment) = (kcx.on_action.clone(), kcx.attachment);
    let button = Button::new(SharedString::from(format!("bk-{key}")))
        .with_size(Size::Small)
        .label(label.clone());
    let button = match element["style"].as_str() {
        Some("primary") => button.primary(),
        Some("danger") => button.danger(),
        _ => button.outline(),
    };
    button
        .when_some(url.clone(), |button, _| button.icon(IconName::ExternalLink))
        .when_some(tooltip, |button, tip| button.tooltip(tip))
        .on_click(move |_, window, cx| {
            let url = url.clone();
            let on_action = on_action.clone();
            let action = action.clone();
            maybe_confirm(confirm.as_ref(), window, cx, move |window, cx| {
                match (&url, &on_action) {
                    (Some(url), _) => cx.open_url(url),
                    (None, Some(on_action)) => {
                        let action = actions::MessageAction::BlockAction {
                            action: action.clone(),
                            attachment,
                        };
                        on_action(action, window, cx);
                    }
                    (None, None) => log_button(&action, window, cx),
                }
            });
        })
}

/// Slack `workflow_button` element (`F-COLLAB-769`, `F-COLLAB-770`, `F-COLLAB-771`): renders
/// with primary/danger/outline style, optional `accessibility_label` tooltip, optional `confirm`
/// dialog, and launches `workflow.trigger.url` via the link router (or dispatches `blocks.actions`).
fn workflow_button(element: &Json, block: &Json, key: String, kcx: &KitCx) -> Button {
    let label = emoji::replace_shortcodes(text_of(&element["text"]).unwrap_or("Run workflow"));
    let trigger_url = element["workflow"]["trigger"]["url"]
        .as_str()
        .or_else(|| element["url"].as_str())
        .map(str::to_owned);
    let confirm = element.get("confirm").filter(|v| v.is_object()).cloned();
    let tooltip = element["accessibility_label"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| Some("Run Slack workflow".to_owned()));
    let action = action_base(element, block);
    let (on_action, attachment, links) = (kcx.on_action.clone(), kcx.attachment, kcx.links.clone());
    let button = Button::new(SharedString::from(format!("bk-wf-{key}")))
        .with_size(Size::Small)
        .icon(IconName::Zap)
        .label(label);
    let button = match element["style"].as_str() {
        Some("primary") => button.primary(),
        Some("danger") => button.danger(),
        _ => button.outline(),
    };
    button
        .when_some(tooltip, |b, tip| b.tooltip(tip))
        .on_click(move |_, window, cx| {
            let trigger_url = trigger_url.clone();
            let on_action = on_action.clone();
            let links = links.clone();
            let action = action.clone();
            maybe_confirm(confirm.as_ref(), window, cx, move |window, cx| {
                if let Some(url) = &trigger_url {
                    messages::route_link(
                        links.as_ref(),
                        LinkTarget::Url(url.clone().into()),
                        window,
                        cx,
                    );
                } else if let Some(on_action) = &on_action {
                    on_action(
                        actions::MessageAction::BlockAction {
                            action: action.clone(),
                            attachment,
                        },
                        window,
                        cx,
                    );
                } else {
                    log_button(&action, window, cx);
                }
            });
        })
}

/// Slack `datetimepicker` element (`F-COLLAB-746`): displays the chosen date & time (formatted
/// from unix epoch `initial_date_time`) with quick preset picks dispatching `selected_date_time`.
fn datetimepicker(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> AnyElement {
    let now = messages::now_secs();
    let initial_ts = element["initial_date_time"].as_i64().unwrap_or(now);
    let current = choice(kcx, &key, cx)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(initial_ts);
    let format_dt = |secs: i64| -> String {
        format!(
            "{} {}",
            messages::iso_date(secs),
            messages::clock(secs, None)
        )
    };
    let presets = [
        ("Now", now),
        ("In 1 hour", now + 3_600),
        ("Tomorrow, 9:00 AM", now + 86_400),
        ("Next week", now + 7 * 86_400),
    ];
    let kit = kcx.kit.cloned();
    let base = action_base(element, block);
    let (on_action, attachment) = (kcx.on_action.clone(), kcx.attachment);
    let confirm = element.get("confirm").filter(|v| v.is_object()).cloned();
    Button::new(SharedString::from(format!("bk-dtp-{key}")))
        .outline()
        .with_size(Size::Small)
        .icon(IconName::Calendar)
        .label(format_dt(current))
        .dropdown_menu(move |mut menu, _, _| {
            for (preset_label, ts_val) in presets {
                let (key, kit, base, on_action, confirm) = (
                    key.clone(),
                    kit.clone(),
                    base.clone(),
                    on_action.clone(),
                    confirm.clone(),
                );
                let formatted = format_dt(ts_val);
                let item_label = format!("{preset_label} ({formatted})");
                menu = menu.item(
                    PopupMenuItem::new(item_label).on_click(move |_, window, cx| {
                        let (key, kit, base, on_action) =
                            (key.clone(), kit.clone(), base.clone(), on_action.clone());
                        let detail = format!("picked {formatted}");
                        maybe_confirm(confirm.as_ref(), window, cx, move |window, cx| {
                            set_choice(kit.as_ref(), &key, &ts_val.to_string(), cx);
                            let mut payload = base.clone();
                            payload["selected_date_time"] = ts_val.into();
                            dispatch_action(
                                payload,
                                on_action.as_ref(),
                                attachment,
                                &key,
                                &detail,
                                window,
                                cx,
                            );
                        });
                    }),
                );
            }
            menu
        })
        .into_any_element()
}

/// A select menu: supports both single-select and real multi-select (`multi_*_select`,
/// `F-COLLAB-775`), opening a checked option list and dispatching live `blocks.actions` with
/// Slack's wire selection fields.
fn select(
    element: &Json,
    block: &Json,
    key: String,
    options: Vec<(String, String)>,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let kind = element["type"].as_str().unwrap_or_default().to_owned();
    let is_multi = kind.starts_with("multi_");
    let placeholder = text_of(&element["placeholder"])
        .unwrap_or("Select an item")
        .to_owned();
    let base = action_base(element, block);

    if is_multi {
        let initial: HashSet<String> = element["initial_options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["value"].as_str().map(str::to_owned))
            .chain(
                element["initial_users"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            )
            .chain(
                element["initial_conversations"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            )
            .chain(
                element["initial_channels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned)),
            )
            .collect();
        let selected_set = kcx
            .kit
            .and_then(|kit| kit.read(cx).checked.get(&key).cloned())
            .unwrap_or_else(|| initial.clone());
        let selected_labels: Vec<&str> = options
            .iter()
            .filter(|(_, v)| selected_set.contains(v))
            .map(|(l, _)| l.as_str())
            .collect();
        let label = match selected_labels.len() {
            0 => placeholder,
            1 => selected_labels[0].to_owned(),
            2 => format!("{}, {}", selected_labels[0], selected_labels[1]),
            n => format!("{}, +{} more", selected_labels[0], n - 1),
        };
        let (kit, on_action, attachment) =
            (kcx.kit.cloned(), kcx.on_action.clone(), kcx.attachment);
        let all_opts = options.clone();
        return Button::new(SharedString::from(format!("bk-{key}")))
            .outline()
            .with_size(Size::Small)
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (text, value) in &options {
                    let is_on = selected_set.contains(value);
                    let (kit, key, value, text_s) =
                        (kit.clone(), key.clone(), value.clone(), text.clone());
                    let (initial, base, kind, all_opts, on_action) = (
                        initial.clone(),
                        base.clone(),
                        kind.clone(),
                        all_opts.clone(),
                        on_action.clone(),
                    );
                    menu = menu.item(PopupMenuItem::new(text.clone()).checked(is_on).on_click(
                        move |_, window, cx| {
                            let next_set = if let Some(kit) = &kit {
                                kit.update(cx, |state, cx| {
                                    let set = state
                                        .checked
                                        .entry(key.clone())
                                        .or_insert_with(|| initial.clone());
                                    if !set.insert(value.clone()) {
                                        set.remove(&value);
                                    }
                                    cx.notify();
                                    set.clone()
                                })
                            } else {
                                let mut set = initial.clone();
                                if !set.insert(value.clone()) {
                                    set.remove(&value);
                                }
                                set
                            };
                            let mut payload = base.clone();
                            let ordered: Vec<(&str, &str)> = all_opts
                                .iter()
                                .filter(|(_, v)| next_set.contains(v))
                                .map(|(t, v)| (t.as_str(), v.as_str()))
                                .collect();
                            match kind.as_str() {
                                "multi_users_select" => {
                                    payload["selected_users"] = Json::Array(
                                        ordered.iter().map(|(_, v)| (*v).into()).collect(),
                                    );
                                }
                                "multi_conversations_select" => {
                                    payload["selected_conversations"] = Json::Array(
                                        ordered.iter().map(|(_, v)| (*v).into()).collect(),
                                    );
                                }
                                "multi_channels_select" => {
                                    payload["selected_channels"] = Json::Array(
                                        ordered.iter().map(|(_, v)| (*v).into()).collect(),
                                    );
                                }
                                _ => {
                                    payload["selected_options"] = Json::Array(
                                        ordered
                                            .iter()
                                            .map(|(t, v)| {
                                                serde_json::json!({
                                                    "text": { "type": "plain_text", "text": t },
                                                    "value": v,
                                                })
                                            })
                                            .collect(),
                                    );
                                }
                            }
                            let verb = if next_set.contains(&value) {
                                "selected"
                            } else {
                                "removed"
                            };
                            dispatch_action(
                                payload,
                                on_action.as_ref(),
                                attachment,
                                &key,
                                &format!("{verb} \u{201C}{text_s}\u{201D}"),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                menu
            })
            .into_any_element();
    }

    let initial = element["initial_option"]["value"]
        .as_str()
        .or_else(|| element["initial_user"].as_str())
        .or_else(|| element["initial_conversation"].as_str())
        .or_else(|| element["initial_channel"].as_str())
        .map(str::to_owned);
    let selected = choice(kcx, &key, cx).or(initial);
    let label = selected
        .as_ref()
        .and_then(|value| options.iter().find(|(_, option)| option == value))
        .map_or(placeholder, |(label, _)| label.clone());
    dropdown(
        key,
        label,
        None,
        options,
        selected,
        kcx.kit.cloned(),
        kcx.on_action.clone(),
        kcx.attachment,
        move |text, value| {
            let mut action = base.clone();
            match kind.as_str() {
                "users_select" => action["selected_user"] = value.into(),
                "conversations_select" => action["selected_conversation"] = value.into(),
                "channels_select" => action["selected_channel"] = value.into(),
                _ => {
                    action["selected_option"] = serde_json::json!({
                        "text": { "type": "plain_text", "text": text },
                        "value": value,
                    });
                }
            }
            action
        },
    )
    .into_any_element()
}

fn parse_iso_ymd(s: &str) -> Option<(i32, u32, u32)> {
    let mut parts = s.split('-');
    let y = parts.next()?.parse::<i32>().ok()?;
    let m = parts.next()?.parse::<u32>().ok()?;
    let d = parts.next()?.parse::<u32>().ok()?;
    if (1..=12).contains(&m) && (1..=31).contains(&d) {
        Some((y, m, d))
    } else {
        None
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 30,
    }
}

/// Day of week for `year-month-day` (0 = Sunday .. 6 = Saturday), Sakamoto's algorithm.
fn weekday_sunday0(year: i32, month: u32, day: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    ((y + y / 4 - y / 100 + y / 400 + T[(month - 1) as usize] + day as i32).rem_euclid(7)) as u32
}

fn month_name(month: u32) -> &'static str {
    const NAMES: [&str; 12] = [
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
    NAMES
        .get((month.saturating_sub(1)) as usize)
        .copied()
        .unwrap_or("")
}

/// Interactive monthly calendar grid date picker (`F-COLLAB-745`).
fn datepicker(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> AnyElement {
    let selected =
        choice(kcx, &key, cx).or_else(|| element["initial_date"].as_str().map(str::to_owned));
    let today_iso = messages::iso_date(messages::now_secs());
    let (today_y, today_m, _) = parse_iso_ymd(&today_iso).unwrap_or((2026, 1, 1));
    let default_ym = selected
        .as_deref()
        .and_then(parse_iso_ymd)
        .map(|(y, m, _)| (y, m))
        .unwrap_or((today_y, today_m));
    let is_open = kcx
        .kit
        .is_some_and(|kit| kit.read(cx).open_calendars.contains(&key));
    let (view_y, view_m) = kcx
        .kit
        .and_then(|kit| kit.read(cx).calendar_months.get(&key).copied())
        .unwrap_or(default_ym);
    let label = selected
        .clone()
        .or_else(|| text_of(&element["placeholder"]).map(str::to_owned))
        .unwrap_or_else(|| "Select a date".to_owned());

    let theme = cx.theme();
    let (border, popover, muted, hover, primary, primary_fg, fg) = (
        theme.border,
        theme.popover,
        theme.muted_foreground,
        theme.accent,
        theme.primary,
        theme.primary_foreground,
        theme.foreground,
    );
    let kit_toggle = kcx.kit.cloned();
    let key_toggle = key.clone();
    let trigger = Button::new(SharedString::from(format!("bk-{key}")))
        .outline()
        .with_size(Size::Small)
        .icon(IconName::Calendar)
        .label(label)
        .dropdown_caret(true)
        .on_click(move |_, _, cx| {
            if let Some(kit) = &kit_toggle {
                kit.update(cx, |state, cx| {
                    if !state.open_calendars.insert(key_toggle.clone()) {
                        state.open_calendars.remove(&key_toggle);
                    }
                    state
                        .calendar_months
                        .entry(key_toggle.clone())
                        .or_insert(default_ym);
                    cx.notify();
                });
            }
        });

    if !is_open {
        return trigger.into_any_element();
    }

    let first_dow = weekday_sunday0(view_y, view_m, 1) as usize;
    let total_days = days_in_month(view_y, view_m) as usize;
    let (prev_y, prev_m) = if view_m == 1 {
        (view_y - 1, 12)
    } else {
        (view_y, view_m - 1)
    };
    let (next_y, next_m) = if view_m == 12 {
        (view_y + 1, 1)
    } else {
        (view_y, view_m + 1)
    };
    let base = action_base(element, block);
    let mut cells: Vec<Option<u32>> = vec![None; first_dow];
    cells.extend((1..=total_days as u32).map(Some));
    while cells.len() % 7 != 0 {
        cells.push(None);
    }

    let kit_prev = kcx.kit.cloned();
    let key_prev = key.clone();
    let kit_next = kcx.kit.cloned();
    let key_next = key.clone();
    let mut grid = v_flex()
        .w(px(236.))
        .p_2()
        .gap_1p5()
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(popover)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .child(
                    Button::new(SharedString::from(format!("bk-cal-prev-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::ChevronLeft)
                        .on_click(move |_, _, cx| {
                            if let Some(kit) = &kit_prev {
                                kit.update(cx, |state, cx| {
                                    state
                                        .calendar_months
                                        .insert(key_prev.clone(), (prev_y, prev_m));
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child(format!("{} {view_y}", month_name(view_m))),
                )
                .child(
                    Button::new(SharedString::from(format!("bk-cal-next-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::ChevronRight)
                        .on_click(move |_, _, cx| {
                            if let Some(kit) = &kit_next {
                                kit.update(cx, |state, cx| {
                                    state
                                        .calendar_months
                                        .insert(key_next.clone(), (next_y, next_m));
                                    cx.notify();
                                });
                            }
                        }),
                ),
        )
        .child(
            h_flex().gap_0p5().children(
                ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"]
                    .into_iter()
                    .map(|wd| {
                        div()
                            .w(px(30.))
                            .text_center()
                            .text_xs()
                            .text_color(muted)
                            .child(wd)
                    }),
            ),
        );

    for week in cells.chunks(7) {
        let mut row = h_flex().gap_0p5();
        for cell in week {
            if let Some(day) = *cell {
                let iso = format!("{view_y:04}-{view_m:02}-{day:02}");
                let is_selected = selected.as_deref() == Some(iso.as_str());
                let is_today = iso == today_iso;
                let (kit, key_c, base, on_action, attachment) = (
                    kcx.kit.cloned(),
                    key.clone(),
                    base.clone(),
                    kcx.on_action.clone(),
                    kcx.attachment,
                );
                let iso_click = iso.clone();
                row = row.child(
                    div()
                        .id(SharedString::from(format!("bk-cal-{key}-{iso}")))
                        .w(px(30.))
                        .h(px(26.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .cursor_pointer()
                        .when(is_selected, |el| {
                            el.bg(primary)
                                .text_color(primary_fg)
                                .font_weight(FontWeight::BOLD)
                        })
                        .when(!is_selected && is_today, |el| {
                            el.border_1().border_color(primary).text_color(fg)
                        })
                        .when(!is_selected, |el| el.hover(|h| h.bg(hover)))
                        .child(day.to_string())
                        .on_click(move |_, window, cx| {
                            if let Some(kit) = &kit {
                                kit.update(cx, |state, cx| {
                                    state.choices.insert(key_c.clone(), iso_click.clone());
                                    state.open_calendars.remove(&key_c);
                                    cx.notify();
                                });
                            }
                            let mut payload = base.clone();
                            payload["selected_date"] = iso_click.clone().into();
                            dispatch_action(
                                payload,
                                on_action.as_ref(),
                                attachment,
                                &key_c,
                                &format!("selected \u{201C}{iso_click}\u{201D}"),
                                window,
                                cx,
                            );
                        }),
                );
            } else {
                row = row.child(div().w(px(30.)).h(px(26.)));
            }
        }
        grid = grid.child(row);
    }

    v_flex()
        .gap_1()
        .child(trigger)
        .child(grid)
        .into_any_element()
}

/// Date / time pickers: the chosen value (or `initial_field`) on a button with `icon`, the
/// next values in a checked menu.
#[allow(clippy::too_many_arguments)]
fn picker(
    element: &Json,
    block: &Json,
    key: String,
    initial_field: &str,
    selected_field: &'static str,
    icon: IconName,
    options: Vec<(String, String)>,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let selected =
        choice(kcx, &key, cx).or_else(|| element[initial_field].as_str().map(str::to_owned));
    let label = selected
        .clone()
        .or_else(|| text_of(&element["placeholder"]).map(str::to_owned))
        .unwrap_or_else(|| "Select".to_owned());
    let base = action_base(element, block);
    dropdown(
        key,
        label,
        Some(icon),
        options,
        selected,
        kcx.kit.cloned(),
        kcx.on_action.clone(),
        kcx.attachment,
        move |_, value| {
            let mut action = base.clone();
            action[selected_field] = value.into();
            action
        },
    )
    .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn dropdown(
    key: String,
    label: String,
    icon: Option<IconName>,
    options: Vec<(String, String)>,
    selected: Option<String>,
    kit: Option<Entity<BlockKitState>>,
    on_action: Option<actions::ActionHandler>,
    attachment: Option<i64>,
    build_action: impl Fn(&str, &str) -> Json + 'static,
) -> impl IntoElement {
    let build_action = std::rc::Rc::new(build_action);
    Button::new(SharedString::from(format!("bk-{key}")))
        .outline()
        .with_size(Size::Small)
        .when_some(icon, |button, icon| button.icon(icon))
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |mut menu, _, _| {
            for (text, value) in &options {
                let (kit, key, value, text_s) =
                    (kit.clone(), key.clone(), value.clone(), text.clone());
                let (on_action, build) = (on_action.clone(), build_action.clone());
                let detail = format!("selected \u{201C}{text}\u{201D}");
                menu = menu.item(
                    PopupMenuItem::new(text.clone())
                        .checked(selected.as_ref() == Some(&value))
                        .on_click(move |_, window, cx| {
                            set_choice(kit.as_ref(), &key, &value, cx);
                            let payload = build(&text_s, &value);
                            dispatch_action(
                                payload,
                                on_action.as_ref(),
                                attachment,
                                &key,
                                &detail,
                                window,
                                cx,
                            );
                        }),
                );
            }
            menu
        })
}

/// The `...` overflow menu: options with a `url` open it via the link router (`F-COLLAB-777`),
/// and every choice dispatches a live `blocks.actions` event.
fn overflow(element: &Json, block: &Json, key: String, kcx: &KitCx) -> impl IntoElement {
    let items: Vec<(String, String, Option<String>)> = element["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some((
                text_of(&option["text"])?.to_owned(),
                option["value"].as_str().unwrap_or_default().to_owned(),
                option["url"].as_str().map(str::to_owned),
            ))
        })
        .collect();
    let base = action_base(element, block);
    let confirm = element.get("confirm").filter(|v| v.is_object()).cloned();
    let (on_action, attachment, links) = (kcx.on_action.clone(), kcx.attachment, kcx.links.clone());
    Button::new(SharedString::from(format!("bk-{key}")))
        .ghost()
        .with_size(Size::Small)
        .icon(IconName::Ellipsis)
        .dropdown_menu(move |mut menu, _, _| {
            for (text, value, url) in &items {
                let (key, base, on_action, links, confirm) = (
                    key.clone(),
                    base.clone(),
                    on_action.clone(),
                    links.clone(),
                    confirm.clone(),
                );
                let (text_s, value_s, url_s) = (text.clone(), value.clone(), url.clone());
                let detail = format!("chose \u{201C}{text}\u{201D} (value {value})");
                let mut item = PopupMenuItem::new(text.clone());
                if url.is_some() {
                    item = item.icon(IconName::ExternalLink);
                }
                menu = menu.item(item.on_click(move |_, window, cx| {
                    let (key, base, on_action, links) =
                        (key.clone(), base.clone(), on_action.clone(), links.clone());
                    let (text_s, value_s, url_s, detail) = (
                        text_s.clone(),
                        value_s.clone(),
                        url_s.clone(),
                        detail.clone(),
                    );
                    maybe_confirm(confirm.as_ref(), window, cx, move |window, cx| {
                        if let Some(url) = &url_s {
                            if let Some(links) = &links {
                                links(messages::LinkTarget::Url(url.clone().into()), window, cx);
                            } else {
                                cx.open_url(url);
                            }
                        }
                        let mut payload = base.clone();
                        payload["selected_option"] = serde_json::json!({
                            "text": { "type": "plain_text", "text": text_s },
                            "value": value_s,
                        });
                        dispatch_action(
                            payload,
                            on_action.as_ref(),
                            attachment,
                            &key,
                            &detail,
                            window,
                            cx,
                        );
                    });
                }));
            }
            menu
        })
}

fn checkboxes(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
    let initial: HashSet<String> = element["initial_options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| option["value"].as_str().map(str::to_owned))
        .collect();
    let checked = kcx
        .kit
        .and_then(|kit| kit.read(cx).checked.get(&key).cloned())
        .unwrap_or_else(|| initial.clone());
    let base = action_base(element, block);
    let all_options = options(element);
    v_flex()
        .gap_1p5()
        .children(all_options.clone().into_iter().map(|(text, value)| {
            let is_checked = checked.contains(&value);
            let (kit, key, initial) = (kcx.kit.cloned(), key.clone(), initial.clone());
            let (on_action, attachment, base, opts) = (
                kcx.on_action.clone(),
                kcx.attachment,
                base.clone(),
                all_options.clone(),
            );
            Checkbox::new(SharedString::from(format!("bk-{key}-{value}")))
                .label(text.clone())
                .checked(is_checked)
                .on_click(move |now: &bool, window, cx| {
                    let selected_set = if let Some(kit) = &kit {
                        kit.update(cx, |state, cx| {
                            let set = state
                                .checked
                                .entry(key.clone())
                                .or_insert_with(|| initial.clone());
                            if *now {
                                set.insert(value.clone());
                            } else {
                                set.remove(&value);
                            }
                            cx.notify();
                            set.clone()
                        })
                    } else {
                        let mut set = initial.clone();
                        if *now {
                            set.insert(value.clone());
                        } else {
                            set.remove(&value);
                        }
                        set
                    };
                    let mut payload = base.clone();
                    payload["selected_options"] = Json::Array(
                        opts.iter()
                            .filter(|(_, v)| selected_set.contains(v))
                            .map(|(t, v)| {
                                serde_json::json!({
                                    "text": { "type": "plain_text", "text": t },
                                    "value": v,
                                })
                            })
                            .collect(),
                    );
                    let verb = if *now { "checked" } else { "unchecked" };
                    dispatch_action(
                        payload,
                        on_action.as_ref(),
                        attachment,
                        &key,
                        &format!("{verb} \u{201C}{text}\u{201D}"),
                        window,
                        cx,
                    );
                })
        }))
}

fn radios(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
    let selected = choice(kcx, &key, cx).or_else(|| {
        element["initial_option"]["value"]
            .as_str()
            .map(str::to_owned)
    });
    let base = action_base(element, block);
    v_flex()
        .gap_1p5()
        .children(options(element).into_iter().map(|(text, value)| {
            let checked = selected.as_ref() == Some(&value);
            let (kit, key, base, on_action, attachment) = (
                kcx.kit.cloned(),
                key.clone(),
                base.clone(),
                kcx.on_action.clone(),
                kcx.attachment,
            );
            Radio::new(SharedString::from(format!("bk-{key}-{value}")))
                .label(text.clone())
                .checked(checked)
                .on_click(move |_, window, cx| {
                    set_choice(kit.as_ref(), &key, &value, cx);
                    let mut payload = base.clone();
                    payload["selected_option"] = serde_json::json!({
                        "text": { "type": "plain_text", "text": text },
                        "value": value,
                    });
                    dispatch_action(
                        payload,
                        on_action.as_ref(),
                        attachment,
                        &key,
                        &format!("chose \u{201C}{text}\u{201D}"),
                        window,
                        cx,
                    );
                })
        }))
}

/// Validate a Block Kit text input element's current value (`F-COLLAB-747`, `776`, `778`, `791`).
fn validate_text_input(element: &Json, value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match element["type"].as_str().unwrap_or_default() {
        "email_text_input" => {
            let valid = trimmed.split_once('@').is_some_and(|(local, domain)| {
                !local.is_empty()
                    && !local.contains(char::is_whitespace)
                    && !domain.contains(char::is_whitespace)
                    && domain.contains('.')
                    && !domain.starts_with('.')
                    && !domain.ends_with('.')
            });
            (!valid).then(|| "Enter a valid email address.".to_owned())
        }
        "url_text_input" => {
            let valid = (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
                && !trimmed.contains(char::is_whitespace)
                && trimmed.split("://").nth(1).is_some_and(|rest| {
                    let host = rest.split('/').next().unwrap_or_default();
                    !host.is_empty() && host.contains('.')
                });
            (!valid).then(|| "Enter a valid URL starting with http:// or https://.".to_owned())
        }
        "number_input" => {
            let decimal_ok = element["is_decimal_allowed"].as_bool().unwrap_or(false);
            let parsed = if decimal_ok {
                trimmed.parse::<f64>().ok()
            } else {
                trimmed.parse::<i64>().ok().map(|n| n as f64)
            };
            let Some(num) = parsed else {
                return Some(if decimal_ok {
                    "Enter a valid number.".to_owned()
                } else {
                    "Enter a whole number.".to_owned()
                });
            };
            if let Some(min) = element["min_value"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| element["min_value"].as_f64())
            {
                if num < min {
                    return Some(format!("Must be at least {min}."));
                }
            }
            if let Some(max) = element["max_value"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| element["max_value"].as_f64())
            {
                if num > max {
                    return Some(format!("Must be at most {max}."));
                }
            }
            None
        }
        "plain_text_input" => {
            let len = value.chars().count();
            if let Some(min) = element["min_length"].as_u64() {
                if (len as u64) < min {
                    return Some(format!("Must be at least {min} characters."));
                }
            }
            if let Some(max) = element["max_length"].as_u64() {
                if (len as u64) > max {
                    return Some(format!("Must be at most {max} characters."));
                }
            }
            None
        }
        _ => None,
    }
}

/// A text input's textarea when [`prepare_inputs`] created one, else a framed placeholder.
fn text_input(element: &Json, key: &str, kcx: &KitCx, cx: &App) -> AnyElement {
    if let Some(input) = kcx
        .kit
        .and_then(|kit| kit.read(cx).inputs.get(key).cloned())
    {
        let value = input.read(cx).value();
        let error = validate_text_input(element, &value);
        let danger = cx.theme().danger;
        return v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .when(error.is_some(), |el| {
                        el.rounded_md().border_1().border_color(danger)
                    })
                    .child(Textarea::new(&input)),
            )
            .when_some(error, |col, msg| {
                col.child(div().text_xs().text_color(danger).child(msg))
            })
            .into_any_element();
    }
    let theme = cx.theme();
    div()
        .w_full()
        .px_2()
        .py_1p5()
        .rounded_md()
        .border_1()
        .border_color(theme.input)
        .text_color(theme.muted_foreground)
        .child(
            text_of(&element["placeholder"])
                .unwrap_or_default()
                .to_owned(),
        )
        .into_any_element()
}

/// The image at `url`: a decoded fixture image, or live, the remote image loaded through the
/// app's HTTP client; a framed placeholder carrying the alt text while there is none (or it
/// fails to load). Images at least [`OPENABLE_IMAGE`] open the viewer on click when the
/// transcript provides [`KitCx::image_open`].
fn picture(
    url: Option<&str>,
    alt: &str,
    width: f32,
    height: f32,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let colors = (theme.border, theme.secondary, theme.muted_foreground);
    let placeholder = {
        let alt = alt.to_owned();
        move || picture_frame(&alt, width, height, colors).into_any_element()
    };
    let source = url.and_then(|url| match kcx.fixtures.block_images.get(url) {
        Some(image) => Some(ImageSource::Image(image.clone())),
        None if !kcx.fixtures.sample && url.starts_with("https://") => {
            Some(ImageSource::from(SharedString::from(url.to_owned())))
        }
        None => None,
    });
    let Some(source) = source else {
        return placeholder();
    };
    let image = div().w(px(width)).h(px(height)).child(
        crate::media::Picture::new(source)
            .fit(ObjectFit::Contain)
            // GPUI clips rectangularly: the picture rounds itself (a rounded wrapper does not
            // round its children).
            .radius(px(8.))
            .fallback(placeholder),
    );
    if let (Some(url), Some(on_open)) = (url, kcx.image_open.clone()) {
        if width >= OPENABLE_IMAGE {
            // Slack's `.p-file_image_thumbnail__wrapper`: 8px radius, a 1px border of the
            // foreground at 13%, and the zoom cursor.
            let origin = ImageOrigin::Url(url.to_owned());
            return div()
                .id(SharedString::from(format!("img-{}", kcx.text_key())))
                // Hug the image: a column would stretch the frame to its width.
                .self_start()
                .cursor_pointer()
                .rounded_lg()
                .overflow_hidden()
                .border_1()
                .border_color(theme.foreground.opacity(0.13))
                .on_click(move |_, window, cx| on_open(origin.clone(), window, cx))
                .child(image)
                .into_any_element();
        }
    }
    image.into_any_element()
}

/// Images at least this large (image blocks, unfurl thumbs, attachment previews) open the
/// viewer on click; the small context/accessory icons stay static.
const OPENABLE_IMAGE: f32 = 48.;

/// The framed stand-in for a missing image: an image glyph over the alt text.
fn picture_frame(alt: &str, width: f32, height: f32, (border, bg, fg): (Hsla, Hsla, Hsla)) -> Div {
    v_flex()
        .w(px(width))
        .h(px(height))
        .items_center()
        .justify_center()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_xs()
        .text_color(fg)
        .child(Icon::new(IconName::Image).text_color(fg).size(px(16.)))
        .when(height >= 48., |frame| frame.child(alt.to_owned()))
}

/// Resolve an image URL from either `image_url` or modern `slack_file.url` (`F-COLLAB-767`).
fn image_url_of(raw: &Json) -> Option<&str> {
    raw["image_url"]
        .as_str()
        .or_else(|| raw["slack_file"]["url"].as_str())
}

/// An `image` element (context, accessory) at `size` px square.
fn image_element(element: &Json, kcx: &KitCx, size: f32, cx: &App) -> AnyElement {
    let alt = element["alt_text"].as_str().unwrap_or_default();
    picture(image_url_of(element), alt, size, size, kcx, cx)
}

fn image_block(raw: &Json, kcx: &KitCx, cx: &App) -> Div {
    let alt = raw["alt_text"].as_str().unwrap_or_default();
    let key = format!(
        "{}-img-{}",
        kcx.ts.as_str(),
        raw["block_id"].as_str().unwrap_or_default()
    );
    let collapsed = kcx
        .kit
        .is_some_and(|kit| kit.read(cx).collapsed.contains(&key));
    let theme = cx.theme();
    // Slack pads an image block 4px above and below.
    v_flex()
        .gap_1()
        .py_1()
        .when_some(text_of(&raw["title"]), |column, title| {
            // Slack's title row: the muted title and a blue caret, which collapses the image
            // (and then points right).
            let row = h_flex()
                .id(SharedString::from(format!("img-title-{key}")))
                .gap(px(6.5))
                .items_center()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(emoji::replace_shortcodes(title))
                .child(div().text_xs().text_color(theme.link).child(if collapsed {
                    "\u{25B8}"
                } else {
                    "\u{25BE}"
                }));
            column.child(match kcx.kit.cloned() {
                Some(kit) => row.cursor_pointer().on_click(move |_, _, cx| {
                    kit.update(cx, |state, cx| {
                        if !state.collapsed.remove(&key) {
                            state.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    });
                }),
                None => row,
            })
        })
        // Slack shows the image at its own size (scaled into the thumbnail box), left-aligned.
        .when(!collapsed, |column| {
            let (width, height) =
                crate::files::fit(raw["image_width"].as_i64(), raw["image_height"].as_i64());
            column.child(picture(image_url_of(raw), alt, width, height, kcx, cx))
        })
}

/// Legacy attachments under the message body, if any.
pub fn attachments(message: &Message, kcx: &KitCx, cx: &App) -> Option<Div> {
    if message.attachments.is_empty() {
        return None;
    }
    let show_previews = crate::preferences::display_prefs(cx).link_previews;
    let cards: Vec<Div> = message
        .attachments
        .iter()
        .enumerate()
        .filter(|(n, attachment)| {
            if !show_previews && is_page_preview_attachment(attachment) {
                return false;
            }
            if is_page_preview_attachment(attachment) {
                let key = format!("{}-unfurl-{n}", kcx.ts.as_str());
                if kcx
                    .kit
                    .is_some_and(|kit| kit.read(cx).dismissed_unfurls.contains(&key))
                {
                    return false;
                }
            }
            true
        })
        .map(|(n, attachment)| attachment_card(attachment, n, kcx, cx))
        .collect();
    if cards.is_empty() {
        return None;
    }
    // Slack's cards sit about 6px under the text above them: the body's gap alone.
    Some(v_flex().gap_2().children(cards))
}

fn is_page_preview_attachment(attachment: &Attachment) -> bool {
    !attachment.is_msg_unfurl
        && attachment.from_url.is_some()
        && attachment.service_name.is_some()
        && attachment.color.is_none()
        && attachment
            .extra
            .get("is_app_unfurl")
            .and_then(|value| value.as_bool())
            != Some(true)
}

/// The color bar's color: `good` / `warning` / `danger`, a hex value, or the border color.
fn bar_color(color: Option<&str>, cx: &App) -> Hsla {
    let theme = cx.theme();
    match color {
        Some("good") => theme.success,
        Some("warning") => theme.warning,
        Some("danger") => theme.danger,
        Some(hex) => messages::hex_color(hex).unwrap_or(theme.border),
        None => theme.border,
    }
}

/// The attachment's own timestamp (`ts`, a number or a numeric string), in Unix seconds.
fn attachment_secs(attachment: &Attachment) -> Option<i64> {
    attachment.ts.as_ref().and_then(|ts| {
        ts.as_i64().or_else(|| {
            ts.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|secs| secs as i64)
        })
    })
}

/// `text` with Slack's link markup reduced to what it displays: `<url|label>` to `label`,
/// `<url>` to `url` (footers show links as plain muted text).
fn link_labels(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>').map(|close| open + close) else {
            break;
        };
        out.push_str(&rest[..open]);
        let inner = &rest[open + 1..close];
        out.push_str(inner.split_once('|').map_or(inner, |(_, label)| label));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// Slack's card for a shared message link (`is_msg_unfurl`): a bordered box with the author's
/// avatar, name and day, "Posted in #channel", then the message itself (its blocks when it
/// has them, else its mrkdwn text — never both, they carry the same words).
fn message_unfurl(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let fixtures = kcx.fixtures;
    let extra = |key: &str| attachment.extra.get(key).and_then(|value| value.as_str());
    let name = extra("author_subname")
        .or(attachment.author_name.as_deref())
        .unwrap_or_default()
        .to_owned();
    // The author's photo; for someone the workspace snapshot lacks, the card's own icon.
    let author = extra("author_id").map(UserId::from);
    let avatar = match (&author, attachment.author_icon.as_deref()) {
        (Some(user), Some(icon)) if fixtures.user(user).is_none() => Some(
            div()
                .size(px(36.))
                .flex_none()
                .rounded(px(7.))
                .overflow_hidden()
                .child(img(SharedString::from(icon.to_owned())).size_full()),
        ),
        (Some(user), _) => Some(avatars::user_chip(fixtures, user, 36.)),
        (None, _) => None,
    };
    let day = attachment_secs(attachment).map(messages::short_date);
    // Slack names where the shared message lives: "Thread in" for a thread's root, "From a
    // thread in" for a reply, else "Posted in", with the channel's icon and an "(archived)"
    // mark.
    let flag = |key: &str| attachment.extra.get(key).and_then(Json::as_bool) == Some(true);
    let place = if flag("is_thread_root_unfurl") {
        "Thread in"
    } else if flag("is_reply_unfurl") {
        "From a thread in"
    } else {
        "Posted in"
    };
    let posted_in = attachment
        .channel_id
        .as_ref()
        .and_then(|channel| Some((channel, fixtures.conversation(channel)?)))
        .filter(|(_, conversation)| conversation.name.is_some())
        .map(|(channel, conversation)| {
            let icon = if conversation.is_archived {
                IconName::Archive
            } else if conversation.kind() == ConversationKind::PrivateChannel {
                IconName::Lock
            } else {
                IconName::Hash
            };
            let name = conversation.name.clone().unwrap_or_default();
            // Like Slack, the channel the shared message lives in opens it.
            let channel = channel.clone();
            let links = kcx.links.clone();
            h_flex()
                .id(SharedString::from(format!(
                    "bk-{}-att-{n}-posted-in",
                    kcx.ts.as_str()
                )))
                .gap_1()
                .items_center()
                .text_size(px(13.))
                .text_color(muted)
                .cursor_pointer()
                .hover(|el| el.underline())
                .on_click(move |_, window, cx| {
                    messages::route_link(
                        links.as_ref(),
                        LinkTarget::Channel(channel.clone()),
                        window,
                        cx,
                    )
                })
                .child(place)
                .child(Icon::new(icon).text_color(muted).size(px(12.)))
                .child(if conversation.is_archived {
                    format!("{name} (archived)")
                } else {
                    name
                })
        });
    let block_ts = Ts::new(format!("{}-att{n}", kcx.ts.as_str()));
    let mut nested = kcx.nested(fixtures, kcx.kit, &block_ts);
    // A shared message's buttons belong to the original message, not this one.
    nested.on_action = None;
    let body: Vec<AnyElement> = if attachment.blocks.is_empty() {
        attachment
            .text
            .as_deref()
            .map(|text| messages::mrkdwn(text, &nested, false, false, cx))
            .map(IntoElement::into_any_element)
            .into_iter()
            .collect()
    } else {
        attachment
            .blocks
            .iter()
            .enumerate()
            .flat_map(|(ix, block)| messages::render_block(block, ix, &nested, false, cx))
            .collect()
    };
    v_flex()
        .max_w(px(640.))
        .min_w_0()
        .p_3()
        .gap_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        // Slack's header: the avatar (badged for another organization's member) beside two
        // 18px lines, name and day over where the message lives, together its height.
        .child(
            h_flex()
                .gap_2()
                .items_start()
                .children(avatar.map(|avatar| {
                    // Slack's card badge is smaller than a message's: ~12px on the 36px
                    // avatar, the size the helper gives a 28px one.
                    avatars::with_external_badge(
                        fixtures,
                        author.as_ref(),
                        avatar,
                        28.,
                        theme.background,
                    )
                }))
                .child(
                    v_flex()
                        .min_w_0()
                        .line_height(px(18.))
                        .child(
                            h_flex()
                                .gap_2()
                                .items_baseline()
                                .child(div().font_weight(FontWeight::BOLD).child(name))
                                .children(
                                    day.map(|day| div().text_xs().text_color(muted).child(day)),
                                ),
                        )
                        .children(posted_in),
                ),
        )
        .children(body)
}

/// Slack's card for a plain link preview (a page's title, description, site and image): a
/// bordered box with the text on the left and the image as a thumbnail on the right.
fn link_unfurl(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    let title = attachment.title.as_deref().map(|title| {
        let title = plain_label(title, cx);
        let url = attachment
            .title_link
            .clone()
            .or_else(|| attachment.from_url.clone());
        match url {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-link", kcx.ts.as_str()),
                title,
                url,
                kcx,
                FontWeight::BOLD,
                cx,
            ),
            None => div()
                .id(SharedString::from(format!(
                    "bk-{}-att-{n}-link",
                    kcx.ts.as_str()
                )))
                .font_weight(FontWeight::BOLD)
                .child(title),
        }
    });
    let description = attachment
        .text
        .as_deref()
        .map(|text| messages::mrkdwn(text, kcx, true, false, cx));
    let site = attachment.service_name.as_deref().map(|name| {
        h_flex()
            .gap_1()
            .items_center()
            .text_xs()
            .text_color(muted)
            .when_some(attachment.service_icon.clone(), |row, icon| {
                row.child(
                    img(SharedString::from(icon))
                        .flex_none()
                        .size(px(14.))
                        .rounded_sm(),
                )
            })
            .child(plain_label(name, cx))
    });
    let is_video_unfurl = attachment.extra.contains_key("video_html")
        || attachment.extra.contains_key("video_html_width");
    let open_url = attachment
        .title_link
        .clone()
        .or_else(|| attachment.from_url.clone());
    let thumb = attachment
        .image_url
        .as_deref()
        .or(attachment.thumb_url.as_deref())
        .map(|url| {
            let mut inner = div()
                .relative()
                .child(picture(Some(url), "", 88., 88., kcx, cx));
            if is_video_unfurl {
                let target = open_url.clone();
                let links = kcx.links.clone();
                inner = inner.child(
                    div()
                        .id(SharedString::from(format!(
                            "bk-{}-att-{n}-play",
                            kcx.ts.as_str()
                        )))
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(32.))
                                .rounded_full()
                                .bg(gpui_kit::gpui::hsla(0., 0., 0., 0.62))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Play)
                                        .size(px(14.))
                                        .text_color(gpui_kit::gpui::hsla(0., 0., 1., 0.95)),
                                ),
                        )
                        .when_some(target, |el, url| {
                            el.on_click(move |_, window, cx| {
                                messages::route_link(
                                    links.as_ref(),
                                    LinkTarget::Url(url.clone().into()),
                                    window,
                                    cx,
                                );
                            })
                        }),
                );
            }
            div()
                .flex_none()
                .p_2()
                .border_l_1()
                .border_color(border)
                .child(inner)
        });
    let dismiss = kcx.kit.cloned().map(|kit| {
        let key = format!("{}-unfurl-{n}", kcx.ts.as_str());
        let hover = theme.accent;
        let fg = theme.foreground;
        div()
            .id(SharedString::from(format!("bk-{key}-dismiss")))
            .absolute()
            .top_1p5()
            .right_1p5()
            .size(px(20.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .text_color(muted)
            .hover(move |b| b.bg(hover).text_color(fg))
            .child(Icon::new(IconName::X).size(px(12.)))
            .on_click(move |_, _, cx| {
                kit.update(cx, |state, cx| {
                    state.dismissed_unfurls.insert(key.clone());
                    cx.notify();
                });
            })
    });
    h_flex()
        .relative()
        .max_w(px(640.))
        .min_w_0()
        .items_stretch()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .p_3()
                .pr_7()
                .gap_1()
                .children(title)
                .children(description)
                .children(site),
        )
        .children(thumb)
        .children(dismiss)
}

fn attachment_card(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
    if attachment.is_msg_unfurl {
        return message_unfurl(attachment, n, kcx, cx);
    }
    // A page preview (no app behind it, no colour bar): Slack's link card.
    let is_page_preview = attachment.from_url.is_some()
        && attachment.service_name.is_some()
        && attachment.color.is_none()
        && attachment
            .extra
            .get("is_app_unfurl")
            .and_then(|value| value.as_bool())
            != Some(true);
    if is_page_preview {
        return link_unfurl(attachment, n, kcx, cx);
    }
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let fixtures = kcx.fixtures;
    let mut body = v_flex().flex_1().min_w_0().gap_1();
    if let Some(author) = &attachment.author_name {
        let author_text = plain_label(author, cx);
        let label = match &attachment.author_link {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-author", kcx.ts.as_str()),
                author_text,
                url.clone(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .text_xs()
            .into_any_element(),
            None => div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(author_text)
                .into_any_element(),
        };
        body = body.child(
            h_flex()
                .items_center()
                .gap_1p5()
                .when_some(attachment.author_icon.clone(), |row, icon| {
                    row.child(
                        img(SharedString::from(icon))
                            .flex_none()
                            .size(px(16.))
                            .rounded_sm(),
                    )
                })
                .child(label),
        );
    }
    if let Some(title) = &attachment.title {
        let title_text = plain_label(title, cx);
        body = body.child(match &attachment.title_link {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-title", kcx.ts.as_str()),
                title_text,
                url.clone(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .into_any_element(),
            None => div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title_text)
                .into_any_element(),
        });
    }
    // Slack shows an attachment's blocks instead of its text when it has both.
    if let Some(text) = attachment
        .text
        .as_ref()
        .filter(|_| attachment.blocks.is_empty())
    {
        // Slack collapses text of 700+ characters or 5+ line breaks behind "Show more";
        // static cards (no kit state to expand into) show it whole.
        let key = format!("{}-att-{n}", kcx.ts.as_str());
        let long = text.chars().count() >= 700 || text.matches('\n').count() >= 5;
        let expanded = kcx
            .kit
            .is_none_or(|kit| kit.read(cx).expanded.contains(&key));
        let head;
        let shown = if long && !expanded {
            head = collapsed(text);
            &head
        } else {
            text
        };
        body = body.child(messages::mrkdwn(shown, kcx, false, false, cx));
        if let Some(kit) = kcx.kit.filter(|_| long).cloned() {
            body = body.child(
                div()
                    .id(SharedString::from(format!("bk-{key}-more")))
                    .text_color(cx.theme().link)
                    .cursor_pointer()
                    .child(if expanded { "Show less" } else { "Show more" })
                    .on_click(move |_, _, cx| {
                        kit.update(cx, |state, cx| {
                            if !state.expanded.remove(&key) {
                                state.expanded.insert(key.clone());
                            }
                            cx.notify();
                        });
                    }),
            );
        }
    }
    // Short fields pair up in two columns; long fields take a full row.
    let mut fields = attachment.fields.iter().peekable();
    while let Some(field) = fields.next() {
        let cell = |field: &slackless_core::types::message::AttachmentField| {
            v_flex()
                .flex_1()
                .min_w_0()
                .when_some(field.title.as_deref(), |cell, title| {
                    cell.child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(plain_label(title, cx)),
                    )
                })
                .when_some(field.value.as_deref(), |cell, value| {
                    cell.child(messages::mrkdwn(value, kcx, false, false, cx))
                })
        };
        let mut row = h_flex().items_start().gap_4().child(cell(field));
        if field.short {
            if let Some(next) = fields.next_if(|next| next.short) {
                row = row.child(cell(next));
            }
        }
        body = body.child(row);
    }
    let block_ts = Ts::new(format!("{}-att{n}", kcx.ts.as_str()));
    let mut nested = kcx.nested(fixtures, kcx.kit, &block_ts);
    nested.attachment = attachment.id;
    for (ix, nested_block) in attachment.blocks.iter().enumerate() {
        body = body.children(messages::render_block(nested_block, ix, &nested, false, cx));
    }
    if let Some(url) = attachment.image_url.as_deref() {
        let img_w = attachment
            .extra
            .get("image_width")
            .and_then(serde_json::Value::as_i64);
        let img_h = attachment
            .extra
            .get("image_height")
            .and_then(serde_json::Value::as_i64);
        let (w, h) = if img_w.is_some() && img_h.is_some() {
            crate::files::fit(img_w, img_h)
        } else {
            (360., 200.)
        };
        body = body.child(picture(Some(url), "attachment image", w, h, kcx, cx));
    }
    // Legacy attachment fallback text (`F-COLLAB-655`) when rich fields are all absent.
    let empty_rich = attachment.title.is_none()
        && attachment.text.is_none()
        && attachment.pretext.is_none()
        && attachment.fields.is_empty()
        && attachment.blocks.is_empty()
        && attachment.image_url.is_none();
    if empty_rich {
        if let Some(fallback) = attachment
            .fallback
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            body = body.child(messages::mrkdwn(fallback, kcx, false, false, cx));
        }
    }
    // Slack's footer line: the footer icon, then "<footer> | Yesterday at 9:25 AM", its
    // links shown as their labels.
    let stamp = attachment_secs(attachment);
    if attachment.footer.is_some() || stamp.is_some() {
        let footer = [
            attachment
                .footer
                .as_deref()
                .map(|f| plain_label(&link_labels(f), cx)),
            stamp.map(|secs| messages::day_and_time(&Ts::new(secs.to_string()))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" | ");
        body = body.child(
            h_flex()
                .gap_1()
                .items_center()
                .text_xs()
                .text_color(muted)
                .when_some(attachment.footer_icon.clone(), |row, icon| {
                    row.child(
                        img(SharedString::from(icon))
                            .flex_none()
                            .size(px(16.))
                            .rounded_sm(),
                    )
                })
                .child(footer),
        );
    }

    // Pure block attachments without a color (such as picker GIFs) render flush-left with no
    // vertical color bar, matching official Slack.
    let has_bar = attachment.color.is_some()
        || attachment.blocks.is_empty()
        || attachment.author_name.is_some()
        || attachment.title.is_some()
        || attachment.text.is_some()
        || !attachment.fields.is_empty()
        || attachment.image_url.is_some()
        || attachment.footer.is_some();

    v_flex()
        .gap_1()
        .min_w_0()
        .when_some(attachment.pretext.as_deref(), |card, pretext| {
            card.child(messages::mrkdwn(pretext, kcx, false, false, cx))
        })
        .child(if has_bar {
            h_flex()
                .items_stretch()
                .gap_3()
                .min_w_0()
                .child(
                    div()
                        .flex_none()
                        .w(px(4.))
                        .rounded_full()
                        .bg(bar_color(attachment.color.as_deref(), cx)),
                )
                .child(body)
                .into_any_element()
        } else {
            body.into_any_element()
        })
}

/// Prepare text input controls for raw modal JSON blocks (`views.open`, `F-COLLAB-725`).
pub fn prepare_raw_inputs(
    kit: &Entity<BlockKitState>,
    ts: &Ts,
    raw_blocks: &[Json],
    window: &mut Window,
    cx: &mut App,
) {
    let synthetic = Message {
        ts: ts.clone(),
        blocks: raw_blocks
            .iter()
            .map(|raw| Block::Unknown {
                r#type: raw["type"].as_str().unwrap_or_default().to_owned(),
                raw: raw.clone(),
            })
            .collect(),
        ..Default::default()
    };
    prepare_inputs(kit, &synthetic, window, cx);
}

/// Render raw Block Kit JSON blocks (used by interactive app modals from `views.open`).
pub fn render_raw_blocks(raw_blocks: &[Json], kcx: &KitCx, cx: &App) -> Div {
    let mut column = v_flex().gap_3().w_full().min_w_0();
    for (ix, raw) in raw_blocks.iter().enumerate() {
        let r#type = raw["type"].as_str().unwrap_or_default();
        if let Some(el) = block(r#type, raw, ix, kcx, cx) {
            column = column.child(el);
        }
    }
    column
}

/// Collect Slack `state.values` from modal `input` blocks and validate required / typed inputs.
pub fn collect_modal_state(
    ts: &Ts,
    raw_blocks: &[Json],
    kit: &Entity<BlockKitState>,
    cx: &App,
) -> Result<Json, String> {
    let mut values = serde_json::Map::new();
    let state = kit.read(cx);
    for (ix, raw) in raw_blocks.iter().enumerate() {
        if raw["type"].as_str() != Some("input") {
            continue;
        }
        let element = &raw["element"];
        let kind = element["type"].as_str().unwrap_or_default();
        let block_id = raw["block_id"]
            .as_str()
            .map_or_else(|| ix.to_string(), str::to_owned);
        let action_id = element["action_id"]
            .as_str()
            .map_or_else(|| "0".to_owned(), str::to_owned);
        let optional = raw["optional"].as_bool().unwrap_or(false);
        let label = text_of(&raw["label"]).unwrap_or(&block_id);
        let key = control_key(ts, raw, ix, element, 0);

        let entry = if TEXT_INPUTS.contains(&kind) {
            let val = state
                .inputs
                .get(&key)
                .map(|i| i.read(cx).value().to_string())
                .or_else(|| element["initial_value"].as_str().map(str::to_owned))
                .unwrap_or_default();
            if !optional && val.trim().is_empty() {
                return Err(format!("{label} is required."));
            }
            if let Some(err) = validate_text_input(element, &val) {
                return Err(format!("{label}: {err}"));
            }
            serde_json::json!({ "type": kind, "value": val })
        } else if kind == "datepicker" {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| element["initial_date"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                return Err(format!("{label} is required."));
            }
            serde_json::json!({ "type": kind, "selected_date": val })
        } else if kind == "timepicker" {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| element["initial_time"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                return Err(format!("{label} is required."));
            }
            serde_json::json!({ "type": kind, "selected_time": val })
        } else if kind == "checkboxes" || kind.starts_with("multi_") {
            let selected = state.checked.get(&key).cloned().unwrap_or_default();
            if !optional && selected.is_empty() {
                return Err(format!("{label} is required."));
            }
            let arr: Vec<Json> = selected
                .into_iter()
                .map(|v| serde_json::json!({ "value": v }))
                .collect();
            serde_json::json!({ "type": kind, "selected_options": arr })
        } else {
            let val = state
                .choices
                .get(&key)
                .cloned()
                .or_else(|| {
                    element["initial_option"]["value"]
                        .as_str()
                        .map(str::to_owned)
                })
                .or_else(|| element["initial_user"].as_str().map(str::to_owned))
                .or_else(|| element["initial_conversation"].as_str().map(str::to_owned))
                .or_else(|| element["initial_channel"].as_str().map(str::to_owned));
            if !optional && val.is_none() {
                return Err(format!("{label} is required."));
            }
            match kind {
                "users_select" => serde_json::json!({ "type": kind, "selected_user": val }),
                "conversations_select" => {
                    serde_json::json!({ "type": kind, "selected_conversation": val })
                }
                "channels_select" => serde_json::json!({ "type": kind, "selected_channel": val }),
                _ => serde_json::json!({
                    "type": kind,
                    "selected_option": val.map(|v| serde_json::json!({ "value": v })),
                }),
            }
        };

        let block_map = values
            .entry(block_id)
            .or_insert_with(|| Json::Object(serde_json::Map::new()));
        if let Some(obj) = block_map.as_object_mut() {
            obj.insert(action_id, entry);
        }
    }
    Ok(Json::Object(values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;
    use serde_json::json;

    #[test]
    fn validates_email_url_number_and_plain_text_inputs() {
        let email = json!({ "type": "email_text_input" });
        assert!(validate_text_input(&email, "").is_none());
        assert!(validate_text_input(&email, "alice@example.com").is_none());
        assert!(validate_text_input(&email, "not-an-email").is_some());

        let url = json!({ "type": "url_text_input" });
        assert!(validate_text_input(&url, "https://slack.com/app").is_none());
        assert!(validate_text_input(&url, "ftp://bad").is_some());

        let int_num = json!({ "type": "number_input", "is_decimal_allowed": false, "min_value": "1", "max_value": "10" });
        assert!(validate_text_input(&int_num, "5").is_none());
        assert!(validate_text_input(&int_num, "5.5").is_some());
        assert!(validate_text_input(&int_num, "0").is_some());
        assert!(validate_text_input(&int_num, "11").is_some());

        let plain = json!({ "type": "plain_text_input", "min_length": 3, "max_length": 5 });
        assert!(validate_text_input(&plain, "hi").is_some());
        assert!(validate_text_input(&plain, "hey").is_none());
        assert!(validate_text_input(&plain, "toolong").is_some());
    }

    #[test]
    fn converts_standard_markdown_to_mrkdwn_preserving_code() {
        let md = "### Deploy Summary\n\
                  Status: **green** — see [dashboard](https://grafana.example) and ~~old~~ `**keep** [a](b)`\n\
                  ```\n\
                  **verbatim** [link](https://example.com)\n\
                  ```";
        let out = standard_markdown_to_mrkdwn(md);
        assert_eq!(
            out,
            "*Deploy Summary*\n\
             Status: *green* — see <https://grafana.example|dashboard> and ~old~ `**keep** [a](b)`\n\
             ```\n\
             **verbatim** [link](https://example.com)\n\
             ```"
        );
    }

    #[gpui::test]
    fn renders_all_modern_block_kit_blocks_without_panicking(cx: &mut gpui::TestAppContext) {
        use gpui::{Render, Window};

        struct BlockHost {
            fixtures: std::sync::Arc<Fixtures>,
            kit: Entity<BlockKitState>,
            blocks: Vec<Json>,
        }
        impl Render for BlockHost {
            fn render(
                &mut self,
                window: &mut Window,
                cx: &mut gpui::Context<Self>,
            ) -> impl IntoElement {
                let ts = Ts::from("1700000001.000100");
                prepare_raw_inputs(&self.kit, &ts, &self.blocks, window, cx);
                let kcx = KitCx::new(&self.fixtures, Some(&self.kit), &ts);
                div()
                    .size_full()
                    .p_4()
                    .child(render_raw_blocks(&self.blocks, &kcx, cx))
            }
        }

        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let blocks = vec![
            json!({ "type": "markdown", "text": "## Summary\nAll **checks** passed on [CI](https://ci.example)." }),
            json!({ "type": "alert", "style": "warning", "title": { "type": "plain_text", "text": "Heads up" }, "text": { "type": "mrkdwn", "text": "Deploy window opens at 14:00." } }),
            json!({ "type": "callout", "severity": "danger", "text": "Incident active" }),
            json!({
                "type": "card",
                "title": { "type": "plain_text", "text": "Service API" },
                "subtitle": "Production",
                "fields": [
                    { "type": "mrkdwn", "text": "*Latency*\n12ms" },
                    { "type": "mrkdwn", "text": "*Errors*\n0.01%" }
                ],
                "actions": [
                    { "type": "button", "action_id": "open_runbook", "text": { "type": "plain_text", "text": "Runbook" }, "value": "rb" }
                ]
            }),
            json!({
                "type": "carousel",
                "elements": [
                    { "type": "card", "title": "Slide 1", "subtitle": "First" },
                    { "type": "card", "title": "Slide 2", "subtitle": "Second" }
                ]
            }),
            json!({
                "type": "video",
                "title": { "type": "plain_text", "text": "Release walkthrough" },
                "title_url": "https://video.example/1",
                "description": { "type": "plain_text", "text": "Demo of v2.4" },
                "provider_name": "Loom"
            }),
            json!({
                "type": "call",
                "call": {
                    "title": "Standup",
                    "active_participants": [{ "slack_id": "U0ALICE" }],
                    "join_url": "https://call.example/join"
                }
            }),
            json!({ "type": "file", "external_id": "report-2026.csv" }),
            json!({
                "type": "rich_text",
                "elements": [
                    { "type": "rich_text_section", "elements": [{ "type": "text", "text": "Nested rich text" }] }
                ]
            }),
            json!({
                "type": "context_actions",
                "block_id": "ctx_act_1",
                "elements": [
                    {
                        "type": "feedback_buttons",
                        "action_id": "fb_1",
                        "positive_button": { "text": { "type": "plain_text", "text": "Helpful" }, "value": "good" },
                        "negative_button": { "text": { "type": "plain_text", "text": "Not helpful" }, "value": "bad" }
                    },
                    {
                        "type": "icon_button",
                        "action_id": "del_1",
                        "text": { "type": "plain_text", "text": "Dismiss" },
                        "value": "dismiss"
                    }
                ]
            }),
            json!({
                "type": "task_card",
                "task_id": "task_1",
                "title": "Run test suite",
                "status": "complete",
                "details": { "type": "mrkdwn", "text": "Executed *155* unit tests." },
                "sources": [{ "type": "url", "url": "https://ci.example/run/1", "text": "CI log" }]
            }),
            json!({
                "type": "plan",
                "title": "Release checklist",
                "tasks": [
                    { "task_id": "t1", "title": "Build macOS bundle", "status": "complete" },
                    { "task_id": "t2", "title": "Publish release notes", "status": "in_progress" },
                    { "task_id": "t3", "title": "Announce in #general", "status": "pending" }
                ]
            }),
            json!({
                "type": "data_table",
                "columns": [
                    { "key": "service", "label": "Service" },
                    { "key": "p99", "label": "p99" }
                ],
                "rows": [
                    { "service": "api", "p99": "12ms" },
                    { "service": "search", "p99": "8ms" }
                ]
            }),
            json!({
                "type": "context",
                "elements": [
                    { "type": "tag", "label": "Production", "color": "green" },
                    { "type": "citation", "title": "Runbook", "url": "https://docs.example/rb" },
                    { "type": "mrkdwn", "text": "Literal https://example.com stays unlinked", "verbatim": true }
                ]
            }),
            json!({
                "type": "actions",
                "block_id": "wf_actions",
                "elements": [
                    {
                        "type": "workflow_button",
                        "text": { "type": "plain_text", "text": "Start deploy" },
                        "style": "primary",
                        "accessibility_label": "Trigger production deploy workflow",
                        "workflow": { "trigger": { "url": "https://slack.com/shortcuts/Ft123" } },
                        "confirm": {
                            "title": { "type": "plain_text", "text": "Deploy now?" },
                            "text": { "type": "mrkdwn", "text": "This starts the rollout." },
                            "confirm": { "type": "plain_text", "text": "Deploy" },
                            "deny": { "type": "plain_text", "text": "Cancel" },
                            "style": "danger"
                        }
                    },
                    {
                        "type": "datetimepicker",
                        "action_id": "dt_1",
                        "initial_date_time": 1_700_000_000
                    }
                ]
            }),
            json!({
                "type": "image",
                "alt_text": "Slack file image",
                "slack_file": { "url": "https://files.slack.com/files-pri/T1-F1/pic.png" }
            }),
            json!({
                "type": "input",
                "block_id": "notes_blk",
                "label": { "type": "plain_text", "text": "Notes" },
                "element": { "type": "plain_text_input", "action_id": "notes_act", "initial_value": "Ready" }
            }),
        ];
        let (_view, cx) = cx.add_window_view(|_, cx| BlockHost {
            fixtures: std::sync::Arc::new(crate::fixtures::load()),
            kit: cx.new(|_| BlockKitState::default()),
            blocks,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
    }
}
