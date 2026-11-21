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

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
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
}

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

/// A text object rendered by its type: mrkdwn through the shared renderer, plain_text with
/// emoji shortcodes substituted.
fn text_object(object: &Json, kcx: &KitCx, muted: bool, cx: &App) -> Option<AnyElement> {
    let text = text_of(object)?;
    if object["type"].as_str() == Some("mrkdwn") {
        return Some(messages::mrkdwn(text, kcx, muted, false, cx).into_any_element());
    }
    Some(
        div()
            .child(emoji::replace_shortcodes(text))
            .into_any_element(),
    )
}

/// One top-level (or attachment) block that is not `rich_text`.
pub fn block(kind: &str, raw: &Json, ix: usize, kcx: &KitCx, cx: &App) -> Option<AnyElement> {
    let theme = cx.theme();
    let element = match kind {
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
            .child(emoji::replace_shortcodes(text_of(&raw["text"])?))
            .into_any_element(),
        "image" => image_block(raw, kcx, cx).into_any_element(),
        "context" => h_flex()
            .flex_wrap()
            .items_center()
            .gap_1p5()
            .text_xs()
            .text_color(theme.muted_foreground)
            .children(raw["elements"].as_array()?.iter().filter_map(|element| {
                match element["type"].as_str()? {
                    "image" => Some(image_element(element, kcx, 16., cx)),
                    _ => text_object(element, kcx, true, cx),
                }
            }))
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
            },
        })
    }
}

/// One interactive (or image) element of `block` (its `actions`, accessory or `input`).
fn block_element(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> AnyElement {
    let kind = element["type"].as_str().unwrap_or_default();
    match kind {
        "button" => button(element, block, key, kcx).into_any_element(),
        "static_select" | "external_select" | "multi_static_select" | "multi_external_select" => {
            select(element, key, options(element), kcx, cx)
        }
        "users_select" | "multi_users_select" => {
            let users = kcx
                .fixtures
                .users
                .iter()
                .map(|user| (user.display_name().to_owned(), user.id.to_string()))
                .collect();
            select(element, key, users, kcx, cx)
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
            select(element, key, channels, kcx, cx)
        }
        "overflow" => overflow(element, key).into_any_element(),
        "datepicker" => {
            let days = (0..14)
                .map(|day| messages::iso_date(messages::now_secs() + day * 86_400))
                .map(|date| (date.clone(), date))
                .collect();
            picker(
                element,
                key,
                "initial_date",
                IconName::Calendar,
                days,
                kcx,
                cx,
            )
        }
        "timepicker" => {
            let times = (8..=18)
                .map(|hour| format!("{hour:02}:00"))
                .map(|t| (t.clone(), t))
                .collect();
            picker(
                element,
                key,
                "initial_time",
                IconName::Clock,
                times,
                kcx,
                cx,
            )
        }
        "checkboxes" => checkboxes(element, key, kcx, cx).into_any_element(),
        "radio_buttons" => radios(element, key, kcx, cx).into_any_element(),
        "image" => image_element(element, kcx, 20., cx),
        kind if TEXT_INPUTS.contains(&kind) => text_input(element, &key, kcx, cx),
        other => div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format!("[{other}]"))
            .into_any_element(),
    }
}

/// A button: a link button opens its URL; any other press goes to the message's action
/// handler with the element and its `block_id` (logged when there is none).
fn button(element: &Json, block: &Json, key: String, kcx: &KitCx) -> Button {
    let label = emoji::replace_shortcodes(text_of(&element["text"]).unwrap_or("Button"));
    let url = element["url"].as_str().map(str::to_owned);
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
        .on_click(move |_, window, cx| match (&url, &on_action) {
            (Some(url), _) => cx.open_url(url),
            (None, Some(on_action)) => {
                let action = actions::MessageAction::BlockAction {
                    action: action.clone(),
                    attachment,
                };
                on_action(action, window, cx);
            }
            (None, None) => log_button(&action, window, cx),
        })
}

/// A select menu: an outlined button showing the choice, opening a checked option list.
fn select(
    element: &Json,
    key: String,
    options: Vec<(String, String)>,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let initial = element["initial_option"]["value"]
        .as_str()
        .or_else(|| element["initial_user"].as_str())
        .or_else(|| element["initial_conversation"].as_str())
        .or_else(|| element["initial_channel"].as_str())
        .map(str::to_owned);
    let selected = choice(kcx, &key, cx).or(initial);
    let placeholder = text_of(&element["placeholder"])
        .unwrap_or("Select an item")
        .to_owned();
    let label = selected
        .as_ref()
        .and_then(|value| options.iter().find(|(_, option)| option == value))
        .map_or(placeholder, |(label, _)| label.clone());
    dropdown(key, label, None, options, selected, kcx.kit.cloned()).into_any_element()
}

/// Date / time pickers: the chosen value (or `initial_field`) on a button with `icon`, the
/// next values in a checked menu.
fn picker(
    element: &Json,
    key: String,
    initial_field: &str,
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
    dropdown(key, label, Some(icon), options, selected, kcx.kit.cloned()).into_any_element()
}

fn dropdown(
    key: String,
    label: String,
    icon: Option<IconName>,
    options: Vec<(String, String)>,
    selected: Option<String>,
    kit: Option<Entity<BlockKitState>>,
) -> impl IntoElement {
    Button::new(SharedString::from(format!("bk-{key}")))
        .outline()
        .with_size(Size::Small)
        .when_some(icon, |button, icon| button.icon(icon))
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |mut menu, _, _| {
            for (text, value) in &options {
                let (kit, key, value) = (kit.clone(), key.clone(), value.clone());
                let detail = format!("selected \u{201C}{text}\u{201D}");
                menu = menu.item(
                    PopupMenuItem::new(text.clone())
                        .checked(selected.as_ref() == Some(&value))
                        .on_click(move |_, window, cx| {
                            set_choice(kit.as_ref(), &key, &value, cx);
                            log_action(&key, &detail, window, cx);
                        }),
                );
            }
            menu
        })
}

/// The `...` overflow menu: each option is an action, not a selection.
fn overflow(element: &Json, key: String) -> impl IntoElement {
    let options = options(element);
    Button::new(SharedString::from(format!("bk-{key}")))
        .ghost()
        .with_size(Size::Small)
        .icon(IconName::Ellipsis)
        .dropdown_menu(move |mut menu, _, _| {
            for (text, value) in &options {
                let key = key.clone();
                let detail = format!("chose \u{201C}{text}\u{201D} (value {value})");
                menu = menu.item(
                    PopupMenuItem::new(text.clone())
                        .on_click(move |_, window, cx| log_action(&key, &detail, window, cx)),
                );
            }
            menu
        })
}

fn checkboxes(element: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
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
    v_flex()
        .gap_1p5()
        .children(options(element).into_iter().map(|(text, value)| {
            let is_checked = checked.contains(&value);
            let (kit, key, initial) = (kcx.kit.cloned(), key.clone(), initial.clone());
            Checkbox::new(SharedString::from(format!("bk-{key}-{value}")))
                .label(text.clone())
                .checked(is_checked)
                .on_click(move |now: &bool, window, cx| {
                    if let Some(kit) = &kit {
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
                        });
                    }
                    let verb = if *now { "checked" } else { "unchecked" };
                    log_action(&key, &format!("{verb} \u{201C}{text}\u{201D}"), window, cx);
                })
        }))
}

fn radios(element: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
    let selected = choice(kcx, &key, cx).or_else(|| {
        element["initial_option"]["value"]
            .as_str()
            .map(str::to_owned)
    });
    v_flex()
        .gap_1p5()
        .children(options(element).into_iter().map(|(text, value)| {
            let checked = selected.as_ref() == Some(&value);
            let (kit, key) = (kcx.kit.cloned(), key.clone());
            Radio::new(SharedString::from(format!("bk-{key}-{value}")))
                .label(text.clone())
                .checked(checked)
                .on_click(move |_, window, cx| {
                    set_choice(kit.as_ref(), &key, &value, cx);
                    log_action(&key, &format!("chose \u{201C}{text}\u{201D}"), window, cx);
                })
        }))
}

/// A text input's textarea when [`prepare_inputs`] created one, else a framed placeholder.
fn text_input(element: &Json, key: &str, kcx: &KitCx, cx: &App) -> AnyElement {
    if let Some(input) = kcx
        .kit
        .and_then(|kit| kit.read(cx).inputs.get(key).cloned())
    {
        return div()
            .w_full()
            .child(Textarea::new(&input))
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

/// An `image` element (context, accessory) at `size` px square.
fn image_element(element: &Json, kcx: &KitCx, size: f32, cx: &App) -> AnyElement {
    let alt = element["alt_text"].as_str().unwrap_or_default();
    picture(element["image_url"].as_str(), alt, size, size, kcx, cx)
}

fn image_block(raw: &Json, kcx: &KitCx, cx: &App) -> Div {
    let alt = raw["alt_text"].as_str().unwrap_or_default();
    v_flex()
        .gap_1()
        .when_some(text_of(&raw["title"]), |column, title| {
            column.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(emoji::replace_shortcodes(title)),
            )
        })
        // Slack shows the image at its own size (scaled into the thumbnail box), left-aligned.
        .child({
            let (width, height) =
                crate::files::fit(raw["image_width"].as_i64(), raw["image_height"].as_i64());
            picture(raw["image_url"].as_str(), alt, width, height, kcx, cx)
        })
}

/// Legacy attachments under the message body, if any.
pub fn attachments(message: &Message, kcx: &KitCx, cx: &App) -> Option<Div> {
    if message.attachments.is_empty() {
        return None;
    }
    Some(
        v_flex().gap_2().pt_1().children(
            message
                .attachments
                .iter()
                .enumerate()
                .map(|(n, attachment)| attachment_card(attachment, n, kcx, cx)),
        ),
    )
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
    let day =
        attachment_secs(attachment).map(|secs| messages::list_stamp(&Ts::new(secs.to_string())));
    // Slack names where the shared message lives: "Thread in" for a thread's root, else
    // "Posted in", with the channel's icon and an "(archived)" mark.
    let thread_root = attachment
        .extra
        .get("is_thread_root_unfurl")
        .and_then(|value| value.as_bool())
        == Some(true);
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
                .text_sm()
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
                .child(if thread_root {
                    "Thread in"
                } else {
                    "Posted in"
                })
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
        .gap_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .child(
            h_flex().gap_2().items_center().children(avatar).child(
                v_flex()
                    .min_w_0()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_baseline()
                            .child(div().font_weight(FontWeight::BOLD).child(name))
                            .children(day.map(|day| div().text_xs().text_color(muted).child(day))),
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
    let title = attachment.title.clone().map(|title| {
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
    let site = attachment.service_name.clone().map(|name| {
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
            .child(name)
    });
    let thumb = attachment
        .image_url
        .as_deref()
        .or(attachment.thumb_url.as_deref())
        .map(|url| {
            div()
                .flex_none()
                .p_2()
                .border_l_1()
                .border_color(border)
                .child(picture(Some(url), "", 88., 88., kcx, cx))
        });
    h_flex()
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
                .gap_1()
                .children(title)
                .children(description)
                .children(site),
        )
        .children(thumb)
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
        body = body.child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .child(author.clone()),
        );
    }
    if let Some(title) = &attachment.title {
        body = body.child(match &attachment.title_link {
            Some(url) => routed_link(
                format!("bk-{}-att-{n}-title", kcx.ts.as_str()),
                title.clone(),
                url.clone(),
                kcx,
                FontWeight::SEMIBOLD,
                cx,
            )
            .into_any_element(),
            None => div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.clone())
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
                .when_some(field.title.clone(), |cell, title| {
                    cell.child(div().font_weight(FontWeight::BOLD).child(title))
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
        body = body.child(picture(Some(url), "attachment image", 360., 160., kcx, cx));
    }
    // Slack's footer line: the footer icon, then "<footer> | Yesterday at 9:25 AM", its
    // links shown as their labels.
    let stamp = attachment_secs(attachment);
    if attachment.footer.is_some() || stamp.is_some() {
        let footer = [
            attachment.footer.as_deref().map(link_labels),
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

    v_flex()
        .gap_1()
        .min_w_0()
        .when_some(attachment.pretext.as_deref(), |card, pretext| {
            card.child(messages::mrkdwn(pretext, kcx, false, false, cx))
        })
        .child(
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
                .child(body),
        )
}
