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
use crate::canvas::parse::{
    classify_media_mime, classify_media_url, media_format_pill, parse_slack_doc_url,
    parse_slack_profile_url, resolve_canvas_emoji,
};
use crate::canvas::{
    CanvasActions, CanvasBlock, CanvasDoc, CanvasEditFn, CanvasEmbedKind, CanvasOpenEmbedFn,
    CanvasOpenMediaFn, CanvasOpenProfileFn, CanvasUploadCompleteFn, CanvasUploadFileFn,
    CanvasUserLookupFn, CanvasUserProfile, MediaKind, OpenBrowserFn, UploadedCanvasFile,
    canvas_view, live_canvas_view, parse_canvas_html, parse_canvas_markdown, resolve_profile_card,
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

#[test]
fn drag_reorder_keeps_blocks_and_section_ids_aligned_and_builds_edit_op() {
    let html = r#"<html><body>
        <h1 id="temp:C:h1">Intro</h1>
        <p id="temp:C:p1">Alpha</p>
        <p id="temp:C:p2">Beta</p>
        <p id="temp:C:p3">Gamma</p>
    </body></html>"#;
    let mut doc = CanvasDoc::from_html("Spec".into(), html);
    assert_eq!(doc.blocks.len(), 4);

    // No-op & out-of-bounds guards.
    assert!(!doc.reorder_block(1, 1));
    assert!(!doc.reorder_block(9, 1));
    assert!(!doc.reorder_block(1, 9));
    assert!(doc.reorder_block_op(2, 2).is_none());

    // Drag downward: index 0 -> index 2 puts Intro after Beta.
    let op = doc
        .reorder_block_op(0, 2)
        .expect("reordering 0 -> 2 produces edit op");
    assert_eq!(
        doc.blocks,
        vec![
            CanvasBlock::Paragraph("Alpha".into()),
            CanvasBlock::Paragraph("Beta".into()),
            CanvasBlock::Heading {
                level: 1,
                text: "Intro".into(),
            },
            CanvasBlock::Paragraph("Gamma".into()),
        ]
    );
    assert_eq!(
        doc.section_ids,
        vec![
            Some("temp:C:p1".into()),
            Some("temp:C:p2".into()),
            Some("temp:C:h1".into()),
            Some("temp:C:p3".into()),
        ]
    );
    assert_eq!(
        op,
        CanvasEditOp::Replace {
            section_id: None,
            markdown: "Alpha\n\nBeta\n\n# Intro\n\nGamma".into(),
        }
    );

    // Drag upward: index 3 (Gamma) -> index 0 puts Gamma first.
    assert!(doc.reorder_block(3, 0));
    assert_eq!(doc.blocks[0], CanvasBlock::Paragraph("Gamma".into()));
    assert_eq!(doc.section_ids[0].as_deref(), Some("temp:C:p3"));

    // Menu move_block stays consistent with reorder_block and preserves section_ids.
    assert!(doc.move_block(0, 1));
    assert_eq!(doc.blocks[1], CanvasBlock::Paragraph("Gamma".into()));
    assert_eq!(doc.section_ids[1].as_deref(), Some("temp:C:p3"));

    // Shorter section_ids vec is padded cleanly before reordering.
    doc.section_ids.truncate(2);
    assert!(doc.reorder_block(0, 3));
    assert_eq!(doc.section_ids.len(), doc.blocks.len());

    // Preview labels stay compact and truncate long blocks cleanly.
    let long_para = CanvasBlock::Paragraph("a".repeat(80));
    let label = long_para.preview_label();
    assert!(label.ends_with('…'));
    assert!(label.chars().count() <= 43);
}

#[test]
fn parses_and_roundtrips_inline_audio_and_video_embeds() {
    // Extension & MIME classifiers.
    assert_eq!(
        classify_media_url("https://files.slack.com/clip.MP3?download=1"),
        Some((MediaKind::Audio, "audio/mpeg"))
    );
    assert_eq!(
        classify_media_url("voice-note.m4a"),
        Some((MediaKind::Audio, "audio/mp4"))
    );
    assert_eq!(
        classify_media_url("demo.webm#t=10"),
        Some((MediaKind::Video, "video/webm"))
    );
    assert_eq!(
        classify_media_url("recording.mov"),
        Some((MediaKind::Video, "video/quicktime"))
    );
    assert_eq!(classify_media_url("spec.pdf"), None);
    assert_eq!(
        classify_media_mime("audio/ogg"),
        Some((MediaKind::Audio, "audio/ogg".into()))
    );
    assert_eq!(
        media_format_pill(Some("video/quicktime"), "https://x/y"),
        Some("MOV".into())
    );
    assert_eq!(
        media_format_pill(Some("audio/mpeg"), "https://x/y"),
        Some("MP3".into())
    );

    // Quip HTML <audio>, <video><source>, and p.embedded-file / lnk[data-slack-file-id].
    let html = r#"<html><body>
        <audio id="temp:C:a1" src="https://files.slack.com/files/standup.mp3" title="Weekly Standup" data-duration="1:24"></audio>
        <video id="temp:C:v1" data-title="Beta Demo" data-duration="3:10" data-slack-file-id="sf:F01VID">
            <source src="https://files.slack.com/files/demo.webm" type="video/webm" />
        </video>
        <p id="temp:C:ef1" class="embedded-file">
            <lnk data-slack-file-id="sf:F02AUD" href="https://files.slack.com/files/note.m4a" data-mime="audio/mp4" data-duration="0:45">Voice memo</lnk>
        </p>
        <p id="temp:C:ef2" class="embedded-file">
            <lnk data-slack-file-id="sf:F03PDF" href="https://files.slack.com/files/spec.pdf">spec.pdf</lnk>
        </p>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Media".into(), html);
    assert_eq!(
        doc.blocks,
        vec![
            CanvasBlock::MediaEmbed {
                kind: MediaKind::Audio,
                title: "Weekly Standup".into(),
                url: "https://files.slack.com/files/standup.mp3".into(),
                file_id: None,
                duration: Some("1:24".into()),
                mime: Some("audio/mpeg".into()),
            },
            CanvasBlock::MediaEmbed {
                kind: MediaKind::Video,
                title: "Beta Demo".into(),
                url: "https://files.slack.com/files/demo.webm".into(),
                file_id: Some("F01VID".into()),
                duration: Some("3:10".into()),
                mime: Some("video/webm".into()),
            },
            CanvasBlock::MediaEmbed {
                kind: MediaKind::Audio,
                title: "Voice memo".into(),
                url: "https://files.slack.com/files/note.m4a".into(),
                file_id: Some("F02AUD".into()),
                duration: Some("0:45".into()),
                mime: Some("audio/mp4".into()),
            },
            // Non-media embedded files stay paragraphs.
            CanvasBlock::Paragraph("spec.pdf".into()),
        ]
    );
    assert_eq!(doc.section_ids[0].as_deref(), Some("temp:C:a1"));
    assert_eq!(doc.section_ids[1].as_deref(), Some("temp:C:v1"));
    assert_eq!(doc.section_ids[2].as_deref(), Some("temp:C:ef1"));

    // Search matching across title, kind, duration, file_id.
    assert!(doc.blocks[0].matches("standup"));
    assert!(doc.blocks[0].matches("audio"));
    assert!(doc.blocks[1].matches("f01vid"));
    assert!(doc.blocks[1].matches("3:10"));
    assert!(!doc.blocks[0].matches("video"));

    // Markdown parsing & round-trip across audio and video extensions.
    let md = "[Kickoff audio](https://cdn.example.com/kickoff.ogg)\n\n\
              https://cdn.example.com/launch.mp4\n\n\
              [Clip](https://cdn.example.com/raw.wav)";
    let parsed = parse_canvas_markdown(md);
    assert_eq!(parsed.len(), 3);
    assert!(matches!(
        &parsed[0],
        CanvasBlock::MediaEmbed {
            kind: MediaKind::Audio,
            title,
            ..
        } if title == "Kickoff audio"
    ));
    assert!(matches!(
        &parsed[1],
        CanvasBlock::MediaEmbed {
            kind: MediaKind::Video,
            title,
            ..
        } if title == "launch.mp4"
    ));
    assert!(matches!(
        &parsed[2],
        CanvasBlock::MediaEmbed {
            kind: MediaKind::Audio,
            ..
        }
    ));
    let roundtripped = parse_canvas_markdown(&parsed[0].to_markdown());
    assert_eq!(roundtripped, vec![parsed[0].clone()]);
}

#[test]
fn parses_resolves_and_roundtrips_user_profile_cards() {
    assert_eq!(
        parse_slack_profile_url("https://acme.slack.com/team/U01ALICE?tab=about"),
        Some("U01ALICE".into())
    );
    assert_eq!(
        parse_slack_profile_url("https://acme.slack.com/archives/C01"),
        None
    );

    let html = r#"<html><body>
        <div id="temp:C:u1" class="user_unfurl" data-user-id="U01ALICE" data-title="Staff Engineer" data-handle="alice">Alice Rivera</div>
        <p id="temp:C:u2"><control><a href="https://acme.slack.com/team/U02BOB">@U02BOB</a></control></p>
        <p id="temp:C:u3"><a href="https://acme.slack.com/team/U03CAROL">Carol Lin</a></p>
        <p id="temp:C:p1">Ask <control><a href="https://acme.slack.com/team/U02BOB">@U02BOB</a></control> before shipping.</p>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Team".into(), html);
    assert_eq!(
        doc.blocks,
        vec![
            CanvasBlock::ProfileCard {
                user_id: "U01ALICE".into(),
                display_name: Some("Alice Rivera".into()),
                title: Some("Staff Engineer".into()),
                handle: Some("alice".into()),
            },
            CanvasBlock::ProfileCard {
                user_id: "U02BOB".into(),
                display_name: None,
                title: None,
                handle: None,
            },
            CanvasBlock::ProfileCard {
                user_id: "U03CAROL".into(),
                display_name: Some("Carol Lin".into()),
                title: None,
                handle: None,
            },
            // Inline mention inside prose must NOT become a standalone profile card.
            CanvasBlock::Paragraph("Ask @U02BOB before shipping.".into()),
        ]
    );
    assert_eq!(doc.section_ids[0].as_deref(), Some("temp:C:u1"));
    assert_eq!(doc.section_ids[1].as_deref(), Some("temp:C:u2"));

    // Pure user resolver seam without booting full app.
    let lookup: CanvasUserLookupFn = Rc::new(|uid: &str| match uid {
        "U02BOB" => Some(CanvasUserProfile {
            user_id: "U02BOB".into(),
            display_name: "Bob Park".into(),
            title: Some("Design Lead".into()),
            handle: Some("bpark".into()),
        }),
        _ => None,
    });
    let resolved_bob = resolve_profile_card("U02BOB", None, None, None, Some(&lookup));
    assert_eq!(resolved_bob.display_name, "Bob Park");
    assert_eq!(resolved_bob.title.as_deref(), Some("Design Lead"));
    assert_eq!(resolved_bob.handle.as_deref(), Some("bpark"));
    assert_eq!(resolved_bob.initials(), "BP");

    // Fallback when lookup misses.
    let fallback_carol =
        resolve_profile_card("U03CAROL", Some("Carol Lin"), None, None, Some(&lookup));
    assert_eq!(fallback_carol.display_name, "Carol Lin");
    assert_eq!(fallback_carol.initials(), "CL");

    let bare_unknown = resolve_profile_card("U09ZED", None, None, None, None);
    assert_eq!(bare_unknown.display_name, "U09ZED");
    assert_eq!(bare_unknown.initials(), "U0");

    // Search matching & markdown round-trip.
    assert!(doc.blocks[0].matches("alice"));
    assert!(doc.blocks[0].matches("staff engineer"));
    assert!(doc.blocks[1].matches("u02bob"));

    let md = "[Carol Lin](https://slack.com/team/U03CAROL)\n\n\
              https://acme.slack.com/team/U02BOB\n\n\
              <@U04DAVE|dave>";
    let parsed_md = parse_canvas_markdown(md);
    assert_eq!(
        parsed_md,
        vec![
            CanvasBlock::ProfileCard {
                user_id: "U03CAROL".into(),
                display_name: Some("Carol Lin".into()),
                title: None,
                handle: None,
            },
            CanvasBlock::ProfileCard {
                user_id: "U02BOB".into(),
                display_name: None,
                title: None,
                handle: None,
            },
            CanvasBlock::ProfileCard {
                user_id: "U04DAVE".into(),
                display_name: None,
                title: None,
                handle: Some("dave".into()),
            },
        ]
    );
    assert_eq!(
        parse_canvas_markdown(&parsed_md[0].to_markdown()),
        vec![parsed_md[0].clone()]
    );
}

#[test]
fn parses_and_roundtrips_embedded_canvases_and_lists_by_link() {
    assert_eq!(
        parse_slack_doc_url("https://acme.slack.com/docs/T01TEAM/F01CANVAS"),
        Some((CanvasEmbedKind::Canvas, "F01CANVAS".into()))
    );
    assert_eq!(
        parse_slack_doc_url("https://acme.slack.com/lists/T01TEAM/F02LIST?view=board"),
        Some((CanvasEmbedKind::List, "F02LIST".into()))
    );
    assert_eq!(
        parse_slack_doc_url("https://acme.slack.com/archives/C01GENERAL"),
        None
    );

    let html = r#"<html><body>
        <p id="temp:C:c1" class="embedded-link"><a href="https://acme.slack.com/docs/T01/F01SPEC">Q4 Launch Spec</a></p>
        <div id="temp:C:c2" class="canvas_unfurl" data-slack-file-id="sf:F02RUNBOOK" data-title="Incident Runbook">Incident Runbook</div>
        <p id="temp:C:l1" class="embedded-link"><a href="https://acme.slack.com/lists/T01/F03BUGS">Release Blockers</a></p>
        <p id="temp:C:p1"><a href="https://example.com/docs/guide">External Docs</a></p>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Hub".into(), html);
    assert_eq!(
        doc.blocks,
        vec![
            CanvasBlock::DocEmbed {
                kind: CanvasEmbedKind::Canvas,
                file_id: "F01SPEC".into(),
                title: "Q4 Launch Spec".into(),
                url: "https://acme.slack.com/docs/T01/F01SPEC".into(),
            },
            CanvasBlock::DocEmbed {
                kind: CanvasEmbedKind::Canvas,
                file_id: "F02RUNBOOK".into(),
                title: "Incident Runbook".into(),
                url: "https://slack.com/docs/T0/F02RUNBOOK".into(),
            },
            CanvasBlock::DocEmbed {
                kind: CanvasEmbedKind::List,
                file_id: "F03BUGS".into(),
                title: "Release Blockers".into(),
                url: "https://acme.slack.com/lists/T01/F03BUGS".into(),
            },
            // Ordinary external links remain paragraphs.
            CanvasBlock::Paragraph("External Docs".into()),
        ]
    );
    assert_eq!(doc.section_ids[0].as_deref(), Some("temp:C:c1"));
    assert_eq!(doc.section_ids[2].as_deref(), Some("temp:C:l1"));

    // Search matching & markdown round-trip.
    assert!(doc.blocks[0].matches("launch spec"));
    assert!(doc.blocks[0].matches("canvas"));
    assert!(doc.blocks[2].matches("f03bugs"));
    assert!(doc.blocks[2].matches("list"));

    let md = "[Q4 Launch Spec](https://acme.slack.com/docs/T01/F01SPEC)\n\n\
              https://acme.slack.com/lists/T01/F03BUGS";
    let parsed_md = parse_canvas_markdown(md);
    assert_eq!(
        parsed_md,
        vec![
            CanvasBlock::DocEmbed {
                kind: CanvasEmbedKind::Canvas,
                file_id: "F01SPEC".into(),
                title: "Q4 Launch Spec".into(),
                url: "https://acme.slack.com/docs/T01/F01SPEC".into(),
            },
            CanvasBlock::DocEmbed {
                kind: CanvasEmbedKind::List,
                file_id: "F03BUGS".into(),
                title: "List F03BUGS".into(),
                url: "https://acme.slack.com/lists/T01/F03BUGS".into(),
            },
        ]
    );
    assert_eq!(
        parse_canvas_markdown(&parsed_md[0].to_markdown()),
        vec![parsed_md[0].clone()]
    );
}

#[gpui::test]
fn canvas_renders_drag_reorder_media_profile_and_doc_embeds_with_mock_callbacks(
    cx: &mut gpui::TestAppContext,
) {
    struct RichCanvasHost {
        doc: CanvasDoc,
        editable: bool,
        media_opened: Rc<Cell<bool>>,
        profile_opened: Rc<Cell<bool>>,
        embed_opened: Rc<Cell<bool>>,
        edited: Rc<Cell<bool>>,
    }
    impl Render for RichCanvasHost {
        fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            let media_opened = self.media_opened.clone();
            let on_open_media: CanvasOpenMediaFn = Rc::new(move |_, _, _, _| {
                media_opened.set(true);
            });
            let profile_opened = self.profile_opened.clone();
            let on_open_profile: CanvasOpenProfileFn = Rc::new(move |_, _, _| {
                profile_opened.set(true);
            });
            let embed_opened = self.embed_opened.clone();
            let on_open_embed: CanvasOpenEmbedFn = Rc::new(move |_, _, _, _, _| {
                embed_opened.set(true);
            });
            let edited = self.edited.clone();
            let on_edit: Option<CanvasEditFn> = self.editable.then(|| {
                Rc::new(move |_: CanvasEditOp, _: &mut Window, _: &mut gpui::App| {
                    edited.set(true);
                }) as CanvasEditFn
            });
            let user_lookup: CanvasUserLookupFn = Rc::new(|uid: &str| {
                (uid == "U01ALICE").then(|| CanvasUserProfile {
                    user_id: "U01ALICE".into(),
                    display_name: "Alice Rivera".into(),
                    title: Some("Engineering Lead".into()),
                    handle: Some("alice".into()),
                })
            });
            let loaded = Ok(self.doc.clone());
            live_canvas_view(
                "F01RICH",
                Some(&loaded),
                "",
                None,
                CanvasActions {
                    on_edit,
                    on_open_media: Some(on_open_media),
                    user_lookup: Some(user_lookup),
                    on_open_profile: Some(on_open_profile),
                    on_open_embed: Some(on_open_embed),
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
    let html = r#"<html><body>
        <h1 id="temp:C:h1">Launch Hub</h1>
        <audio id="temp:C:a1" src="https://files.slack.com/sync.mp3" title="Launch Sync" data-duration="1:15"></audio>
        <video id="temp:C:v1" src="https://files.slack.com/demo.mp4" title="Walkthrough" data-duration="2:40"></video>
        <p id="temp:C:u1"><control><a href="https://acme.slack.com/team/U01ALICE">@U01ALICE</a></control></p>
        <p id="temp:C:c1" class="embedded-link"><a href="https://acme.slack.com/docs/T01/F01SPEC">Design Doc</a></p>
        <p id="temp:C:l1" class="embedded-link"><a href="https://acme.slack.com/lists/T01/F02TASKS">Launch Tasks</a></p>
    </body></html>"#;
    let doc = CanvasDoc::from_html("Launch Hub".into(), html);
    assert_eq!(doc.blocks.len(), 6);

    for editable in [true, false] {
        let media_opened = Rc::new(Cell::new(false));
        let profile_opened = Rc::new(Cell::new(false));
        let embed_opened = Rc::new(Cell::new(false));
        let edited = Rc::new(Cell::new(false));
        let doc_c = doc.clone();
        let (m, p, e, ed) = (
            media_opened.clone(),
            profile_opened.clone(),
            embed_opened.clone(),
            edited.clone(),
        );
        let (_root, cx) = cx.add_window_view(move |_, _| RichCanvasHost {
            doc: doc_c,
            editable,
            media_opened: m,
            profile_opened: p,
            embed_opened: e,
            edited: ed,
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            let _ = window.draw(cx);
        });
        cx.run_until_parked();
        assert!(!media_opened.get());
        assert!(!profile_opened.get());
        assert!(!embed_opened.get());
        assert!(!edited.get());
    }
}
