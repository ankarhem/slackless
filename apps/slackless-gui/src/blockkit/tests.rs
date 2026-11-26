use super::*;
use gpui_kit::gpui;
use serde_json::json;

/// [`collect_modal_state_errors`] with the first error (sorted by block) as the message.
fn collect_modal_state(
    ts: &Ts,
    raw_blocks: &[Json],
    kit: &Entity<BlockKitState>,
    cx: &App,
) -> Result<Json, String> {
    collect_modal_state_errors(ts, raw_blocks, kit, cx).map_err(|mut map| {
        let mut keys: Vec<String> = map.keys().cloned().collect();
        keys.sort();
        keys.first()
            .and_then(|k| map.remove(k))
            .unwrap_or_else(|| "Missing or invalid field.".to_owned())
    })
}

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

#[test]
fn timepicker_slots_cover_the_day_in_15_minute_steps() {
    // Timepicker 15-minute slots cover full 24-hour day (96 slots: 00:00..23:45)
    let slots = timepicker_slots();
    assert_eq!(slots.len(), 96);
    assert_eq!(
        slots.first(),
        Some(&("00:00".to_owned(), "00:00".to_owned()))
    );
    assert_eq!(slots[48], ("12:00".to_owned(), "12:00".to_owned()));
    assert_eq!(
        slots.last(),
        Some(&("23:45".to_owned(), "23:45".to_owned()))
    );
}

#[test]
fn filtered_conversations_supports_private_im_mpim_and_exclude_bot_users() {
    use slackless_core::types::{Conversation, User};

    let mut fixtures = crate::fixtures::load();
    fixtures.users.push(User {
        id: UserId::from("U_BOT"),
        name: "bot".into(),
        is_bot: true,
        ..Default::default()
    });
    fixtures.users.push(User {
        id: UserId::from("U_HUM"),
        name: "human".into(),
        ..Default::default()
    });
    fixtures.channels = vec![
        Conversation {
            id: ChannelId::from("C_PUB"),
            name: Some("pub".into()),
            is_channel: true,
            ..Default::default()
        },
        Conversation {
            id: ChannelId::from("G_PRIV"),
            name: Some("priv".into()),
            is_channel: true,
            is_private: true,
            ..Default::default()
        },
        Conversation {
            id: ChannelId::from("D_HUM"),
            is_im: true,
            user: Some(UserId::from("U_HUM")),
            ..Default::default()
        },
        Conversation {
            id: ChannelId::from("D_BOT"),
            is_im: true,
            user: Some(UserId::from("U_BOT")),
            ..Default::default()
        },
        Conversation {
            id: ChannelId::from("G_MPIM"),
            name: Some("mpdm-a-b".into()),
            is_mpim: true,
            ..Default::default()
        },
    ];

    let ids = |el: &Json, kind: &str| -> Vec<String> {
        filtered_conversations(el, kind, &fixtures)
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    };

    assert_eq!(
        ids(
            &json!({ "filter": { "include": ["private"] } }),
            "conversations_select"
        ),
        vec!["G_PRIV"]
    );
    assert_eq!(
        ids(
            &json!({ "filter": { "include": ["im"], "exclude_bot_users": true } }),
            "conversations_select"
        ),
        vec!["D_HUM"]
    );
    assert_eq!(
        ids(
            &json!({ "filter": { "include": ["mpim"] } }),
            "conversations_select"
        ),
        vec!["G_MPIM"]
    );
    assert_eq!(ids(&json!({}), "channels_select"), vec!["C_PUB"]);
}

#[test]
fn attachment_helpers_classify_page_previews_parse_ts_and_strip_link_markup() {
    let mut preview = Attachment {
        from_url: Some("https://example.com".into()),
        service_name: Some("Example".into()),
        ..Default::default()
    };
    assert!(attachments::is_page_preview_attachment(&preview));

    // Color bar disqualifies plain page preview.
    preview.color = Some("good".into());
    assert!(!attachments::is_page_preview_attachment(&preview));
    preview.color = None;

    // Message unfurl or app unfurl disqualifies page preview.
    preview.is_msg_unfurl = true;
    assert!(!attachments::is_page_preview_attachment(&preview));
    preview.is_msg_unfurl = false;
    preview.extra.insert("is_app_unfurl".into(), json!(true));
    assert!(!attachments::is_page_preview_attachment(&preview));

    // attachment_secs handles int, float string, and missing/invalid.
    let mut att = Attachment::default();
    assert_eq!(attachments::attachment_secs(&att), None);
    att.ts = Some(json!(1_700_000_000));
    assert_eq!(attachments::attachment_secs(&att), Some(1_700_000_000));
    att.ts = Some(json!("1700000123.456000"));
    assert_eq!(attachments::attachment_secs(&att), Some(1_700_000_123));
    att.ts = Some(json!("not-a-ts"));
    assert_eq!(attachments::attachment_secs(&att), None);

    // link_labels strips Slack angle brackets cleanly.
    assert_eq!(
        attachments::link_labels("Built by <https://ci.example|Buildkite> · <https://status.dev>"),
        "Built by Buildkite · https://status.dev"
    );
    assert_eq!(attachments::link_labels("unclosed <tag"), "unclosed <tag");
}

#[gpui::test]
fn collect_modal_state_covers_all_input_types_required_and_validation_errors(
    cx: &mut gpui::TestAppContext,
) {
    let ts = Ts::from("m1");
    let blocks = vec![
        json!({
            "type": "input",
            "block_id": "b_date",
            "label": { "type": "plain_text", "text": "Date" },
            "element": { "type": "datepicker", "action_id": "a_date", "initial_date": "2026-01-01" }
        }),
        json!({
            "type": "input",
            "block_id": "b_time",
            "label": { "type": "plain_text", "text": "Time" },
            "element": { "type": "timepicker", "action_id": "a_time", "initial_time": "09:30" }
        }),
        json!({
            "type": "input",
            "block_id": "b_chk",
            "label": { "type": "plain_text", "text": "Checks" },
            "element": { "type": "checkboxes", "action_id": "a_chk" }
        }),
        json!({
            "type": "input",
            "block_id": "b_sel",
            "label": { "type": "plain_text", "text": "Priority" },
            "element": {
                "type": "static_select",
                "action_id": "a_sel",
                "initial_option": { "value": "p2" }
            }
        }),
        json!({
            "type": "input",
            "block_id": "b_usr",
            "label": { "type": "plain_text", "text": "Owner" },
            "element": { "type": "users_select", "action_id": "a_usr", "initial_user": "U0ME" }
        }),
        json!({
            "type": "input",
            "block_id": "b_conv",
            "label": { "type": "plain_text", "text": "Channel" },
            "element": {
                "type": "conversations_select",
                "action_id": "a_conv",
                "initial_conversation": "C0ENG"
            }
        }),
    ];

    // Missing required checkboxes returns an error naming the label.
    let empty_kit = cx.new(|_| BlockKitState::default());
    cx.update(|cx| {
        let err = collect_modal_state(&ts, &blocks, &empty_kit, cx).unwrap_err();
        assert!(err.contains("Checks"), "unexpected err: {err}");
    });

    // Populating state overrides initial values and produces Slack's state.values schema.
    let filled_kit = cx.new(|_| {
        let mut st = BlockKitState::default();
        st.choices
            .insert("m1/b_date/a_date".into(), "2026-12-31".into());
        st.checked
            .insert("m1/b_chk/a_chk".into(), HashSet::from(["opt_a".to_owned()]));
        st
    });
    cx.update(|cx| {
        let state = collect_modal_state(&ts, &blocks, &filled_kit, cx).expect("valid state");
        assert_eq!(state["b_date"]["a_date"]["selected_date"], "2026-12-31");
        assert_eq!(state["b_time"]["a_time"]["selected_time"], "09:30");
        assert_eq!(
            state["b_chk"]["a_chk"]["selected_options"][0]["value"],
            "opt_a"
        );
        assert_eq!(state["b_sel"]["a_sel"]["selected_option"]["value"], "p2");
        assert_eq!(state["b_usr"]["a_usr"]["selected_user"], "U0ME");
        assert_eq!(state["b_conv"]["a_conv"]["selected_conversation"], "C0ENG");
    });

    // Text input validation error surfaces with field label.
    let bad_email = vec![json!({
        "type": "input",
        "block_id": "b_email",
        "label": { "type": "plain_text", "text": "Contact" },
        "element": {
            "type": "email_text_input",
            "action_id": "a_email",
            "initial_value": "not-an-email"
        }
    })];
    cx.update(|cx| {
        let err = collect_modal_state(&ts, &bad_email, &empty_kit, cx).unwrap_err();
        assert!(err.contains("Contact"), "unexpected err: {err}");
    });
}

#[gpui::test]
fn external_select_inline_errors_workflow_and_hydrated_call_blocks_render(
    cx: &mut gpui::TestAppContext,
) {
    use gpui::{Render, Window};

    struct RichBlockHost {
        fixtures: std::sync::Arc<Fixtures>,
        kit: Entity<BlockKitState>,
        blocks: Vec<Json>,
        on_suggest: SuggestHandler,
        on_wf: WorkflowDetailsHandler,
        on_call: CallRefreshHandler,
    }
    impl Render for RichBlockHost {
        fn render(
            &mut self,
            window: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ts = Ts::from("modal-V_TEST");
            prepare_raw_inputs(&self.kit, &ts, &self.blocks, window, cx);
            let kcx = KitCx::new(&self.fixtures, Some(&self.kit), &ts)
                .with_suggestions(self.on_suggest.clone())
                .with_workflow_details(self.on_wf.clone())
                .with_call_refresh(self.on_call.clone());
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
        json!({
            "type": "input",
            "block_id": "b_title",
            "label": { "type": "plain_text", "text": "Title" },
            "element": { "type": "plain_text_input", "action_id": "a_title" }
        }),
        json!({
            "type": "input",
            "block_id": "b_ext",
            "label": { "type": "plain_text", "text": "Issue" },
            "element": {
                "type": "external_select",
                "action_id": "a_ext",
                "min_query_length": 2,
                "placeholder": { "type": "plain_text", "text": "Pick issue" },
                "initial_option": {
                    "text": { "type": "plain_text", "text": "ENG-101" },
                    "value": "ENG-101"
                }
            }
        }),
        json!({
            "type": "input",
            "block_id": "b_mext",
            "optional": true,
            "label": { "type": "plain_text", "text": "Labels" },
            "element": {
                "type": "multi_external_select",
                "action_id": "a_mext",
                "placeholder": { "type": "plain_text", "text": "Pick labels" }
            }
        }),
        json!({
            "type": "actions",
            "block_id": "b_wf",
            "elements": [
                {
                    "type": "workflow_button",
                    "action_id": "wf_run",
                    "style": "primary",
                    "text": { "type": "plain_text", "text": "Deploy now" },
                    "workflow": {
                        "trigger": {
                            "url": "https://slack.com/shortcuts/Ft0123ABC/xyz",
                            "visible_to_user_ids": ["U0ME"]
                        }
                    }
                },
                {
                    "type": "workflow_button",
                    "action_id": "wf_hidden",
                    "text": { "type": "plain_text", "text": "Admin only" },
                    "workflow": {
                        "trigger": {
                            "url": "https://slack.com/shortcuts/Ft9999/xyz",
                            "visible_to_user_ids": ["U0OTHER"]
                        }
                    }
                }
            ]
        }),
        json!({
            "type": "call",
            "call_id": "R0CALL1",
            "call": {
                "v1": {
                    "id": "R0CALL1",
                    "title": "Design Sync",
                    "join_url": "https://acme.zoom.us/j/5551234",
                    "desktop_app_join_url": "zoommtg://zoom.us/join?confno=5551234",
                    "active_participants": [{ "slack_id": "U0ME" }]
                }
            }
        }),
        json!({
            "type": "call",
            "call_id": "R0CALL_END",
            "call": {
                "id": "R0CALL_END",
                "title": "Retro",
                "date_start": 1700000000,
                "date_end": 1700001800,
                "join_url": "https://meet.google.com/abc-defg-hij"
            }
        }),
    ];

    let fixtures = std::sync::Arc::new(crate::fixtures::load());
    let kit = cx.new(|_| {
        let mut st = BlockKitState::default();
        st.set_block_errors(HashMap::from([(
            "b_title".to_owned(),
            "Title is already in use.".to_owned(),
        )]));
        st.set_suggestions(
            "modal-V_TEST/b_ext/a_ext".to_owned(),
            vec![
                ("ENG-101 · Fix OAuth".to_owned(), "ENG-101".to_owned()),
                ("ENG-204 · GPUI perf".to_owned(), "ENG-204".to_owned()),
            ],
        );
        st.set_call_info(
            "R0CALL1".to_owned(),
            slackless_core::api::CallInfo {
                id: "R0CALL1".to_owned(),
                title: Some("Design Sync (Hydrated)".to_owned()),
                date_start: Some(1700000000),
                date_end: None,
                join_url: Some("https://acme.zoom.us/j/5551234".to_owned()),
                desktop_app_join_url: Some("zoommtg://zoom.us/join?confno=5551234".to_owned()),
                users: vec![slackless_core::api::CallParticipant {
                    slack_id: Some("U0ME".to_owned()),
                    ..Default::default()
                }],
                ..Default::default()
            },
        );
        st
    });

    let kit_for_check = kit.clone();
    let blocks_for_check = blocks.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| RichBlockHost {
        fixtures,
        kit,
        blocks,
        on_suggest: Rc::new(|_, _| {}),
        on_wf: Rc::new(|_, _, _| {}),
        on_call: Rc::new(|_, _| {}),
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        let st = kit_for_check.read(cx);
        assert_eq!(st.block_error("b_title"), Some("Title is already in use."));
        assert!(
            st.suggestion_queries
                .contains_key("modal-V_TEST/b_ext/a_ext")
        );
        assert!(
            st.suggestion_queries
                .contains_key("modal-V_TEST/b_mext/a_mext")
        );
        assert_eq!(
            st.call_info("R0CALL1").and_then(|c| c.title.as_deref()),
            Some("Design Sync (Hydrated)")
        );

        let ts = Ts::from("modal-V_TEST");
        let errs =
            collect_modal_state_errors(&ts, &blocks_for_check, &kit_for_check, cx).unwrap_err();
        assert!(errs.contains_key("b_title"));
        assert!(
            !errs.contains_key("b_ext"),
            "initial_option satisfies required external_select"
        );
    });
}
