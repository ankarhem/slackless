//! Read-only Slack List view: renders both the sample workspace's fixture list and live Slack
//! Lists fetched via `files.info` + `lists.records.list` / `slackLists.items.list` as a
//! searchable, sortable table with typed columns (text, person, select, date, checkbox,
//! number, link) (`docs/research/api-canvases-lists.md`).

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, Entity, FontWeight, Hsla, InteractiveElement as _, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, rgb,
};
use slackless_core::api::{SlackListCellValue, SlackListColumn, SlackListData, SlackListRow};

use crate::canvas::{OpenBrowserFn, read_only_badge};
use crate::fixtures::Fixtures;
use crate::sidebar::team_initials;
use crate::theme;

/// Callback invoked when the user clicks a live list column header to sort by `col_idx`.
pub type SortColumnFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A column's field type; drives its header icon and how its cells render.
#[derive(Clone, Copy)]
enum FieldKind {
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
#[derive(Clone, Copy)]
enum Status {
    Todo,
    InProgress,
    Blocked,
    Done,
}

enum Field {
    Text(&'static str),
    Person(&'static str),
    Select(Status),
    Date(&'static str),
    Checkbox(bool),
}

struct Column {
    name: &'static str,
    kind: FieldKind,
    width: f32,
}

struct SlackList {
    title: &'static str,
    columns: Vec<Column>,
    rows: Vec<Vec<Field>>,
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
                Text("Sign the macOS build"),
                Person("Maya Chen"),
                Select(Status::Done),
                Date("Oct 10"),
                Checkbox(true),
            ],
            vec![
                Text("Beta invite list"),
                Person("Leo Park"),
                Select(Status::InProgress),
                Date("Nov 1"),
                Checkbox(false),
            ],
            vec![
                Text("Release notes"),
                Person("Sam Rivera"),
                Select(Status::Todo),
                Date("Nov 20"),
                Checkbox(false),
            ],
            vec![
                Text("Native huddle media"),
                Person("Ana Silva"),
                Select(Status::Blocked),
                Date("TBD"),
                Checkbox(false),
            ],
            vec![
                Text("Search index migration"),
                Person("Leo Park"),
                Select(Status::Done),
                Date("Oct 28"),
                Checkbox(true),
            ],
        ],
    }
}

/// The List tab body: title row plus a horizontally and vertically scrollable table.
pub fn list_view(cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let list = fixture();

    let header_row = list.columns.iter().fold(
        h_flex()
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD),
        |row, column| {
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

    let rows = list.rows.iter().map(|fields| {
        list.columns.iter().zip(fields).fold(
            h_flex().border_t_1().border_color(theme.border).text_sm(),
            |row, (column, field)| {
                row.child(
                    h_flex()
                        .w(px(column.width))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(cell(field, cx)),
                )
            },
        )
    });

    let table = v_flex()
        .flex_none()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(header_row)
        .children(rows);

    div().id("list-scroll").size_full().overflow_scroll().child(
        v_flex()
            .px_6()
            .py_5()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
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
                    .child({
                        let csv = fixture_to_csv(&list);
                        let suggested = format!("{}.csv", sanitize_filename(list.title));
                        Button::new("list-export-csv-fixture")
                            .outline()
                            .with_size(Size::XSmall)
                            .icon(IconName::Download)
                            .label("CSV")
                            .tooltip("Download list as CSV")
                            .on_click(move |_, window, cx| {
                                export_file(csv.clone(), suggested.clone(), "CSV", window, cx);
                            })
                    })
                    .child({
                        let json = fixture_to_json(&list);
                        let suggested = format!("{}.json", sanitize_filename(list.title));
                        Button::new("list-export-json-fixture")
                            .outline()
                            .with_size(Size::XSmall)
                            .icon(IconName::Download)
                            .label("JSON")
                            .tooltip("Download list as JSON")
                            .on_click(move |_, window, cx| {
                                export_file(json.clone(), suggested.clone(), "JSON", window, cx);
                            })
                    })
                    .child(read_only_badge(cx)),
            )
            .child(table),
    )
}

fn cell(field: &Field, cx: &App) -> Div {
    let theme = cx.theme();
    match field {
        Field::Text(text) => div().truncate().child(*text),
        Field::Date(text) => div().text_color(theme.muted_foreground).child(*text),
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
            .child(div().truncate().child(*name)),
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

/// Native live read-only Slack Lists viewer: shows loading, error with browser fallback, or a
/// searchable, column-sortable table of typed Slack List records.
#[allow(clippy::too_many_arguments)]
pub fn live_list_view(
    loaded: Option<&Result<SlackListData, String>>,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    on_sort: Option<SortColumnFn>,
    open_browser: Option<OpenBrowserFn>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
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
        Some(Ok(data)) => render_live_list(
            data,
            fixtures,
            query,
            search,
            sort,
            on_sort,
            open_browser,
            cx,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_live_list(
    data: &SlackListData,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    on_sort: Option<SortColumnFn>,
    open_browser: Option<OpenBrowserFn>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let visible_indices = filter_and_sort_rows(data, fixtures, query, sort);

    let header_row = data.columns.iter().enumerate().fold(
        h_flex()
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD),
        |row, (ix, column)| {
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

    let rows_ui = visible_indices.iter().map(|&row_ix| {
        let rec = &data.rows[row_ix];
        data.columns.iter().fold(
            h_flex().border_t_1().border_color(theme.border).text_sm(),
            |row, column| {
                let width = live_column_width(column);
                let val = row_cell(rec, column);
                row.child(
                    h_flex()
                        .w(px(width))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(live_cell(val, column, fixtures, cx)),
                )
            },
        )
    });

    let body = if data.columns.is_empty() {
        div()
            .py_8()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("This list has no columns.")
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

    let find_bar = search.map(|input| {
        h_flex()
            .w(px(320.))
            .h(px(30.))
            .px_2p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(input).appearance(false).bordered(false)),
            )
            .when(!query.trim().is_empty(), |row| {
                let clear_input = input.clone();
                row.child(
                    Button::new("list-find-clear")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            clear_input.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                        }),
                )
            })
    });

    div().id("list-scroll").size_full().overflow_scroll().child(
        v_flex()
            .px_6()
            .py_5()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Icon::new(IconName::ListChecks)
                            .text_color(theme.success)
                            .size(px(18.)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(data.title.clone()),
                                    )
                                    .child(
                                        div().text_xs().text_color(theme.muted_foreground).child(
                                            format!(
                                                "{} of {} items",
                                                visible_indices.len(),
                                                data.rows.len()
                                            ),
                                        ),
                                    ),
                            )
                            .when(!data.description.is_empty(), |col| {
                                col.child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(data.description.clone()),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .children(find_bar)
                    .child({
                        let csv = list_to_csv(data, fixtures);
                        let suggested = format!("{}.csv", sanitize_filename(&data.title));
                        Button::new("list-export-csv")
                            .outline()
                            .with_size(Size::XSmall)
                            .icon(IconName::Download)
                            .label("CSV")
                            .tooltip("Download list as CSV")
                            .on_click(move |_, window, cx| {
                                export_file(csv.clone(), suggested.clone(), "CSV", window, cx);
                            })
                    })
                    .child({
                        let json = list_to_json(data, fixtures);
                        let suggested = format!("{}.json", sanitize_filename(&data.title));
                        Button::new("list-export-json")
                            .outline()
                            .with_size(Size::XSmall)
                            .icon(IconName::Download)
                            .label("JSON")
                            .tooltip("Download list as JSON")
                            .on_click(move |_, window, cx| {
                                export_file(json.clone(), suggested.clone(), "JSON", window, cx);
                            })
                    })
                    .child(read_only_badge(cx))
                    .when_some(open_browser, |row, open| {
                        row.child(
                            Button::new("list-open-browser")
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::ExternalLink)
                                .label("Open in browser")
                                .on_click(move |_, window, cx| open(window, cx)),
                        )
                    }),
            )
            .child(body),
    )
}

fn row_cell<'a>(row: &'a SlackListRow, column: &SlackListColumn) -> Option<&'a SlackListCellValue> {
    row.cells
        .get(&column.id)
        .or_else(|| row.cells.get(&column.key))
}

fn live_column_icon(kind: &str) -> IconName {
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

fn live_column_width(column: &SlackListColumn) -> f32 {
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

fn slack_choice_color(name: &str, cx: &App) -> Hsla {
    let theme = cx.theme();
    match name {
        "green" => theme.success,
        "red" => theme.danger,
        "blue" | "indigo" => theme.link,
        "yellow" | "orange" => rgb(0xd97706).into(),
        "purple" => rgb(0x7c3aed).into(),
        "pink" => rgb(0xdb2777).into(),
        "cyan" => rgb(0x0891b2).into(),
        "brown" => rgb(0x92400e).into(),
        _ => theme.muted_foreground,
    }
}

fn cell_sort_key(
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
    if let Some((col_ix, asc)) = sort {
        if let Some(col) = data.columns.get(col_ix) {
            indices.sort_by(|&a, &b| {
                let ka = cell_sort_key(row_cell(&data.rows[a], col), col, fixtures);
                let kb = cell_sort_key(row_cell(&data.rows[b], col), col, fixtures);
                if asc { ka.cmp(&kb) } else { kb.cmp(&ka) }
            });
        }
    }
    indices
}

fn live_cell(
    val: Option<&SlackListCellValue>,
    column: &SlackListColumn,
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
                        "list-link-{href}"
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

fn fixture_to_csv(list: &SlackList) -> String {
    let mut out = String::new();
    for (ix, col) in list.columns.iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        push_csv_field(&mut out, col.name);
    }
    out.push('\n');
    for row in &list.rows {
        for (ix, field) in row.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            let val = match field {
                Field::Text(s) | Field::Person(s) | Field::Date(s) => *s,
                Field::Select(Status::Todo) => "To do",
                Field::Select(Status::InProgress) => "In progress",
                Field::Select(Status::Blocked) => "Blocked",
                Field::Select(Status::Done) => "Done",
                Field::Checkbox(true) => "true",
                Field::Checkbox(false) => "false",
            };
            push_csv_field(&mut out, val);
        }
        out.push('\n');
    }
    out
}

/// Serialize a live Slack List to RFC 4180 CSV (`F-COLLAB-217`), resolving user IDs and
/// select option labels cleanly.
pub fn list_to_csv(data: &SlackListData, fixtures: &Fixtures) -> String {
    let mut out = String::new();
    for (ix, col) in data.columns.iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        push_csv_field(&mut out, &col.name);
    }
    out.push('\n');
    for row in &data.rows {
        for (ix, col) in data.columns.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            let val = match row_cell(row, col) {
                None | Some(SlackListCellValue::Empty) => String::new(),
                Some(SlackListCellValue::Text(s)) | Some(SlackListCellValue::Date(s)) => s.clone(),
                Some(SlackListCellValue::Link { url, label }) => {
                    if label.is_empty() || label == url {
                        url.clone()
                    } else {
                        format!("{label} ({url})")
                    }
                }
                Some(SlackListCellValue::Number(n)) => n.to_string(),
                Some(SlackListCellValue::Checkbox(checked)) => {
                    if *checked { "true" } else { "false" }.to_owned()
                }
                Some(SlackListCellValue::Users(users)) => users
                    .iter()
                    .map(|u| fixtures.user_name(u))
                    .collect::<Vec<_>>()
                    .join("; "),
                Some(SlackListCellValue::Select(opts)) => opts
                    .iter()
                    .map(|opt| {
                        col.choices
                            .iter()
                            .find(|c| c.value == *opt)
                            .map(|c| c.label.as_str())
                            .unwrap_or(opt.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            push_csv_field(&mut out, &val);
        }
        out.push('\n');
    }
    out
}

fn push_csv_field(out: &mut String, field: &str) {
    if field.contains([',', '"', '\n', '\r']) {
        out.push('"');
        for ch in field.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(field);
    }
}

fn sanitize_filename(name: &str) -> String {
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

fn fixture_to_json(list: &SlackList) -> String {
    use crate::blockkit::serde_json::{Map, Value, json};
    let rows: Vec<Value> = list
        .rows
        .iter()
        .map(|fields| {
            let mut obj = Map::new();
            for (col, field) in list.columns.iter().zip(fields) {
                let val = match field {
                    Field::Text(s) | Field::Person(s) | Field::Date(s) => {
                        Value::String((*s).into())
                    }
                    Field::Select(Status::Todo) => Value::String("To do".into()),
                    Field::Select(Status::InProgress) => Value::String("In progress".into()),
                    Field::Select(Status::Blocked) => Value::String("Blocked".into()),
                    Field::Select(Status::Done) => Value::String("Done".into()),
                    Field::Checkbox(b) => Value::Bool(*b),
                };
                obj.insert(col.name.to_owned(), val);
            }
            Value::Object(obj)
        })
        .collect();
    let payload = json!({
        "title": list.title,
        "columns": list.columns.iter().map(|c| c.name).collect::<Vec<_>>(),
        "items": rows,
    });
    crate::blockkit::serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// Serialize a live Slack List to pretty-printed JSON (`F-COLLAB-218`), resolving user IDs and
/// select option labels cleanly.
pub fn list_to_json(data: &SlackListData, fixtures: &Fixtures) -> String {
    use crate::blockkit::serde_json::{Map, Value, json};
    let items: Vec<Value> = data
        .rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            obj.insert("id".to_owned(), Value::String(row.id.clone()));
            for col in &data.columns {
                let val = match row_cell(row, col) {
                    None | Some(SlackListCellValue::Empty) => Value::Null,
                    Some(SlackListCellValue::Text(s)) | Some(SlackListCellValue::Date(s)) => {
                        Value::String(s.clone())
                    }
                    Some(SlackListCellValue::Number(n)) => n
                        .parse::<f64>()
                        .ok()
                        .and_then(crate::blockkit::serde_json::Number::from_f64)
                        .map(Value::Number)
                        .unwrap_or_else(|| Value::String(n.clone())),
                    Some(SlackListCellValue::Checkbox(b)) => Value::Bool(*b),
                    Some(SlackListCellValue::Link { label, url }) => json!({
                        "label": label,
                        "url": url,
                    }),
                    Some(SlackListCellValue::Users(users)) => Value::Array(
                        users
                            .iter()
                            .map(|u| Value::String(fixtures.user_name(u).to_owned()))
                            .collect(),
                    ),
                    Some(SlackListCellValue::Select(opts)) => Value::Array(
                        opts.iter()
                            .map(|opt| {
                                let label = col
                                    .choices
                                    .iter()
                                    .find(|c| c.value == *opt)
                                    .map(|c| c.label.as_str())
                                    .unwrap_or(opt.as_str());
                                Value::String(label.to_owned())
                            })
                            .collect(),
                    ),
                };
                obj.insert(col.name.clone(), val);
            }
            Value::Object(obj)
        })
        .collect();
    let payload = json!({
        "file_id": data.file_id,
        "title": data.title,
        "description": data.description,
        "columns": data.columns.iter().map(|c| json!({ "id": c.id, "name": c.name, "type": c.kind })).collect::<Vec<_>>(),
        "items": items,
    });
    crate::blockkit::serde_json::to_string_pretty(&payload).unwrap_or_default()
}

fn export_file(
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
            let result = std::fs::write(&path, content);
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

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::api::SlackListChoice;
    use std::collections::HashMap;

    #[test]
    fn filters_and_sorts_live_slack_list_rows() {
        let fixtures = crate::fixtures::load();
        let data = SlackListData {
            file_id: "F1".into(),
            title: "Tasks".into(),
            description: String::new(),
            columns: vec![
                SlackListColumn {
                    id: "C1".into(),
                    key: "task".into(),
                    name: "Task".into(),
                    kind: "text".into(),
                    is_primary: true,
                    choices: Vec::new(),
                },
                SlackListColumn {
                    id: "C2".into(),
                    key: "status".into(),
                    name: "Status".into(),
                    kind: "select".into(),
                    is_primary: false,
                    choices: vec![
                        SlackListChoice {
                            value: "opt_done".into(),
                            label: "Done".into(),
                            color: "green".into(),
                        },
                        SlackListChoice {
                            value: "opt_todo".into(),
                            label: "To do".into(),
                            color: "gray".into(),
                        },
                    ],
                },
            ],
            rows: vec![
                SlackListRow {
                    id: "R1".into(),
                    cells: HashMap::from([
                        ("C1".into(), SlackListCellValue::Text("Zebra work".into())),
                        (
                            "C2".into(),
                            SlackListCellValue::Select(vec!["opt_todo".into()]),
                        ),
                    ]),
                },
                SlackListRow {
                    id: "R2".into(),
                    cells: HashMap::from([
                        ("C1".into(), SlackListCellValue::Text("Alpha launch".into())),
                        (
                            "C2".into(),
                            SlackListCellValue::Select(vec!["opt_done".into()]),
                        ),
                    ]),
                },
            ],
        };

        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "", Some((0, true))),
            vec![1, 0]
        );
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "", Some((0, false))),
            vec![0, 1]
        );
        // Query matches resolved select choice label ("done") as well as text.
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "done", None),
            vec![1]
        );
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "zebra", None),
            vec![0]
        );

        // RFC 4180 CSV export resolves select labels and preserves column order.
        let mut csv_data = data.clone();
        csv_data.rows[0].cells.insert(
            "C1".into(),
            SlackListCellValue::Text("Zebra, \"quoted\" work".into()),
        );
        let csv = list_to_csv(&csv_data, &fixtures);
        assert_eq!(
            csv,
            "Task,Status\n\"Zebra, \"\"quoted\"\" work\",To do\nAlpha launch,Done\n"
        );

        // JSON export resolves select labels and preserves metadata + items (`F-COLLAB-218`).
        let json_str = list_to_json(&data, &fixtures);
        let parsed: crate::blockkit::serde_json::Value =
            crate::blockkit::serde_json::from_str(&json_str).expect("valid JSON");
        assert_eq!(parsed["title"], "Tasks");
        assert_eq!(parsed["items"][0]["Task"], "Zebra work");
        assert_eq!(parsed["items"][0]["Status"][0], "To do");
        assert_eq!(parsed["items"][1]["Status"][0], "Done");
    }
}
