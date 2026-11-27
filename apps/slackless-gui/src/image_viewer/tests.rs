use super::item::{ViewerPdf, empty_placeholder_source, message_images, poster_source_if_present};
use super::pdf::{VIEWER_PDF_MAX_H, VIEWER_PDF_MAX_W};
use super::*;
use crate::blockkit::serde_json::json;
use gpui::{IntoElement, Render, px};
use slackless_core::types::{File, FileId, Message, Ts, UserId};

#[test]
fn message_images_collects_videos_alongside_images_in_order() {
    let fixtures = crate::fixtures::load();
    let mut video_file = File {
        id: FileId::from("F_VID1"),
        name: Some("demo.mp4".into()),
        mimetype: Some("video/mp4".into()),
        filetype: Some("mp4".into()),
        thumb_480: Some("https://files.slack.com/tmb/demo_480.jpg".into()),
        url_private: Some("https://files.slack.com/pri/demo.mp4".into()),
        url_private_download: Some("https://files.slack.com/dl/demo.mp4".into()),
        user: Some(UserId::from("U_ALICE")),
        timestamp: Some(1_700_000_123),
        ..Default::default()
    };
    video_file.extra.insert("duration_ms".into(), json!(18_500));
    video_file.extra.insert(
        "mp4".into(),
        json!("https://files.slack.com/stream/demo.mp4"),
    );
    video_file
        .extra
        .insert("transcript".into(), json!("Hello from the demo video"));
    video_file
        .extra
        .insert("vtt".into(), json!("https://files.slack.com/demo.vtt"));

    let image_file = File {
        id: FileId::from("F_IMG1"),
        name: Some("shot.png".into()),
        mimetype: Some("image/png".into()),
        url_private: Some("https://files.slack.com/pri/shot.png".into()),
        ..Default::default()
    };

    let video_no_poster = File {
        id: FileId::from("F_VID2"),
        name: Some("clip.mov".into()),
        filetype: Some("mov".into()),
        thumb_video: Some("https://files.slack.com/tmb/clip.mp4".into()),
        url_private: Some("https://files.slack.com/pri/clip.mov".into()),
        ..Default::default()
    };

    let video_no_url = File {
        id: FileId::from("F_VID_EMPTY"),
        name: Some("broken.mp4".into()),
        mimetype: Some("video/mp4".into()),
        ..Default::default()
    };

    let msg = Message {
        ts: Ts::from("1700000000.000100"),
        user: Some(UserId::from("U_BOB")),
        files: vec![image_file, video_file, video_no_poster, video_no_url],
        ..Default::default()
    };

    let entries = message_images(&fixtures, &msg);
    assert_eq!(entries.len(), 3, "video without playback URL is skipped");

    assert_eq!(entries[0].origin, ImageOrigin::File(0));
    assert_eq!(entries[0].image.media, ViewerMedia::Image);
    assert_eq!(entries[0].image.name, "shot.png");

    assert_eq!(entries[1].origin, ImageOrigin::File(1));
    assert_eq!(entries[1].image.name, "demo.mp4");
    assert_eq!(entries[1].image.user, Some(UserId::from("U_ALICE")));
    assert_eq!(entries[1].image.taken_at, Some(1_700_000_123));
    assert_eq!(
        entries[1].image.download,
        Some((
            "https://files.slack.com/dl/demo.mp4".to_owned(),
            "demo.mp4".to_owned()
        ))
    );
    assert!(poster_source_if_present(&entries[1].image.source).is_some());
    assert_eq!(
        entries[1].image.media,
        ViewerMedia::Video(ViewerVideo {
            file_id: "F_VID1".to_owned(),
            url: "https://files.slack.com/stream/demo.mp4".to_owned(),
            ext: "mp4".to_owned(),
            duration_ms: Some(18_500),
            inline_transcript: Some("Hello from the demo video".to_owned()),
            vtt_url: Some("https://files.slack.com/demo.vtt".to_owned()),
        })
    );

    assert_eq!(entries[2].origin, ImageOrigin::File(2));
    assert!(
        poster_source_if_present(&entries[2].image.source).is_none(),
        "video with only .mp4 thumb_video uses empty placeholder"
    );
    assert_eq!(
        entries[2].image.media,
        ViewerMedia::Video(ViewerVideo {
            file_id: "F_VID2".to_owned(),
            url: "https://files.slack.com/pri/clip.mov".to_owned(),
            ext: "mov".to_owned(),
            duration_ms: None,
            inline_transcript: None,
            vtt_url: None,
        })
    );

    // `collect_viewer_images` resolves the clicked video origin accurately.
    let (all, ix) = collect_viewer_images(
        &fixtures,
        std::iter::once(&msg),
        &msg.ts,
        &ImageOrigin::File(1),
    )
    .expect("clicked video must be found");
    assert_eq!(all.len(), 3);
    assert_eq!(ix, 1);
    let ViewerMedia::Video(video) = &all[ix].media else {
        panic!("the clicked item must be a video");
    };
    assert_eq!(video.file_id, "F_VID1");
}

#[gpui::test]
fn viewer_steps_between_image_and_video_and_cleans_up_on_close(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let items = Rc::new(vec![
        ViewerImage {
            source: empty_placeholder_source(),
            name: "photo.png".into(),
            user: None,
            taken_at: None,
            download: None,
            media: ViewerMedia::Image,
        },
        ViewerImage {
            source: empty_placeholder_source(),
            name: "demo.mp4".into(),
            user: None,
            taken_at: None,
            download: Some((
                "https://files.slack.com/dl/demo.mp4".into(),
                "demo.mp4".into(),
            )),
            media: ViewerMedia::Video(ViewerVideo {
                file_id: "F_DEMO".into(),
                url: "https://files.slack.com/pri/demo.mp4".into(),
                ext: "mp4".into(),
                duration_ms: Some(12_000),
                inline_transcript: Some("Welcome to the walkthrough clip.".into()),
                vtt_url: None,
            }),
        },
    ]);
    let (viewer, cx) = cx.add_window_view(|window, cx| {
        let entity = ImageViewer::new(
            Arc::new(crate::fixtures::load()),
            "#norce".into(),
            items,
            0,
            window,
            cx,
        );
        // Unwrap the created entity into a host view that mounts `ImageViewer`.
        struct Host(Entity<ImageViewer>);
        impl Render for Host {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                self.0.clone()
            }
        }
        Host(entity)
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let iv = viewer.read(cx).0.clone();
        assert!(!iv.read(cx).is_video());
        iv.update(cx, |v, cx| v.step(1, window, cx));
        assert!(iv.read(cx).is_video());
        assert!(iv.read(cx).video_playback().is_some());
        assert!(!iv.read(cx).showing_transcript());
        iv.update(cx, |v, cx| v.toggle_transcript(window, cx));
        assert!(iv.read(cx).showing_transcript());
        let _ = window.draw(cx);
        // Stepping back to an image stops and clears video playback immediately and hides the transcript drawer.
        iv.update(cx, |v, cx| v.step(-1, window, cx));
        assert!(!iv.read(cx).is_video());
        assert!(!iv.read(cx).showing_transcript());
        assert!(iv.read(cx).video_playback().is_none());
        // Stepping onto video then closing via `evict_assets` stops playback immediately.
        iv.update(cx, |v, cx| v.step(1, window, cx));
        assert!(iv.read(cx).video_playback().is_some());
        iv.update(cx, |v, cx| v.toggle_transcript(window, cx));
        assert!(!iv.read(cx).showing_transcript());
        iv.update(cx, |v, cx| v.evict_assets(window, cx));
        assert!(iv.read(cx).video_playback().is_none());
    });
    cx.run_until_parked();
}

#[test]
fn message_images_collects_pdfs_alongside_images_and_videos() {
    let fixtures = crate::fixtures::load();
    let pdf_file = File {
        id: FileId::from("F_PDF1"),
        name: Some("architecture.pdf".into()),
        mimetype: Some("application/pdf".into()),
        filetype: Some("pdf".into()),
        thumb_pdf: Some("https://files.slack.com/tmb/architecture.png".into()),
        url_private: Some("https://files.slack.com/pri/architecture.pdf".into()),
        url_private_download: Some("https://files.slack.com/dl/architecture.pdf".into()),
        ..Default::default()
    };
    let msg = Message {
        ts: Ts::from("1700000001.000100"),
        user: Some(UserId::from("U_ALICE")),
        files: vec![pdf_file],
        ..Default::default()
    };
    let entries = message_images(&fixtures, &msg);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].origin, ImageOrigin::File(0));
    assert_eq!(entries[0].image.name, "architecture.pdf");
    assert_eq!(
        entries[0].image.media,
        ViewerMedia::Pdf(ViewerPdf {
            file_id: "F_PDF1".into(),
            url: "https://files.slack.com/dl/architecture.pdf".into(),
            local_path: None,
        })
    );
}

#[gpui::test]
fn viewer_renders_timestamped_clickable_transcript_cues(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let cues = vec![
        TranscriptCue {
            start_secs: 0.0,
            end_secs: 3.0,
            text: "First spoken segment.".into(),
        },
        TranscriptCue {
            start_secs: 3.5,
            end_secs: 7.0,
            text: "Second spoken segment.".into(),
        },
    ];
    let fixtures = Arc::new(crate::fixtures::load().with_file_transcript_cues(
        FileId::from("F_TIMED"),
        "First spoken segment. Second spoken segment.".into(),
        cues.clone(),
    ));
    let items = Rc::new(vec![ViewerImage {
        source: empty_placeholder_source(),
        name: "walkthrough.mp4".into(),
        user: None,
        taken_at: None,
        download: None,
        media: ViewerMedia::Video(ViewerVideo {
            file_id: "F_TIMED".into(),
            url: "https://files.slack.com/pri/walkthrough.mp4".into(),
            ext: "mp4".into(),
            duration_ms: Some(10_000),
            inline_transcript: None,
            vtt_url: None,
        }),
    }]);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let entity = ImageViewer::new(fixtures, "#eng".into(), items, 0, window, cx);
        struct Host(Entity<ImageViewer>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.clone()
            }
        }
        Host(entity)
    });
    let (iv, transcript) = cx.update(|window, cx| {
        let iv = host.read(cx).0.clone();
        iv.update(cx, |v, cx| v.toggle_transcript(window, cx));
        assert!(iv.read(cx).showing_transcript());
        let _ = window.draw(cx);
        let transcript = iv
            .read(cx)
            .transcript
            .as_ref()
            .map(|(transcript, _)| transcript.clone())
            .expect("the open drawer is mounted");
        assert_eq!(
            transcript.read(cx).text.as_deref(),
            Some("First spoken segment. Second spoken segment.")
        );
        assert_eq!(&*transcript.read(cx).cues, cues.as_slice());
        // Verify cue row click tracking persists across re-renders while rejecting drag
        // selections and mismatched cue indices.
        transcript.update(cx, |t, cx| {
            t.record_cue_mouse_down(1, 1, gpui::point(px(40.), px(60.)));
            cx.notify();
        });
        let _ = window.draw(cx);
        assert!(
            transcript.update(cx, |t, _| t
                .take_cue_click(1, gpui::point(px(41.), px(61.)))),
            "cue click must survive a re-render between mouse-down and mouse-up"
        );
        transcript.update(cx, |t, cx| {
            t.record_cue_mouse_down(1, 1, gpui::point(px(40.), px(60.)));
            cx.notify();
        });
        let _ = window.draw(cx);
        assert!(
            !transcript.update(cx, |t, _| t
                .take_cue_click(1, gpui::point(px(75.), px(60.)))),
            "text drag-selection must NOT trigger a cue click seek"
        );
        (iv, transcript)
    });

    // The playhead repaints the drawer only when the cue under it changes, so the 30 fps
    // playback tick leaves the cue list alone.
    let repaints = Rc::new(std::cell::Cell::new(0));
    let counter = repaints.clone();
    let _watch =
        cx.update(|_, cx| cx.observe(&transcript, move |_, _| counter.set(counter.get() + 1)));
    for position in [0.5, 1.0, 2.5, 4.0, 4.5] {
        cx.update(|_, cx| {
            transcript.update(cx, |t, cx| t.set_playhead(position, true, cx));
        });
    }
    assert_eq!(repaints.get(), 2);
    assert_eq!(transcript.read_with(cx, |t, _| t.active_cue), Some(1));
    cx.update(|window, cx| iv.update(cx, |v, cx| v.evict_assets(window, cx)));
    cx.run_until_parked();
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn viewer_renders_and_steps_multipage_local_pdf(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let path = std::env::temp_dir().join(format!(
        "slackless-viewer-pdf-test-{}.pdf",
        std::process::id()
    ));
    let pdf_bytes = {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offs = Vec::new();
        offs.push(out.len());
        out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        offs.push(out.len());
        out.extend_from_slice(
            b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        );
        offs.push(out.len());
        out.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>\nendobj\n",
        );
        offs.push(out.len());
        out.extend_from_slice(
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] >>\nendobj\n",
        );
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    };
    std::fs::write(&path, pdf_bytes).expect("write test PDF");

    let items = Rc::new(vec![ViewerImage {
        source: empty_placeholder_source(),
        name: "spec.pdf".into(),
        user: None,
        taken_at: None,
        download: None,
        media: ViewerMedia::Pdf(ViewerPdf {
            file_id: "F_LOCAL_PDF".into(),
            url: String::new(),
            local_path: Some(path.clone()),
        }),
    }]);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let entity = ImageViewer::new(
            Arc::new(crate::fixtures::load()),
            "#eng".into(),
            items,
            0,
            window,
            cx,
        );
        struct Host(Entity<ImageViewer>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.clone()
            }
        }
        Host(entity)
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let iv = host.read(cx).0.clone();
        assert!(iv.read(cx).is_pdf());
        assert_eq!(iv.read(cx).pdf_page(), 0);
        assert_eq!(iv.read(cx).pdf_page_count(), Some(2));
        let _ = window.draw(cx);
        assert!(iv.update(cx, |v, cx| v.step_pdf_page(1, window, cx)));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        let iv = host.read(cx).0.clone();
        assert_eq!(iv.read(cx).pdf_page(), 1);
        assert!(!iv.update(cx, |v, cx| v.step_pdf_page(1, window, cx)));
        let _ = window.draw(cx);
        // Stepping back to page 0 hits the LRU cache synchronously.
        assert!(iv.update(cx, |v, cx| v.step_pdf_page(-1, window, cx)));
        assert_eq!(iv.read(cx).pdf_page(), 0);
        iv.update(cx, |v, cx| v.evict_assets(window, cx));
        assert!(iv.read(cx).pdf_playback().is_none());
        assert!(
            crate::media::cached_pdf_page("F_LOCAL_PDF", 0, VIEWER_PDF_MAX_W, VIEWER_PDF_MAX_H, cx)
                .is_none(),
            "evict_assets must drop full-resolution viewer PDF pages from cache"
        );
    });
    cx.run_until_parked();
    let _ = std::fs::remove_file(&path);
}

#[gpui::test]
fn viewer_keystroke_isolation_handles_image_video_and_pdf(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let ks = |spec: &'static str| gpui::Keystroke::parse(spec).unwrap();
    let items = Rc::new(vec![
        ViewerImage {
            source: empty_placeholder_source(),
            name: "photo.png".into(),
            user: None,
            taken_at: None,
            download: None,
            media: ViewerMedia::Image,
        },
        ViewerImage {
            source: empty_placeholder_source(),
            name: "demo.mp4".into(),
            user: None,
            taken_at: None,
            download: None,
            media: ViewerMedia::Video(ViewerVideo {
                file_id: "F_VID_KEYS".into(),
                url: String::new(),
                ext: "mp4".into(),
                duration_ms: Some(12_000),
                vtt_url: None,
                inline_transcript: Some("hello world".into()),
            }),
        },
        ViewerImage {
            source: empty_placeholder_source(),
            name: "deck.pdf".into(),
            user: None,
            taken_at: None,
            download: None,
            media: ViewerMedia::Pdf(ViewerPdf {
                file_id: "F_PDF_KEYS".into(),
                url: String::new(),
                local_path: None,
            }),
        },
    ]);
    let (host, cx) = cx.add_window_view(|window, cx| {
        let entity = ImageViewer::new(
            Arc::new(crate::fixtures::load()),
            "#eng".into(),
            items,
            0,
            window,
            cx,
        );
        struct Host(Entity<ImageViewer>);
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.0.clone()
            }
        }
        Host(entity)
    });
    cx.update(|window, cx| {
        let iv = host.read(cx).0.clone();
        // On an image: up/down/space are absorbed so hidden transcript does not move;
        // plain `c` and `Cmd+C` are ignored; `Escape` requests close; `Cmd+Left` ignored.
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("up"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("space"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("c"), window, cx)),
            ViewerKeyOutcome::Ignored
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("cmd-c"), window, cx)),
            ViewerKeyOutcome::Ignored
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("cmd-left"), window, cx)),
            ViewerKeyOutcome::Ignored
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("escape"), window, cx)),
            ViewerKeyOutcome::Close
        );

        // Step to video item: plain `c` toggles transcript, `Cmd+C` still passes through.
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("right"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        assert!(iv.read(cx).is_video());
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("cmd-c"), window, cx)),
            ViewerKeyOutcome::Ignored
        );
        assert!(!iv.read(cx).showing_transcript());
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("c"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        assert!(iv.read(cx).showing_transcript());

        // Step to PDF item: shift-left on page 0 is still Handled (no leak to hidden UI).
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("right"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        assert!(iv.read(cx).is_pdf());
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(
                &ks("shift-left"),
                window,
                cx
            )),
            ViewerKeyOutcome::Handled
        );
        assert_eq!(
            iv.update(cx, |v, cx| v.handle_keystroke(&ks("up"), window, cx)),
            ViewerKeyOutcome::Handled
        );
        iv.update(cx, |v, cx| v.evict_assets(window, cx));
    });
}
