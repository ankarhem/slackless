//! Live Slack List view: loading/error states, Table and Board layouts, and row grouping.

use super::*;

/// Native live editable Slack Lists view (`F-COLLAB-226..229`, `F-COLLAB-234..239`, `F-COLLAB-248`).
pub fn live_list_view(
    loaded: Option<&Result<SlackListData, String>>,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    actions: ListActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let open_browser = actions.open_browser.clone();
    match loaded {
        None => v_flex()
            .id("list-loading")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Loading list…")
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("list-loading-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Err(err)) => v_flex()
            .id("list-error")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_8()
            .text_sm()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Could not load list"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(err.clone()),
            )
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("list-err-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Ok(data)) => render_live_list(data, fixtures, query, search, sort, actions, cx),
    }
}

/// Group live list row indices into named lanes by the first `select` / `multi_select` column
/// (or a single `"All items"` lane when no select column exists) for Board layout (`F-COLLAB-235`, `F-COLLAB-237`).
pub(crate) fn group_live_rows(
    data: &SlackListData,
    visible_indices: &[usize],
) -> Vec<(String, String, Vec<usize>)> {
    let Some(group_col) = data
        .columns
        .iter()
        .find(|c| matches!(c.kind.as_str(), "select" | "multi_select"))
    else {
        return vec![(
            "All items".to_owned(),
            "gray".to_owned(),
            visible_indices.to_vec(),
        )];
    };
    let mut lanes: Vec<(String, String, String, Vec<usize>)> = group_col
        .choices
        .iter()
        .map(|c| {
            (
                c.value.clone(),
                c.label.clone(),
                c.color.clone(),
                Vec::new(),
            )
        })
        .collect();
    let mut uncategorized = Vec::new();
    for &row_ix in visible_indices {
        let val = row_cell(&data.rows[row_ix], group_col);
        let first_opt = match val {
            Some(SlackListCellValue::Select(opts)) => opts.first().map(String::as_str),
            _ => None,
        };
        if let Some(opt) = first_opt {
            if let Some(lane) = lanes.iter_mut().find(|(v, _, _, _)| v == opt) {
                lane.3.push(row_ix);
            } else {
                lanes.push((
                    opt.to_owned(),
                    opt.to_owned(),
                    "gray".to_owned(),
                    vec![row_ix],
                ));
            }
        } else {
            uncategorized.push(row_ix);
        }
    }
    let mut out: Vec<(String, String, Vec<usize>)> = lanes
        .into_iter()
        .map(|(_, label, color, rows)| (label, color, rows))
        .collect();
    if !uncategorized.is_empty() || out.is_empty() {
        out.push(("Uncategorized".to_owned(), "gray".to_owned(), uncategorized));
    }
    out
}

pub(super) fn render_live_list(
    data: &SlackListData,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    actions: ListActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let list_key = data.file_id.clone();
    let state = panel_state(&list_key, cx);
    let visible_indices: Vec<usize> = filter_archived_rows(
        filter_and_sort_rows(data, fixtures, query, sort),
        data,
        &state.archived_rows,
        state.show_archived,
    );
    let completed_to_archive = completed_row_ids(data, &state.archived_rows);
    let archived_count = data
        .rows
        .iter()
        .filter(|r| state.archived_rows.contains(&r.id))
        .count();
    let on_sort = actions.on_sort.clone();

    let primary_ix = data.columns.iter().position(|c| c.is_primary).unwrap_or(0);
    let visible_cols: Vec<usize> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, col)| *ix == primary_ix || !state.hidden_cols.contains(&col.id))
        .map(|(ix, _)| ix)
        .collect();
    let toggleable_cols: Vec<(String, String)> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, _)| *ix != primary_ix)
        .map(|(_, c)| (c.id.clone(), c.name.clone()))
        .collect();

    let header_row = visible_cols.iter().fold(
        h_flex()
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD),
        |row, &ix| {
            let column = &data.columns[ix];
            let width = live_column_width(column);
            let active_sort = sort.filter(|(c, _)| *c == ix).map(|(_, asc)| asc);
            let on_sort_cb = on_sort.clone();
            let hover = theme.accent;
            let cell = h_flex()
                .id(("list-col-hdr", ix))
                .w(px(width))
                .flex_none()
                .items_center()
                .gap_1p5()
                .px_3()
                .py_2()
                .text_color(theme.muted_foreground)
                .when(on_sort_cb.is_some(), |el| {
                    el.cursor_pointer().hover(move |h| h.bg(hover))
                })
                .when_some(on_sort_cb, |el, cb| {
                    el.on_click(move |_, window, cx| cb(ix, window, cx))
                })
                .child(
                    Icon::new(live_column_icon(&column.kind))
                        .text_color(theme.muted_foreground)
                        .size(px(14.)),
                )
                .child(div().truncate().child(column.name.clone()))
                .when_some(active_sort, |el, asc| {
                    el.child(div().text_xs().text_color(theme.foreground).child(if asc {
                        "\u{2191}"
                    } else {
                        "\u{2193}"
                    }))
                });
            row.child(cell)
        },
    );

    let hover_bg = theme.secondary.opacity(0.45);
    let selected_bg = theme.accent.opacity(0.28);
    let rows_ui = visible_indices.iter().map(|&row_ix| {
        let rec = &data.rows[row_ix];
        let is_sel = state.selected_row == Some(row_ix);
        let is_archived = state.archived_rows.contains(&rec.id);
        let is_subtask = is_subtask_row(rec, &data.columns);
        let key_click = list_key.clone();
        let rec_id_click = rec.id.clone();
        let load_comments_row = actions.on_load_comments.clone();
        let row_el = visible_cols.iter().fold(
            h_flex()
                .id(("list-live-row", row_ix))
                .border_t_1()
                .border_color(theme.border)
                .text_sm()
                .cursor_pointer()
                .when(is_archived, |r| r.opacity(0.55))
                .when(is_sel, |r| r.bg(selected_bg))
                .when(!is_sel, |r| r.hover(move |h| h.bg(hover_bg)))
                .on_click(move |_, window, cx| {
                    let mut opened = false;
                    mutate_panel(
                        &key_click,
                        |s| {
                            if s.selected_row == Some(row_ix) {
                                s.selected_row = None;
                            } else {
                                s.selected_row = Some(row_ix);
                                opened = true;
                            }
                        },
                        window,
                        cx,
                    );
                    if opened && let Some(cb) = &load_comments_row {
                        cb(rec_id_click.clone(), window, cx);
                    }
                }),
            |row, &col_ix| {
                let column = &data.columns[col_ix];
                let width = live_column_width(column);
                let val = row_cell(rec, column);
                let is_checkbox = matches!(
                    column.kind.as_str(),
                    "checkbox" | "todo_completed" | "completed"
                );
                let cell_el = if is_checkbox {
                    if let Some(update_cb) = actions.on_update_cell.clone() {
                        let checked = matches!(val, Some(SlackListCellValue::Checkbox(true)));
                        let row_id = rec.id.clone();
                        let col_id = column.id.clone();
                        div()
                            .id(SharedString::from(format!(
                                "list-live-chk-{row_ix}-{col_ix}"
                            )))
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
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
                            .child(live_cell(val, column, fixtures, cx))
                            .into_any_element()
                    } else {
                        live_cell(val, column, fixtures, cx).into_any_element()
                    }
                } else {
                    live_cell(val, column, fixtures, cx).into_any_element()
                };
                row.child(
                    h_flex()
                        .w(px(width))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .py_2()
                        .when(col_ix == primary_ix && is_subtask, |c| {
                            c.pl_6().gap_1().child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("\u{21B3}"),
                            )
                        })
                        .child(cell_el),
                )
            },
        );
        // Inline archive button on subtask rows (`F-COLLAB-256`).
        if is_subtask && !is_archived {
            let key_sub = list_key.clone();
            let sub_id = rec.id.clone();
            let on_arch = actions.on_archive_item.clone();
            row_el.child(
                div().pr_2().flex_none().child(
                    Button::new(SharedString::from(format!("list-subtask-arch-{row_ix}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::Archive)
                        .tooltip("Archive subtask")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
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
                        }),
                ),
            )
        } else {
            row_el
        }
    });

    let body = if data.columns.is_empty() {
        div()
            .py_8()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("This list has no columns.")
    } else if state.layout == ListLayout::Board {
        let lanes = group_live_rows(data, &visible_indices);
        let group_col_ix = data
            .columns
            .iter()
            .position(|c| matches!(c.kind.as_str(), "select" | "multi_select"));
        let card_hover = theme.secondary.opacity(0.6);
        let ring = theme.ring;
        h_flex()
            .items_start()
            .gap_3()
            .children(
                lanes
                    .into_iter()
                    .map(|(lane_label, color_name, lane_rows)| {
                        let color = slack_choice_color(&color_name, cx);
                        v_flex()
                            .w(px(260.))
                            .flex_none()
                            .gap_2()
                            .p_2p5()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted.opacity(0.6))
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_md()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .bg(color.opacity(0.18))
                                            .text_color(color)
                                            .child(lane_label),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(lane_rows.len().to_string()),
                                    ),
                            )
                            .children(lane_rows.into_iter().map(|row_ix| {
                                let rec = &data.rows[row_ix];
                                let is_sel = state.selected_row == Some(row_ix);
                                let key_click = list_key.clone();
                                let rec_id_card = rec.id.clone();
                                let load_comments_card = actions.on_load_comments.clone();
                                let primary_col = &data.columns[primary_ix];
                                let mut card = v_flex()
                                    .id(("list-live-card", row_ix))
                                    .gap_1p5()
                                    .p_2p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(if is_sel { ring } else { theme.border })
                                    .bg(theme.background)
                                    .cursor_pointer()
                                    .hover(move |c| c.bg(card_hover))
                                    .on_click(move |_, window, cx| {
                                        mutate_panel(
                                            &key_click,
                                            |s| s.selected_row = Some(row_ix),
                                            window,
                                            cx,
                                        );
                                        if let Some(cb) = &load_comments_card {
                                            cb(rec_id_card.clone(), window, cx);
                                        }
                                    })
                                    .child(
                                        div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                            live_cell(
                                                row_cell(rec, primary_col),
                                                primary_col,
                                                fixtures,
                                                cx,
                                            ),
                                        ),
                                    );
                                for &col_ix in &visible_cols {
                                    if col_ix == primary_ix || Some(col_ix) == group_col_ix {
                                        continue;
                                    }
                                    let col = &data.columns[col_ix];
                                    let val = row_cell(rec, col);
                                    if matches!(val, None | Some(SlackListCellValue::Empty)) {
                                        continue;
                                    }
                                    card = card.child(
                                        h_flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .text_xs()
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_color(theme.muted_foreground)
                                                    .child(col.name.clone()),
                                            )
                                            .child(live_cell(val, col, fixtures, cx)),
                                    );
                                }
                                card
                            }))
                    }),
            )
    } else if visible_indices.is_empty() {
        v_flex()
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(header_row)
            .child(
                div()
                    .px_4()
                    .py_6()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if query.trim().is_empty() {
                        "This list has no items yet."
                    } else {
                        "No matching items."
                    }),
            )
    } else {
        v_flex()
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(header_row)
            .children(rows_ui)
    };

    let render_ctx = ListRenderCtx {
        data,
        fixtures,
        list_key: &list_key,
        primary_ix,
        state: &state,
        actions: &actions,
    };
    let detail_drawer = detail::render_live_detail_drawer(&render_ctx, cx);
    let add_bar = toolbar::render_live_add_bar(&render_ctx, cx);
    let find_bar = toolbar::render_live_find_bar(query, search, cx);
    let header_bar = toolbar::render_live_toolbar(
        &render_ctx,
        toolbar::ToolbarCounts {
            visible_count: visible_indices.len(),
            archived_count,
            completed_to_archive: &completed_to_archive,
            toggleable_cols,
        },
        find_bar,
        cx,
    );

    h_flex()
        .id("list-live-root")
        .size_full()
        .child(
            div()
                .id("list-scroll")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_scroll()
                .child(
                    v_flex()
                        .px_6()
                        .py_5()
                        .gap_4()
                        .child(header_bar)
                        .children(add_bar)
                        .child(body),
                ),
        )
        .children(detail_drawer)
}
