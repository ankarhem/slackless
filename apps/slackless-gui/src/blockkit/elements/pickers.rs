//! Block Kit datepicker, datetimepicker and timepicker elements (calendar math via jiff).

use jiff::ToSpan as _;
use jiff::civil::Date;

use super::super::*;
use super::selects::{DropdownSpec, dropdown};
use super::{action_base, dispatch_action, maybe_confirm};

/// Slack `datetimepicker` element (`F-COLLAB-746`): displays the chosen date & time (formatted
/// from unix epoch `initial_date_time`) with quick preset picks dispatching `selected_date_time`.
pub(in crate::blockkit) fn datetimepicker(
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

pub(in crate::blockkit) fn timepicker_slots() -> Vec<(String, String)> {
    (0..24)
        .flat_map(|hour| [0u32, 15, 30, 45].into_iter().map(move |min| (hour, min)))
        .map(|(hour, min)| {
            let t = format!("{hour:02}:{min:02}");
            (t.clone(), t)
        })
        .collect()
}

/// `YYYY-MM-DD` (Slack's date format) as the first of that month; `None` for any other shape.
fn parse_month(iso: &str) -> Option<Date> {
    let date = Date::strptime("%Y-%m-%d", iso).ok()?;
    Some(date.first_of_month())
}

/// Interactive monthly calendar grid date picker (`F-COLLAB-745`).
pub(in crate::blockkit) fn datepicker(
    element: &Json,
    block: &Json,
    key: String,
    kcx: &KitCx,
    cx: &App,
) -> AnyElement {
    let selected =
        choice(kcx, &key, cx).or_else(|| element["initial_date"].as_str().map(str::to_owned));
    let today_iso = messages::iso_date(messages::now_secs());
    let today = parse_month(&today_iso).unwrap_or_default();
    let default_month = selected.as_deref().and_then(parse_month).unwrap_or(today);
    let is_open = kcx
        .kit
        .is_some_and(|kit| kit.read(cx).open_calendars.contains(&key));
    let view_month = kcx
        .kit
        .and_then(|kit| kit.read(cx).calendar_months.get(&key).copied())
        .unwrap_or(default_month);
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
                        .or_insert(default_month);
                    cx.notify();
                });
            }
        });

    if !is_open {
        return trigger.into_any_element();
    }

    let first_dow = usize::try_from(view_month.weekday().to_sunday_zero_offset()).unwrap_or(0);
    let total_days = usize::try_from(view_month.days_in_month()).unwrap_or(30);
    let prev_month = view_month.checked_add(-1.month()).unwrap_or(view_month);
    let next_month = view_month.checked_add(1.month()).unwrap_or(view_month);
    let (view_y, view_m) = (view_month.year(), view_month.month());
    let base = action_base(element, block);
    let mut cells: Vec<Option<u32>> = vec![None; first_dow];
    cells.extend((1..=total_days as u32).map(Some));
    while !cells.len().is_multiple_of(7) {
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
                                    state.calendar_months.insert(key_prev.clone(), prev_month);
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
                        .child(view_month.strftime("%B %Y").to_string()),
                )
                .child(
                    Button::new(SharedString::from(format!("bk-cal-next-{key}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::ChevronRight)
                        .on_click(move |_, _, cx| {
                            if let Some(kit) = &kit_next {
                                kit.update(cx, |state, cx| {
                                    state.calendar_months.insert(key_next.clone(), next_month);
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

/// Parameters for [`picker`].
pub(in crate::blockkit) struct PickerSpec<'a> {
    pub(in crate::blockkit) element: &'a Json,
    pub(in crate::blockkit) block: &'a Json,
    pub(in crate::blockkit) key: String,
    pub(in crate::blockkit) initial_field: &'a str,
    pub(in crate::blockkit) selected_field: &'static str,
    pub(in crate::blockkit) icon: IconName,
    pub(in crate::blockkit) options: Vec<(String, String)>,
}

/// Date / time pickers: the chosen value (or `initial_field`) on a button with `icon`, the
/// next values in a checked menu.
pub(in crate::blockkit) fn picker(spec: PickerSpec<'_>, kcx: &KitCx, cx: &App) -> AnyElement {
    let selected = choice(kcx, &spec.key, cx)
        .or_else(|| spec.element[spec.initial_field].as_str().map(str::to_owned));
    let label = selected
        .clone()
        .or_else(|| text_of(&spec.element["placeholder"]).map(str::to_owned))
        .unwrap_or_else(|| "Select".to_owned());
    let base = action_base(spec.element, spec.block);
    let selected_field = spec.selected_field;
    dropdown(
        DropdownSpec {
            key: spec.key,
            label,
            icon: Some(spec.icon),
            options: spec.options,
            selected,
            kit: kcx.kit.cloned(),
            on_action: kcx.on_action.clone(),
            attachment: kcx.attachment,
        },
        move |_, value| {
            let mut action = base.clone();
            action[selected_field] = value.into();
            action
        },
    )
    .into_any_element()
}
