//! Block Kit text inputs, typed input validation, and file input elements.

use super::super::*;
use super::{action_base, dispatch_action};

/// Validate a Block Kit text input element's current value (`F-COLLAB-747`, `776`, `778`, `791`).
pub(in crate::blockkit) fn validate_text_input(element: &Json, value: &str) -> Option<String> {
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
                && num < min
            {
                return Some(format!("Must be at least {min}."));
            }
            if let Some(max) = element["max_value"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| element["max_value"].as_f64())
                && num > max
            {
                return Some(format!("Must be at most {max}."));
            }
            None
        }
        "plain_text_input" => {
            let len = value.chars().count();
            if let Some(min) = element["min_length"].as_u64()
                && (len as u64) < min
            {
                return Some(format!("Must be at least {min} characters."));
            }
            if let Some(max) = element["max_length"].as_u64()
                && (len as u64) > max
            {
                return Some(format!("Must be at most {max} characters."));
            }
            None
        }
        _ => None,
    }
}

/// Slack `file_input` element (`F-COLLAB-751`): native file chooser storing selected filenames
/// in `BlockKitState::files` for `views.submit`.
pub(in crate::blockkit) fn file_input_element(
    element: &Json,
    key: &str,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let max_files = element["max_files"].as_u64().unwrap_or(10) as usize;
    let chosen = kcx
        .kit
        .and_then(|kit| kit.read(cx).files.get(key).cloned())
        .unwrap_or_default();
    let kit_pick = kcx.kit.cloned();
    let key_pick = key.to_owned();
    let kit_clear = kcx.kit.cloned();
    let key_clear = key.to_owned();
    let multiple = max_files > 1;

    v_flex()
        .gap_1p5()
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("bk-file-{key}")))
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::Paperclip)
                        .label(if chosen.is_empty() {
                            "Choose files\u{2026}".to_owned()
                        } else {
                            format!("{} selected", chosen.len())
                        })
                        .on_click(move |_, _, cx| {
                            let rx = cx.prompt_for_paths(gpui_kit::gpui::PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple,
                                prompt: Some("Select files".into()),
                            });
                            let kit = kit_pick.clone();
                            let key = key_pick.clone();
                            cx.spawn(async move |cx| {
                                if let Ok(Ok(Some(paths))) = rx.await {
                                    let names: Vec<String> = paths
                                        .into_iter()
                                        .take(max_files)
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .collect();
                                    if let Some(kit) = kit {
                                        kit.update(cx, |state, cx| {
                                            state.files.insert(key, names);
                                            cx.notify();
                                        });
                                    }
                                }
                            })
                            .detach();
                        }),
                )
                .when(!chosen.is_empty(), |row| {
                    row.child(
                        Button::new(SharedString::from(format!("bk-file-clear-{key}")))
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(move |_, _, cx| {
                                if let Some(kit) = &kit_clear {
                                    kit.update(cx, |state, cx| {
                                        state.files.remove(&key_clear);
                                        cx.notify();
                                    });
                                }
                            }),
                    )
                }),
        )
        .when(!chosen.is_empty(), |col| {
            col.child(v_flex().gap_0p5().children(chosen.into_iter().map(|path| {
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(&path)
                    .to_owned();
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(name)
            })))
        })
        .into_any_element()
}

/// A text input's textarea when [`prepare_inputs`] created one, else a framed placeholder.
/// Supports `dispatch_action` / `dispatch_action_config` (`F-COLLAB-764`) by offering an inline
/// Apply button that dispatches `blocks.actions` with the current value.
pub(in crate::blockkit) fn text_input(
    element: &Json,
    block: &Json,
    key: &str,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    if let Some(input) = kcx
        .kit
        .and_then(|kit| kit.read(cx).inputs.get(key).cloned())
    {
        let value = input.read(cx).value();
        let error = validate_text_input(element, &value);
        let danger = cx.theme().danger;
        let dispatch_enabled = block["dispatch_action"].as_bool().unwrap_or(false)
            || element["dispatch_action_config"]["trigger_actions_on"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
        let multi_line = element["multiline"].as_bool().unwrap_or(false)
            || element["type"].as_str() == Some("rich_text_input");
        let apply_btn = dispatch_enabled.then(|| {
            let input_c = input.clone();
            let base = action_base(element, block);
            let kind = element["type"]
                .as_str()
                .unwrap_or("plain_text_input")
                .to_owned();
            let on_action = kcx.on_action.clone();
            let attachment = kcx.attachment;
            let key_c = key.to_owned();
            crate::preferences::form_button(
                Button::new(SharedString::from(format!("bk-dispatch-{key}")))
                    .outline()
                    .label("Apply"),
                cx,
            )
            .on_click(move |_, window, cx| {
                let val = input_c.read(cx).value().to_string();
                let mut payload = base.clone();
                if kind == "rich_text_input" {
                    payload["rich_text_value"] = rich_text_input_value(&val);
                } else {
                    payload["value"] = val.clone().into();
                }
                dispatch_action(
                    payload,
                    on_action.as_ref(),
                    attachment,
                    &key_c,
                    &format!("submitted \u{201C}{val}\u{201D}"),
                    window,
                    cx,
                );
            })
        });
        return v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_1p5()
                    .child(
                        crate::preferences::text_input_box(&input, multi_line, cx)
                            .flex_1()
                            .min_w_0()
                            .when(error.is_some(), |el| el.border_color(danger)),
                    )
                    .children(apply_btn),
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

pub(in crate::blockkit) fn rich_text_input_value(text: &str) -> Json {
    serde_json::json!({
        "type": "rich_text",
        "elements": [{
            "type": "rich_text_section",
            "elements": [{ "type": "text", "text": text }]
        }]
    })
}
