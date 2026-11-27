//! Slack Lists cell formatting, filtering/sorting, CSV/JSON import & export, and item comment threads.

use super::*;

pub(crate) use slackless_core::domain::lists::parse_csv_rows;
pub(super) use slackless_core::domain::lists::{map_csv_row_to_cells, row_cell};

pub(super) fn live_column_icon(kind: &str) -> IconName {
    match kind {
        "user" | "todo_assignee" | "assignee" | "created_by" | "last_edited_by" | "vote" => {
            IconName::User
        }
        "select" | "multi_select" => IconName::CircleDot,
        "date" | "todo_due_date" | "due_date" | "created_time" | "last_edited_time" => {
            IconName::Calendar
        }
        "checkbox" | "todo_completed" | "completed" => IconName::SquareCheck,
        "link" => IconName::Link,
        "number" | "rating" | "currency" => IconName::Hash,
        _ => IconName::Type,
    }
}

pub(super) fn live_column_width(column: &SlackListColumn) -> f32 {
    if column.is_primary {
        return 260.;
    }
    match column.kind.as_str() {
        "checkbox" | "todo_completed" | "completed" => 90.,
        "date" | "todo_due_date" | "due_date" | "number" | "rating" => 120.,
        "select" | "multi_select" => 150.,
        "user" | "todo_assignee" | "assignee" => 180.,
        _ => 210.,
    }
}

pub(super) fn slack_choice_color(name: &str, cx: &App) -> Hsla {
    crate::theme::list_choice_color(name, cx.theme())
}

pub(super) fn cell_sort_key(
    val: Option<&SlackListCellValue>,
    col: &SlackListColumn,
    fixtures: &Fixtures,
) -> String {
    match val {
        None | Some(SlackListCellValue::Empty) => String::new(),
        Some(SlackListCellValue::Text(s))
        | Some(SlackListCellValue::Date(s))
        | Some(SlackListCellValue::Number(s)) => s.to_lowercase(),
        Some(SlackListCellValue::Link { label, .. }) => label.to_lowercase(),
        Some(SlackListCellValue::Checkbox(b)) => if *b { "1" } else { "0" }.to_owned(),
        Some(SlackListCellValue::Users(users)) => users
            .iter()
            .map(|u| fixtures.user_name(u).to_lowercase())
            .collect::<Vec<_>>()
            .join(", "),
        Some(SlackListCellValue::Select(opts)) => opts
            .iter()
            .map(|v| {
                col.choices
                    .iter()
                    .find(|c| c.value == *v)
                    .map(|c| c.label.to_lowercase())
                    .unwrap_or_else(|| v.to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Filter live list rows by `query` across all columns and sort by optional `(col_idx, ascending)`.
pub(crate) fn filter_and_sort_rows(
    data: &SlackListData,
    fixtures: &Fixtures,
    query: &str,
    sort: Option<(usize, bool)>,
) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let mut indices: Vec<usize> =
        data.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.is_empty()
                    || data.columns.iter().any(|col| {
                        cell_sort_key(row_cell(row, col), col, fixtures).contains(&needle)
                    })
            })
            .map(|(ix, _)| ix)
            .collect();
    if let Some((col_ix, asc)) = sort
        && let Some(col) = data.columns.get(col_ix)
    {
        indices.sort_by(|&a, &b| {
            let ka = cell_sort_key(row_cell(&data.rows[a], col), col, fixtures);
            let kb = cell_sort_key(row_cell(&data.rows[b], col), col, fixtures);
            if asc { ka.cmp(&kb) } else { kb.cmp(&ka) }
        });
    }
    indices
}

/// Whether `row` is marked completed via a checkbox column or a "Done"/"Completed" select choice (`F-COLLAB-257`).
pub(crate) fn is_row_completed(row: &SlackListRow, columns: &[SlackListColumn]) -> bool {
    columns.iter().any(|col| match col.kind.as_str() {
        "checkbox" | "todo_completed" | "completed" => {
            matches!(row_cell(row, col), Some(SlackListCellValue::Checkbox(true)))
        }
        "select" | "multi_select" => match row_cell(row, col) {
            Some(SlackListCellValue::Select(opts)) => opts.iter().any(|opt| {
                let label = col
                    .choices
                    .iter()
                    .find(|c| c.value == *opt)
                    .map(|c| c.label.as_str())
                    .unwrap_or(opt.as_str());
                matches!(
                    label.trim().to_ascii_lowercase().as_str(),
                    "done" | "complete" | "completed"
                )
            }),
            _ => false,
        },
        _ => false,
    })
}

/// IDs of completed, not-yet-archived rows in the current list view (`F-COLLAB-257`).
pub(crate) fn completed_row_ids(
    data: &SlackListData,
    already_archived: &HashSet<String>,
) -> Vec<String> {
    data.rows
        .iter()
        .filter(|row| !already_archived.contains(&row.id) && is_row_completed(row, &data.columns))
        .map(|row| row.id.clone())
        .collect()
}

/// Filter `indices` according to `archived` and the view-level `show_archived` toggle (`F-COLLAB-257`).
pub(crate) fn filter_archived_rows(
    indices: Vec<usize>,
    data: &SlackListData,
    archived: &HashSet<String>,
    show_archived: bool,
) -> Vec<usize> {
    if show_archived {
        return indices;
    }
    indices
        .into_iter()
        .filter(|&ix| data.rows.get(ix).is_none_or(|r| !archived.contains(&r.id)))
        .collect()
}

/// Parent record ID of `row` when it is a subtask row (`F-COLLAB-256`).
pub(crate) fn subtask_parent_id(row: &SlackListRow, columns: &[SlackListColumn]) -> Option<String> {
    for key in [
        "parent_record_id",
        "parent_item_id",
        "parent_id",
        "subtask_of",
    ] {
        if let Some(SlackListCellValue::Text(parent)) = row.cells.get(key)
            && !parent.trim().is_empty()
        {
            return Some(parent.trim().to_owned());
        }
    }
    for col in columns {
        let k = col.key.to_ascii_lowercase();
        let kind = col.kind.to_ascii_lowercase();
        if (k.contains("parent") || kind.contains("subtask") || kind == "parent_record")
            && let Some(SlackListCellValue::Text(parent)) = row_cell(row, col)
            && !parent.trim().is_empty()
        {
            return Some(parent.trim().to_owned());
        }
    }
    None
}

/// Whether `row` represents a subtask in the list (`F-COLLAB-256`).
pub(crate) fn is_subtask_row(row: &SlackListRow, columns: &[SlackListColumn]) -> bool {
    subtask_parent_id(row, columns).is_some()
}

/// Child subtask row indices belonging to `parent_id` (respecting `archived` and `show_archived`).
pub(crate) fn subtask_rows_for_parent(
    data: &SlackListData,
    parent_id: &str,
    archived: &HashSet<String>,
    show_archived: bool,
) -> Vec<usize> {
    data.rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            (show_archived || !archived.contains(&row.id))
                && subtask_parent_id(row, &data.columns).as_deref() == Some(parent_id)
        })
        .map(|(ix, _)| ix)
        .collect()
}

pub(super) fn live_cell(
    val: Option<&SlackListCellValue>,
    column: &SlackListColumn,
    id_scope: &str,
    fixtures: &Fixtures,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    match val {
        None | Some(SlackListCellValue::Empty) => {
            div().text_color(theme.muted_foreground).child("—")
        }
        Some(SlackListCellValue::Text(t)) => div().truncate().child(t.clone()),
        Some(SlackListCellValue::Date(d)) => {
            div().text_color(theme.muted_foreground).child(d.clone())
        }
        Some(SlackListCellValue::Number(n)) => {
            div().font_weight(FontWeight::MEDIUM).child(n.clone())
        }
        Some(SlackListCellValue::Checkbox(checked)) => {
            let (icon, color) = if *checked {
                (IconName::SquareCheck, theme.success)
            } else {
                (IconName::Square, theme.muted_foreground)
            };
            div().child(Icon::new(icon).text_color(color).size(px(16.)))
        }
        Some(SlackListCellValue::Link { label, url }) => {
            let href = url.clone();
            div().child(
                div()
                    .id(gpui_kit::gpui::SharedString::from(format!(
                        "list-link-{id_scope}-{href}"
                    )))
                    .truncate()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|el| el.underline())
                    .child(label.clone())
                    .on_click(move |_, _, cx| cx.open_url(&href)),
            )
        }
        Some(SlackListCellValue::Users(users)) => {
            let mut row = h_flex()
                .gap_1p5()
                .items_center()
                .min_w_0()
                .overflow_hidden();
            for uid in users.iter().take(2) {
                let name = fixtures.user_name(uid).to_owned();
                row = row.child(
                    h_flex()
                        .gap_1()
                        .items_center()
                        .min_w_0()
                        .child(
                            div()
                                .size(px(20.))
                                .flex_none()
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .bg(theme::avatar_color(&name))
                                .text_color(theme::avatar_foreground())
                                .child(team_initials(&name)),
                        )
                        .child(div().truncate().child(name)),
                );
            }
            if users.len() > 2 {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("+{}", users.len() - 2)),
                );
            }
            row
        }
        Some(SlackListCellValue::Select(opts)) => {
            let mut row = h_flex().gap_1().items_center().flex_wrap();
            for opt in opts {
                let choice = column.choices.iter().find(|c| c.value == *opt);
                let label = choice.map(|c| c.label.as_str()).unwrap_or(opt.as_str());
                let color = choice
                    .map(|c| slack_choice_color(&c.color, cx))
                    .unwrap_or(theme.muted_foreground);
                row = row.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .bg(color.opacity(0.18))
                        .text_color(color)
                        .child(label.to_owned()),
                );
            }
            row
        }
    }
}

/// Serialize a live Slack List to RFC 4180 CSV (`F-COLLAB-217`), resolving user IDs and
/// select option labels cleanly via [`slackless_core::domain::lists::list_to_csv`].
pub fn list_to_csv(data: &SlackListData, fixtures: &Fixtures) -> String {
    slackless_core::domain::lists::list_to_csv(data, |u| fixtures.user_name(u).to_owned())
}

pub(super) fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "slack-list".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Serialize a live Slack List to pretty-printed JSON (`F-COLLAB-218`), resolving user IDs and
/// select option labels cleanly via [`slackless_core::domain::lists::list_to_json`].
pub fn list_to_json(data: &SlackListData, fixtures: &Fixtures) -> String {
    slackless_core::domain::lists::list_to_json(data, |u| fixtures.user_name(u).to_owned())
}

pub(super) fn render_record_comments_section(
    list_key: &str,
    record_id: &str,
    state: &ListPanelState,
    fixtures: &Fixtures,
    on_open_in_threads: Option<&DeleteListItemFn>,
    view_only: bool,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let mut all_comments = record_comments(list_key, record_id, cx);
    if let Some(local) = state.local_comments.get(record_id) {
        all_comments.extend(local.iter().cloned());
    }
    let key_open = list_key.to_owned();
    let key_post = list_key.to_owned();
    let rid_post = record_id.to_owned();
    let rid_threads = record_id.to_owned();
    let open_threads_cb = on_open_in_threads.cloned();

    let mut section = v_flex()
        .gap_2()
        .pt_3()
        .mt_1()
        .border_t_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .items_center()
                .justify_between()
                .gap_1p5()
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted_foreground)
                        .child(
                            Icon::new(IconName::MessageSquare)
                                .size(px(12.))
                                .text_color(theme.muted_foreground),
                        )
                        .child(format!("Comments ({})", all_comments.len())),
                )
                .when_some(
                    (!all_comments.is_empty())
                        .then_some(open_threads_cb)
                        .flatten(),
                    |row, cb| {
                        row.child(
                            Button::new("list-item-open-threads")
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("View in Threads")
                                .on_click(move |_, window, cx| {
                                    cb(rid_threads.clone(), window, cx);
                                }),
                        )
                    },
                ),
        )
        .when(view_only, |col| {
            col.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("View access: you can comment on this item's thread."),
            )
        });

    if all_comments.is_empty() {
        section = section.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("No comments on this item yet."),
        );
    } else {
        for (author, body) in &all_comments {
            section = section.child(
                v_flex()
                    .gap_0p5()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(author.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(body.clone()),
                    ),
            );
        }
    }

    if let Some(input) = state.comment_input.clone() {
        let input_send = input.clone();
        let draft_val = input.read(cx).value().to_string();
        // Inline `@mention` autocomplete (`F-COLLAB-251`) when typing `@<query>` at the end.
        let mention_matches: Vec<(String, String, slackless_core::types::UserId)> = draft_val
            .rsplit_once('@')
            .filter(|(before, token)| {
                (before.is_empty()
                    || before
                        .chars()
                        .last()
                        .is_some_and(|c| c.is_whitespace() || c == '('))
                    && !token.contains(char::is_whitespace)
            })
            .map(|(_, token)| {
                let q = token.to_ascii_lowercase();
                fixtures
                    .people()
                    .filter(|u| !u.deleted && !u.is_bot)
                    .filter(|u| {
                        q.is_empty()
                            || u.name.to_ascii_lowercase().contains(&q)
                            || u.display_name().to_ascii_lowercase().contains(&q)
                    })
                    .take(5)
                    .map(|u| (u.display_name().to_owned(), u.name.clone(), u.id.clone()))
                    .collect()
            })
            .unwrap_or_default();
        section = section
            .child(crate::preferences::text_input_box(&input, true, cx))
            .when(!mention_matches.is_empty(), |col| {
                let mut picks = v_flex()
                    .gap_0p5()
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover);
                for (ix, (disp, handle, uid)) in mention_matches.into_iter().enumerate() {
                    let input_pick = input.clone();
                    let draft_base = draft_val.clone();
                    let handle_insert = handle.clone();
                    picks = picks.child(
                        h_flex()
                            .id(("list-comment-mention", ix))
                            .items_center()
                            .gap_1p5()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_xs()
                            .hover(|el| el.bg(theme.accent))
                            .child(crate::avatars::user_chip(fixtures, &uid, 18.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(disp))
                            .child(
                                div()
                                    .text_color(theme.muted_foreground)
                                    .child(format!("@{handle}")),
                            )
                            .on_click(move |_, window, cx| {
                                let next = match draft_base.rsplit_once('@') {
                                    Some((prefix, _)) => format!("{prefix}@{handle_insert} "),
                                    None => format!("{draft_base}@{handle_insert} "),
                                };
                                input_pick.update(cx, |st, cx| st.set_value(next, window, cx));
                                window.refresh();
                            }),
                    );
                }
                col.child(picks)
            })
            .child(
                h_flex().justify_end().child(
                    Button::new("list-item-comment-post")
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Post comment")
                        .on_click(move |_, window, cx| {
                            let text = input_send.read(cx).value().trim().to_owned();
                            if text.is_empty() {
                                return;
                            }
                            input_send.update(cx, |st, cx| st.set_value(String::new(), window, cx));
                            let rid = rid_post.clone();
                            mutate_panel(
                                &key_post,
                                |s| {
                                    s.local_comments
                                        .entry(rid)
                                        .or_default()
                                        .push(("You".to_owned(), text));
                                },
                                window,
                                cx,
                            );
                        }),
                ),
            );
    } else {
        section = section.child(
            Button::new("list-item-comment-add")
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::MessageSquare)
                .label("Add comment")
                .on_click(move |_, window, cx| {
                    let input = cx.new(|cx| {
                        TextareaState::new(window, cx)
                            .placeholder("Write a comment on this item…")
                            .auto_grow(1, 3)
                    });
                    mutate_panel(&key_open, |s| s.comment_input = Some(input), window, cx);
                }),
        );
    }

    section
}

pub(super) fn export_file(
    content: String,
    suggested: String,
    format_label: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    let dir = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let receiver = cx.prompt_for_new_path(&dir, Some(&suggested));
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let write_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move { std::fs::write(&write_path, content) })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => crate::actions::notify(
                    "List exported",
                    format!("Saved {format_label} to {}", path.display()),
                    window,
                    cx,
                ),
                Err(err) => crate::actions::notify("Export failed", err.to_string(), window, cx),
            });
        })
        .detach();
}
