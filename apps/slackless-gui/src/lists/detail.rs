//! Live Slack List item detail drawer: typed field editors, subtasks, and record comments.

use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_live_detail_drawer(
    data: &SlackListData,
    fixtures: &Fixtures,
    list_key: &str,
    primary_ix: usize,
    state: &ListPanelState,
    actions: &ListActions,
    cx: &App,
) -> Option<Stateful<Div>> {
    let theme = cx.theme();
    let list_key = list_key.to_owned();
    state
        .selected_row
        .and_then(|row_ix| data.rows.get(row_ix))
        .map(|rec| {
            let key_close = list_key.clone();
            let key_del = list_key.clone();
            let key_arch = list_key.clone();
            let row_id_del = rec.id.clone();
            let row_id_arch = rec.id.clone();
            let on_delete = actions.on_delete_item.clone();
            let on_archive = actions.on_archive_item.clone();
            let mut panel = v_flex()
                .id("list-live-detail")
                .w(px(340.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.background)
                .p_4()
                .gap_3()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Item details"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .when_some(on_archive, |row, arch| {
                                    row.child(
                                        Button::new("list-live-detail-archive")
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Archive)
                                            .tooltip("Archive item")
                                            .on_click(move |_, window, cx| {
                                                let rid = row_id_arch.clone();
                                                mutate_panel(
                                                    &key_arch,
                                                    |s| {
                                                        s.archived_rows.insert(rid.clone());
                                                        s.selected_row = None;
                                                    },
                                                    window,
                                                    cx,
                                                );
                                                arch(row_id_arch.clone(), window, cx);
                                            }),
                                    )
                                })
                                .when_some(on_delete, |row, del| {
                                    row.child(
                                        Button::new("list-live-detail-delete")
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Trash)
                                            .tooltip("Delete item")
                                            .on_click(move |_, window, cx| {
                                                mutate_panel(
                                                    &key_del,
                                                    |s| s.selected_row = None,
                                                    window,
                                                    cx,
                                                );
                                                del(row_id_del.clone(), window, cx);
                                            }),
                                    )
                                })
                                .child(
                                    Button::new("list-live-detail-close")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::X)
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                &key_close,
                                                |s| {
                                                    s.selected_row = None;
                                                    s.editing_cell = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                );
            for (col_ix, col) in data.columns.iter().enumerate() {
                let val = row_cell(rec, col);
                let control = if let Some(update_cb) = actions.on_update_cell.clone() {
                    let row_id = rec.id.clone();
                    let col_id = col.id.clone();
                    match col.kind.as_str() {
                        "checkbox" | "todo_completed" | "completed" => {
                            let checked = matches!(val, Some(SlackListCellValue::Checkbox(true)));
                            h_flex()
                                .id(SharedString::from(format!("list-det-chk-{col_ix}")))
                                .items_center()
                                .gap_1p5()
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    update_cb(
                                        row_id.clone(),
                                        ListCellInput {
                                            column_id: col_id.clone(),
                                            value: ListCellInputValue::Checkbox(!checked),
                                        },
                                        window,
                                        cx,
                                    );
                                })
                                .child(live_cell(val, col, fixtures, cx))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(if checked { "Checked" } else { "Unchecked" }),
                                )
                                .into_any_element()
                        }
                        "select" | "multi_select" if !col.choices.is_empty() => {
                            let current = match val {
                                Some(SlackListCellValue::Select(opts)) => opts.clone(),
                                _ => Vec::new(),
                            };
                            let current_label = current
                                .first()
                                .and_then(|v| col.choices.iter().find(|c| c.value == *v))
                                .map(|c| c.label.clone())
                                .unwrap_or_else(|| "Select…".to_owned());
                            let choices = col.choices.clone();
                            Button::new(SharedString::from(format!("list-det-sel-{col_ix}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .label(current_label)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for ch in &choices {
                                        let cb = update_cb.clone();
                                        let rid = row_id.clone();
                                        let cid = col_id.clone();
                                        let opt_val = ch.value.clone();
                                        let checked = current.contains(&opt_val);
                                        menu = menu.item(
                                            PopupMenuItem::new(ch.label.clone())
                                                .checked(checked)
                                                .on_click(move |_, window, cx| {
                                                    cb(
                                                        rid.clone(),
                                                        ListCellInput {
                                                            column_id: cid.clone(),
                                                            value: ListCellInputValue::Select(
                                                                vec![opt_val.clone()],
                                                            ),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element()
                        }
                        "user" | "todo_assignee" | "assignee" => {
                            let current: Vec<String> = match val {
                                Some(SlackListCellValue::Users(u)) => {
                                    u.iter().map(ToString::to_string).collect()
                                }
                                _ => Vec::new(),
                            };
                            let label = match val {
                                Some(SlackListCellValue::Users(u)) if !u.is_empty() => u
                                    .iter()
                                    .map(|id| fixtures.user_name(id))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                _ => "Assign…".to_owned(),
                            };
                            let users: Vec<(slackless_core::types::UserId, String)> = fixtures
                                .users
                                .iter()
                                .filter(|u| !u.deleted && !u.is_bot)
                                .take(40)
                                .map(|u| (u.id.clone(), fixtures.user_name(&u.id).to_owned()))
                                .collect();
                            Button::new(SharedString::from(format!("list-det-usr-{col_ix}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::User)
                                .label(label)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (uid, name) in &users {
                                        let cb = update_cb.clone();
                                        let rid = row_id.clone();
                                        let cid = col_id.clone();
                                        let uid_val = uid.clone();
                                        let checked = current.contains(&uid.to_string());
                                        menu = menu.item(
                                            PopupMenuItem::new(name.clone())
                                                .checked(checked)
                                                .on_click(move |_, window, cx| {
                                                    cb(
                                                        rid.clone(),
                                                        ListCellInput {
                                                            column_id: cid.clone(),
                                                            value: ListCellInputValue::Users(vec![
                                                                uid_val.clone(),
                                                            ]),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element()
                        }
                        kind if !matches!(
                            kind,
                            "created_by" | "last_edited_by" | "created_time" | "last_edited_time"
                        ) =>
                        {
                            let editing_this =
                                state.editing_cell.as_ref().and_then(|(er, ec, ed)| {
                                    (er == &row_id && ec == &col_id).then(|| ed.clone())
                                });
                            if let Some(input) = editing_this {
                                let input_save = input.clone();
                                let key_save = list_key.clone();
                                let key_cancel = list_key.clone();
                                let kind_owned = kind.to_owned();
                                v_flex()
                                    .w_full()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(theme.border)
                                            .px_2()
                                            .py_1()
                                            .child(
                                                Textarea::new(&input)
                                                    .appearance(false)
                                                    .bordered(false),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_end()
                                            .gap_1()
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "list-det-cancel-{col_ix}"
                                                )))
                                                .ghost()
                                                .with_size(Size::XSmall)
                                                .label("Cancel")
                                                .on_click(move |_, window, cx| {
                                                    mutate_panel(
                                                        &key_cancel,
                                                        |s| s.editing_cell = None,
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "list-det-save-{col_ix}"
                                                )))
                                                .primary()
                                                .with_size(Size::XSmall)
                                                .label("Save")
                                                .on_click(move |_, window, cx| {
                                                    let raw = input_save
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .to_owned();
                                                    let cell_val = match kind_owned.as_str() {
                                                        "date" | "todo_due_date" | "due_date" => {
                                                            if raw.is_empty() {
                                                                ListCellInputValue::Date(Vec::new())
                                                            } else {
                                                                ListCellInputValue::Date(vec![raw])
                                                            }
                                                        }
                                                        "number" | "rating" | "currency" => {
                                                            if let Ok(n) = raw.parse::<f64>() {
                                                                ListCellInputValue::Number(vec![n])
                                                            } else {
                                                                ListCellInputValue::Text(raw)
                                                            }
                                                        }
                                                        "link" => ListCellInputValue::Link {
                                                            url: raw,
                                                            display_name: None,
                                                        },
                                                        _ => ListCellInputValue::Text(raw),
                                                    };
                                                    mutate_panel(
                                                        &key_save,
                                                        |s| s.editing_cell = None,
                                                        window,
                                                        cx,
                                                    );
                                                    update_cb(
                                                        row_id.clone(),
                                                        ListCellInput {
                                                            column_id: col_id.clone(),
                                                            value: cell_val,
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .into_any_element()
                            } else {
                                let initial = match val {
                                    Some(SlackListCellValue::Text(s))
                                    | Some(SlackListCellValue::Date(s))
                                    | Some(SlackListCellValue::Number(s)) => s.clone(),
                                    Some(SlackListCellValue::Link { url, .. }) => url.clone(),
                                    _ => String::new(),
                                };
                                let key_ed = list_key.clone();
                                let rid_ed = row_id.clone();
                                let cid_ed = col_id.clone();
                                let hover = theme.secondary.opacity(0.4);
                                h_flex()
                                    .id(SharedString::from(format!("list-det-edit-{col_ix}")))
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_1p5()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(move |el| el.bg(hover))
                                    .on_click(move |_, window, cx| {
                                        let input = cx.new(|cx| {
                                            TextareaState::new(window, cx)
                                                .default_value(initial.clone())
                                        });
                                        mutate_panel(
                                            &key_ed,
                                            |s| {
                                                s.editing_cell =
                                                    Some((rid_ed.clone(), cid_ed.clone(), input));
                                            },
                                            window,
                                            cx,
                                        );
                                    })
                                    .child(live_cell(val, col, fixtures, cx))
                                    .child(
                                        Icon::new(IconName::Pencil)
                                            .size(px(12.))
                                            .text_color(theme.muted_foreground),
                                    )
                                    .into_any_element()
                            }
                        }
                        _ => h_flex()
                            .items_center()
                            .text_sm()
                            .child(live_cell(val, col, fixtures, cx))
                            .into_any_element(),
                    }
                } else {
                    h_flex()
                        .items_center()
                        .text_sm()
                        .child(live_cell(val, col, fixtures, cx))
                        .into_any_element()
                };
                panel = panel.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(
                                    Icon::new(live_column_icon(&col.kind))
                                        .size(px(12.))
                                        .text_color(theme.muted_foreground),
                                )
                                .child(col.name.clone()),
                        )
                        .child(h_flex().w_full().items_center().text_sm().child(control)),
                );
            }
            let child_subtasks =
                subtask_rows_for_parent(data, &rec.id, &state.archived_rows, state.show_archived);
            if !child_subtasks.is_empty() {
                let mut sub_section = v_flex()
                    .gap_1p5()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(format!("Subtasks ({})", child_subtasks.len())),
                    );
                let primary_col = data.columns.get(primary_ix);
                for sub_ix in child_subtasks {
                    let sub_row = &data.rows[sub_ix];
                    let sub_id = sub_row.id.clone();
                    let sub_archived = state.archived_rows.contains(&sub_id);
                    let key_sub = list_key.clone();
                    let on_arch = actions.on_archive_item.clone();
                    let sub_label = match primary_col {
                        Some(col) => live_cell(row_cell(sub_row, col), col, fixtures, cx),
                        None => div().child(sub_row.id.clone()),
                    };
                    sub_section = sub_section.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(theme.secondary.opacity(0.35))
                            .when(sub_archived, |r| r.opacity(0.55))
                            .child(div().flex_1().min_w_0().text_xs().child(sub_label))
                            .when(!sub_archived, |r| {
                                r.child(
                                    Button::new(SharedString::from(format!(
                                        "list-det-subtask-arch-{sub_ix}"
                                    )))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::Archive)
                                    .tooltip("Archive subtask")
                                    .on_click(
                                        move |_, window, cx| {
                                            let rid = sub_id.clone();
                                            mutate_panel(
                                                &key_sub,
                                                |s| {
                                                    s.archived_rows.insert(rid.clone());
                                                },
                                                window,
                                                cx,
                                            );
                                            if let Some(cb) = &on_arch {
                                                cb(sub_id.clone(), window, cx);
                                            }
                                        },
                                    ),
                                )
                            }),
                    );
                }
                panel = panel.child(sub_section);
            }
            panel = panel.child(render_record_comments_section(
                &list_key, &rec.id, state, fixtures, cx,
            ));
            panel
        })
}
