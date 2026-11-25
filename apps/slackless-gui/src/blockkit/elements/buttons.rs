//! Block Kit button, workflow button, and overflow menu elements.

use super::super::*;
use super::{action_base, dispatch_action, log_button, maybe_confirm};

/// A button: respects optional `confirm` dialogs (`F-COLLAB-765`, `F-COLLAB-766`); a link
/// button opens its URL; any other press goes to the message's action handler.
pub(in crate::blockkit) fn button(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
) -> Button {
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
pub(in crate::blockkit) fn workflow_button(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
) -> Button {
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

/// The `...` overflow menu: options with a `url` open it via the link router (`F-COLLAB-777`),
/// and every choice dispatches a live `blocks.actions` event.
pub(in crate::blockkit) fn overflow(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
) -> impl IntoElement {
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
