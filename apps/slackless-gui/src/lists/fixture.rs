//! Sample workspace fixture Slack List data model and interactive Table/Board/Detail view.

use super::*;

/// A column's field type; drives its header icon and how its cells render.
#[derive(Clone, Copy)]
pub(super) enum FieldKind {
    Text,
    Person,
    Select,
    Date,
    Checkbox,
}

impl FieldKind {
    fn icon(self) -> IconName {
        match self {
            FieldKind::Text => IconName::Type,
            FieldKind::Person => IconName::User,
            FieldKind::Select => IconName::CircleDot,
            FieldKind::Date => IconName::Calendar,
            FieldKind::Checkbox => IconName::SquareCheck,
        }
    }
}

/// Select options, colored like Slack's option chips.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Status {
    Todo,
    InProgress,
    Blocked,
    Done,
}

impl Status {
    const ALL: [Status; 4] = [
        Status::Todo,
        Status::InProgress,
        Status::Blocked,
        Status::Done,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Status::Todo => "To do",
            Status::InProgress => "In progress",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
        }
    }

    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Status::Todo => theme.muted_foreground,
            Status::InProgress => theme.link,
            Status::Blocked => theme.danger,
            Status::Done => theme.success,
        }
    }
}

#[derive(Clone)]
pub(super) enum Field {
    Text(String),
    Person(String),
    Select(Status),
    Date(String),
    Checkbox(bool),
}

pub(super) struct Column {
    pub(super) name: &'static str,
    kind: FieldKind,
    width: f32,
}

pub(super) struct SlackList {
    pub(super) title: &'static str,
    pub(super) columns: Vec<Column>,
    pub(super) rows: Vec<Vec<Field>>,
}

/// The sample list every conversation shows in fixture mode.
fn fixture() -> SlackList {
    use Field::*;
    let column = |name, kind, width| Column { name, kind, width };
    SlackList {
        title: "Launch tasks",
        columns: vec![
            column("Task", FieldKind::Text, 260.),
            column("Assignee", FieldKind::Person, 170.),
            column("Status", FieldKind::Select, 140.),
            column("Due", FieldKind::Date, 100.),
            column("Shipped", FieldKind::Checkbox, 80.),
        ],
        rows: vec![
            vec![
                Text("Sign the macOS build".into()),
                Person("Maya Chen".into()),
                Select(Status::Done),
                Date("Oct 10".into()),
                Checkbox(true),
            ],
            vec![
                Text("Beta invite list".into()),
                Person("Leo Park".into()),
                Select(Status::InProgress),
                Date("Nov 1".into()),
                Checkbox(false),
            ],
            vec![
                Text("Release notes".into()),
                Person("Sam Rivera".into()),
                Select(Status::Todo),
                Date("Nov 20".into()),
                Checkbox(false),
            ],
            vec![
                Text("Native huddle media".into()),
                Person("Ana Silva".into()),
                Select(Status::Blocked),
                Date("TBD".into()),
                Checkbox(false),
            ],
            vec![
                Text("Search index migration".into()),
                Person("Leo Park".into()),
                Select(Status::Done),
                Date("Oct 28".into()),
                Checkbox(true),
            ],
        ],
    }
}

/// The List tab body: Table or Board layout, hideable fields, CSV/JSON export, inline editing,
/// and click-to-open item detail drawer (`F-COLLAB-226..229`, `F-COLLAB-234..237`, `F-COLLAB-239`).
pub fn list_view(cx: &App) -> Stateful<Div> {
    const KEY: &str = "fixture";
    let theme = cx.theme();
    let mut list = fixture();
    let state = panel_state(KEY, cx);
    if let Some(custom_rows) = state.fixture_rows.clone() {
        list.rows = custom_rows;
    }
    let default_rows = list.rows.clone();
    let visible_cols: Vec<usize> = list
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, col)| *ix == 0 || !state.hidden_cols.contains(col.name))
        .map(|(ix, _)| ix)
        .collect();
    let toggleable_cols: Vec<(String, String)> = list
        .columns
        .iter()
        .skip(1)
        .map(|c| (c.name.to_owned(), c.name.to_owned()))
        .collect();

    let body: Div = match state.layout {
        ListLayout::Table => {
            let header_row = visible_cols.iter().fold(
                h_flex()
                    .bg(theme.muted)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD),
                |row, &col_ix| {
                    let column = &list.columns[col_ix];
                    row.child(
                        h_flex()
                            .w(px(column.width))
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .px_3()
                            .py_2()
                            .text_color(theme.muted_foreground)
                            .child(
                                Icon::new(column.kind.icon())
                                    .text_color(theme.muted_foreground)
                                    .size(px(14.)),
                            )
                            .child(column.name),
                    )
                },
            );

            let hover_bg = theme.secondary.opacity(0.45);
            let selected_bg = theme.accent.opacity(0.28);
            let rows = list.rows.iter().enumerate().map(|(row_ix, fields)| {
                let is_sel = state.selected_row == Some(row_ix);
                let base_rows = default_rows.clone();
                visible_cols.iter().fold(
                    h_flex()
                        .id(("list-fix-row", row_ix))
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .cursor_pointer()
                        .when(is_sel, |r| r.bg(selected_bg))
                        .when(!is_sel, |r| r.hover(move |h| h.bg(hover_bg)))
                        .on_click(move |_, window, cx| {
                            mutate_panel(
                                KEY,
                                |s| {
                                    s.selected_row = if s.selected_row == Some(row_ix) {
                                        None
                                    } else {
                                        Some(row_ix)
                                    };
                                },
                                window,
                                cx,
                            );
                        }),
                    |row, &col_ix| {
                        let column = &list.columns[col_ix];
                        let cell_el = if matches!(fields[col_ix], Field::Checkbox(_)) {
                            let rows_seed = base_rows.clone();
                            div()
                                .id(SharedString::from(format!(
                                    "list-fix-chk-{row_ix}-{col_ix}"
                                )))
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    mutate_panel(
                                        KEY,
                                        |s| {
                                            let r = s
                                                .fixture_rows
                                                .get_or_insert_with(|| rows_seed.clone());
                                            if let Some(Field::Checkbox(v)) =
                                                r.get_mut(row_ix).and_then(|f| f.get_mut(col_ix))
                                            {
                                                *v = !*v;
                                            }
                                        },
                                        window,
                                        cx,
                                    );
                                })
                                .child(cell(&fields[col_ix], cx))
                                .into_any_element()
                        } else {
                            cell(&fields[col_ix], cx).into_any_element()
                        };
                        row.child(
                            h_flex()
                                .w(px(column.width))
                                .flex_none()
                                .items_center()
                                .px_3()
                                .py_2()
                                .child(cell_el),
                        )
                    },
                )
            });

            v_flex()
                .flex_none()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .child(header_row)
                .children(rows)
        }
        ListLayout::Board => {
            let hover_bg = theme.secondary.opacity(0.6);
            let ring = theme.ring;
            h_flex()
                .items_start()
                .gap_3()
                .children(Status::ALL.iter().map(|&status| {
                    let color = status.color(cx);
                    let cards: Vec<usize> = list
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, fields)| {
                            matches!(fields.get(2), Some(Field::Select(s)) if *s == status)
                        })
                        .map(|(ix, _)| ix)
                        .collect();
                    v_flex()
                        .w(px(250.))
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
                                        .child(status.label()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(cards.len().to_string()),
                                ),
                        )
                        .children(cards.into_iter().map(|row_ix| {
                            let fields = &list.rows[row_ix];
                            let is_sel = state.selected_row == Some(row_ix);
                            let mut card = v_flex()
                                .id(("list-fix-card", row_ix))
                                .gap_1p5()
                                .p_2p5()
                                .rounded_md()
                                .border_1()
                                .border_color(if is_sel { ring } else { theme.border })
                                .bg(theme.background)
                                .cursor_pointer()
                                .hover(move |c| c.bg(hover_bg))
                                .on_click(move |_, window, cx| {
                                    mutate_panel(
                                        KEY,
                                        |s| s.selected_row = Some(row_ix),
                                        window,
                                        cx,
                                    );
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(cell(&fields[0], cx)),
                                );
                            for &col_ix in &visible_cols {
                                if col_ix == 0 || col_ix == 2 {
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
                                                .text_color(theme.muted_foreground)
                                                .child(list.columns[col_ix].name),
                                        )
                                        .child(cell(&fields[col_ix], cx)),
                                );
                            }
                            card
                        }))
                }))
        }
    };

    let detail_drawer = state
        .selected_row
        .and_then(|row_ix| list.rows.get(row_ix).map(|fields| (row_ix, fields)))
        .map(|(row_ix, fields)| {
            let rows_seed_del = default_rows.clone();
            let mut panel = v_flex()
                .w(px(320.))
                .flex_none()
                .h_full()
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
                                .child(
                                    Button::new("list-fix-detail-delete")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Trash)
                                        .tooltip("Delete item")
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    let r =
                                                        s.fixture_rows.get_or_insert_with(|| {
                                                            rows_seed_del.clone()
                                                        });
                                                    if row_ix < r.len() {
                                                        r.remove(row_ix);
                                                    }
                                                    s.selected_row = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .child(
                                    Button::new("list-fix-detail-close")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::X)
                                        .on_click(|_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| s.selected_row = None,
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                );
            for (col_ix, (col, field)) in list.columns.iter().zip(fields).enumerate() {
                let rows_seed = default_rows.clone();
                let control = match field {
                    Field::Select(current) => {
                        let current = *current;
                        Button::new(SharedString::from(format!(
                            "list-fix-sel-{row_ix}-{col_ix}"
                        )))
                        .outline()
                        .with_size(Size::XSmall)
                        .label(current.label())
                        .dropdown_menu(move |mut menu, _, _| {
                            for st in Status::ALL {
                                let seed = rows_seed.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(st.label())
                                        .checked(st == current)
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    let r = s
                                                        .fixture_rows
                                                        .get_or_insert_with(|| seed.clone());
                                                    if let Some(slot) = r
                                                        .get_mut(row_ix)
                                                        .and_then(|f| f.get_mut(col_ix))
                                                    {
                                                        *slot = Field::Select(st);
                                                    }
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
                    Field::Checkbox(checked) => {
                        let checked = *checked;
                        h_flex()
                            .id(SharedString::from(format!(
                                "list-fix-det-chk-{row_ix}-{col_ix}"
                            )))
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                mutate_panel(
                                    KEY,
                                    |s| {
                                        let r =
                                            s.fixture_rows.get_or_insert_with(|| rows_seed.clone());
                                        if let Some(Field::Checkbox(v)) =
                                            r.get_mut(row_ix).and_then(|f| f.get_mut(col_ix))
                                        {
                                            *v = !*v;
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })
                            .child(cell(field, cx))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if checked { "Checked" } else { "Unchecked" }),
                            )
                            .into_any_element()
                    }
                    _ => h_flex()
                        .items_center()
                        .text_sm()
                        .child(cell(field, cx))
                        .into_any_element(),
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
                                    Icon::new(col.kind.icon())
                                        .size(px(12.))
                                        .text_color(theme.muted_foreground),
                                )
                                .child(col.name),
                        )
                        .child(h_flex().items_center().child(control)),
                );
            }
            panel
        });

    let add_bar = state.adding.then(|| {
        state.new_item_input.clone().map(|input| {
            let input_submit = input.clone();
            let rows_seed = default_rows.clone();
            h_flex()
                .items_center()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary.opacity(0.3))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Textarea::new(&input).appearance(false).bordered(false)),
                )
                .child(
                    Button::new("list-fix-add-save")
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Add")
                        .on_click(move |_, window, cx| {
                            let text = input_submit.read(cx).value().trim().to_owned();
                            if !text.is_empty() {
                                mutate_panel(
                                    KEY,
                                    |s| {
                                        let r =
                                            s.fixture_rows.get_or_insert_with(|| rows_seed.clone());
                                        r.push(vec![
                                            Field::Text(text),
                                            Field::Person("Unassigned".into()),
                                            Field::Select(Status::Todo),
                                            Field::Date("TBD".into()),
                                            Field::Checkbox(false),
                                        ]);
                                        s.adding = false;
                                        s.new_item_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }
                        }),
                )
                .child(
                    Button::new("list-fix-add-cancel")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(|_, window, cx| {
                            mutate_panel(
                                KEY,
                                |s| {
                                    s.adding = false;
                                    s.new_item_input = None;
                                },
                                window,
                                cx,
                            );
                        }),
                )
        })
    });

    h_flex()
        .id("list-root")
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
                        .child(
                            h_flex()
                                .items_center()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::ListChecks)
                                        .text_color(theme.success)
                                        .size(px(18.)),
                                )
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(list.title))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{} items", list.rows.len())),
                                )
                                .child(div().flex_1())
                                .child(
                                    Button::new("list-fix-add-item")
                                        .primary()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("Add item")
                                        .on_click(|_, window, cx| {
                                            let input = cx.new(|cx| {
                                                TextareaState::new(window, cx)
                                                    .placeholder("New task title…")
                                            });
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    s.adding = true;
                                                    s.new_item_input = Some(input);
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .child(layout_switcher(KEY, state.layout, cx))
                                .child(fields_menu(KEY, toggleable_cols, &state.hidden_cols))
                                .child({
                                    let csv = fixture_to_csv(&list);
                                    let suggested =
                                        format!("{}.csv", sanitize_filename(list.title));
                                    Button::new("list-export-csv-fixture")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("CSV")
                                        .tooltip("Download list as CSV")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                csv.clone(),
                                                suggested.clone(),
                                                "CSV",
                                                window,
                                                cx,
                                            );
                                        })
                                })
                                .child({
                                    let json = fixture_to_json(&list);
                                    let suggested =
                                        format!("{}.json", sanitize_filename(list.title));
                                    Button::new("list-export-json-fixture")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("JSON")
                                        .tooltip("Download list as JSON")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                json.clone(),
                                                suggested.clone(),
                                                "JSON",
                                                window,
                                                cx,
                                            );
                                        })
                                }),
                        )
                        .children(add_bar.flatten())
                        .child(body),
                ),
        )
        .children(detail_drawer)
}

fn cell(field: &Field, cx: &App) -> Div {
    let theme = cx.theme();
    match field {
        Field::Text(text) => div().truncate().child(text.clone()),
        Field::Date(text) => div().text_color(theme.muted_foreground).child(text.clone()),
        Field::Person(name) => h_flex()
            .gap_1p5()
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
                    .bg(theme::avatar_color(name))
                    .text_color(theme::avatar_foreground())
                    .child(team_initials(name)),
            )
            .child(div().truncate().child(name.clone())),
        Field::Select(status) => {
            let (label, color): (&str, Hsla) = match status {
                Status::Todo => ("To do", theme.muted_foreground),
                Status::InProgress => ("In progress", theme.link),
                Status::Blocked => ("Blocked", theme.danger),
                Status::Done => ("Done", theme.success),
            };
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .bg(color.opacity(0.18))
                .text_color(color)
                .child(label)
        }
        Field::Checkbox(checked) => {
            let (icon, color) = if *checked {
                (IconName::SquareCheck, theme.success)
            } else {
                (IconName::Square, theme.muted_foreground)
            };
            div().child(Icon::new(icon).text_color(color).size(px(16.)))
        }
    }
}
