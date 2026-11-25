//! Block Kit single/multi select menus, conversation filters, checkboxes, and radio buttons.

use super::super::*;
use super::{action_base, dispatch_action};

/// Parameters for [`dropdown`].
pub(in crate::blockkit) struct DropdownSpec {
    pub(in crate::blockkit) key: String,
    pub(in crate::blockkit) label: String,
    pub(in crate::blockkit) icon: Option<IconName>,
    pub(in crate::blockkit) options: Vec<(String, String)>,
    pub(in crate::blockkit) selected: Option<String>,
    pub(in crate::blockkit) kit: Option<Entity<BlockKitState>>,
    pub(in crate::blockkit) on_action: Option<actions::ActionHandler>,
    pub(in crate::blockkit) attachment: Option<i64>,
}

/// A select menu: supports both single-select and real multi-select (`multi_*_select`,
/// `F-COLLAB-775`), opening a checked option list and dispatching live `blocks.actions` with
/// Slack's wire selection fields.
pub(in crate::blockkit) fn select(
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
        DropdownSpec {
            key,
            label,
            icon: None,
            options,
            selected,
            kit: kcx.kit.cloned(),
            on_action: kcx.on_action.clone(),
            attachment: kcx.attachment,
        },
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

pub(in crate::blockkit) fn dropdown(
    spec: DropdownSpec,
    build_action: impl Fn(&str, &str) -> Json + 'static,
) -> impl IntoElement {
    let DropdownSpec {
        key,
        label,
        icon,
        options,
        selected,
        kit,
        on_action,
        attachment,
    } = spec;
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

/// Option item metadata preserving `text` (honoring `plain_text` vs `mrkdwn`, `F-COLLAB-760`)
/// and optional `description` subtext (`F-COLLAB-759`).
pub(in crate::blockkit) struct RichOption {
    pub(in crate::blockkit) label: String,
    pub(in crate::blockkit) value: String,
    pub(in crate::blockkit) text_obj: Json,
    pub(in crate::blockkit) description: Option<Json>,
}

pub(in crate::blockkit) fn rich_options(element: &Json) -> Vec<RichOption> {
    element["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let label = text_of(&option["text"])?.to_owned();
            let value = option["value"].as_str()?.to_owned();
            let description = option
                .get("description")
                .filter(|d| d.is_object() && text_of(d).is_some())
                .cloned();
            Some(RichOption {
                label,
                value,
                text_obj: option["text"].clone(),
                description,
            })
        })
        .collect()
}

pub(in crate::blockkit) fn checkboxes(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> Div {
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
    let rich = rich_options(element);
    v_flex().gap_1p5().children(rich.into_iter().map(|opt| {
        let RichOption {
            label: text,
            value,
            text_obj,
            description,
        } = opt;
        let is_checked = checked.contains(&value);
        let (kit, key, initial) = (kcx.kit.cloned(), key.clone(), initial.clone());
        let (on_action, attachment, base, opts) = (
            kcx.on_action.clone(),
            kcx.attachment,
            base.clone(),
            all_options.clone(),
        );
        let desc_el = description
            .as_ref()
            .and_then(|d| text_object(d, kcx, true, cx));
        let rich_label = (text_obj["type"].as_str() == Some("mrkdwn"))
            .then(|| text_object(&text_obj, kcx, false, cx))
            .flatten();
        let cb = Checkbox::new(SharedString::from(format!("bk-{key}-{value}")))
            .when(rich_label.is_none(), |c| c.label(plain_label(&text, cx)))
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
            });
        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(cb)
                    .children(rich_label),
            )
            .when_some(desc_el, |col, desc| {
                col.child(
                    div()
                        .pl(px(22.))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(desc),
                )
            })
    }))
}

pub(in crate::blockkit) fn radios(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> Div {
    let selected = choice(kcx, &key, cx).or_else(|| {
        element["initial_option"]["value"]
            .as_str()
            .map(str::to_owned)
    });
    let base = action_base(element, block);
    v_flex()
        .gap_1p5()
        .children(rich_options(element).into_iter().map(|opt| {
            let RichOption {
                label: text,
                value,
                text_obj,
                description,
            } = opt;
            let checked = selected.as_ref() == Some(&value);
            let (kit, key, base, on_action, attachment) = (
                kcx.kit.cloned(),
                key.clone(),
                base.clone(),
                kcx.on_action.clone(),
                kcx.attachment,
            );
            let desc_el = description
                .as_ref()
                .and_then(|d| text_object(d, kcx, true, cx));
            let rich_label = (text_obj["type"].as_str() == Some("mrkdwn"))
                .then(|| text_object(&text_obj, kcx, false, cx))
                .flatten();
            let r = Radio::new(SharedString::from(format!("bk-{key}-{value}")))
                .when(rich_label.is_none(), |rad| {
                    rad.label(plain_label(&text, cx))
                })
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
                });
            v_flex()
                .gap_0p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(r)
                        .children(rich_label),
                )
                .when_some(desc_el, |col, desc| {
                    col.child(
                        div()
                            .pl(px(22.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(desc),
                    )
                })
        }))
}

/// Filter workspace conversations according to Slack's `conversations_select` / `channels_select`
/// `filter` object (`include`, `exclude_external_shared_channels`, `exclude_bot_users`, `F-COLLAB-763`).
pub(in crate::blockkit) fn filtered_conversations(
    element: &Json,
    kind: &str,
    fixtures: &Fixtures,
) -> Vec<(String, String)> {
    let public_only = matches!(kind, "channels_select" | "multi_channels_select");
    let filter = element.get("filter").filter(|v| v.is_object());
    let include: Option<Vec<&str>> = filter
        .and_then(|f| f["include"].as_array())
        .map(|arr| arr.iter().filter_map(Json::as_str).collect());
    let exclude_ext = filter
        .and_then(|f| f["exclude_external_shared_channels"].as_bool())
        .unwrap_or(false);
    let exclude_bots = filter
        .and_then(|f| f["exclude_bot_users"].as_bool())
        .unwrap_or(false);

    fixtures
        .channels
        .iter()
        .filter(|ch| {
            if public_only && (!ch.is_channel || ch.is_private || ch.is_im || ch.is_mpim) {
                return false;
            }
            if exclude_ext && (ch.is_ext_shared || ch.is_shared) {
                return false;
            }
            if exclude_bots
                && ch.is_im
                && let Some(uid) = ch.user.as_ref()
                && fixtures
                    .user(uid)
                    .is_some_and(|u| u.is_bot || u.is_app_user)
            {
                return false;
            }
            if let Some(inc) = &include
                && !inc.is_empty()
            {
                let tag = if ch.is_im {
                    "im"
                } else if ch.is_mpim {
                    "mpim"
                } else if ch.is_private || ch.is_group {
                    "private"
                } else {
                    "public"
                };
                if !inc.contains(&tag) {
                    return false;
                }
            }
            true
        })
        .map(|channel| (fixtures.conversation_label(channel), channel.id.to_string()))
        .collect()
}
