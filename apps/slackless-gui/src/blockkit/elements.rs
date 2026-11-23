//! Interactive Block Kit elements, pickers, inputs, and action dispatch.

use super::*;

/// Fixture mode has no app to receive `block_actions`: log the interaction and toast it.
pub(super) fn log_action(key: &str, detail: &str, window: &mut Window, cx: &mut App) {
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

/// A button: respects optional `confirm` dialogs (`F-COLLAB-765`, `F-COLLAB-766`); a link
/// button opens its URL; any other press goes to the message's action handler.
pub(super) fn button(element: &Json, block: &Json, key: String, kcx: &KitCx) -> Button {
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
pub(super) fn workflow_button(element: &Json, block: &Json, key: String, kcx: &KitCx) -> Button {
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
pub(super) fn datetimepicker(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
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
pub(super) fn select(
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

pub(super) fn parse_iso_ymd(s: &str) -> Option<(i32, u32, u32)> {
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

pub(super) fn days_in_month(year: i32, month: u32) -> u32 {
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
pub(super) fn weekday_sunday0(year: i32, month: u32, day: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if month < 3 { year - 1 } else { year };
    ((y + y / 4 - y / 100 + y / 400 + T[(month - 1) as usize] + day as i32).rem_euclid(7)) as u32
}

pub(super) fn month_name(month: u32) -> &'static str {
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
pub(super) fn datepicker(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
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
pub(super) fn picker(
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
pub(super) fn dropdown(
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
pub(super) fn overflow(element: &Json, block: &Json, key: String, kcx: &KitCx) -> impl IntoElement {
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

/// Option item metadata preserving `text` (honoring `plain_text` vs `mrkdwn`, `F-COLLAB-760`)
/// and optional `description` subtext (`F-COLLAB-759`).
pub(super) struct RichOption {
    pub(super) label: String,
    pub(super) value: String,
    pub(super) text_obj: Json,
    pub(super) description: Option<Json>,
}

pub(super) fn rich_options(element: &Json) -> Vec<RichOption> {
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

pub(super) fn checkboxes(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
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

pub(super) fn radios(element: &Json, block: &Json, key: String, kcx: &KitCx, cx: &App) -> Div {
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

/// Validate a Block Kit text input element's current value (`F-COLLAB-747`, `776`, `778`, `791`).
pub(super) fn validate_text_input(element: &Json, value: &str) -> Option<String> {
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

/// Filter workspace conversations according to Slack's `conversations_select` / `channels_select`
/// `filter` object (`include`, `exclude_external_shared_channels`, `exclude_bot_users`, `F-COLLAB-763`).
pub(super) fn filtered_conversations(
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
            if exclude_bots && ch.is_im {
                if let Some(uid) = ch.user.as_ref() {
                    if fixtures
                        .user(uid)
                        .is_some_and(|u| u.is_bot || u.is_app_user)
                    {
                        return false;
                    }
                }
            }
            if let Some(inc) = &include {
                if !inc.is_empty() {
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
            }
            true
        })
        .map(|channel| (fixtures.conversation_label(channel), channel.id.to_string()))
        .collect()
}

/// Slack `file_input` element (`F-COLLAB-751`): native file chooser storing selected filenames
/// in `BlockKitState::files` for `views.submit`.
pub(super) fn file_input_element(element: &Json, key: &str, kcx: &KitCx, cx: &App) -> AnyElement {
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
pub(super) fn text_input(
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
            Button::new(SharedString::from(format!("bk-dispatch-{key}")))
                .outline()
                .with_size(Size::Small)
                .label("Apply")
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
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(error.is_some(), |el| {
                                el.rounded_md().border_1().border_color(danger)
                            })
                            .child(Textarea::new(&input)),
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

pub(super) fn rich_text_input_value(text: &str) -> Json {
    serde_json::json!({
        "type": "rich_text",
        "elements": [{
            "type": "rich_text_section",
            "elements": [{ "type": "text", "text": text }]
        }]
    })
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
