use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::v_flex;
use gpui_kit::gpui::{
    self, App, AppContext as _, ClipboardItem, Context, Entity, Image, ImageFormat, IntoElement,
    Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, TestAppContext, VisualTestContext, Window, div,
};
use slackless_core::api::{GifItem, MessageContent};
use slackless_core::types::Message;

use super::attachments::{
    clipboard_image_ext, format_upload_rejection, image_format_for_ext, load_attachment_paths,
};
use super::snippet::{apply_snippet_extension, normalize_snippet_filename};
use super::submit::{compose_payload, custom_schedule_text};
use super::{AlsoSendTarget, Composer, ComposerEvent, TypingNames, emoji_display_text};
use crate::drafts::SendLater;
use crate::editor::from_mrkdwn;
use crate::emoji::EmojiChoice;

struct ComposerHarness {
    composer: Entity<Composer>,
}

impl Render for ComposerHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.composer.clone())
    }
}

fn click_offset(
    composer: &Entity<Composer>,
    offset: usize,
    click_count: usize,
    cx: &mut VisualTestContext,
) {
    let pos = composer.read_with(cx, |composer, cx| {
        let b = composer
            .input
            .read(cx)
            .range_to_bounds(&(offset..offset + 1))
            .unwrap_or_else(|| panic!("no bounds for offset {offset}"));
        b.center()
    });
    cx.simulate_event(MouseDownEvent {
        position: pos,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: pos,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count,
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn selected_slice(composer: &Entity<Composer>, cx: &mut VisualTestContext) -> String {
    composer.read_with(cx, |composer, cx| {
        let state = composer.input.read(cx);
        let range = state.selected_range();
        state.text().slice(range).to_string()
    })
}

fn key_event(key: &str, shift: bool, alt: bool, cx: &App) -> gpui_kit::gpui::KeyDownEvent {
    let mut modifiers = Modifiers::secondary_key(cx);
    modifiers.shift = shift;
    modifiers.alt = alt;
    gpui_kit::gpui::KeyDownEvent {
        keystroke: gpui_kit::gpui::Keystroke {
            modifiers,
            key: key.to_owned(),
            key_char: None,
        },
        is_held: false,
        prefer_character_input: false,
    }
}

fn sample_gif(id: &str) -> GifItem {
    GifItem {
        id: id.to_owned(),
        title: format!("GIF {id}"),
        still_url: format!("https://media.example/{id}-still.gif"),
        preview_url: format!("https://media.example/{id}-preview.gif"),
        url: format!("https://media.example/{id}.gif"),
    }
}

#[test]
fn typing_names_of_and_bold_ranges_handle_counts_and_utf8() {
    assert_eq!(TypingNames::of(&[]), None);

    let one = TypingNames::of(&[SharedString::from("Åsa")]).unwrap();
    assert_eq!(one, TypingNames::One("Åsa".into()));
    let (line, ranges) = one.line();
    assert_eq!(line, "Åsa is typing");
    assert_eq!(ranges.len(), 1);
    assert_eq!(&line[ranges[0].clone()], "Åsa");

    let two = TypingNames::of(&[SharedString::from("Åsa"), SharedString::from("Bö")]).unwrap();
    assert_eq!(two, TypingNames::Two("Åsa".into(), "Bö".into()));
    let (line, ranges) = two.line();
    assert_eq!(line, "Åsa and Bö are typing");
    assert_eq!(ranges.len(), 2);
    assert_eq!(&line[ranges[0].clone()], "Åsa");
    assert_eq!(&line[ranges[1].clone()], "Bö");

    let several = TypingNames::of(&[
        SharedString::from("A"),
        SharedString::from("B"),
        SharedString::from("C"),
    ])
    .unwrap();
    assert_eq!(several, TypingNames::Several);
    let (line, ranges) = several.line();
    assert_eq!(line, "Several people are typing");
    assert!(ranges.is_empty());
}

#[test]
fn attachment_ext_detection_limits_and_disk_loading() {
    assert_eq!(
        image_format_for_ext(Path::new("shot.PNG")),
        Some(ImageFormat::Png)
    );
    assert_eq!(
        image_format_for_ext(Path::new("photo.jpeg")),
        Some(ImageFormat::Jpeg)
    );
    assert_eq!(
        image_format_for_ext(Path::new("anim.webp")),
        Some(ImageFormat::Webp)
    );
    assert_eq!(
        image_format_for_ext(Path::new("scan.TIF")),
        Some(ImageFormat::Tiff)
    );
    assert_eq!(image_format_for_ext(Path::new("notes.txt")), None);
    assert_eq!(image_format_for_ext(Path::new("no_extension")), None);

    assert_eq!(clipboard_image_ext(ImageFormat::Png), "png");
    assert_eq!(clipboard_image_ext(ImageFormat::Jpeg), "jpg");
    assert_eq!(clipboard_image_ext(ImageFormat::Svg), "svg");

    assert_eq!(format_upload_rejection(&[]), None);
    assert_eq!(
        format_upload_rejection(&["big.mov".into(), "dump.bin".into()]),
        Some("Skipped big.mov, dump.bin (exceeds Slack's 1 GB per-file upload limit).".into())
    );

    // Isolated temp directory exercising real file loading, size limits, empty & directory skips.
    let dir = std::env::temp_dir().join(format!(
        "slackless-composer-attach-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let small_txt = dir.join("note.txt");
    let small_png = dir.join("shot.png");
    let empty_file = dir.join("empty.txt");
    let oversized = dir.join("huge.bin");
    let subdir = dir.join("nested");
    let missing = dir.join("does-not-exist.txt");

    std::fs::write(&small_txt, b"hello").unwrap();
    std::fs::write(&small_png, b"\x89PNG\r\n").unwrap();
    std::fs::write(&empty_file, b"").unwrap();
    std::fs::write(&oversized, vec![0u8; 64]).unwrap();
    std::fs::create_dir(&subdir).unwrap();

    let (staged, rejected) = load_attachment_paths(
        vec![small_txt, small_png, empty_file, oversized, subdir, missing],
        16,
    );
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(rejected, vec!["huge.bin"]);
    assert_eq!(staged.len(), 2);
    assert_eq!(staged[0].filename, "note.txt");
    assert_eq!(staged[0].bytes, b"hello");
    assert!(staged[0].preview.is_none());
    assert_eq!(staged[1].filename, "shot.png");
    assert!(staged[1].preview.is_some());
}

#[test]
fn snippet_filename_normalization_and_extension_switching() {
    assert_eq!(normalize_snippet_filename(""), "snippet.txt");
    assert_eq!(normalize_snippet_filename("   "), "snippet.txt");
    assert_eq!(normalize_snippet_filename("query"), "query.txt");
    assert_eq!(normalize_snippet_filename("  query.sql  "), "query.sql");

    assert_eq!(apply_snippet_extension("", "rs"), "snippet.rs");
    assert_eq!(apply_snippet_extension("   ", "py"), "snippet.py");
    assert_eq!(apply_snippet_extension("main", "ts"), "main.ts");
    assert_eq!(apply_snippet_extension("main.txt", "rs"), "main.rs");
    assert_eq!(
        apply_snippet_extension("archive.tar.gz", "sh"),
        "archive.tar.sh"
    );
    assert_eq!(apply_snippet_extension(".hidden", "md"), "snippet.md");
}

#[test]
fn submit_payload_schedule_validation_and_emoji_display_seams() {
    let fixtures = crate::fixtures::load();
    let doc = from_mrkdwn("hello *world*", &fixtures);
    let blank = from_mrkdwn("   \n\t ", &fixtures);

    assert_eq!(custom_schedule_text(&doc), Some("hello *world*".into()));
    assert_eq!(custom_schedule_text(&blank), None);

    assert!(compose_payload(None, None).is_none());

    let text_only = compose_payload(Some(&doc), None).unwrap();
    assert_eq!(text_only.text, "hello *world*");
    assert!(text_only.attachments.is_empty());

    let gif_only = compose_payload(None, Some(sample_gif("g1"))).unwrap();
    assert!(gif_only.text.is_empty());
    assert!(gif_only.blocks.is_empty());
    assert_eq!(gif_only.attachments.len(), 1);

    let both = compose_payload(Some(&doc), Some(sample_gif("g2"))).unwrap();
    assert_eq!(both.text, "hello *world*");
    assert_eq!(both.blocks.len(), 1);
    assert_eq!(both.attachments.len(), 1);

    let unicode = EmojiChoice {
        name: "rocket".into(),
        glyph: Some("🚀".into()),
    };
    let custom = EmojiChoice {
        name: "shipit".into(),
        glyph: None,
    };
    assert_eq!(emoji_display_text(&unicode), "🚀");
    assert_eq!(emoji_display_text(&custom), ":shipit:");
}

#[gpui::test]
fn composer_multi_click_is_bounded_in_inline_code_and_code_blocks(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let mrkdwn = "see `order.customerType: \"Org\"` ok\n```line 1\nline 2```";
    let expected_text = "see `order.customerType: \"Org\"` ok\n```\nline 1\nline 2\n```";
    let slot = Rc::new(RefCell::new(None));
    let (root, cx) = cx.add_window_view(|window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures, window, cx);
        composer.update(cx, |composer, cx| {
            composer.load_draft(mrkdwn, cx);
        });
        *slot.borrow_mut() = Some(composer.clone());
        let view = cx.new(|_| ComposerHarness { composer });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let _ = window.draw(cx);
    });

    let text = composer.read_with(cx, |c, _| c.editor.doc().text().to_owned());
    assert_eq!(text, expected_text);

    // Single, double, and triple click inside inline code (`customerType`).
    let word_ix = text.find("customerType").unwrap() + 2;
    click_offset(&composer, word_ix, 1, cx);
    click_offset(&composer, word_ix, 2, cx);
    assert_eq!(selected_slice(&composer, cx), "customerType");
    click_offset(&composer, word_ix, 3, cx);
    assert_eq!(selected_slice(&composer, cx), "order.customerType: \"Org\"");

    // Triple click on prose on the same line still selects the entire line.
    let prose_ix = text.find("see").unwrap() + 1;
    click_offset(&composer, prose_ix, 1, cx);
    click_offset(&composer, prose_ix, 2, cx);
    click_offset(&composer, prose_ix, 3, cx);
    assert_eq!(
        selected_slice(&composer, cx),
        "see `order.customerType: \"Org\"` ok"
    );

    // Double and triple click inside a multi-line fenced code block.
    let l2_ix = text.find("line 2").unwrap() + 1;
    click_offset(&composer, l2_ix, 1, cx);
    click_offset(&composer, l2_ix, 2, cx);
    assert_eq!(selected_slice(&composer, cx), "line");
    click_offset(&composer, l2_ix, 3, cx);
    assert_eq!(selected_slice(&composer, cx), "line 1\nline 2");

    // Scoped SelectAll: first Cmd+A inside a fenced code block selects just the block,
    // second Cmd+A selects the entire draft.
    click_offset(&composer, l2_ix, 1, cx);
    cx.dispatch_action(gpui_kit::component::input::SelectAll);
    cx.run_until_parked();
    assert_eq!(selected_slice(&composer, cx), "line 1\nline 2");
    cx.dispatch_action(gpui_kit::component::input::SelectAll);
    cx.run_until_parked();
    assert_eq!(selected_slice(&composer, cx), expected_text);

    // Scoped SelectAll inside inline code: first selects inner text, second selects all.
    click_offset(&composer, word_ix, 1, cx);
    cx.dispatch_action(gpui_kit::component::input::SelectAll);
    cx.run_until_parked();
    assert_eq!(selected_slice(&composer, cx), "order.customerType: \"Org\"");
    cx.dispatch_action(gpui_kit::component::input::SelectAll);
    cx.run_until_parked();
    assert_eq!(selected_slice(&composer, cx), expected_text);
    drop(root);
}

#[gpui::test]
fn composer_keys_paste_escape_up_auto_fence_and_typed_emoji_state_machines(
    cx: &mut TestAppContext,
) {
    use gpui_kit::component::input::{Enter, Escape, MoveUp};

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let slash_cmds: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let edit_last_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let cancel_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

    struct SpyHost {
        composer: Entity<Composer>,
        _sub: Subscription,
    }
    impl Render for SpyHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.composer.clone())
        }
    }

    let slot = Rc::new(RefCell::new(None));
    let slot_c = slot.clone();
    let sub_c = submitted.clone();
    let slash_c = slash_cmds.clone();
    let edit_c = edit_last_count.clone();
    let cancel_c = cancel_count.clone();

    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures, window, cx);
        *slot_c.borrow_mut() = Some(composer.clone());
        let view = cx.new(|cx| {
            let sub = cx.subscribe(&composer, move |_, _, ev: &ComposerEvent, _| match ev {
                ComposerEvent::Submit(c) => sub_c.borrow_mut().push(c.text.clone()),
                ComposerEvent::SlashCommand(inv) => slash_c.borrow_mut().push(inv.name.clone()),
                ComposerEvent::EditLast => *edit_c.borrow_mut() += 1,
                ComposerEvent::Cancel => *cancel_c.borrow_mut() += 1,
                _ => {}
            });
            SpyHost {
                composer,
                _sub: sub,
            }
        });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // 1. on_up on empty composer emits EditLast; non-empty draft does not.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| c.on_up(&MoveUp, window, cx));
    });
    assert_eq!(*edit_last_count.borrow(), 1);
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("draft text");
            c.on_up(&MoveUp, window, cx);
        });
    });
    assert_eq!(*edit_last_count.borrow(), 1);

    // 2. on_enter: enter_sends = true submits; Shift+Enter does not; enter_sends = false does not; Cmd+Enter submits.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("hello");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: true,
                },
                window,
                cx,
            );
        });
    });
    assert!(submitted.borrow().is_empty(), "Shift+Enter must not submit");

    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.set_enter_sends(false, cx);
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
        });
    });
    assert!(
        submitted.borrow().is_empty(),
        "Enter must not submit when enter_sends = false"
    );

    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.on_enter(
                &Enter {
                    secondary: true,
                    shift: false,
                },
                window,
                cx,
            );
        });
    });
    assert_eq!(submitted.borrow().as_slice(), ["hello"]);

    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.set_enter_sends(true, cx);
            c.editor.replace_all("second");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
        });
    });
    assert_eq!(submitted.borrow().as_slice(), ["hello", "second"]);

    // 3. Menu pick on partial slash command vs submitting a full typed slash command.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("/sh");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert_eq!(c.editor.doc().text(), "/shrug ");
        });
    });
    assert!(slash_cmds.borrow().is_empty());

    // Full typed command submits SlashCommand instead of re-completing.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("/away");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
        });
    });
    assert_eq!(slash_cmds.borrow().as_slice(), ["away"]);

    // 4. on_escape sticky menu dismiss: stays dismissed while typing same trigger.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("hi @");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_escape(&Escape, window, cx);
            assert!(c.menu.is_none());
            assert_eq!(c.dismissed, Some(3));

            // Continuing the same `@` trigger keeps menu dismissed.
            c.editor.insert_text("a");
            c.changed(cx);
            assert!(c.menu.is_none());

            // Moving past space clears dismissal.
            c.editor.insert_text(" ");
            c.changed(cx);
            assert_eq!(c.dismissed, None);
        });
    });

    // 5. on_paste: URL over selection wraps as [text](url); empty selection or non-URL returns false.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("read docs now");
            c.editor.select(5..9);
            let url_clip = ClipboardItem::new_string("https://example.com".into());
            assert!(c.on_paste(&url_clip, window, cx));
            assert_eq!(
                c.editor.doc().text(),
                "read [docs](https://example.com) now"
            );

            // Empty selection -> false
            assert!(!c.on_paste(&url_clip, window, cx));

            // Non-URL over selection -> false
            c.editor.select(0..4);
            let plain_clip = ClipboardItem::new_string("not a url".into());
            assert!(!c.on_paste(&plain_clip, window, cx));
        });
    });

    // 6. auto_open_code_block opens on opening fence and does NOT open on closing fence.
    cx.update(|_, cx| {
        composer.update(cx, |c, _| {
            c.editor.replace_all("```");
            assert!(c.auto_open_code_block());
            assert_eq!(c.editor.doc().text(), "```\n\n```");
            assert_eq!(c.editor.selection(), 4..4);

            // Closing an already-opened fence must not auto-open again.
            c.editor.replace_all("```\ncode\n```");
            assert!(!c.auto_open_code_block());
            assert_eq!(c.editor.doc().text(), "```\ncode\n```");
        });
    });

    // 7. convert_typed_emoji converts `:smile:` in prose, skips inline code, fenced code, and link URLs.
    cx.update(|_, cx| {
        composer.update(cx, |c, _| {
            c.editor.replace_all("hi :smile:");
            assert!(c.convert_typed_emoji());
            assert_eq!(c.editor.doc().atoms().len(), 1);
            assert_eq!(c.editor.doc().to_mrkdwn(), "hi :smile:");

            // Skipped inside inline code
            c.editor.replace_all("`:smile:`");
            c.editor.select(8..8);
            assert!(!c.convert_typed_emoji());
            assert!(c.editor.doc().atoms().is_empty());

            // Skipped inside fenced code block
            c.editor.replace_all("```\n:smile:\n```");
            c.editor.select(11..11);
            assert!(!c.convert_typed_emoji());
            assert!(c.editor.doc().atoms().is_empty());

            // Skipped inside link URL
            c.editor.replace_all("[lbl](https://x.dev/:smile:)");
            c.editor.select(27..27);
            assert!(!c.convert_typed_emoji());
            assert!(c.editor.doc().atoms().is_empty());

            // Adjacent custom emoji atom `:party:` followed by `smile:` must not eat `:party:`'s
            // closing colon.
            c.editor.replace_all("");
            c.editor.place_atom(
                0..0,
                slackless_core::domain::mrkdwn::Atom::Emoji("party".into()),
                ":party:",
            );
            let after_atom = c.editor.doc().text().len();
            c.editor.select(after_atom..after_atom);
            c.editor.insert_text("smile:");
            assert!(!c.convert_typed_emoji());
            assert_eq!(c.editor.doc().atoms().len(), 1);
            assert_eq!(c.editor.doc().text(), ":party:smile:");
        });
    });

    // 8. Edit mode Escape emits Cancel.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.edit_mode = true;
            c.on_escape(&Escape, window, cx);
        });
    });
    assert_eq!(*cancel_count.borrow(), 1);

    drop(root);
}

#[gpui::test]
fn composer_formatting_shortcuts_link_prompt_and_undo_redo(cx: &mut TestAppContext) {
    use gpui_kit::component::input::{Enter, Escape, Redo, Undo};

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let slot_c = slot.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures, window, cx);
        *slot_c.borrow_mut() = Some(composer.clone());
        let view = cx.new(|_| ComposerHarness { composer });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // Inline styles: Cmd+B, Cmd+I, Cmd+U, Cmd+Shift+X, Cmd+Shift+C, Cmd+\
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("hello world");
            c.editor.select(0..5);
            let ev = key_event("b", false, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "*hello* world");
            assert_eq!(c.editor.selection(), 1..6);

            let ev = key_event("i", false, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "*_hello_* world");

            let ev = key_event("u", false, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "*_<u>hello</u>_* world");

            let ev = key_event("x", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "*_<u>~hello~</u>_* world");

            // Cmd+\ clears all inline styles
            let ev = key_event("\\", false, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "hello world");

            // Cmd+Shift+C wraps in inline code
            c.editor.select(6..11);
            let ev = key_event("c", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "hello `world`");

            // Undo restores before inline code, redo reapplies it
            c.on_undo(&Undo, window, cx);
            assert_eq!(c.editor.doc().text(), "hello world");
            c.on_redo(&Redo, window, cx);
            assert_eq!(c.editor.doc().text(), "hello `world`");
        });
    });

    // Link prompt (Cmd+Shift+U): open -> confirm bare domain -> unlink -> open -> Esc cancel
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("read docs");
            c.editor.select(5..9);
            let ev = key_event("u", true, false, cx);
            c.on_key(&ev, window, cx);
            assert!(c.link_open, "Cmd+Shift+U opens link prompt");

            c.link_input
                .update(cx, |st, cx| st.set_value("slack.com/guide", window, cx));
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert!(!c.link_open);
            assert_eq!(
                c.editor.doc().text(),
                "read [docs](https://slack.com/guide)"
            );

            // Cmd+Shift+U on an existing link unlinks it
            c.editor.select(7..9);
            let ev = key_event("u", true, false, cx);
            c.on_key(&ev, window, cx);
            assert!(!c.link_open);
            assert_eq!(c.editor.doc().text(), "read docs");

            // Open link prompt again and cancel with Esc
            c.editor.select(5..9);
            let ev = key_event("u", true, false, cx);
            c.on_key(&ev, window, cx);
            assert!(c.link_open);
            c.link_input
                .update(cx, |st, cx| st.set_value("ignored.dev", window, cx));
            c.on_escape(&Escape, window, cx);
            assert!(!c.link_open);
            assert_eq!(c.editor.doc().text(), "read docs");
        });
    });

    // Block shortcuts: Cmd+Shift+7 (ordered), Cmd+Shift+8 (bullet), Cmd+Shift+9 (quote), Cmd+Alt+Shift+C (code block)
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("item one");
            c.editor.select(0..8);
            let ev = key_event("7", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "1. item one");

            let ev = key_event("8", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "- item one");

            let ev = key_event("9", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "> item one");

            // Toggle quote off, then wrap in a fenced code block
            let ev = key_event("9", true, false, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "item one");

            let ev = key_event("c", true, true, cx);
            c.on_key(&ev, window, cx);
            assert_eq!(c.editor.doc().text(), "```\nitem one\n```");

            // Undo steps back through block transformations
            c.on_undo(&Undo, window, cx);
            assert_eq!(c.editor.doc().text(), "item one");
        });
    });

    drop(root);
}

#[gpui::test]
fn composer_multiline_list_editing_and_paste_workflows(cx: &mut TestAppContext) {
    use gpui_kit::component::input::Enter;

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let slot_c = slot.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures, window, cx);
        *slot_c.borrow_mut() = Some(composer.clone());
        let view = cx.new(|_| ComposerHarness { composer });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            // Start a bullet list item, Shift+Enter inserts a soft break inside the item
            c.editor.replace_all("- first line");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: true,
                },
                window,
                cx,
            );
            assert_eq!(c.editor.doc().text(), "- first line\n    ");

            // Type continuation text, then Enter continues with a new bullet
            c.editor.insert_text("continued");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert_eq!(c.editor.doc().text(), "- first line\n    continued\n- ");

            // Enter on the empty bullet item exits the list cleanly
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert_eq!(c.editor.doc().text(), "- first line\n    continued\n");

            // Multi-line plain text paste returns false (delegates to textarea)
            let multiline = ClipboardItem::new_string("alpha\nbeta\ngamma".into());
            assert!(!c.on_paste(&multiline, window, cx));
            c.editor.replace_all("alpha\nbeta\ngamma");
            assert_eq!(c.editor.doc().text(), "alpha\nbeta\ngamma");

            // URL pasted over selection inside fenced code block must NOT linkify (returns false)
            let url_clip = ClipboardItem::new_string("https://docs.rs".into());
            c.editor.replace_all("```\nlet url = foo;\n```");
            let foo_at = c.editor.doc().text().find("foo").unwrap();
            c.editor.select(foo_at..foo_at + 3);
            assert!(!c.on_paste(&url_clip, window, cx));
            assert_eq!(c.editor.doc().text(), "```\nlet url = foo;\n```");

            // URL pasted over selection inside inline code must NOT linkify
            c.editor.replace_all("see `token` ok");
            let tok_at = c.editor.doc().text().find("token").unwrap();
            c.editor.select(tok_at..tok_at + 5);
            assert!(!c.on_paste(&url_clip, window, cx));
            assert_eq!(c.editor.doc().text(), "see `token` ok");

            // URL pasted at empty caret returns false
            c.editor.replace_all("plain text");
            c.editor.select(5..5);
            assert!(!c.on_paste(&url_clip, window, cx));
        });
    });

    drop(root);
}

#[gpui::test]
fn composer_autocomplete_menu_navigation_pick_escape_and_suppression(cx: &mut TestAppContext) {
    use gpui_kit::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp};
    use slackless_core::types::{Usergroup, UsergroupId};

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let slot = Rc::new(RefCell::new(None));
    let slot_c = slot.clone();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        let mut fixtures = crate::fixtures::load();
        fixtures.usergroups.push(Usergroup {
            id: UsergroupId::from("S0BACKEND"),
            handle: "backend-team".into(),
            name: "Backend Team".into(),
            ..Default::default()
        });
        let composer = Composer::new("Message".into(), Arc::new(fixtures), window, cx);
        *slot_c.borrow_mut() = Some(composer.clone());
        let view = cx.new(|_| ComposerHarness { composer });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            // @user with Down/Up wrap navigation and Tab pick preserving surrounding text & caret
            c.editor.replace_all("hello @ world");
            c.editor.select(7..7);
            c.changed(cx);
            let count = c.menu.as_ref().expect("@ menu should open").entries.len();
            assert!(count >= 2);
            assert_eq!(c.menu.as_ref().unwrap().selected, 0);
            c.on_up(&MoveUp, window, cx);
            assert_eq!(c.menu.as_ref().unwrap().selected, count - 1);
            c.on_down(&MoveDown, window, cx);
            assert_eq!(c.menu.as_ref().unwrap().selected, 0);
            c.on_tab(&IndentInline, window, cx);
            assert!(c.menu.is_none());
            assert!(c.editor.doc().text().starts_with("hello @"));
            assert!(c.editor.doc().text().ends_with(" world"));
            assert_eq!(c.editor.doc().atoms().len(), 1);
            let caret = c.editor.selection().start;
            assert_eq!(&c.editor.doc().text()[caret..], "world");

            // @usergroup trigger & Enter pick
            c.editor.replace_all("ping @back");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert!(c.editor.doc().to_mrkdwn().contains("<!subteam^S0BACKEND>"));

            // @here broadcast trigger & Tab pick
            c.editor.replace_all("attn @her");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_tab(&IndentInline, window, cx);
            assert_eq!(c.editor.doc().to_mrkdwn().trim(), "attn <!here>");

            // #channel trigger & Enter pick
            c.editor.replace_all("see #gen");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert!(c.editor.doc().to_mrkdwn().contains("<#C0GENERAL>"));

            // :emoji: trigger & Tab pick
            c.editor.replace_all("nice :rocke");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_tab(&IndentInline, window, cx);
            assert!(c.editor.doc().to_mrkdwn().contains(":rocket:"));

            // Esc sticky dismissal stays closed on same trigger, reopens on new trigger
            c.editor.replace_all("hi @");
            c.changed(cx);
            assert!(c.menu.is_some());
            c.on_escape(&Escape, window, cx);
            assert!(c.menu.is_none());
            c.editor.insert_text("a");
            c.changed(cx);
            assert!(c.menu.is_none(), "dismissed trigger stays closed");
            c.editor.insert_text(" #gen");
            c.changed(cx);
            assert!(c.menu.is_some(), "new trigger reopens menu");

            // Non-triggering inside inline code, fenced code blocks, and link URLs
            c.editor.replace_all("`@al`");
            c.editor.select(4..4);
            c.changed(cx);
            assert!(c.menu.is_none(), "no menu inside inline code");

            c.editor.replace_all("```\n#gen\n```");
            c.editor.select(8..8);
            c.changed(cx);
            assert!(c.menu.is_none(), "no menu inside fenced code block");

            c.editor.replace_all("[docs](https://example.com/@al)");
            c.editor.select(30..30);
            c.changed(cx);
            assert!(c.menu.is_none(), "no menu inside link URL");
        });
    });

    drop(root);
}

#[gpui::test]
fn composer_edit_mode_blank_rejection_attachments_and_draft_roundtrip(cx: &mut TestAppContext) {
    use gpui_kit::component::input::{Enter, Escape};

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    type FileSubmitLog = Rc<RefCell<Vec<(Option<String>, Vec<String>)>>>;
    let submits: Rc<RefCell<Vec<MessageContent>>> = Rc::new(RefCell::new(Vec::new()));
    let file_submits: FileSubmitLog = Rc::new(RefCell::new(Vec::new()));
    let cancels: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));

    struct Host {
        composer: Entity<Composer>,
        edit_composer: Entity<Composer>,
        _subs: Vec<Subscription>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .size_full()
                .child(self.composer.clone())
                .child(self.edit_composer.clone())
        }
    }

    let comp_slot = Rc::new(RefCell::new(None));
    let edit_slot = Rc::new(RefCell::new(None));
    let (comp_s, edit_s) = (comp_slot.clone(), edit_slot.clone());
    let (sub_c, file_c, cancel_c) = (submits.clone(), file_submits.clone(), cancels.clone());

    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures.clone(), window, cx);
        let edit_composer = Composer::new_edit(fixtures, window, cx);
        *comp_s.borrow_mut() = Some(composer.clone());
        *edit_s.borrow_mut() = Some(edit_composer.clone());
        let view = cx.new(|cx| {
            let sub_c2 = sub_c.clone();
            let cancel_c2 = cancel_c.clone();
            let s1 = cx.subscribe(&composer, move |_, _, ev: &ComposerEvent, _| match ev {
                ComposerEvent::Submit(c) => sub_c.borrow_mut().push(c.clone()),
                ComposerEvent::SubmitWithFiles { content, files } => {
                    file_c.borrow_mut().push((
                        content.as_ref().map(|c| c.text.clone()),
                        files.iter().map(|f| f.filename.clone()).collect(),
                    ));
                }
                _ => {}
            });
            let s2 = cx.subscribe(
                &edit_composer,
                move |_, _, ev: &ComposerEvent, _| match ev {
                    ComposerEvent::Submit(c) => sub_c2.borrow_mut().push(c.clone()),
                    ComposerEvent::Cancel => *cancel_c2.borrow_mut() += 1,
                    _ => {}
                },
            );
            Host {
                composer,
                edit_composer,
                _subs: vec![s1, s2],
            }
        });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = comp_slot.borrow_mut().take().unwrap();
    let edit_composer = edit_slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // 1. Empty & whitespace-only send rejection
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            c.editor.replace_all("   \n\t  ");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
        });
    });
    assert!(submits.borrow().is_empty());

    // 2. Attachment chips add / remove / submit with files
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            let img1 = Image::from_bytes(ImageFormat::Png, vec![1, 2, 3, 4]);
            let img2 = Image::from_bytes(ImageFormat::Jpeg, vec![5, 6, 7]);
            c.attach_clipboard_image(img1, cx);
            c.attach_clipboard_image(img2, cx);
            assert_eq!(c.attachments.len(), 2);
            assert_eq!(c.attachments[0].filename, "image-1.png");
            assert_eq!(c.attachments[1].filename, "image-2.jpg");

            let first_id = c.attachments[0].id;
            c.remove_attachment(first_id, cx);
            assert_eq!(c.attachments.len(), 1);
            assert_eq!(c.attachments[0].filename, "image-2.jpg");

            c.editor.replace_all("caption");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            assert!(c.attachments.is_empty());
        });
    });
    assert_eq!(
        file_submits.borrow().as_slice(),
        [(Some("caption".to_owned()), vec!["image-2.jpg".to_owned()])]
    );

    // 3. Draft save/restore round-trip preserving rich formatting & atoms
    cx.update(|_, cx| {
        composer.update(cx, |c, cx| {
            let draft = "Hi <@U0ALICE> in <#C0GENERAL> <!here> :rocket:\n\
                         *bold* _italic_ <u>under</u> ~strike~ `code` <https://example.com|docs>\n\
                         - bullet one\n\
                         > quoted note";
            c.load_draft(draft, cx);
            let saved = c.text();
            assert!(saved.contains("<@U0ALICE>"));
            assert!(saved.contains("<#C0GENERAL>"));
            assert!(saved.contains("<!here>"));
            assert!(saved.contains(":rocket:"));
            assert!(saved.contains("*bold*"));
            assert!(saved.contains("_italic_"));
            assert!(saved.contains("<u>under</u>"));
            assert!(saved.contains("~strike~"));
            assert!(saved.contains("`code`"));
            assert!(saved.contains("<https://example.com|docs>"));
            assert!(saved.contains("bullet one"));
            assert!(saved.contains("&gt; quoted note"));

            // Reloading saved draft yields the exact same serialized form & atom count
            let atom_count = c.editor.doc().atoms().len();
            assert_eq!(atom_count, 4);
            c.load_draft(&saved, cx);
            assert_eq!(c.text(), saved);
            assert_eq!(c.editor.doc().atoms().len(), atom_count);
        });
    });

    // 4. Edit-message mode: load_message, Enter submits edit, Esc cancels
    cx.update(|window, cx| {
        edit_composer.update(cx, |c, cx| {
            let msg = Message {
                text: "Original *bold* message".into(),
                ..Default::default()
            };
            c.load_message(&msg, window, cx);
            assert_eq!(c.text(), "Original *bold* message");
            c.editor.insert_text(" edited");
            c.on_enter(
                &Enter {
                    secondary: false,
                    shift: false,
                },
                window,
                cx,
            );
            // Re-load and cancel with Esc
            c.load_message(&msg, window, cx);
            c.on_escape(&Escape, window, cx);
        });
    });
    assert_eq!(submits.borrow().len(), 1);
    assert_eq!(submits.borrow()[0].text, "Original *bold* message edited");
    assert_eq!(*cancels.borrow(), 1);

    drop(root);
}

#[gpui::test]
fn composer_schedule_gif_staging_also_send_and_emoji_insertion(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let scheduled: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let custom_scheduled: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let submits: Rc<RefCell<Vec<MessageContent>>> = Rc::new(RefCell::new(Vec::new()));

    struct Host {
        composer: Entity<Composer>,
        _sub: Subscription,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.composer.clone())
        }
    }

    let slot = Rc::new(RefCell::new(None));
    let slot_c = slot.clone();
    let sched_c = scheduled.clone();
    let custom_c = custom_scheduled.clone();
    let sub_c = submits.clone();

    let (root, cx) = cx.add_window_view(move |window, cx| {
        let fixtures = Arc::new(crate::fixtures::load());
        let composer = Composer::new("Message".into(), fixtures, window, cx);
        *slot_c.borrow_mut() = Some(composer.clone());
        let view = cx.new(|cx| {
            let sub = cx.subscribe(&composer, move |_, _, ev: &ComposerEvent, _| match ev {
                ComposerEvent::Schedule { text, .. } => sched_c.borrow_mut().push(text.clone()),
                ComposerEvent::ScheduleCustom(text) => custom_c.borrow_mut().push(text.clone()),
                ComposerEvent::Submit(msg) => sub_c.borrow_mut().push(msg.clone()),
                _ => {}
            });
            Host {
                composer,
                _sub: sub,
            }
        });
        gpui_kit::base::Root::new(view, window, cx)
    });
    let composer = slot.borrow_mut().take().unwrap();
    cx.run_until_parked();

    // Blank draft must not emit Schedule or ScheduleCustom.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.schedule(SendLater::TomorrowMorning, window, cx);
            c.schedule_custom(cx);
        });
    });
    assert!(scheduled.borrow().is_empty());
    assert!(custom_scheduled.borrow().is_empty());

    // schedule_custom emits without clearing the draft.
    cx.update(|_, cx| {
        composer.update(cx, |c, cx| {
            c.editor.replace_all("ship tomorrow");
            c.schedule_custom(cx);
            assert_eq!(c.text(), "ship tomorrow");
        });
    });
    assert_eq!(custom_scheduled.borrow().as_slice(), ["ship tomorrow"]);

    // schedule emits and clears the draft.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.schedule(SendLater::TomorrowMorning, window, cx);
            assert!(c.text().is_empty());
        });
    });
    assert_eq!(scheduled.borrow().as_slice(), ["ship tomorrow"]);

    // GIF staging replaces previous GIF, tracks stale URL, and submits standalone.
    let g1 = sample_gif("one");
    let g2 = sample_gif("two");
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            c.stage_gif(g1.clone(), window, cx);
            c.stage_gif(g2.clone(), window, cx);
            assert_eq!(c.stale_gif_urls, vec![g1.preview_url.clone()]);
            c.submit(window, cx);
            assert!(c.pending_gif.is_none());
        });
    });
    assert_eq!(submits.borrow().len(), 1);
    assert_eq!(submits.borrow()[0].attachments.len(), 1);

    // remove_gif, insert_emoji, and AlsoSendTarget state transitions.
    cx.update(|window, cx| {
        composer.update(cx, |c, cx| {
            // remove_gif clears pending GIF and marks its preview URL stale.
            c.stage_gif(g2.clone(), window, cx);
            c.remove_gif(cx);
            assert!(c.pending_gif.is_none());
            assert!(c.stale_gif_urls.contains(&g2.preview_url));

            // insert_emoji inserts both unicode and custom emoji atoms (with trailing space).
            c.insert_emoji(
                &EmojiChoice {
                    name: "rocket".into(),
                    glyph: Some("🚀".into()),
                },
                window,
                cx,
            );
            c.insert_emoji(
                &EmojiChoice {
                    name: "shipit".into(),
                    glyph: None,
                },
                window,
                cx,
            );
            assert_eq!(c.text(), ":rocket: :shipit: ");

            // AlsoSendTarget ticks & resets on take_also_send.
            assert!(!c.take_also_send(cx));
            c.set_also_send(
                Some(AlsoSendTarget::Channel {
                    name: "general".into(),
                    private: false,
                }),
                cx,
            );
            if let Some((_, ticked)) = &mut c.also_send {
                *ticked = true;
            }
            assert!(c.take_also_send(cx));
            assert!(!c.take_also_send(cx));
        });
    });

    drop(root);
}

#[gpui::test]
fn outgoing_transforms_emoticons_preserve_underline_fences_header_tracking_and_edit_slash(
    cx: &mut TestAppContext,
) {
    use super::{Menu, apply_outgoing_transforms, convert_emoticons, set_outgoing_guard_prefs};
    use slackless_core::types::rich_text::{
        LinkElement, RichTextHeader, RichTextSection, TextElement,
    };
    use slackless_core::types::{
        Block, RichTextBlock, RichTextContainer, RichTextElement, TextStyle,
    };

    // 1. HTML-escaped `<3` (`&lt;3`), raw `<3`, triple-backtick fences with single backticks
    //    inside, and multi-backtick inline spans.
    assert_eq!(convert_emoticons("I <3 Rust"), "I :heart: Rust");
    assert_eq!(convert_emoticons("I &lt;3 Rust"), "I :heart: Rust");
    assert_eq!(
        convert_emoticons("```\n` :) `\n``` and :)"),
        "```\n` :) `\n``` and :slightly_smiling_face:"
    );
    assert_eq!(
        convert_emoticons("`` ` :) `` and &lt;3"),
        "`` ` :) `` and :heart:"
    );

    // 2. apply_outgoing_transforms preserves rich-text underline while converting emoticons
    //    and strips tracking query params inside RichTextContainer::Header links.
    let fixtures = crate::fixtures::load();
    cx.update(|cx| {
        let settings = crate::preferences::Settings {
            convert_emoticons: true,
            ..Default::default()
        };
        crate::preferences::set_display_prefs(&settings, cx);
        set_outgoing_guard_prefs(true, true, cx);

        let underline = Some(TextStyle {
            underline: true,
            ..Default::default()
        });
        let mut content = MessageContent {
            text: "Hello :) https://example.com/p?utm_source=slack&keep=1".to_owned(),
            blocks: vec![Block::RichText(RichTextBlock {
                block_id: None,
                elements: vec![
                    RichTextContainer::Section(RichTextSection {
                        elements: vec![RichTextElement::Text(TextElement {
                            text: "Hello :)".to_owned(),
                            style: underline,
                        })],
                    }),
                    RichTextContainer::Header(RichTextHeader {
                        level: Some(1),
                        elements: vec![RichTextElement::Link(LinkElement {
                            url: "https://example.com/p?utm_source=slack&keep=1".to_owned(),
                            text: Some("doc".to_owned()),
                            style: None,
                        })],
                    }),
                ],
            })],
            attachments: Vec::new(),
        };

        apply_outgoing_transforms(&mut content, &fixtures, cx);

        assert_eq!(
            content.text,
            "Hello :slightly_smiling_face: https://example.com/p?keep=1"
        );
        let [Block::RichText(rich)] = content.blocks.as_slice() else {
            panic!("expected one RichText block");
        };
        let RichTextContainer::Section(sec) = &rich.elements[0] else {
            panic!("expected section");
        };
        assert_eq!(sec.elements.len(), 2);
        match &sec.elements[0] {
            RichTextElement::Text(t) => {
                assert_eq!(t.text, "Hello ");
                assert_eq!(
                    t.style, underline,
                    "underline style must survive emoticon conversion"
                );
            }
            other => panic!("expected Text element, got {other:?}"),
        }
        match &sec.elements[1] {
            RichTextElement::Emoji(e) => {
                assert_eq!(e.name, "slightly_smiling_face");
                assert_eq!(e.style, underline);
            }
            other => panic!("expected Emoji element, got {other:?}"),
        }
        let RichTextContainer::Header(hdr) = &rich.elements[1] else {
            panic!("expected header");
        };
        let [RichTextElement::Link(link)] = hdr.elements.as_slice() else {
            panic!("expected link inside header");
        };
        assert_eq!(link.url, "https://example.com/p?keep=1");
    });

    // 3. Edit-mode slash suppression: `/topic` opens slash menu when allow_slash=true,
    //    is suppressed when allow_slash=false, while `@` mentions inside `/topic @...` still work.
    let mut editor = crate::editor::Editor::default();
    editor.replace_all("/topic");
    assert!(
        Menu::at_caret(&editor, &fixtures).is_some(),
        "normal composer opens slash menu"
    );
    assert!(
        Menu::at_caret_with_slash(&editor, &fixtures, false).is_none(),
        "edit composer suppresses slash menu"
    );
    editor.replace_all("/topic @he");
    assert!(
        Menu::at_caret_with_slash(&editor, &fixtures, false).is_some(),
        "edit composer still completes @mentions inside messages starting with `/`"
    );
}
