use super::*;
use gpui_kit::gpui;
use serde_json::json;

#[test]
fn validates_email_url_number_and_plain_text_inputs() {
    let email = json!({ "type": "email_text_input" });
    assert!(validate_text_input(&email, "").is_none());
    assert!(validate_text_input(&email, "alice@example.com").is_none());
    assert!(validate_text_input(&email, "not-an-email").is_some());

    let url = json!({ "type": "url_text_input" });
    assert!(validate_text_input(&url, "https://slack.com/app").is_none());
    assert!(validate_text_input(&url, "ftp://bad").is_some());

    let int_num = json!({ "type": "number_input", "is_decimal_allowed": false, "min_value": "1", "max_value": "10" });
    assert!(validate_text_input(&int_num, "5").is_none());
    assert!(validate_text_input(&int_num, "5.5").is_some());
    assert!(validate_text_input(&int_num, "0").is_some());
    assert!(validate_text_input(&int_num, "11").is_some());

    let plain = json!({ "type": "plain_text_input", "min_length": 3, "max_length": 5 });
    assert!(validate_text_input(&plain, "hi").is_some());
    assert!(validate_text_input(&plain, "hey").is_none());
    assert!(validate_text_input(&plain, "toolong").is_some());
}

#[test]
fn converts_standard_markdown_to_mrkdwn_preserving_code() {
    let md = "### Deploy Summary\n\
              Status: **green** — see [dashboard](https://grafana.example) and ~~old~~ `**keep** [a](b)`\n\
              ```\n\
              **verbatim** [link](https://example.com)\n\
              ```";
    let out = standard_markdown_to_mrkdwn(md);
    assert_eq!(
        out,
        "*Deploy Summary*\n\
         Status: *green* — see <https://grafana.example|dashboard> and ~old~ `**keep** [a](b)`\n\
         ```\n\
         **verbatim** [link](https://example.com)\n\
         ```"
    );
}

#[gpui::test]
fn renders_all_modern_block_kit_blocks_without_panicking(cx: &mut gpui::TestAppContext) {
    use gpui::{Render, Window};

    struct BlockHost {
        fixtures: std::sync::Arc<Fixtures>,
        kit: Entity<BlockKitState>,
        blocks: Vec<Json>,
    }
    impl Render for BlockHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ts = Ts::from("1700000001.000100");
            prepare_raw_inputs(&self.kit, &ts, &self.blocks, window, cx);
            let kcx = KitCx::new(&self.fixtures, Some(&self.kit), &ts);
            div()
                .size_full()
                .p_4()
                .child(render_raw_blocks(&self.blocks, &kcx, cx))
        }
    }

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let blocks = vec![
        json!({ "type": "markdown", "text": "## Summary\nAll **checks** passed on [CI](https://ci.example)." }),
        json!({ "type": "alert", "style": "warning", "title": { "type": "plain_text", "text": "Heads up" }, "text": { "type": "mrkdwn", "text": "Deploy window opens at 14:00." } }),
        json!({ "type": "callout", "severity": "danger", "text": "Incident active" }),
        json!({
            "type": "card",
            "title": { "type": "plain_text", "text": "Service API" },
            "subtitle": "Production",
            "fields": [
                { "type": "mrkdwn", "text": "*Latency*\n12ms" },
                { "type": "mrkdwn", "text": "*Errors*\n0.01%" }
            ],
            "actions": [
                { "type": "button", "action_id": "open_runbook", "text": { "type": "plain_text", "text": "Runbook" }, "value": "rb" }
            ]
        }),
        json!({
            "type": "carousel",
            "elements": [
                { "type": "card", "title": "Slide 1", "subtitle": "First" },
                { "type": "card", "title": "Slide 2", "subtitle": "Second" }
            ]
        }),
        json!({
            "type": "video",
            "title": { "type": "plain_text", "text": "Release walkthrough" },
            "title_url": "https://video.example/1",
            "description": { "type": "plain_text", "text": "Demo of v2.4" },
            "provider_name": "Loom"
        }),
        json!({
            "type": "call",
            "call": {
                "title": "Standup",
                "active_participants": [{ "slack_id": "U0ALICE" }],
                "join_url": "https://call.example/join"
            }
        }),
        json!({ "type": "file", "external_id": "report-2026.csv" }),
        json!({
            "type": "rich_text",
            "elements": [
                { "type": "rich_text_section", "elements": [{ "type": "text", "text": "Nested rich text" }] }
            ]
        }),
        json!({
            "type": "context_actions",
            "block_id": "ctx_act_1",
            "elements": [
                {
                    "type": "feedback_buttons",
                    "action_id": "fb_1",
                    "positive_button": { "text": { "type": "plain_text", "text": "Helpful" }, "value": "good" },
                    "negative_button": { "text": { "type": "plain_text", "text": "Not helpful" }, "value": "bad" }
                },
                {
                    "type": "icon_button",
                    "action_id": "del_1",
                    "text": { "type": "plain_text", "text": "Dismiss" },
                    "value": "dismiss"
                }
            ]
        }),
        json!({
            "type": "task_card",
            "task_id": "task_1",
            "title": "Run test suite",
            "status": "complete",
            "details": { "type": "mrkdwn", "text": "Executed *155* unit tests." },
            "sources": [{ "type": "url", "url": "https://ci.example/run/1", "text": "CI log" }]
        }),
        json!({
            "type": "plan",
            "title": "Release checklist",
            "tasks": [
                { "task_id": "t1", "title": "Build macOS bundle", "status": "complete" },
                { "task_id": "t2", "title": "Publish release notes", "status": "in_progress" },
                { "task_id": "t3", "title": "Announce in #general", "status": "pending" }
            ]
        }),
        json!({
            "type": "data_table",
            "columns": [
                { "key": "service", "label": "Service" },
                { "key": "p99", "label": "p99" }
            ],
            "rows": [
                { "service": "api", "p99": "12ms" },
                { "service": "search", "p99": "8ms" }
            ]
        }),
        json!({
            "type": "context",
            "elements": [
                { "type": "tag", "label": "Production", "color": "green" },
                { "type": "citation", "title": "Runbook", "url": "https://docs.example/rb" },
                { "type": "mrkdwn", "text": "Literal https://example.com stays unlinked", "verbatim": true }
            ]
        }),
        json!({
            "type": "actions",
            "block_id": "wf_actions",
            "elements": [
                {
                    "type": "workflow_button",
                    "text": { "type": "plain_text", "text": "Start deploy" },
                    "style": "primary",
                    "accessibility_label": "Trigger production deploy workflow",
                    "workflow": { "trigger": { "url": "https://slack.com/shortcuts/Ft123" } },
                    "confirm": {
                        "title": { "type": "plain_text", "text": "Deploy now?" },
                        "text": { "type": "mrkdwn", "text": "This starts the rollout." },
                        "confirm": { "type": "plain_text", "text": "Deploy" },
                        "deny": { "type": "plain_text", "text": "Cancel" },
                        "style": "danger"
                    }
                },
                {
                    "type": "datetimepicker",
                    "action_id": "dt_1",
                    "initial_date_time": 1_700_000_000
                }
            ]
        }),
        json!({
            "type": "image",
            "alt_text": "Slack file image",
            "slack_file": { "url": "https://files.slack.com/files-pri/T1-F1/pic.png" }
        }),
        json!({
            "type": "input",
            "block_id": "notes_blk",
            "label": { "type": "plain_text", "text": "Notes" },
            "element": { "type": "plain_text_input", "action_id": "notes_act", "initial_value": "Ready" }
        }),
    ];
    let (_view, cx) = cx.add_window_view(|_, cx| BlockHost {
        fixtures: std::sync::Arc::new(crate::fixtures::load()),
        kit: cx.new(|_| BlockKitState::default()),
        blocks,
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

#[test]
fn filters_conversations_and_preserves_option_group_headings() {
    let fixtures = crate::fixtures::load();
    let pub_only = json!({
        "type": "conversations_select",
        "filter": { "include": ["public"], "exclude_external_shared_channels": true }
    });
    let filtered = filtered_conversations(&pub_only, "conversations_select", &fixtures);
    assert!(!filtered.is_empty());
    for (_, id) in &filtered {
        let ch = fixtures
            .conversation(&slackless_core::types::ChannelId::from(id.as_str()))
            .unwrap();
        assert!(!ch.is_im && !ch.is_mpim && !ch.is_private);
        assert!(!ch.is_ext_shared && !ch.is_shared);
    }

    let grouped = json!({
        "type": "static_select",
        "option_groups": [
            {
                "label": { "type": "plain_text", "text": "Priority" },
                "options": [
                    { "text": { "type": "plain_text", "text": "High" }, "value": "p1" }
                ]
            }
        ]
    });
    assert_eq!(
        options(&grouped),
        vec![("Priority · High".to_owned(), "p1".to_owned())]
    );
}

#[gpui::test]
fn collects_rich_text_input_and_file_input_modal_state(cx: &mut gpui::TestAppContext) {
    let ts = Ts::from("modal-1");
    let blocks = vec![
        json!({
            "type": "input",
            "block_id": "rt_blk",
            "label": { "type": "plain_text", "text": "Summary" },
            "element": {
                "type": "rich_text_input",
                "action_id": "rt_act",
                "initial_value": {
                    "type": "rich_text",
                    "elements": [{
                        "type": "rich_text_section",
                        "elements": [{ "type": "text", "text": "Ship it" }]
                    }]
                }
            }
        }),
        json!({
            "type": "input",
            "block_id": "file_blk",
            "optional": true,
            "label": { "type": "plain_text", "text": "Attachment" },
            "element": { "type": "file_input", "action_id": "file_act", "max_files": 3 }
        }),
    ];
    let kit = cx.new(|_| {
        let mut st = BlockKitState::default();
        st.files.insert(
            "modal-1/file_blk/file_act".to_owned(),
            vec!["/tmp/spec.pdf".to_owned()],
        );
        st
    });
    cx.update(|cx| {
        let state = collect_modal_state(&ts, &blocks, &kit, cx).expect("valid modal state");
        assert_eq!(
            state["rt_blk"]["rt_act"]["rich_text_value"]["elements"][0]["elements"][0]["text"],
            "Ship it"
        );
        assert_eq!(
            state["file_blk"]["file_act"]["files"][0]["name"],
            "/tmp/spec.pdf"
        );
    });
}

#[test]
fn parses_hex_colors_and_rich_option_descriptions() {
    assert!(parse_hex_color("#2BAC76").is_some());
    assert!(parse_hex_color("#0f8").is_some());
    assert!(parse_hex_color("not-a-color").is_none());

    let el = json!({
        "type": "radio_buttons",
        "options": [
            {
                "text": { "type": "mrkdwn", "text": "*Fast* path" },
                "value": "fast",
                "description": { "type": "plain_text", "text": "Skips optional checks" }
            }
        ]
    });
    let parsed = rich_options(&el);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].value, "fast");
    assert_eq!(parsed[0].text_obj["type"], "mrkdwn");
    assert_eq!(
        parsed[0].description.as_ref().and_then(text_of),
        Some("Skips optional checks")
    );
}
