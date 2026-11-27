//! Interactive Block Kit elements, pickers, inputs, image blocks, and action dispatch.

mod buttons;
mod inputs;
mod pickers;
mod selects;

use super::*;

pub(super) use buttons::{button, overflow, workflow_button};
pub(super) use inputs::{
    file_input_element, rich_text_input_value, text_input, validate_text_input,
};
pub(super) use pickers::{PickerSpec, datepicker, datetimepicker, picker, timepicker_slots};
#[cfg(test)]
pub(super) use selects::rich_options;
pub(super) use selects::{checkboxes, external_select, filtered_conversations, radios, select};

/// Fixture mode has no app to receive `block_actions`: log the interaction and toast it.
pub(super) fn log_action(key: &str, detail: &str, window: &mut Window, cx: &mut App) {
    tracing::warn!("slackless: block action {key}: {detail}");
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
pub(super) fn action_base(element: &Json, block: &Json) -> Json {
    let mut action = element.clone();
    if let Some(block_id) = block["block_id"].as_str() {
        action["block_id"] = block_id.into();
    }
    action
}

/// Run `proceed` immediately unless `confirm` is a Slack Block Kit confirmation dialog object
/// (`F-COLLAB-765`, `F-COLLAB-766`), in which case show an alert dialog with Slack's title,
/// body, confirm/deny labels, and `danger`/`primary` button variant first.
pub(super) fn maybe_confirm(
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
pub(super) fn dispatch_action(
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
pub(super) fn block_element(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let kind = element["type"].as_str().unwrap_or_default();
    match kind {
        "button" => button(element, block, key, kcx).into_any_element(),
        "workflow_button" => workflow_button(element, block, key, kcx, cx).into_any_element(),
        "static_select" | "multi_static_select" => {
            select(element, block, key, options(element), kcx, cx)
        }
        "external_select" | "multi_external_select" => {
            external_select(element, block, key, kcx, cx)
        }
        "users_select" | "multi_users_select" => {
            let users = kcx
                .fixtures
                .people()
                .map(|user| (user.display_name().to_owned(), user.id.to_string()))
                .collect();
            select(element, block, key, users, kcx, cx)
        }
        "conversations_select"
        | "channels_select"
        | "multi_conversations_select"
        | "multi_channels_select" => {
            let channels = filtered_conversations(element, kind, kcx.fixtures);
            select(element, block, key, channels, kcx, cx)
        }
        "feedback_buttons" => {
            let mut row = h_flex().items_center().gap_1p5();
            for (sub_ix, field) in ["positive_button", "negative_button"].iter().enumerate() {
                if let Some(btn) = element.get(*field).filter(|v| v.is_object()) {
                    let mut synthetic = btn.clone();
                    synthetic["type"] = "button".into();
                    if synthetic["action_id"].is_null() {
                        synthetic["action_id"] = element["action_id"].clone();
                    }
                    row = row.child(button(&synthetic, block, format!("{key}-fb{sub_ix}"), kcx));
                }
            }
            row.into_any_element()
        }
        "icon_button" => {
            let mut synthetic = element.clone();
            synthetic["type"] = "button".into();
            if synthetic["text"].is_null() {
                let lbl = element["accessibility_label"]
                    .as_str()
                    .or_else(|| element["icon"].as_str())
                    .unwrap_or("Action");
                synthetic["text"] = serde_json::json!({ "type": "plain_text", "text": lbl });
            }
            button(&synthetic, block, key, kcx).into_any_element()
        }
        "file_input" => file_input_element(element, &key, kcx, cx),
        "overflow" => overflow(element, block, key, kcx).into_any_element(),
        "datepicker" => datepicker(element, block, key, kcx, cx),
        "datetimepicker" => datetimepicker(element, block, key, kcx, cx),
        "timepicker" => picker(
            PickerSpec {
                element,
                block,
                key,
                initial_field: "initial_time",
                selected_field: "selected_time",
                icon: IconName::Clock,
                options: timepicker_slots(),
            },
            kcx,
            cx,
        ),
        "checkboxes" => checkboxes(element, block, key, kcx, cx).into_any_element(),
        "radio_buttons" => radios(element, block, key, kcx, cx).into_any_element(),
        "image" => image_element(element, kcx, 20., cx),
        "tag" => tag_element(element, cx).into_any_element(),
        "citation" => citation_element(element, 0, kcx, cx),
        "color" => color_element(element, cx).into_any_element(),
        "file" => file_block(element, cx).into_any_element(),
        "canvas" | "canvas_unfurl" => canvas_element(element, 0, kcx, cx).into_any_element(),
        "list_record" | "slack_list_record" => {
            list_record_element(element, 0, kcx, cx).into_any_element()
        }
        "work_object" => work_object_element(element, 0, kcx, cx),
        "workflow" => workflow_link_element(element, 0, kcx, cx),
        "url_source" => url_source_element(element, 0, kcx, cx),
        kind if TEXT_INPUTS.contains(&kind) => text_input(element, block, &key, kcx, cx),
        other => div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format!("[{other}]"))
            .into_any_element(),
    }
}

/// The image at `url`: a decoded fixture image, or live, the remote image loaded through the
/// app's HTTP client; a framed placeholder carrying the alt text while there is none (or it
/// fails to load). Images at least [`OPENABLE_IMAGE`] open the viewer on click when the
/// transcript provides [`KitCx::image_open`].
pub(super) fn picture(
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
    if let (Some(url), Some(on_open)) = (url, kcx.image_open.clone())
        && width >= OPENABLE_IMAGE
    {
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
    image.into_any_element()
}

/// Images at least this large (image blocks, unfurl thumbs, attachment previews) open the
/// viewer on click; the small context/accessory icons stay static.
const OPENABLE_IMAGE: f32 = 48.;

/// The framed stand-in for a missing image: an image glyph over the alt text.
pub(super) fn picture_frame(
    alt: &str,
    width: f32,
    height: f32,
    (border, bg, fg): (Hsla, Hsla, Hsla),
) -> Div {
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
pub(super) fn image_url_of(raw: &Json) -> Option<&str> {
    raw["image_url"]
        .as_str()
        .or_else(|| raw["slack_file"]["url"].as_str())
}

/// An `image` element (context, accessory) at `size` px square.
pub(super) fn image_element(element: &Json, kcx: &KitCx, size: f32, cx: &App) -> AnyElement {
    let alt = element["alt_text"].as_str().unwrap_or_default();
    picture(image_url_of(element), alt, size, size, kcx, cx)
}

pub(super) fn image_block(raw: &Json, kcx: &KitCx, cx: &App) -> Div {
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
