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

/// Slack `workflow_button` element (`F-COLLAB-404`, `F-COLLAB-409`, `F-COLLAB-432`,
/// `F-COLLAB-769..771`): renders with primary/danger/outline style, honors `visible_to_user_ids`
/// and optional `confirm` dialog, trips `workflow.trigger.url`, and offers a companion menu to
/// view workflow details (`workflows.triggers.info`) or copy the workflow link.
pub(in crate::blockkit) fn workflow_button(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    if let Some(allowed) = element["workflow"]["trigger"]["visible_to_user_ids"]
        .as_array()
        .or_else(|| element["visible_to_user_ids"].as_array())
        && !allowed.is_empty()
        && !allowed
            .iter()
            .filter_map(Json::as_str)
            .any(|id| id == kcx.fixtures.self_user.as_str())
    {
        return div().into_any_element();
    }
    let label = emoji::replace_shortcodes(text_of(&element["text"]).unwrap_or("Run workflow"));
    let trigger_url = element["workflow"]["trigger"]["url"]
        .as_str()
        .or_else(|| element["url"].as_str())
        .map(str::to_owned);
    let trigger_id = element["workflow"]["trigger"]["id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            trigger_url
                .as_deref()
                .and_then(slackless_core::api::extract_workflow_trigger_id)
        });
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
        .label(label.clone());
    let button = match element["style"].as_str() {
        Some("primary") => button.primary(),
        Some("danger") => button.danger(),
        _ => button.outline(),
    };
    let run_trigger_url = trigger_url.clone();
    let run_on_action = on_action.clone();
    let run_links = links.clone();
    let run_action = action.clone();
    let run_confirm = confirm.clone();
    let primary_btn =
        button
            .when_some(tooltip, |b, tip| b.tooltip(tip))
            .on_click(move |_, window, cx| {
                let trigger_url = run_trigger_url.clone();
                let on_action = run_on_action.clone();
                let links = run_links.clone();
                let action = run_action.clone();
                maybe_confirm(run_confirm.as_ref(), window, cx, move |window, cx| {
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
            });

    if trigger_url.is_none() && trigger_id.is_none() {
        return primary_btn.into_any_element();
    }

    let on_details = kcx.on_workflow_details.clone();
    let details_req = WorkflowDetailsRequest {
        trigger_id: trigger_id.clone(),
        url: trigger_url.clone(),
        title: label.clone(),
    };
    let copy_url = trigger_url.clone();
    let menu_btn = Button::new(SharedString::from(format!("bk-wf-more-{key}")))
        .ghost()
        .with_size(Size::Small)
        .icon(IconName::ChevronDown)
        .tooltip("Workflow options")
        .dropdown_menu(move |mut menu, _, _| {
            if let Some(url) = trigger_url.clone() {
                let links = links.clone();
                let confirm = confirm.clone();
                menu = menu.item(
                    PopupMenuItem::new("Run workflow")
                        .icon(IconName::Zap)
                        .on_click(move |_, window, cx| {
                            let url = url.clone();
                            let links = links.clone();
                            maybe_confirm(confirm.as_ref(), window, cx, move |window, cx| {
                                messages::route_link(
                                    links.as_ref(),
                                    LinkTarget::Url(url.clone().into()),
                                    window,
                                    cx,
                                );
                            });
                        }),
                );
            }
            if let Some(on_details) = on_details.clone() {
                let req = details_req.clone();
                menu = menu.item(
                    PopupMenuItem::new("View workflow details")
                        .icon(IconName::Info)
                        .on_click(move |_, window, cx| on_details(req.clone(), window, cx)),
                );
            }
            if let Some(url) = copy_url.clone() {
                menu = menu.item(
                    PopupMenuItem::new("Copy workflow link")
                        .icon(IconName::Link)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                url.clone(),
                            ));
                            actions::notify(
                                "Workflow link copied",
                                "Copied workflow URL to clipboard.",
                                window,
                                cx,
                            );
                        }),
                );
            }
            menu
        });

    let _ = cx;
    h_flex()
        .items_center()
        .gap_0p5()
        .child(primary_btn)
        .child(menu_btn)
        .into_any_element()
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
