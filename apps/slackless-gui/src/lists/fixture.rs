//! Sample workspace fixture Slack List backed by the shared [`SlackListData`] renderer in
//! [`super::live`].

use std::collections::HashMap;
use std::sync::OnceLock;

use slackless_core::api::SlackListChoice;
use slackless_core::types::{Profile, User, UserId};

use super::*;

const FIXTURE_LIST_KEY: &str = "fixture";

/// Pre-select a record in the sample fixture list by its record ID (e.g., when navigating from
/// a list item thread card in Threads view).
pub fn select_fixture_record(record_id: &str, cx: &mut App) {
    let data = sample_list_data();
    select_list_record(FIXTURE_LIST_KEY, record_id, &data, cx);
}

fn sample_list_data() -> SlackListData {
    let columns = vec![
        SlackListColumn {
            id: "col_task".into(),
            key: "task".into(),
            name: "Task".into(),
            kind: "text".into(),
            is_primary: true,
            choices: Vec::new(),
        },
        SlackListColumn {
            id: "col_assignee".into(),
            key: "assignee".into(),
            name: "Assignee".into(),
            kind: "user".into(),
            is_primary: false,
            choices: Vec::new(),
        },
        SlackListColumn {
            id: "col_status".into(),
            key: "status".into(),
            name: "Status".into(),
            kind: "select".into(),
            is_primary: false,
            choices: vec![
                SlackListChoice {
                    value: "todo".into(),
                    label: "To do".into(),
                    color: "gray".into(),
                },
                SlackListChoice {
                    value: "in_progress".into(),
                    label: "In progress".into(),
                    color: "blue".into(),
                },
                SlackListChoice {
                    value: "blocked".into(),
                    label: "Blocked".into(),
                    color: "red".into(),
                },
                SlackListChoice {
                    value: "done".into(),
                    label: "Done".into(),
                    color: "green".into(),
                },
            ],
        },
        SlackListColumn {
            id: "col_due".into(),
            key: "due".into(),
            name: "Due".into(),
            kind: "date".into(),
            is_primary: false,
            choices: Vec::new(),
        },
        SlackListColumn {
            id: "col_shipped".into(),
            key: "shipped".into(),
            name: "Shipped".into(),
            kind: "checkbox".into(),
            is_primary: false,
            choices: Vec::new(),
        },
    ];

    let row =
        |id: &str, task: &str, person: &str, status: &str, due: &str, shipped: bool| SlackListRow {
            id: id.into(),
            cells: HashMap::from([
                ("col_task".into(), SlackListCellValue::Text(task.into())),
                (
                    "col_assignee".into(),
                    SlackListCellValue::Users(vec![UserId::from(person)]),
                ),
                (
                    "col_status".into(),
                    SlackListCellValue::Select(vec![status.into()]),
                ),
                ("col_due".into(), SlackListCellValue::Date(due.into())),
                ("col_shipped".into(), SlackListCellValue::Checkbox(shipped)),
            ]),
        };

    SlackListData {
        file_id: FIXTURE_LIST_KEY.into(),
        title: "Launch tasks".into(),
        description: String::new(),
        columns,
        rows: vec![
            row(
                "fix_1",
                "Sign the macOS build",
                "U_MAYA",
                "done",
                "Oct 10",
                true,
            ),
            row(
                "fix_2",
                "Beta invite list",
                "U_LEO",
                "in_progress",
                "Nov 1",
                false,
            ),
            row("fix_3", "Release notes", "U_SAM", "todo", "Nov 20", false),
            row(
                "fix_4",
                "Native huddle media",
                "U_ANA",
                "blocked",
                "TBD",
                false,
            ),
            row(
                "fix_5",
                "Search index migration",
                "U_LEO",
                "done",
                "Oct 28",
                true,
            ),
        ],
    }
}

fn fixture_users_snapshot() -> &'static Fixtures {
    static SNAPSHOT: OnceLock<Fixtures> = OnceLock::new();
    SNAPSHOT.get_or_init(|| {
        let mk_user = |id: &str, handle: &str, real_name: &str| User {
            id: UserId::from(id),
            name: handle.into(),
            real_name: Some(real_name.into()),
            profile: Profile {
                display_name: Some(real_name.into()),
                real_name: Some(real_name.into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut f = crate::fixtures::load();
        f.replace_users(vec![
            mk_user("U_MAYA", "maya", "Maya Chen"),
            mk_user("U_LEO", "leo", "Leo Park"),
            mk_user("U_SAM", "sam", "Sam Rivera"),
            mk_user("U_ANA", "ana", "Ana Silva"),
        ]);
        f
    })
}

fn cell_input_to_value(value: ListCellInputValue) -> SlackListCellValue {
    match value {
        ListCellInputValue::Text(s) => SlackListCellValue::Text(s),
        ListCellInputValue::Checkbox(b) => SlackListCellValue::Checkbox(b),
        ListCellInputValue::Select(opts) => SlackListCellValue::Select(opts),
        ListCellInputValue::Date(dates) => dates
            .into_iter()
            .next()
            .map(SlackListCellValue::Date)
            .unwrap_or(SlackListCellValue::Empty),
        ListCellInputValue::Users(users) => SlackListCellValue::Users(users),
        ListCellInputValue::Number(nums) => nums
            .into_iter()
            .next()
            .map(|n| SlackListCellValue::Number(n.to_string()))
            .unwrap_or(SlackListCellValue::Empty),
        ListCellInputValue::Link { url, display_name } => SlackListCellValue::Link {
            label: display_name.unwrap_or_else(|| url.clone()),
            url,
        },
        ListCellInputValue::Clear => SlackListCellValue::Empty,
    }
}

/// The List tab body in fixture mode: feeds the sample [`SlackListData`] into the shared
/// Table/Board/Detail renderer with local in-memory mutations.
pub fn list_view(cx: &App) -> Stateful<Div> {
    let state = panel_state(FIXTURE_LIST_KEY, cx);
    let data = state.fixture_data.unwrap_or_else(sample_list_data);

    let on_create_item: CreateListItemFn = Rc::new(|cells, window, cx| {
        mutate_panel(
            FIXTURE_LIST_KEY,
            |s| {
                let list = s.fixture_data.get_or_insert_with(sample_list_data);
                let next_id = format!("fix_{}", list.rows.len() + 1);
                let mut map = HashMap::from([
                    (
                        "col_status".to_owned(),
                        SlackListCellValue::Select(vec!["todo".into()]),
                    ),
                    ("col_due".to_owned(), SlackListCellValue::Date("TBD".into())),
                    (
                        "col_shipped".to_owned(),
                        SlackListCellValue::Checkbox(false),
                    ),
                ]);
                for cell in cells {
                    map.insert(cell.column_id, cell_input_to_value(cell.value));
                }
                list.rows.push(SlackListRow {
                    id: next_id,
                    cells: map,
                });
            },
            window,
            cx,
        );
    });

    let on_update_cell: UpdateListCellFn = Rc::new(|row_id, cell, window, cx| {
        mutate_panel(
            FIXTURE_LIST_KEY,
            |s| {
                let list = s.fixture_data.get_or_insert_with(sample_list_data);
                if let Some(row) = list.rows.iter_mut().find(|r| r.id == row_id) {
                    row.cells
                        .insert(cell.column_id, cell_input_to_value(cell.value));
                }
            },
            window,
            cx,
        );
    });

    let on_delete_item: DeleteListItemFn = Rc::new(|row_id, window, cx| {
        mutate_panel(
            FIXTURE_LIST_KEY,
            |s| {
                let list = s.fixture_data.get_or_insert_with(sample_list_data);
                list.rows.retain(|r| r.id != row_id);
                if s.selected_record_id.as_deref() == Some(row_id.as_str()) {
                    s.selected_record_id = None;
                }
            },
            window,
            cx,
        );
    });

    let on_archive_item: DeleteListItemFn = Rc::new(|row_id, window, cx| {
        mutate_panel(
            FIXTURE_LIST_KEY,
            |s| {
                s.archived_rows.insert(row_id);
            },
            window,
            cx,
        );
    });

    let actions = ListActions {
        on_create_item: Some(on_create_item),
        on_update_cell: Some(on_update_cell),
        on_delete_item: Some(on_delete_item),
        on_archive_item: Some(on_archive_item),
        ..Default::default()
    };

    live::render_live_list(&data, fixture_users_snapshot(), "", None, None, actions, cx)
}
