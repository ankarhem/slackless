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
