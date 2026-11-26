use super::*;
use gpui_kit::gpui;
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
    let csv =
        "Task,Due,Done\r\n\"Ship \"\"v2\"\", fast\",2026-12-01,true\r\nCleanup,2026-12-05,false\n";
    let parsed = parse_csv_rows(csv);
    assert_eq!(parsed.len(), 3);
    assert_eq!(parsed[0], vec!["Task", "Due", "Done"]);
    assert_eq!(parsed[1], vec!["Ship \"v2\", fast", "2026-12-01", "true"]);
    assert_eq!(parsed[2], vec!["Cleanup", "2026-12-05", "false"]);

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

    // Board lane grouping partitions rows by the first select column (`F-COLLAB-235`, `F-COLLAB-237`).
    let lanes = group_live_rows(&data, &[0, 1]);
    assert_eq!(lanes.len(), 2);
    assert_eq!(lanes[0].0, "Done");
    assert_eq!(lanes[0].2, vec![1]);
    assert_eq!(lanes[1].0, "To do");
    assert_eq!(lanes[1].2, vec![0]);
}

#[test]
fn map_csv_row_to_cells_handles_types_fallbacks_and_empty_values() {
    let cols = vec![
        ("C_TITLE".into(), "Task".into(), "text".into()),
        ("C_DONE".into(), "Done".into(), "checkbox".into()),
        ("C_DUE".into(), "Due".into(), "due_date".into()),
        ("C_POINTS".into(), "Points".into(), "number".into()),
        ("C_STARS".into(), "Rating".into(), "rating".into()),
        ("C_BUDGET".into(), "Budget".into(), "currency".into()),
        ("C_URL".into(), "Spec".into(), "link".into()),
    ];
    let headers: Vec<String> = vec![
        "TitleMismatch".into(), // col 0 falls back to primary_ix
        "DONE".into(),          // case-insensitive match
        "due".into(),
        "Points".into(),
        "Rating".into(),
        "Budget".into(),
        "spec".into(),
        "UnknownCol".into(),
    ];
    let values: Vec<String> = vec![
        "  Ship release  ".into(),
        "YES".into(),
        "2026-12-15".into(),
        "3.5".into(),
        "not-a-num".into(), // falls back to Text
        "100".into(),
        "https://example.com/spec".into(),
        "ignored".into(),
    ];

    let cells = map_csv_row_to_cells(&headers, &values, &cols, 0);
    assert_eq!(cells.len(), 7);
    assert_eq!(cells[0].column_id, "C_TITLE");
    assert!(matches!(&cells[0].value, ListCellInputValue::Text(s) if s == "Ship release"));
    assert_eq!(cells[1].column_id, "C_DONE");
    assert!(matches!(cells[1].value, ListCellInputValue::Checkbox(true)));
    assert_eq!(cells[2].column_id, "C_DUE");
    assert!(matches!(&cells[2].value, ListCellInputValue::Date(d) if d == &["2026-12-15"]));
    assert_eq!(cells[3].column_id, "C_POINTS");
    assert!(matches!(&cells[3].value, ListCellInputValue::Number(n) if n == &[3.5]));
    assert_eq!(cells[4].column_id, "C_STARS");
    assert!(matches!(&cells[4].value, ListCellInputValue::Text(s) if s == "not-a-num"));
    assert_eq!(cells[5].column_id, "C_BUDGET");
    assert!(matches!(&cells[5].value, ListCellInputValue::Number(n) if n == &[100.0]));
    assert_eq!(cells[6].column_id, "C_URL");
    assert!(matches!(
        &cells[6].value,
        ListCellInputValue::Link { url, display_name: None } if url == "https://example.com/spec"
    ));

    // Checkbox false variants and skipping empty/whitespace values
    let values2: Vec<String> = vec!["   ".into(), "no".into(), "".into()];
    let cells2 = map_csv_row_to_cells(&headers, &values2, &cols, 0);
    assert_eq!(cells2.len(), 1);
    assert_eq!(cells2[0].column_id, "C_DONE");
    assert!(matches!(
        cells2[0].value,
        ListCellInputValue::Checkbox(false)
    ));
}

#[test]
fn parse_csv_rows_handles_embedded_newlines_blank_lines_and_trailing_field() {
    let input = "A,B\n\n  ,  \n\"line1\nline2\",\"val\"\"2\"\r\nlast1,last2";
    let rows = parse_csv_rows(input);
    assert_eq!(
        rows,
        vec![
            vec!["A", "B"],
            vec!["line1\nline2", "val\"2"],
            vec!["last1", "last2"],
        ]
    );
    assert!(parse_csv_rows("").is_empty());
    assert!(parse_csv_rows("\n\r\n   \n").is_empty());
    // Trailing empty field after comma on last line without newline
    assert_eq!(parse_csv_rows("a,b,"), vec![vec!["a", "b", ""]]);
}

#[test]
fn cell_sort_key_covers_all_variants_and_sanitize_filename_cleans_names() {
    let fixtures = crate::fixtures::load();
    let col = SlackListColumn {
        id: "C1".into(),
        key: "k".into(),
        name: "Col".into(),
        kind: "select".into(),
        is_primary: false,
        choices: vec![SlackListChoice {
            value: "opt_a".into(),
            label: "In Progress".into(),
            color: "blue".into(),
        }],
    };
    assert_eq!(cell_sort_key(None, &col, &fixtures), "");
    assert_eq!(
        cell_sort_key(Some(&SlackListCellValue::Empty), &col, &fixtures),
        ""
    );
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Text("Hello".into())),
            &col,
            &fixtures
        ),
        "hello"
    );
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Date("2026-01-02".into())),
            &col,
            &fixtures
        ),
        "2026-01-02"
    );
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Number("42".into())),
            &col,
            &fixtures
        ),
        "42"
    );
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Link {
                label: "Docs".into(),
                url: "https://example.com".into(),
            }),
            &col,
            &fixtures
        ),
        "docs"
    );
    assert_eq!(
        cell_sort_key(Some(&SlackListCellValue::Checkbox(true)), &col, &fixtures),
        "1"
    );
    assert_eq!(
        cell_sort_key(Some(&SlackListCellValue::Checkbox(false)), &col, &fixtures),
        "0"
    );
    let user_id = fixtures.users[0].id.clone();
    let expected_user = fixtures.user_name(&user_id).to_lowercase();
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Users(vec![user_id])),
            &col,
            &fixtures
        ),
        expected_user
    );
    assert_eq!(
        cell_sort_key(
            Some(&SlackListCellValue::Select(vec![
                "opt_a".into(),
                "OptUnknown".into(),
            ])),
            &col,
            &fixtures
        ),
        "in progress, optunknown"
    );

    assert_eq!(
        sanitize_filename("Q4 / Launch: Plan*"),
        "Q4 _ Launch_ Plan_"
    );
    assert_eq!(sanitize_filename("  valid-name_1  "), "valid-name_1");
    assert_eq!(sanitize_filename("   "), "slack-list");
    assert_eq!(sanitize_filename(""), "slack-list");
}

#[test]
fn group_live_rows_handles_uncategorized_unknown_options_and_no_select_column() {
    // 1. No select column -> single "All items" lane.
    let no_select = SlackListData {
        file_id: "F1".into(),
        title: "Plain".into(),
        description: String::new(),
        columns: vec![SlackListColumn {
            id: "C1".into(),
            key: "task".into(),
            name: "Task".into(),
            kind: "text".into(),
            is_primary: true,
            choices: Vec::new(),
        }],
        rows: vec![
            SlackListRow {
                id: "R1".into(),
                cells: HashMap::new(),
            },
            SlackListRow {
                id: "R2".into(),
                cells: HashMap::new(),
            },
        ],
    };
    let lanes = group_live_rows(&no_select, &[0, 1]);
    assert_eq!(lanes, vec![("All items".into(), "gray".into(), vec![0, 1])]);

    // 2. Select column with missing value -> "Uncategorized" lane + unknown choice creates ad-hoc lane.
    let with_uncat = SlackListData {
        file_id: "F2".into(),
        title: "Board".into(),
        description: String::new(),
        columns: vec![SlackListColumn {
            id: "C2".into(),
            key: "status".into(),
            name: "Status".into(),
            kind: "select".into(),
            is_primary: false,
            choices: vec![SlackListChoice {
                value: "done".into(),
                label: "Done".into(),
                color: "green".into(),
            }],
        }],
        rows: vec![
            SlackListRow {
                id: "R1".into(),
                cells: HashMap::new(),
            },
            SlackListRow {
                id: "R2".into(),
                cells: HashMap::from([(
                    "C2".into(),
                    SlackListCellValue::Select(vec!["custom_opt".into()]),
                )]),
            },
        ],
    };
    let lanes = group_live_rows(&with_uncat, &[0, 1]);
    assert_eq!(lanes.len(), 3);
    assert_eq!(lanes[0], ("Done".into(), "green".into(), vec![]));
    assert_eq!(lanes[1], ("custom_opt".into(), "gray".into(), vec![1]));
    assert_eq!(lanes[2], ("Uncategorized".into(), "gray".into(), vec![0]));
}

#[test]
fn subtask_archiving_and_view_level_archive_helpers_work_cleanly() {
    let data = SlackListData {
        file_id: "F_ARCH".into(),
        title: "Sprint".into(),
        description: String::new(),
        columns: vec![
            SlackListColumn {
                id: "C_TITLE".into(),
                key: "task".into(),
                name: "Task".into(),
                kind: "text".into(),
                is_primary: true,
                choices: Vec::new(),
            },
            SlackListColumn {
                id: "C_DONE".into(),
                key: "completed".into(),
                name: "Done".into(),
                kind: "checkbox".into(),
                is_primary: false,
                choices: Vec::new(),
            },
            SlackListColumn {
                id: "C_STATUS".into(),
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
                        value: "opt_wip".into(),
                        label: "In progress".into(),
                        color: "blue".into(),
                    },
                ],
            },
        ],
        rows: vec![
            SlackListRow {
                id: "R_PARENT".into(),
                cells: HashMap::from([
                    ("C_TITLE".into(), SlackListCellValue::Text("Ship v2".into())),
                    ("C_DONE".into(), SlackListCellValue::Checkbox(false)),
                    (
                        "C_STATUS".into(),
                        SlackListCellValue::Select(vec!["opt_wip".into()]),
                    ),
                ]),
            },
            SlackListRow {
                id: "R_SUB1".into(),
                cells: HashMap::from([
                    (
                        "C_TITLE".into(),
                        SlackListCellValue::Text("Subtask A".into()),
                    ),
                    (
                        "parent_record_id".into(),
                        SlackListCellValue::Text("R_PARENT".into()),
                    ),
                    ("C_DONE".into(), SlackListCellValue::Checkbox(true)),
                ]),
            },
            SlackListRow {
                id: "R_DONE_SEL".into(),
                cells: HashMap::from([
                    ("C_TITLE".into(), SlackListCellValue::Text("Docs".into())),
                    (
                        "C_STATUS".into(),
                        SlackListCellValue::Select(vec!["opt_done".into()]),
                    ),
                ]),
            },
        ],
    };

    // Subtask detection & parent lookup (`F-COLLAB-256`)
    assert!(!is_subtask_row(&data.rows[0], &data.columns));
    assert!(is_subtask_row(&data.rows[1], &data.columns));
    assert_eq!(
        subtask_parent_id(&data.rows[1], &data.columns).as_deref(),
        Some("R_PARENT")
    );

    let mut archived = HashSet::new();
    assert_eq!(
        subtask_rows_for_parent(&data, "R_PARENT", &archived, false),
        vec![1]
    );

    // Completed rows detected by either checkbox or Done select option (`F-COLLAB-257`)
    assert!(!is_row_completed(&data.rows[0], &data.columns));
    assert!(is_row_completed(&data.rows[1], &data.columns));
    assert!(is_row_completed(&data.rows[2], &data.columns));
    assert_eq!(
        completed_row_ids(&data, &archived),
        vec!["R_SUB1".to_owned(), "R_DONE_SEL".to_owned()]
    );

    // Archiving a subtask removes it from active view & parent subtask list unless show_archived is true
    archived.insert("R_SUB1".to_owned());
    assert!(subtask_rows_for_parent(&data, "R_PARENT", &archived, false).is_empty());
    assert_eq!(
        subtask_rows_for_parent(&data, "R_PARENT", &archived, true),
        vec![1]
    );
    assert_eq!(
        filter_archived_rows(vec![0, 1, 2], &data, &archived, false),
        vec![0, 2]
    );
    assert_eq!(
        filter_archived_rows(vec![0, 1, 2], &data, &archived, true),
        vec![0, 1, 2]
    );
    assert_eq!(
        completed_row_ids(&data, &archived),
        vec!["R_DONE_SEL".to_owned()]
    );
}

#[gpui::test]
fn list_share_drawer_and_ui_store_round_trip(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);

        let meta = ListMeta {
            file_id: "F0SHARE".into(),
            title: "Launch Tracker".into(),
            permalink: Some("https://acme.slack.com/lists/T1/F0SHARE".into()),
            editable: true,
            access: Some("owner".into()),
            org_or_workspace_access: Some("read".into()),
            editors: vec![UserId::from("U0ME")],
            shared_channels: vec![ChannelId::from("C0ENG")],
            row_count: Some(12),
            row_count_limit: Some(5000),
            over_row_maximum: false,
            comment_channel_id: ChannelId::from("C0SHARE"),
            ..Default::default()
        };
        set_list_meta("F0SHARE", meta.clone(), cx);
        assert_eq!(list_meta("F0SHARE", cx), Some(meta));

        set_list_status("F0SHARE", false, "Shared with #engineering".into(), cx);
        assert_eq!(
            list_status("F0SHARE", cx),
            Some((false, "Shared with #engineering".to_owned()))
        );
        // Subsequent client-side validation error replaces prior server status cleanly (never shadowed).
        cx.default_global::<ListUiStore>()
            .panels
            .entry("F0SHARE".to_owned())
            .or_default()
            .share_status = Some((true, "Enter at least one channel or user ID.".into()));
        assert_eq!(
            panel_state("F0SHARE", cx).share_status,
            Some((true, "Enter at least one channel or user ID.".to_owned()))
        );
        assert_eq!(
            list_status("F0SHARE", cx),
            Some((true, "Enter at least one channel or user ID.".to_owned()))
        );
        // And a subsequent server response replaces the client-side error.
        set_list_status("F0SHARE", false, "Access updated".into(), cx);
        assert_eq!(
            panel_state("F0SHARE", cx).share_status,
            Some((false, "Access updated".to_owned()))
        );

        let data = SlackListData {
            file_id: "F0SHARE".into(),
            title: "Launch Tracker".into(),
            description: String::new(),
            columns: vec![SlackListColumn {
                id: "C1".into(),
                key: "task".into(),
                name: "Task".into(),
                kind: "text".into(),
                is_primary: true,
                choices: Vec::new(),
            }],
            rows: vec![
                SlackListRow {
                    id: "RecA".into(),
                    cells: HashMap::new(),
                },
                SlackListRow {
                    id: "RecB".into(),
                    cells: HashMap::new(),
                },
            ],
        };
        select_list_record("F0SHARE", "RecB", &data, cx);
        assert_eq!(panel_state("F0SHARE", cx).selected_row, Some(1));

        select_fixture_record("fix_2", cx);
        assert_eq!(panel_state("fixture", cx).selected_row, Some(1));

        // Render live_list_view with share drawer open across all three share tabs.
        let fixtures = crate::fixtures::load();
        for tab in [
            ListShareTab::Share,
            ListShareTab::Access,
            ListShareTab::Settings,
        ] {
            cx.default_global::<ListUiStore>()
                .panels
                .entry("F0SHARE".to_owned())
                .or_default()
                .share_open = true;
            cx.default_global::<ListUiStore>()
                .panels
                .entry("F0SHARE".to_owned())
                .or_default()
                .share_tab = tab;
            let _ = live_list_view(
                Some(&Ok(data.clone())),
                &fixtures,
                "",
                None,
                None,
                ListActions::default(),
                cx,
            );
        }
    });
}
