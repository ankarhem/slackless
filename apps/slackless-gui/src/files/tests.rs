use gpui_kit::assets::IconName;
use gpui_kit::component::v_flex;
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    self, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window,
};
use slackless_core::types::{File, FileId, FileMode};

use super::gallery::{COMPACT_BELOW, WIDE_FROM, gallery_tiles_for_dims};
use super::meta::{
    FileTileTone, INLINE_MAX_BYTES, MAX_THUMB_H, MAX_THUMB_W, SNIPPET_MAX_EXPANDED_LINES,
    audio_wave_bars, clip_vtt_url, download_url, external_provider_label, external_subtitle,
    file_ext, file_icon_kind, file_name, fit, format_duration, human_size, inline_allowed,
    inline_clip_transcript, is_audio, is_decodable_image_url, is_snippet, is_video,
    media_duration_ms, parse_webvtt, remote_thumbnail, snippet_hidden_lines, snippet_is_truncated,
    snippet_lines, snippet_visible_and_omitted, video_poster_url,
};
use super::snippet::{SnippetCard, SnippetStore};
use crate::blockkit::serde_json::json;

#[test]
fn human_size_fit_and_inline_limits() {
    assert_eq!(human_size(-50), "0 B");
    assert_eq!(human_size(0), "0 B");
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1024), "1.0 KiB");
    assert_eq!(human_size(52 * 1024), "52.0 KiB");
    assert_eq!(human_size(2 * 1024 * 1024 + 300 * 1024), "2.3 MiB");

    // `fit` preserves aspect ratio, never upscales small images, and falls back on missing/invalid dims.
    assert_eq!(fit(Some(200), Some(100)), (200., 100.));
    assert_eq!(fit(Some(720), Some(360)), (360., 180.));
    let (pw, ph) = fit(Some(400), Some(800));
    assert!((pw - 120.).abs() < 0.01 && (ph - 240.).abs() < 0.01);
    assert_eq!(fit(None, None), (MAX_THUMB_W, MAX_THUMB_H));
    assert_eq!(fit(Some(0), Some(100)), (MAX_THUMB_W, MAX_THUMB_H));
    assert_eq!(fit(Some(-10), Some(100)), (MAX_THUMB_W, MAX_THUMB_H));

    // Inline size limit honors `show_large`.
    let small = File {
        size: Some(INLINE_MAX_BYTES),
        ..Default::default()
    };
    let big = File {
        size: Some(INLINE_MAX_BYTES + 1),
        ..Default::default()
    };
    assert!(inline_allowed(&small, false));
    assert!(!inline_allowed(&big, false));
    assert!(inline_allowed(&big, true));
}

#[test]
fn file_classification_naming_urls_and_icon_kinds() {
    let bare = File {
        id: FileId::from("F0BARE"),
        ..Default::default()
    };
    assert_eq!(file_name(&bare), "F0BARE");
    assert_eq!(file_ext(&bare, "bin"), "bin");
    assert_eq!(download_url(&bare), None);
    assert_eq!(remote_thumbnail(&bare), None);
    assert!(!is_audio(&bare));
    assert!(!is_video(&bare));
    assert!(!is_snippet(&bare));
    assert_eq!(file_icon_kind(&bare), (IconName::File, FileTileTone::Link));

    let titled = File {
        id: FileId::from("F0TITLE"),
        title: Some("Design Spec".into()),
        url_private: Some("https://files.slack.com/priv".into()),
        ..Default::default()
    };
    assert_eq!(file_name(&titled), "Design Spec");
    assert_eq!(
        download_url(&titled).as_deref(),
        Some("https://files.slack.com/priv")
    );

    let downloadable = File {
        id: FileId::from("F0DL"),
        name: Some("report.pdf".into()),
        filetype: Some("pdf".into()),
        url_private: Some("https://files.slack.com/priv".into()),
        url_private_download: Some("https://files.slack.com/dl".into()),
        ..Default::default()
    };
    assert_eq!(file_name(&downloadable), "report.pdf");
    assert_eq!(file_ext(&downloadable, "txt"), "pdf");
    assert_eq!(
        download_url(&downloadable).as_deref(),
        Some("https://files.slack.com/dl")
    );
    assert_eq!(
        file_icon_kind(&downloadable),
        (IconName::FileText, FileTileTone::Danger)
    );

    // Extension inferred from filename when `filetype` is absent.
    let by_name = File {
        name: Some("archive.tar.gz".into()),
        ..Default::default()
    };
    assert_eq!(file_ext(&by_name, "bin"), "gz");

    // Thumbnail preference: GIF animated thumbs win over static thumbs; non-image ignored.
    let img = File {
        mimetype: Some("image/gif".into()),
        thumb_360: Some("https://t/360".into()),
        thumb_480: Some("https://t/480".into()),
        thumb_360_gif: Some("https://t/360.gif".into()),
        thumb_480_gif: Some("https://t/480.gif".into()),
        ..Default::default()
    };
    assert_eq!(remote_thumbnail(&img).as_deref(), Some("https://t/480.gif"));
    let non_img = File {
        mimetype: Some("application/pdf".into()),
        thumb_480: Some("https://t/480".into()),
        ..Default::default()
    };
    assert_eq!(remote_thumbnail(&non_img), None);

    // Non-image container URLs (e.g. `.mp4` `thumb_video`, `.heic` originals) are skipped.
    assert!(is_decodable_image_url(
        "https://files.slack.com/files-tmb/T1-F1/img_720.jpg"
    ));
    assert!(is_decodable_image_url(
        "https://files.slack.com/files-tmb/T1-F1/img_720.png?t=123"
    ));
    assert!(!is_decodable_image_url(
        "https://files.slack.com/files-tmb/T1-F1/img_2040.mp4"
    ));
    assert!(!is_decodable_image_url(
        "https://files.slack.com/files-pri/T1-F1/img_1217.HEIC"
    ));
    let video_with_mp4_thumb = File {
        mimetype: Some("video/mp4".into()),
        thumb_video: Some("https://files.slack.com/files-tmb/T1-F1/clip.mp4".into()),
        thumb_480: Some("https://files.slack.com/files-tmb/T1-F1/clip_480.jpg".into()),
        ..Default::default()
    };
    assert_eq!(
        video_poster_url(&video_with_mp4_thumb).as_deref(),
        Some("https://files.slack.com/files-tmb/T1-F1/clip_480.jpg")
    );
    let video_only_mp4_thumb = File {
        mimetype: Some("video/mp4".into()),
        thumb_video: Some("https://files.slack.com/files-tmb/T1-F1/clip.mp4".into()),
        ..Default::default()
    };
    assert_eq!(video_poster_url(&video_only_mp4_thumb), None);

    // Audio / video detection by mimetype and filetype.
    assert!(is_audio(&File {
        mimetype: Some("audio/mpeg".into()),
        ..Default::default()
    }));
    assert!(is_audio(&File {
        filetype: Some("opus".into()),
        ..Default::default()
    }));
    assert!(is_video(&File {
        mimetype: Some("video/mp4".into()),
        ..Default::default()
    }));
    assert!(is_video(&File {
        filetype: Some("webm".into()),
        ..Default::default()
    }));

    // Icon classification across major Slack file types.
    for (ft, expected) in [
        ("xlsx", (IconName::FileSpreadsheet, FileTileTone::Success)),
        ("zip", (IconName::FileArchive, FileTileTone::Warning)),
        ("png", (IconName::FileImage, FileTileTone::Link)),
        ("mp4", (IconName::Video, FileTileTone::Link)),
        ("m4a", (IconName::Mic, FileTileTone::Link)),
        ("quip", (IconName::StickyNote, FileTileTone::Link)),
        ("list", (IconName::Table2, FileTileTone::Link)),
    ] {
        let f = File {
            filetype: Some(ft.into()),
            ..Default::default()
        };
        assert_eq!(file_icon_kind(&f), expected, "filetype {ft}");
    }
}

#[test]
fn external_provider_labels_and_subtitles() {
    let mk = |ext: Option<&str>, pretty: Option<&str>| File {
        external_type: ext.map(str::to_owned),
        pretty_type: pretty.map(str::to_owned),
        ..Default::default()
    };
    assert_eq!(
        external_provider_label(&mk(Some("gdrive"), None)),
        "Google Drive"
    );
    assert_eq!(
        external_provider_label(&mk(Some("SharePoint"), None)),
        "OneDrive"
    );
    assert_eq!(external_provider_label(&mk(Some("figma"), None)), "Figma");
    assert_eq!(external_provider_label(&mk(Some("linear"), None)), "Linear");
    assert_eq!(external_provider_label(&mk(None, None)), "External file");

    assert_eq!(
        external_subtitle(&mk(Some("gdrive"), Some("Google Docs"))),
        "Google Drive \u{b7} Google Docs"
    );
    // Duplicate pretty_type matching provider name (case-insensitive) is deduplicated.
    assert_eq!(
        external_subtitle(&mk(Some("dropbox"), Some("dropbox"))),
        "Dropbox"
    );
}

#[test]
fn clip_transcript_webvtt_duration_and_waveform_seams() {
    assert_eq!(format_duration(0), "0:00");
    assert_eq!(format_duration(5_000), "0:05");
    assert_eq!(format_duration(125_999), "2:05");

    let mut f = File::default();
    assert_eq!(media_duration_ms(&f), None);
    f.extra.insert("duration_ms".into(), json!(0));
    assert_eq!(media_duration_ms(&f), None);
    f.extra.insert("duration_ms".into(), json!(42_500.0));
    assert_eq!(media_duration_ms(&f), Some(42_500));

    // Inline transcript precedence: transcription.preview.content > transcription.text > transcript > preview.
    let mut tf = File {
        preview: Some("  from preview  ".into()),
        ..Default::default()
    };
    assert_eq!(inline_clip_transcript(&tf).as_deref(), Some("from preview"));
    tf.extra
        .insert("transcript".into(), json!("from transcript"));
    assert_eq!(
        inline_clip_transcript(&tf).as_deref(),
        Some("from transcript")
    );
    tf.extra
        .insert("transcription".into(), json!({ "text": "from text" }));
    assert_eq!(inline_clip_transcript(&tf).as_deref(), Some("from text"));
    tf.extra.insert(
        "transcription".into(),
        json!({ "text": "from text", "preview": { "content": "from preview content" } }),
    );
    assert_eq!(
        inline_clip_transcript(&tf).as_deref(),
        Some("from preview content")
    );

    // WebVTT URL extraction & http filtering.
    let mut vf = File::default();
    vf.extra.insert("vtt".into(), json!("not-a-url"));
    assert_eq!(clip_vtt_url(&vf), None);
    vf.extra.remove("vtt");
    vf.extra.insert(
        "transcription".into(),
        json!({ "vtt": "https://files.slack.com/clip.vtt" }),
    );
    assert_eq!(
        clip_vtt_url(&vf).as_deref(),
        Some("https://files.slack.com/clip.vtt")
    );

    // WebVTT stripping.
    let vtt = "WEBVTT\n\nNOTE generated\n\n1\n00:00:00.000 --> 00:00:02.000\nHello team\n\n2\n00:00:02.000 --> 00:00:04.000\nReady to ship.\n";
    assert_eq!(parse_webvtt(vtt), "Hello team Ready to ship.");

    // Audio waveform normalization & downsampling cap at 28 bars.
    let mut wf = File::default();
    assert!(audio_wave_bars(&wf).is_empty());
    let samples: Vec<f64> = (0..100).map(|i| i as f64).collect();
    wf.extra.insert("audio_wave_samples".into(), json!(samples));
    let bars = audio_wave_bars(&wf);
    assert_eq!(bars.len(), 28);
    assert!(bars.iter().all(|&b| (0.12..=1.0).contains(&b)));
}

#[test]
fn gallery_tiles_layout_across_compact_narrow_and_wide_widths() {
    let dims = [
        (Some(400), Some(1000)),  // tall
        (Some(1000), Some(1000)), // square
        (Some(1600), Some(900)),  // wide
        (None, None),             // missing metadata
    ];

    // Compact (< 376px): 2 square columns.
    let compact = gallery_tiles_for_dims(COMPACT_BELOW - 10., &dims);
    assert_eq!(compact.len(), 4);
    assert!(compact.iter().all(|&(cell, _, h)| (cell - h).abs() < 0.01));

    // Narrow (376..664px): row height capped at 136px, spans 2/3/4 columns.
    let narrow = gallery_tiles_for_dims(COMPACT_BELOW + 40., &dims);
    assert_eq!(narrow[0].2, 136.);
    assert!(narrow[0].1 < narrow[1].1);
    assert!(narrow[1].1 < narrow[2].1);

    // Wide (>= 664px): row height capped at 232px, spans 3/5/7 columns.
    let wide = gallery_tiles_for_dims(WIDE_FROM + 100., &dims);
    assert_eq!(wide[0].2, 232.);
    assert!(wide[0].1 < wide[1].1);
    assert!(wide[1].1 < wide[2].1);
    // Missing dimensions fall back cleanly to minimum 40px tile width.
    assert_eq!(wide[3].1, 40.);
}

#[test]
fn snippet_detection_truncation_and_line_counts_match_slack() {
    // Untruncated 7-line preview.
    let snippet = File {
        id: FileId::from("F0SNIP"),
        name: Some("Chalk_Custom.terminal".into()),
        mode: Some(FileMode::Snippet),
        filetype: Some("xml".into()),
        preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n".into()),
        ..Default::default()
    };
    assert!(is_snippet(&snippet));
    let lines = snippet_lines(&snippet);
    assert_eq!(lines.len(), 7);
    assert!(!snippet_is_truncated(&snippet, lines.len()));
    assert_eq!(snippet_hidden_lines(&snippet, lines.len(), false), 2);

    // Server-truncated Slack snippet: only 5 preview lines, `lines: 252`, `lines_more: 247`.
    let mut truncated = File {
        id: FileId::from("F0TRUNC"),
        name: Some("Chalk.terminal".into()),
        mode: Some(FileMode::Snippet),
        filetype: Some("xml".into()),
        preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\n".into()),
        ..Default::default()
    };
    truncated
        .extra
        .insert("preview_is_truncated".into(), json!(true));
    truncated.extra.insert("lines".into(), json!(252));
    truncated.extra.insert("lines_more".into(), json!(247));
    let trunc_lines = snippet_lines(&truncated);
    assert_eq!(trunc_lines.len(), 5);
    assert!(snippet_is_truncated(&truncated, trunc_lines.len()));
    assert_eq!(
        snippet_hidden_lines(&truncated, trunc_lines.len(), false),
        247
    );
    // Once full text is fetched (say 252 lines), hidden lines computes from loaded text.
    assert_eq!(snippet_hidden_lines(&truncated, 252, true), 247);

    // String-encoded `lines_more` without `lines` or `preview_is_truncated` still triggers expansion.
    let mut string_meta = File {
        id: FileId::from("F0STR"),
        mode: Some(FileMode::Snippet),
        preview: Some("a\nb\nc\nd\ne\n".into()),
        ..Default::default()
    };
    string_meta.extra.insert("lines_more".into(), json!("19"));
    assert!(snippet_is_truncated(&string_meta, 5));
    assert_eq!(snippet_hidden_lines(&string_meta, 5, false), 19);

    // Short 3-line snippet is NOT truncated and has 0 hidden lines.
    let short = File {
        id: FileId::from("F0SHORT"),
        mode: Some(FileMode::Snippet),
        preview: Some("a\nb\nc\n".into()),
        ..Default::default()
    };
    assert!(!snippet_is_truncated(&short, 3));
    assert_eq!(snippet_hidden_lines(&short, 3, false), 0);

    // Non-snippet canvas / PDF files must not render as code snippet cards.
    let canvas = File {
        id: FileId::from("F0CANV"),
        filetype: Some("quip".into()),
        preview: Some("hello".into()),
        ..Default::default()
    };
    assert!(!is_snippet(&canvas));

    // Expanding a 252-line file shows the entire file inline; files beyond the inline cap
    // report the exact omitted line count so the UI tells the user it was truncated.
    assert_eq!(snippet_visible_and_omitted(252, false), (5, 0));
    assert_eq!(snippet_visible_and_omitted(252, true), (252, 0));
    assert_eq!(
        snippet_visible_and_omitted(SNIPPET_MAX_EXPANDED_LINES + 500, true),
        (SNIPPET_MAX_EXPANDED_LINES, 500)
    );
}

#[gpui::test]
fn snippet_card_renders_and_persists_expansion_across_unmount(cx: &mut TestAppContext) {
    struct Host {
        visible: bool,
        files: Vec<File>,
    }
    impl Render for Host {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            v_flex()
                .size_full()
                .gap_4()
                .p_4()
                .when(self.visible, |col| {
                    col.children(self.files.iter().enumerate().map(|(ix, file)| SnippetCard {
                        file: file.clone(),
                        order_base: (ix as u64 + 1) * 1024,
                    }))
                })
        }
    }

    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });

    let mut truncated = File {
        id: FileId::from("F0TRUNC"),
        name: Some("Chalk_Custom.terminal".into()),
        title: Some("Chalk_Custom.terminal".into()),
        mode: Some(FileMode::Snippet),
        pretty_type: Some("Plain Text".into()),
        preview: Some("line 1\nline 2\nline 3\nline 4\nline 5\n".into()),
        ..Default::default()
    };
    truncated
        .extra
        .insert("preview_is_truncated".into(), json!(true));
    truncated.extra.insert("lines".into(), json!(252));
    truncated.extra.insert("lines_more".into(), json!(247));

    let untruncated = File {
        id: FileId::from("F0FULL"),
        name: Some("deploy.sh".into()),
        title: Some("deploy.sh".into()),
        mode: Some(FileMode::Snippet),
        pretty_type: Some("Shell".into()),
        preview: Some("1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n".into()),
        ..Default::default()
    };

    let huge_text: String = (1..=SNIPPET_MAX_EXPANDED_LINES + 50)
        .map(|n| format!("line {n}\n"))
        .collect();
    let huge = File {
        id: FileId::from("F0HUGE"),
        name: Some("dump.log".into()),
        title: Some("dump.log".into()),
        mode: Some(FileMode::Snippet),
        pretty_type: Some("Plain Text".into()),
        preview: Some(huge_text),
        ..Default::default()
    };

    let (view, cx) = cx.add_window_view(|_, _| Host {
        visible: true,
        files: vec![truncated, untruncated, huge],
    });
    cx.update(|window, cx| {
        // Simulate expanding `F0TRUNC` with fetched full text cached in `SnippetStore`,
        // and expanding `F0HUGE` (> 1,000 lines) to exercise the truncation notice banner.
        let entries = &mut cx.default_global::<SnippetStore>().entries;
        let entry = entries.entry(FileId::from("F0TRUNC")).or_default();
        entry.expanded = true;
        entry.full_text = Some("line 1\nline 2\nline 3\nline 4\nline 5\nline 6\n".into());
        entries.entry(FileId::from("F0HUGE")).or_default().expanded = true;
        let _ = window.draw(cx);
    });

    // Unmount the snippet cards completely (simulating scrolling out of virtualized list view).
    view.update(cx, |host, cx| {
        host.visible = false;
        cx.notify();
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    // Remount (scroll back into view) and confirm expanded state & cached full text survived.
    view.update(cx, |host, cx| {
        host.visible = true;
        cx.notify();
    });
    cx.update(|window, cx| {
        let _ = window.draw(cx);
        let stored = cx
            .global::<SnippetStore>()
            .entries
            .get(&FileId::from("F0TRUNC"))
            .expect("snippet state must survive unmount/scroll-out");
        assert!(stored.expanded);
        assert!(
            stored
                .full_text
                .as_deref()
                .is_some_and(|t| t.contains("line 6"))
        );
    });
    cx.run_until_parked();
}
