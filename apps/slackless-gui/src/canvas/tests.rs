//! Unit and GPUI render tests for the modularized Canvas subsystem (`F-COLLAB-125..145`).

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::gpui::{self, IntoElement, Render, Window};
use slackless_core::api::CanvasEditOp;

use crate::canvas::format::{
    BlockFormat, InlineFormat, SlashCommand, add_table_column_at, add_table_row_at,
    apply_block_format, apply_inline_format, apply_slash_command, canvas_file_markdown,
    cycle_last_line_heading, detect_slash_query, filter_slash_commands, insert_markdown_table,
    insert_text_at, is_image_filename, move_last_line,
};
use crate::canvas::parse::resolve_canvas_emoji;
use crate::canvas::{
    CanvasActions, CanvasBlock, CanvasDoc, CanvasUploadCompleteFn, CanvasUploadFileFn,
    OpenBrowserFn, UploadedCanvasFile, canvas_view, parse_canvas_html, parse_canvas_markdown,
    section_delete_op, section_insert_after_op, section_replace_op,
};

#[test]
fn parses_canvas_html_blocks_and_filters_by_query() {
    let html = r#"<html><head><style>body{color:red}</style></head><body>
        <h1>Architecture &amp; Plan</h1>
        <p>Overview of the <b>native</b> reader.</p>
        <ul class="checklist">
            <li data-checked="true">Fetch HTML</li>
            <li data-checked="false">Add find bar</li>
        </ul>
        <pre><code>cargo test</code></pre>
        <blockquote>Keep it fast.</blockquote>
        <hr/>
        <table><tr><th>Owner</th><th>Task</th></tr><tr><td>Ana</td><td>Ship</td></tr></table>
    </body></html>"#;
    let blocks = parse_canvas_html(html);
    assert_eq!(
        blocks,
        vec![
            CanvasBlock::Heading {
                level: 1,
                text: "Architecture & Plan".into(),
            },
            CanvasBlock::Paragraph("Overview of the native reader.".into()),
            CanvasBlock::Checklist(vec![
                (true, "Fetch HTML".into()),
                (false, "Add find bar".into()),
            ]),
            CanvasBlock::CodeBlock("cargo test".into()),
            CanvasBlock::Quote("Keep it fast.".into()),
            CanvasBlock::Divider,
            CanvasBlock::Table {
                header: vec!["Owner".into(), "Task".into()],
                rows: vec![vec!["Ana".into(), "Ship".into()]],
            },
        ]
    );
    assert!(blocks[0].matches("architecture"));
    assert!(!blocks[0].matches("cargo"));
    assert!(blocks[3].matches("cargo"));
}

#[test]
fn parses_canvas_images_and_extracts_heading_outline() {
    let html = r#"<html><body>
        <h1>Launch Plan</h1>
        <img src="https://files.slack.com/files-pri/T1-F1/arch.png" alt="Architecture diagram" width="800" height="400" />
        <h2>Milestones</h2>
        <h3>Risks</h3>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Launch Plan".into(), html);
    assert!(doc.blocks.iter().any(|b| matches!(
        b,
        CanvasBlock::Image {
            url,
            alt,
            width: Some(800),
            height: Some(400),
        }
            if url == "https://files.slack.com/files-pri/T1-F1/arch.png"
            && alt == "Architecture diagram"
    )));
    assert_eq!(
        doc.outline_headings(),
        vec![(1, "Launch Plan"), (2, "Milestones"), (3, "Risks")]
    );
}

#[test]
fn preserves_section_ids_and_roundtrips_canvas_markdown() {
    let html = r#"<html><body>
        <h1 id="temp:C:h1">Runbook</h1>
        <ul id="temp:C:chk1" class="checklist">
            <li data-checked="true">Verify backup</li>
            <li data-checked="false">Cut release</li>
        </ul>
        <p id="temp:C:p1">All clear.</p>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Runbook".into(), html);
    assert_eq!(
        doc.section_ids,
        vec![
            Some("temp:C:h1".to_owned()),
            Some("temp:C:chk1".to_owned()),
            Some("temp:C:p1".to_owned()),
        ]
    );
    let md = doc.to_markdown();
    assert!(md.contains("# Runbook"));
    assert!(md.contains("- [x] Verify backup"));
    assert!(md.contains("- [ ] Cut release"));
    let reparsed = CanvasDoc::from_markdown("Runbook".into(), "Live".into(), &md);
    assert_eq!(reparsed.blocks, doc.blocks);
}

#[test]
fn cycles_heading_levels_and_moves_lines() {
    assert_eq!(
        cycle_last_line_heading("Notes\nShip it"),
        "Notes\n# Ship it"
    );
    assert_eq!(
        cycle_last_line_heading("Notes\n# Ship it"),
        "Notes\n## Ship it"
    );
    assert_eq!(
        cycle_last_line_heading("Notes\n## Ship it"),
        "Notes\n### Ship it"
    );
    assert_eq!(
        cycle_last_line_heading("Notes\n### Ship it"),
        "Notes\nShip it"
    );
    assert_eq!(
        move_last_line("First\nSecond\nThird", -1),
        "First\nThird\nSecond"
    );
    assert_eq!(move_last_line("Only", -1), "Only");
}

#[test]
fn context_menu_block_operations_copy_duplicate_move_and_delete() {
    let mut doc = CanvasDoc::from_markdown(
        "Spec".into(),
        "Live".into(),
        "# Intro\n\nFirst para\n\n- item a\n- item b",
    );
    assert_eq!(doc.blocks.len(), 3);
    assert_eq!(doc.block_plain_text(0).as_deref(), Some("# Intro"));
    assert_eq!(doc.block_plain_text(99), None);

    assert!(doc.duplicate_block(1));
    assert_eq!(doc.blocks.len(), 4);
    assert_eq!(doc.block_plain_text(1), doc.block_plain_text(2));
    assert!(!doc.duplicate_block(99));

    assert!(doc.move_block(3, -1));
    assert_eq!(
        doc.block_plain_text(2).as_deref(),
        Some("- item a\n- item b")
    );
    assert!(!doc.move_block(0, -1));
    assert!(!doc.move_block(3, 1));

    assert!(doc.delete_block(1));
    assert_eq!(doc.blocks.len(), 3);
    assert!(!doc.delete_block(99));

    let exported = doc.to_markdown();
    assert!(exported.starts_with("# Intro"));
    assert!(exported.contains("- item a\n- item b"));
}

#[test]
fn selection_inline_formatting_wraps_unwraps_and_handles_empty_selection() {
    // Bold wrap & toggle unwrap (both inner markers and surrounding markers).
    let (bolded, sel) = apply_inline_format("hello world", 6..11, InlineFormat::Bold);
    assert_eq!(bolded, "hello **world**");
    assert_eq!(&bolded[sel.clone()], "world");

    let (unwrapped, sel2) = apply_inline_format(&bolded, sel, InlineFormat::Bold);
    assert_eq!(unwrapped, "hello world");
    assert_eq!(&unwrapped[sel2], "world");

    // Unwrap when markers themselves are included in the selection.
    let (unwrapped_outer, _) = apply_inline_format("hello **world**", 6..15, InlineFormat::Bold);
    assert_eq!(unwrapped_outer, "hello world");

    // Italic does not accidentally unwrap bold `**...**`.
    let (italic_on_bold, _) = apply_inline_format("**world**", 2..7, InlineFormat::Italic);
    assert_eq!(italic_on_bold, "***world***");

    // Strike & Inline Code.
    let (struck, sel_s) = apply_inline_format("ship old", 5..8, InlineFormat::Strike);
    assert_eq!(struck, "ship ~~old~~");
    let (unstruck, _) = apply_inline_format(&struck, sel_s, InlineFormat::Strike);
    assert_eq!(unstruck, "ship old");

    let (coded, sel_c) = apply_inline_format("run cargo", 4..9, InlineFormat::Code);
    assert_eq!(coded, "run `cargo`");
    let (uncoded, _) = apply_inline_format(&coded, sel_c, InlineFormat::Code);
    assert_eq!(uncoded, "run cargo");

    // Link wrap and unwrap.
    let (linked, url_sel) = apply_inline_format("see docs here", 4..8, InlineFormat::Link);
    assert_eq!(linked, "see [docs](https://) here");
    assert_eq!(&linked[url_sel], "https://");
    let (unlinked, _) = apply_inline_format("see [docs](https://x.io)", 4..24, InlineFormat::Link);
    assert_eq!(unlinked, "see docs");

    // Empty selection inserts placeholder and selects it.
    let (empty_bold, ph_sel) = apply_inline_format("hi ", 3..3, InlineFormat::Bold);
    assert_eq!(empty_bold, "hi **bold**");
    assert_eq!(&empty_bold[ph_sel], "bold");

    // Multi-byte UTF-8 boundary clamping never panics.
    let (utf8_bold, _) = apply_inline_format("🚀 launch", 1..4, InlineFormat::Bold);
    assert!(utf8_bold.contains("**"));
}

#[test]
fn block_formatting_toggles_headings_quotes_lists_tables_and_code() {
    let (h1, _) = apply_block_format("Title", 0..5, BlockFormat::CycleHeading);
    assert_eq!(h1, "# Title");
    let (h2, _) = apply_block_format(&h1, 0..h1.len(), BlockFormat::CycleHeading);
    assert_eq!(h2, "## Title");
    let (h3, _) = apply_block_format(&h2, 0..h2.len(), BlockFormat::CycleHeading);
    assert_eq!(h3, "### Title");
    let (plain, _) = apply_block_format(&h3, 0..h3.len(), BlockFormat::CycleHeading);
    assert_eq!(plain, "Title");

    // Multi-line quote toggle.
    let (quoted, qsel) = apply_block_format("line 1\nline 2", 0..13, BlockFormat::Quote);
    assert_eq!(quoted, "> line 1\n> line 2");
    let (unquoted, _) = apply_block_format(&quoted, qsel, BlockFormat::Quote);
    assert_eq!(unquoted, "line 1\nline 2");

    // Numbered list toggle.
    let (numbered, nsel) = apply_block_format("alpha\nbeta", 0..10, BlockFormat::Numbered);
    assert_eq!(numbered, "1. alpha\n2. beta");
    let (unnumbered, _) = apply_block_format(&numbered, nsel, BlockFormat::Numbered);
    assert_eq!(unnumbered, "alpha\nbeta");

    // Bullets & checklist toggle.
    let (bullets, bsel) = apply_block_format("one\ntwo", 0..7, BlockFormat::Bullet);
    assert_eq!(bullets, "- one\n- two");
    let (unbullets, _) = apply_block_format(&bullets, bsel, BlockFormat::Bullet);
    assert_eq!(unbullets, "one\ntwo");

    let (checks, csel) = apply_block_format("task a\ntask b", 0..13, BlockFormat::Checklist);
    assert_eq!(checks, "- [ ] task a\n- [ ] task b");
    let (unchecks, _) = apply_block_format(&checks, csel, BlockFormat::Checklist);
    assert_eq!(unchecks, "task a\ntask b");

    // Code block & Divider.
    let (fenced, _) = apply_block_format("fn main() {}", 0..12, BlockFormat::CodeBlock);
    assert_eq!(fenced, "```\nfn main() {}\n```");
    let (with_div, _) = apply_block_format("top", 3..3, BlockFormat::Divider);
    assert_eq!(with_div, "top\n\n---");
}

#[test]
fn parses_numbered_lists_multiline_quotes_and_quip_section_styles() {
    let html = r#"<html><body>
        <ol id="temp:C:ol1"><li>Step one</li><li>Step two</li></ol>
        <ul id="temp:C:ol2" data-section-style="6"><li>Alpha</li><li>Beta</li></ul>
        <blockquote id="temp:C:q1"><p>Line one</p><p>Line two</p></blockquote>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Steps".into(), html);
    assert_eq!(
        doc.blocks,
        vec![
            CanvasBlock::NumberedList(vec!["Step one".into(), "Step two".into()]),
            CanvasBlock::NumberedList(vec!["Alpha".into(), "Beta".into()]),
            CanvasBlock::Quote("Line one\nLine two".into()),
        ]
    );
    let md = doc.to_markdown();
    assert!(md.contains("1. Step one\n2. Step two"));
    assert!(md.contains("> Line one\n> Line two"));
    let reparsed = parse_canvas_markdown(&md);
    assert_eq!(reparsed, doc.blocks);
}

#[test]
fn interactive_checklist_add_toggle_and_delete_helpers() {
    let mut doc = CanvasDoc::from_markdown(
        "Tasks".into(),
        "Live".into(),
        "- [ ] Draft RFC\n- [x] Review API",
    );
    doc.section_ids = vec![Some("temp:C:chk".into())];

    let toggled_md = doc.toggle_checklist_item(0, 0).expect("toggle first item");
    assert_eq!(toggled_md, "- [x] Draft RFC\n- [x] Review API");

    let added_md = doc
        .add_checklist_item(0, "Ship v1".into())
        .expect("add checklist item");
    assert_eq!(added_md, "- [x] Draft RFC\n- [x] Review API\n- [ ] Ship v1");
    // Reject blank items.
    assert!(doc.add_checklist_item(0, "   ".into()).is_none());

    let after_del = doc
        .delete_checklist_item(0, 1)
        .expect("delete middle item")
        .expect("list still non-empty");
    assert_eq!(after_del, "- [x] Draft RFC\n- [ ] Ship v1");

    // Section op targets section_id when available.
    let op = section_replace_op(
        doc.section_ids[0].clone(),
        after_del.clone(),
        doc.to_markdown(),
    );
    assert_eq!(
        op,
        CanvasEditOp::Replace {
            section_id: Some("temp:C:chk".into()),
            markdown: after_del,
        }
    );

    // Deleting the remaining items removes the block entirely.
    assert!(doc.delete_checklist_item(0, 0).expect("del 1").is_some());
    assert!(doc.delete_checklist_item(0, 0).expect("del last").is_none());
    assert!(doc.blocks.is_empty());
}

#[test]
fn rich_tables_parse_serialize_and_support_row_and_column_helpers() {
    let html = r#"<html><body>
        <table>
            <thead><tr><th>Metric</th><th>Q3</th><th>Q4</th></tr></thead>
            <tbody>
                <tr><td>Latency</td><td>40ms</td></tr>
                <tr><td>Uptime</td><td>99.9%</td><td>99.99%</td></tr>
            </tbody>
        </table>
    </body></html>"#;
    let blocks = parse_canvas_html(html);
    assert_eq!(
        blocks,
        vec![CanvasBlock::Table {
            header: vec!["Metric".into(), "Q3".into(), "Q4".into()],
            rows: vec![
                vec!["Latency".into(), "40ms".into(), "".into()],
                vec!["Uptime".into(), "99.9%".into(), "99.99%".into()],
            ],
        }]
    );

    // Markdown pipe table with escaped pipes & alignment separator.
    let md = "| Key | Expr |\n| :--- | ---: |\n| or | a \\| b |";
    let parsed = parse_canvas_markdown(md);
    assert_eq!(
        parsed,
        vec![CanvasBlock::Table {
            header: vec!["Key".into(), "Expr".into()],
            rows: vec![vec!["or".into(), "a | b".into()]],
        }]
    );
    assert!(parsed[0].to_markdown().contains("a \\| b"));

    // Document-level table helpers.
    let mut doc = CanvasDoc::from_markdown("Metrics".into(), "Live".into(), md);
    let after_row = doc.table_add_row(0).expect("add row");
    assert_eq!(after_row.lines().count(), 4);
    let after_col = doc.table_add_column(0).expect("add column");
    assert!(after_col.contains("Column 3"));

    // Editor-level table insert, add row, add column.
    let (inserted, _) = insert_markdown_table("# Metrics", 9..9);
    assert!(inserted.contains("| Column 1 | Column 2 | Column 3 |"));
    let (with_row, _) = add_table_row_at(&inserted, inserted.len());
    assert_eq!(with_row.lines().count(), inserted.lines().count() + 1);
    let (with_col, _) = add_table_column_at(&with_row, with_row.len());
    assert!(with_col.contains("Column 4"));
}

#[test]
fn parses_quip_html_emoji_and_shortcodes_in_canvas_text() {
    let html = r#"<html><body>
        <h1>Launch <control><img alt="rocket" src="https://a.slack-edge.com/emoji.png"/></control></h1>
        <p>Status: <img alt=":white_check_mark:" class="emoji" src="https://x/y.png"/> and :tada: :thumbsup::skin-tone-3: :unknown_custom_999:</p>
    </body></html>"#;
    let blocks = parse_canvas_html(html);
    assert_eq!(
        blocks[0],
        CanvasBlock::Heading {
            level: 1,
            text: "Launch 🚀".into(),
        }
    );
    if let CanvasBlock::Paragraph(text) = &blocks[1] {
        assert!(text.contains('✅'));
        assert!(text.contains('🎉'));
        assert!(text.contains("👍🏼"));
        assert!(text.contains(":unknown_custom_999:"));
    } else {
        panic!("expected paragraph block");
    }

    assert_eq!(resolve_canvas_emoji("Ship :fire: now"), "Ship 🔥 now");
    let (with_emoji, _) = insert_text_at("Done ", 5..5, "🚀");
    assert_eq!(with_emoji, "Done 🚀");
}

#[test]
fn file_upload_markdown_and_image_roundtrip() {
    assert!(is_image_filename("diagram.PNG"));
    assert!(is_image_filename("photo.webp"));
    assert!(!is_image_filename("report.pdf"));

    let img_md = canvas_file_markdown("arch.png", "https://files.slack.com/files/arch.png");
    assert_eq!(
        img_md,
        "![arch.png](https://files.slack.com/files/arch.png)"
    );
    let parsed = parse_canvas_markdown(&img_md);
    assert_eq!(
        parsed,
        vec![CanvasBlock::Image {
            url: "https://files.slack.com/files/arch.png".into(),
            alt: "arch.png".into(),
            width: None,
            height: None,
        }]
    );

    let pdf_md = canvas_file_markdown("spec.pdf", "https://files.slack.com/files/spec.pdf");
    assert_eq!(pdf_md, "[spec.pdf](https://files.slack.com/files/spec.pdf)");
}

#[test]
fn slash_command_detection_filtering_and_insertion() {
    assert_eq!(detect_slash_query("/", 1), Some((0..1, String::new())));
    assert_eq!(
        detect_slash_query("Intro\n/tab", 10),
        Some((6..10, "tab".into()))
    );
    // Mid-line slashes (e.g. URLs) must NOT trigger slash commands.
    assert_eq!(detect_slash_query("https://example.com/path", 24), None);
    assert_eq!(detect_slash_query("/two words", 10), None);

    let table_hits = filter_slash_commands("tab");
    assert_eq!(table_hits.len(), 1);
    assert_eq!(table_hits[0].cmd, SlashCommand::Table);

    let all_hits = filter_slash_commands("");
    assert_eq!(all_hits.len(), 12);

    let (applied, _) = apply_slash_command("Intro\n/h2", 6..9, SlashCommand::Heading2);
    assert_eq!(applied, "Intro\n## Section");

    let (applied_num, _) = apply_slash_command("/num", 0..4, SlashCommand::NumberedList);
    assert_eq!(applied_num, "1. First item");
}

#[test]
fn section_actions_ops_target_section_ids_when_present() {
    let mut doc = CanvasDoc::from_markdown("Doc".into(), "Live".into(), "# A\n\nPara");
    doc.section_ids = vec![Some("temp:C:1".into()), Some("temp:C:2".into())];

    assert!(doc.insert_block_after(0, CanvasBlock::Paragraph("Inserted".into())));
    assert_eq!(doc.blocks.len(), 3);
    assert_eq!(doc.section_ids[1], None);

    let ins_op = section_insert_after_op(
        Some("temp:C:1".into()),
        "Inserted".into(),
        doc.to_markdown(),
    );
    assert_eq!(
        ins_op,
        CanvasEditOp::InsertAfter {
            section_id: "temp:C:1".into(),
            markdown: "Inserted".into(),
        }
    );

    let del_op = section_delete_op(Some("temp:C:2".into()), doc.to_markdown());
    assert_eq!(
        del_op,
        CanvasEditOp::Delete {
            section_id: "temp:C:2".into(),
        }
    );

    let fallback_del = section_delete_op(None, doc.to_markdown());
    assert!(matches!(
        fallback_del,
        CanvasEditOp::Replace {
            section_id: None,
            ..
        }
    ));
}

#[gpui::test]
fn canvas_renders_context_menus_pin_tab_and_rich_blocks_without_os_dialogs(
    cx: &mut gpui::TestAppContext,
) {
    struct CanvasHost {
        pinned: Rc<Cell<bool>>,
        uploaded: Rc<Cell<bool>>,
    }
    impl Render for CanvasHost {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let pinned = self.pinned.clone();
            let on_pin_tab: OpenBrowserFn = Rc::new(move |_, _| {
                pinned.set(true);
            });
            let uploaded = self.uploaded.clone();
            let on_upload_file: CanvasUploadFileFn =
                Rc::new(move |on_done: CanvasUploadCompleteFn, window, cx| {
                    uploaded.set(true);
                    on_done(
                        UploadedCanvasFile {
                            filename: "mock.png".into(),
                            url: "https://files.slack.com/mock.png".into(),
                        },
                        window,
                        cx,
                    );
                });
            canvas_view(
                "",
                None,
                CanvasActions {
                    on_pin_tab: Some(on_pin_tab),
                    on_upload_file: Some(on_upload_file),
                    ..Default::default()
                },
                cx,
            )
        }
    }

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let pinned = Rc::new(Cell::new(false));
    let uploaded = Rc::new(Cell::new(false));
    let pinned_c = pinned.clone();
    let uploaded_c = uploaded.clone();
    let (_root, cx) = cx.add_window_view(move |_, _| CanvasHost {
        pinned: pinned_c,
        uploaded: uploaded_c,
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
    assert!(!pinned.get());
    assert!(!uploaded.get());
}
