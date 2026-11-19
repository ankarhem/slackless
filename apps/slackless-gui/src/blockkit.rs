//! Block Kit rendering: the app-authored blocks a client must display, and legacy
//! attachments.
//!
//! `slackless_types` models only `rich_text` (rendered by `messages.rs`); every other block
//! arrives as `Block::Unknown { r#type, raw }` with its JSON preserved, which this module
//! renders: `section` (text, `fields`, `accessory`), `divider`, `image`, `context`, `header`,
//! `actions`, and `input`. Interactive elements are real gpui-kit widgets: `Button`s (primary /
//! danger / link styles), dropdown menus for `static_select` / `users_select` /
//! `conversations_select` / `overflow` / `datepicker` / `timepicker`, `Checkbox` and `Radio`
//! groups, and `Textarea`s for text inputs. Fixture mode has no app backend, so an
//! interaction updates the local [`BlockKitState`] (selection, checks, drafts) and is logged
//! to stderr with a toast instead of sending `block_actions`.
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
use gpui_kit::component::link::Link;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::radio::Radio;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, App, AppContext as _, Div, Entity, FontWeight, Hsla, ImageSource, IntoElement,
    ObjectFit, ParentElement as _, SharedString, Styled as _, StyledImage as _, Window, div, img,
    px,
};
use slackless_core::types::{Attachment, Block, Message, Ts};

use crate::actions;
use crate::emoji;
use crate::fixtures::Fixtures;
use crate::messages;

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
}

/// What one message's blocks render with.
pub struct KitCx<'a> {
    pub fixtures: &'a Fixtures,
    /// Interactive state; `None` renders controls from the payload's initial values only.
    pub kit: Option<&'a Entity<BlockKitState>>,
    pub ts: &'a Ts,
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

/// The `text` of a text object.
fn text_of(object: &Json) -> Option<&str> {
    object["text"].as_str()
}

/// A text object rendered by its type: mrkdwn through the shared renderer, plain_text with
/// emoji shortcodes substituted.
fn text_object(object: &Json, kcx: &KitCx, muted: bool, cx: &App) -> Option<AnyElement> {
    let text = text_of(object)?;
    if object["type"].as_str() == Some("mrkdwn") {
        return Some(messages::mrkdwn(text, kcx.fixtures, muted, false, cx).into_any_element());
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
                        block_element(element, key, kcx, cx)
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
            block_element(element, key, kcx, cx)
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
        .child(block_element(element, key, kcx, cx))
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

/// One interactive (or image) element inside `actions`, an accessory or an `input`.
fn block_element(element: &Json, key: String, kcx: &KitCx, cx: &App) -> AnyElement {
    let kind = element["type"].as_str().unwrap_or_default();
    match kind {
        "button" => button(element, key).into_any_element(),
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

fn button(element: &Json, key: String) -> Button {
    let label = emoji::replace_shortcodes(text_of(&element["text"]).unwrap_or("Button"));
    let url = element["url"].as_str().map(str::to_owned);
    let value = element["value"].as_str().unwrap_or_default().to_owned();
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
        .on_click(move |_, window, cx| match &url {
            Some(url) => cx.open_url(url),
            None => log_action(
                &key,
                &format!("pressed \u{201C}{label}\u{201D} (value {value})"),
                window,
                cx,
            ),
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

/// A decoded fixture image for `url`, or a framed placeholder carrying the alt text.
fn picture(
    url: Option<&str>,
    alt: &str,
    width: f32,
    height: f32,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    if let Some(image) = url.and_then(|url| kcx.fixtures.block_images.get(url)) {
        return img(ImageSource::Image(image.clone()))
            .w(px(width))
            .h(px(height))
            .object_fit(ObjectFit::Contain)
            .rounded_md()
            .into_any_element();
    }
    let theme = cx.theme();
    v_flex()
        .w(px(width))
        .h(px(height))
        .items_center()
        .justify_center()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(
            Icon::new(IconName::Image)
                .text_color(theme.muted_foreground)
                .size(px(16.)),
        )
        .when(height >= 48., |frame| frame.child(alt.to_owned()))
        .into_any_element()
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
        .child(picture(raw["image_url"].as_str(), alt, 400., 172., kcx, cx))
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

fn attachment_card(attachment: &Attachment, n: usize, kcx: &KitCx, cx: &App) -> Div {
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
            Some(url) => div().font_weight(FontWeight::SEMIBOLD).child(
                Link::new(SharedString::from(format!(
                    "bk-{}-att-{n}-title",
                    kcx.ts.as_str()
                )))
                .href(url.clone())
                .child(title.clone()),
            ),
            None => div().font_weight(FontWeight::SEMIBOLD).child(title.clone()),
        });
    }
    if let Some(text) = &attachment.text {
        body = body.child(messages::mrkdwn(text, fixtures, false, false, cx));
    }
    // Short fields pair up in two columns; long fields take a full row.
    let mut fields = attachment.fields.iter().peekable();
    while let Some(field) = fields.next() {
        let cell = |field: &slackless_core::types::message::AttachmentField| {
            v_flex()
                .flex_1()
                .min_w_0()
                .when_some(field.title.clone(), |cell, title| {
                    cell.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(title),
                    )
                })
                .when_some(field.value.as_deref(), |cell, value| {
                    cell.child(messages::mrkdwn(value, fixtures, false, false, cx))
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
    let nested = KitCx {
        fixtures,
        kit: kcx.kit,
        ts: &block_ts,
    };
    for (ix, nested_block) in attachment.blocks.iter().enumerate() {
        body = body.children(messages::render_block(nested_block, ix, &nested, false, cx));
    }
    if let Some(url) = attachment.image_url.as_deref() {
        body = body.child(picture(Some(url), "attachment image", 360., 160., kcx, cx));
    }
    let stamp = attachment.ts.as_ref().and_then(|ts| {
        ts.as_i64().or_else(|| {
            ts.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|secs| secs as i64)
        })
    });
    if attachment.footer.is_some() || stamp.is_some() {
        let footer = [
            attachment.footer.clone(),
            stamp.map(|secs| messages::list_stamp(&Ts::new(secs.to_string()))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" \u{00B7} ");
        body = body.child(div().text_xs().text_color(muted).child(footer));
    }

    v_flex()
        .gap_1()
        .min_w_0()
        .when_some(attachment.pretext.as_deref(), |card, pretext| {
            card.child(messages::mrkdwn(pretext, fixtures, false, false, cx))
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
