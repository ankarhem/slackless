use clap::Parser as _;

use slackless_core::types::time::days_from_civil;

use crate::cli::{
    AuthCommand, CanvasAccessArg, CanvasCommand, Cli, Commands, ExportFormat, HuddleCommand,
    ListCommand, ListReadFormat, READ_LIMIT, SEARCH_LIMIT, SavedCommand, StatusCommand,
};
use crate::commands::maintenance::{StatsSummary, format_stats_human};
use crate::commands::messages::{
    SearchCmdOpts, cmd_search_in_archive, format_markdown_export, read_channel_messages,
};
use crate::commands::parse_since_cutoff;
use crate::output::{MessageRow, ReactionRow};

#[test]
fn formats_markdown_export_and_stats_human() {
    let rows = vec![MessageRow {
        channel: "C1".into(),
        ts: "1700000001.000100".into(),
        user: Some("U1".into()),
        user_name: Some("Ana".into()),
        bot_id: None,
        text: "Shipped release".into(),
        thread_ts: None,
        subtype: None,
        deleted: false,
        edited: false,
        reply_count: None,
        reactions: vec![ReactionRow {
            name: "tada".into(),
            count: 2,
        }],
        files: vec!["notes.md".into()],
        permalink: None,
    }];
    let md = format_markdown_export("#eng", &rows);
    assert!(md.contains("# #eng"));
    assert!(md.contains("**Ana** (`1700000001.000100`)"));
    assert!(md.contains("Shipped release"));
    assert!(md.contains("_Files:_ notes.md"));
    assert!(md.contains(":tada: 2"));

    let stats = format_stats_human(&StatsSummary {
        workspace: "T1",
        db_path: Some(std::path::Path::new("/tmp/T1.sqlite")),
        db_bytes: 2 * 1024 * 1024,
        messages: 42,
        revisions: 50,
        conversations: 7,
        users: 12,
        offline_queued: 1,
    });
    assert!(stats.contains("2.0 MiB"));
    assert!(stats.contains("42 (50 revisions)"));
    assert!(stats.contains("Offline queue:\t1"));
}

#[test]
fn parses_since_cutoffs_and_schedule_offsets() {
    let now = 1_764_000_000;
    assert_eq!(parse_since_cutoff("30m", now), Ok(now - 1800));
    assert_eq!(parse_since_cutoff("24h", now), Ok(now - 86_400));
    assert_eq!(parse_since_cutoff("7d", now), Ok(now - 7 * 86_400));
    assert_eq!(parse_since_cutoff("2w", now), Ok(now - 14 * 86_400));
    assert_eq!(parse_since_cutoff("1700000000", now), Ok(1_700_000_000));
    assert_eq!(parse_since_cutoff("1970-01-02", now), Ok(86_400));
    assert_eq!(parse_since_cutoff("2026-01-01", now), Ok(1_767_225_600));
    assert!(parse_since_cutoff("bogus", now).is_err());

    assert_eq!(
        crate::write::parse_schedule_at("+15m", 1_700_000_000),
        Ok(1_700_000_900)
    );
    assert_eq!(
        crate::write::parse_schedule_at("+2h", 1_700_000_000),
        Ok(1_700_007_200)
    );
    assert_eq!(
        crate::write::parse_schedule_at("1700005000", 1_700_000_000),
        Ok(1_700_005_000)
    );
    assert!(crate::write::parse_schedule_at("1699999999", 1_700_000_000).is_err());
    assert!(crate::write::parse_schedule_at("+0m", 1_700_000_000).is_err());
}

#[test]
fn parse_since_cutoff_validates_calendar_dates_leap_years_and_edges() {
    let now = 1_764_000_000i64;

    // Civil date epoch checks and leap years (2024, 2000) vs non-leap (1900, 2025, 2100).
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
    assert_eq!(days_from_civil(1970, 1, 2), 1);
    assert_eq!(days_from_civil(2000, 2, 29), 11_016);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(2024, 2, 29), 19_782);
    assert_eq!(days_from_civil(2024, 3, 1), 19_783);
    assert_eq!(
        days_from_civil(2025, 12, 31) + 1,
        days_from_civil(2026, 1, 1)
    );
    assert_eq!(
        days_from_civil(1900, 2, 28) + 1,
        days_from_civil(1900, 3, 1)
    );
    assert_eq!(
        days_from_civil(2100, 2, 28) + 1,
        days_from_civil(2100, 3, 1)
    );

    // Epoch, near-epoch, leap days, century non-leap years, and month ends (28/29/30/31).
    for (date, expected_days) in [
        ("1969-12-31", -1),
        ("1970-01-01", 0),
        ("1970-01-02", 1),
        ("1900-02-28", days_from_civil(1900, 2, 28)),
        ("2000-02-29", days_from_civil(2000, 2, 29)),
        ("2024-02-29", days_from_civil(2024, 2, 29)),
        ("2025-02-28", days_from_civil(2025, 2, 28)),
        ("2100-02-28", days_from_civil(2100, 2, 28)),
        ("2026-01-31", days_from_civil(2026, 1, 31)),
        ("2026-03-31", days_from_civil(2026, 3, 31)),
        ("2026-04-30", days_from_civil(2026, 4, 30)),
        ("2026-05-31", days_from_civil(2026, 5, 31)),
        ("2026-06-30", days_from_civil(2026, 6, 30)),
        ("2026-07-31", days_from_civil(2026, 7, 31)),
        ("2026-08-31", days_from_civil(2026, 8, 31)),
        ("2026-09-30", days_from_civil(2026, 9, 30)),
        ("2026-10-31", days_from_civil(2026, 10, 31)),
        ("2026-11-30", days_from_civil(2026, 11, 30)),
        ("2025-12-31", days_from_civil(2025, 12, 31)),
        ("2026-01-01", days_from_civil(2026, 1, 1)),
        ("2026-12-31", days_from_civil(2026, 12, 31)),
    ] {
        assert_eq!(
            parse_since_cutoff(date, now),
            Ok(expected_days * 86_400),
            "unexpected cutoff for {date}"
        );
    }

    // Invalid calendar dates (including century non-leap 1900/2100 & signed tokens) must fail.
    for bad in [
        "2026-02-29",
        "2026-02-31",
        "2025-02-29",
        "1900-02-29",
        "2100-02-29",
        "2024-02-30",
        "2026-04-31",
        "2026-06-31",
        "2026-09-31",
        "2026-11-31",
        "2026-00-10",
        "2026-13-01",
        "2026-01-00",
        "2026-01-32",
        "+2026-01-01",
        "2026-+01-01",
        "2026-01-+01",
        "2026-01-01-01",
        "26-01-01",
        "2026-01",
    ] {
        assert!(
            parse_since_cutoff(bad, now).is_err(),
            "expected Err for invalid date {bad}"
        );
    }

    // Empty / whitespace rejected.
    assert!(parse_since_cutoff("", now).is_err());
    assert!(parse_since_cutoff("   ", now).is_err());

    // Negative epoch passes through as i64.
    assert_eq!(parse_since_cutoff("-86400", now), Ok(-86_400));

    // Huge duration subtraction saturates cleanly instead of overflowing.
    assert_eq!(
        parse_since_cutoff(&format!("{}w", u64::MAX), -10),
        Ok(i64::MIN)
    );
}

#[test]
fn format_stats_human_covers_b_kib_mib_and_none_path() {
    let bytes = format_stats_human(&StatsSummary {
        workspace: "T1",
        db_path: None,
        db_bytes: 512,
        messages: 1,
        revisions: 2,
        conversations: 3,
        users: 4,
        offline_queued: 0,
    });
    assert!(bytes.contains("Database:\t(unknown) (512 B)"));

    let kib = format_stats_human(&StatsSummary {
        workspace: "T1",
        db_path: Some(std::path::Path::new("/db/T1.sqlite")),
        db_bytes: 1536,
        messages: 10,
        revisions: 12,
        conversations: 2,
        users: 5,
        offline_queued: 0,
    });
    assert!(kib.contains("Database:\t/db/T1.sqlite (1.5 KiB)"));

    let mib = format_stats_human(&StatsSummary {
        workspace: "T1",
        db_path: Some(std::path::Path::new("/db/T1.sqlite")),
        db_bytes: 3 * 1024 * 1024,
        messages: 100,
        revisions: 150,
        conversations: 8,
        users: 20,
        offline_queued: 2,
    });
    assert!(mib.contains("3.0 MiB"));
}

#[test]
fn format_markdown_export_covers_deleted_empty_bot_unknown_and_reactions() {
    let rows = vec![
        MessageRow {
            channel: "C1".into(),
            ts: "1700000001.000100".into(),
            user: None,
            user_name: None,
            bot_id: Some("B123".into()),
            text: String::new(),
            thread_ts: None,
            subtype: None,
            deleted: true,
            edited: false,
            reply_count: None,
            reactions: vec![
                ReactionRow {
                    name: "fire".into(),
                    count: 3,
                },
                ReactionRow {
                    name: "rocket".into(),
                    count: 1,
                },
            ],
            files: vec![],
            permalink: None,
        },
        MessageRow {
            channel: "C1".into(),
            ts: "1700000002.000100".into(),
            user: None,
            user_name: None,
            bot_id: None,
            text: "anonymous".into(),
            thread_ts: None,
            subtype: None,
            deleted: false,
            edited: false,
            reply_count: None,
            reactions: vec![],
            files: vec![],
            permalink: None,
        },
    ];
    let md = format_markdown_export("#ops", &rows);
    assert!(md.contains(
        "**B123** (`1700000001.000100`) _(deleted)_\n\n_Reactions:_ :fire: 3 · :rocket: 1\n\n---"
    ));
    assert!(md.contains("**unknown** (`1700000002.000100`)\n\nanonymous\n\n---"));
}

#[test]
fn cli_try_parse_from_covers_major_subcommands_globals_defaults_and_errors() {
    // Global flags before and after subcommand + search defaults.
    let cli = Cli::try_parse_from([
        "slackless",
        "--json",
        "--workspace",
        "T123",
        "search",
        "launch in:#eng",
    ])
    .unwrap();
    assert!(cli.json);
    assert_eq!(cli.workspace.as_deref(), Some("T123"));
    match cli.command {
        Commands::Search(args) => {
            assert_eq!(args.query, "launch in:#eng");
            assert_eq!(args.limit, SEARCH_LIMIT);
            assert!(!args.all_workspaces);
            assert!(!args.include_deleted);
            assert_eq!(args.format, None);
            assert_eq!(args.out, None);
        }
        _ => panic!("expected Search"),
    }

    // Global flags after subcommand + read with --since.
    let cli = Cli::try_parse_from([
        "slackless",
        "read",
        "#general",
        "--since",
        "24h",
        "--json",
        "--workspace",
        "T9",
    ])
    .unwrap();
    assert!(cli.json);
    assert_eq!(cli.workspace.as_deref(), Some("T9"));
    match cli.command {
        Commands::Read(args) => {
            assert_eq!(args.channel, "#general");
            assert_eq!(args.limit, READ_LIMIT);
            assert_eq!(args.since.as_deref(), Some("24h"));
            assert!(!args.include_deleted);
        }
        _ => panic!("expected Read"),
    }

    assert!(matches!(
        Cli::try_parse_from(["slackless", "search", "foo", "--include-deleted"])
            .unwrap()
            .command,
        Commands::Search(args) if args.include_deleted
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "read", "#eng", "--include-deleted"])
            .unwrap()
            .command,
        Commands::Read(args) if args.include_deleted
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "send", "#eng", "hello", "--allow-secrets"])
            .unwrap()
            .command,
        Commands::Send(args) if args.allow_secrets
    ));

    // Pre-send secret guard blocks API keys unless --allow-secrets is set.
    let ctx = crate::Ctx { json: false };
    let clean = crate::write::check_secrets(&ctx, "Hej teamet, allt ser bra ut!", false);
    assert!(clean.is_none());
    let leaked = format!(
        "token={} please check",
        ["xoxb", "1234567890", "abcdefghij123456"].join("-")
    );
    assert!(crate::write::check_secrets(&ctx, &leaked, false).is_some());
    assert!(crate::write::check_secrets(&ctx, &leaked, true).is_none());

    // Huddle transcripts & --transcribe flags.
    assert!(matches!(
        Cli::try_parse_from([
            "slackless",
            "huddle",
            "transcripts",
            "#eng",
            "--search",
            "möte",
            "--lang",
            "sv"
        ])
        .unwrap()
        .command,
        Commands::Huddle { command }
            if matches!(
                command.as_ref(),
                HuddleCommand::Transcripts {
                    channel: Some(ch),
                    search: Some(q),
                    lang: Some(l),
                    ..
                } if ch == "#eng" && q == "möte" && l == "sv"
            )
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "slackless",
            "huddle",
            "join",
            "#eng",
            "--transcribe",
            "parakeet"
        ])
        .unwrap()
        .command,
        Commands::Huddle { command }
            if matches!(command.as_ref(), HuddleCommand::Join { transcribe, .. } if transcribe == "parakeet")
    ));

    // Thread live, digest defaults, upload multi-file, export defaults, nested subcommands.
    assert!(matches!(
        Cli::try_parse_from(["slackless", "thread", "#eng", "1700.1", "--live"])
            .unwrap()
            .command,
        Commands::Thread(args) if args.live && args.limit == 200
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "digest"]).unwrap().command,
        Commands::Digest(args) if args.since == "24h" && !args.all_workspaces && args.limit == 25
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "slackless",
            "upload",
            "#eng",
            "a.txt",
            "b.txt",
            "-c",
            "here",
            "--thread-ts",
            "1700.1",
            "--allow-secrets",
        ])
        .unwrap()
        .command,
        Commands::Upload(args)
            if args.files.len() == 2
                && args.comment.is_some()
                && args.thread_ts.is_some()
                && args.allow_secrets
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "export", "#eng"])
            .unwrap()
            .command,
        Commands::Export(args)
            if args.format == ExportFormat::Md && args.limit == 10_000 && args.out.is_none()
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "mark-thread", "#eng", "1700.1", "--unread"])
            .unwrap()
            .command,
        Commands::MarkThread(args) if args.unread && args.ts.is_none()
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "saved", "list"]).unwrap().command,
        Commands::Saved { command }
            if matches!(command.as_ref(), SavedCommand::List { filter, limit: 50 } if filter == "saved")
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "unschedule", "#eng", "Dr01"]).unwrap().command,
        Commands::Unschedule { ref channel, ref id } if channel == "#eng" && id == "Dr01"
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "canvas", "read", "F1", "--html"])
            .unwrap()
            .command,
        Commands::Canvas { command }
            if matches!(command.as_ref(), CanvasCommand::Read { html: true, .. })
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "list", "read", "F2", "--format", "csv"])
            .unwrap()
            .command,
        Commands::List { command }
            if matches!(
                command.as_ref(),
                ListCommand::Read {
                    format: ListReadFormat::Csv,
                    ..
                }
            )
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "slackless",
            "status",
            "set",
            "In a meeting",
            "--emoji",
            "calendar",
            "--expires-in",
            "1800"
        ])
        .unwrap()
        .command,
        Commands::Status { command }
            if matches!(
                command.as_ref(),
                StatusCommand::Set {
                    expires_in: Some(1800),
                    ..
                }
            )
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "slackless",
            "huddle",
            "join",
            "#eng",
            "--muted",
            "--duration",
            "5"
        ])
        .unwrap()
        .command,
        Commands::Huddle { command }
            if matches!(
                command.as_ref(),
                HuddleCommand::Join {
                    muted: true,
                    duration: Some(5),
                    ..
                }
            )
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "auth", "login", "acme"])
            .unwrap()
            .command,
        Commands::Auth { command }
            if matches!(command.as_ref(), AuthCommand::Login { domain: Some(_) })
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "update", "--check"])
            .unwrap()
            .command,
        Commands::Update {
            check: true,
            force: false,
            restart: false
        }
    ));
    let transcribe_cli = Cli::try_parse_from([
        "slackless",
        "transcribe",
        "F0123VIDEO",
        "--file",
        "/tmp/demo.mp4",
        "--provider",
        "whisper",
        "--workspace",
        "T42",
        "--json",
    ])
    .unwrap();
    assert!(transcribe_cli.json);
    assert_eq!(transcribe_cli.workspace.as_deref(), Some("T42"));
    match transcribe_cli.command {
        Commands::Transcribe(args) => {
            assert_eq!(args.file_id, "F0123VIDEO");
            assert_eq!(
                args.file.as_deref(),
                Some(std::path::Path::new("/tmp/demo.mp4"))
            );
            assert_eq!(args.provider.as_deref(), Some("whisper"));
        }
        _ => panic!("expected Transcribe"),
    }

    // Additional simple subcommands parse cleanly.
    for argv in [
        vec!["slackless", "channels"],
        vec!["slackless", "dms"],
        vec!["slackless", "users", "--include-deleted", "ana"],
        vec!["slackless", "send", "#eng", "hello"],
        vec!["slackless", "reply", "#eng", "1700.1", "hi"],
        vec!["slackless", "edit", "#eng", "1700.1", "fixed"],
        vec!["slackless", "delete", "#eng", "1700.1"],
        vec!["slackless", "react", "#eng", "1700.1", ":tada:"],
        vec!["slackless", "unreact", "#eng", "1700.1", "tada"],
        vec!["slackless", "mark-read", "#eng"],
        vec!["slackless", "pins", "#eng"],
        vec!["slackless", "pin", "#eng", "1700.1"],
        vec!["slackless", "unpin", "#eng", "1700.1"],
        vec!["slackless", "schedule", "#eng", "later", "--at", "+15m"],
        vec!["slackless", "scheduled"],
        vec!["slackless", "stats", "--breakdown"],
        vec!["slackless", "vacuum"],
        vec!["slackless", "threads", "--limit", "10"],
        vec!["slackless", "revisions", "#eng", "1700.1"],
        vec!["slackless", "files", "--types", "pdfs"],
        vec!["slackless", "watch", "--count", "3"],
        vec!["slackless", "unread"],
        vec!["slackless", "download", "F123"],
        vec!["slackless", "transcribe", "F123"],
        vec!["slackless", "backfill", "#eng", "--pages", "2"],
        vec!["slackless", "auth", "status"],
        vec!["slackless", "auth", "logout"],
    ] {
        assert!(
            Cli::try_parse_from(&argv).is_ok(),
            "expected CLI parse success for {argv:?}"
        );
    }

    // Required argument errors & invalid enum values.
    assert!(Cli::try_parse_from(["slackless"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "send", "#eng"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "upload", "#eng"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "schedule", "#eng", "hi"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "export", "#eng", "--format", "xml"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "transcribe"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "bogus-cmd"]).is_err());
}

#[test]
fn cli_try_parse_from_covers_all_canvas_subcommands_and_conflicts() {
    let parse_canvas = |args: &[&str]| -> CanvasCommand {
        match Cli::try_parse_from(args).unwrap().command {
            Commands::Canvas { command } => *command,
            _ => panic!("expected Canvas command for {args:?}"),
        }
    };

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "info", "F123"]),
        CanvasCommand::Info { ref canvas_id } if canvas_id == "F123"
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "delete", "F123", "--json"]),
        CanvasCommand::Delete { ref canvas_id } if canvas_id == "F123"
    ));

    assert!(matches!(
        parse_canvas(&[
            "slackless",
            "canvas",
            "share",
            "F123",
            "--access",
            "write",
            "--channel",
            "#eng",
            "--channel",
            "C456",
            "--post",
        ]),
        CanvasCommand::Share {
            ref canvas_id,
            access: CanvasAccessArg::Write,
            ref channels,
            ref users,
            post: true,
        } if canvas_id == "F123" && channels == &["#eng", "C456"] && users.is_empty()
    ));

    assert!(matches!(
        parse_canvas(&[
            "slackless",
            "canvas",
            "share",
            "F123",
            "--access",
            "owner",
            "--user",
            "U1",
            "--user",
            "U2",
        ]),
        CanvasCommand::Share {
            access: CanvasAccessArg::Owner,
            ref channels,
            ref users,
            post: false,
            ..
        } if channels.is_empty() && users == &["U1", "U2"]
    ));

    assert!(matches!(
        parse_canvas(&[
            "slackless",
            "canvas",
            "unshare",
            "F123",
            "--channel",
            "#eng",
        ]),
        CanvasCommand::Unshare {
            ref canvas_id,
            ref channels,
            ref users,
        } if canvas_id == "F123" && channels == &["#eng"] && users.is_empty()
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "comments", "F123"]),
        CanvasCommand::Comments {
            ref canvas_id,
            resolved: false,
            all: false,
        } if canvas_id == "F123"
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "comments", "F123", "--resolved"]),
        CanvasCommand::Comments {
            resolved: true,
            all: false,
            ..
        }
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "comments", "F123", "--all"]),
        CanvasCommand::Comments {
            resolved: false,
            all: true,
            ..
        }
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "comment", "F123", "Looks great"]),
        CanvasCommand::Comment {
            ref canvas_id,
            ref text,
            thread: None,
            allow_secrets: false,
        } if canvas_id == "F123" && text == "Looks great"
    ));

    assert!(matches!(
        parse_canvas(&[
            "slackless",
            "canvas",
            "comment",
            "F123",
            "Replying",
            "--thread",
            "1700.1",
            "--allow-secrets",
        ]),
        CanvasCommand::Comment {
            ref text,
            ref thread,
            allow_secrets: true,
            ..
        } if text == "Replying" && thread.as_deref() == Some("1700.1")
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "resolve", "F123", "--thread", "1700.1"]),
        CanvasCommand::Resolve {
            ref canvas_id,
            ref thread,
            all: false,
            reopen: false,
        } if canvas_id == "F123" && thread.as_deref() == Some("1700.1")
    ));

    assert!(matches!(
        parse_canvas(&[
            "slackless",
            "canvas",
            "resolve",
            "F123",
            "--thread",
            "1700.1",
            "--reopen",
        ]),
        CanvasCommand::Resolve {
            ref thread,
            all: false,
            reopen: true,
            ..
        } if thread.as_deref() == Some("1700.1")
    ));

    assert!(matches!(
        parse_canvas(&["slackless", "canvas", "resolve", "F123", "--all"]),
        CanvasCommand::Resolve {
            thread: None,
            all: true,
            reopen: false,
            ..
        }
    ));

    // Invalid invocations & mutually exclusive clap flags.
    assert!(Cli::try_parse_from(["slackless", "canvas", "delete"]).is_err());
    assert!(Cli::try_parse_from(["slackless", "canvas", "info"]).is_err());
    assert!(
        Cli::try_parse_from(["slackless", "canvas", "share", "F123", "--channel", "C1"]).is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slackless",
            "canvas",
            "share",
            "F123",
            "--access",
            "admin",
            "--channel",
            "C1",
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slackless",
            "canvas",
            "comments",
            "F123",
            "--resolved",
            "--all",
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["slackless", "canvas", "comment", "F123"]).is_err());
    assert!(
        Cli::try_parse_from([
            "slackless",
            "canvas",
            "resolve",
            "F123",
            "--thread",
            "1700.1",
            "--all",
        ])
        .is_err()
    );
}

#[test]
fn read_channel_messages_does_not_starve_limit_on_deleted_or_since() {
    use slackless_core::store::Store;
    use slackless_core::types::{ChannelId, Message, TeamId, Ts};

    let ws = TeamId::from("T1");
    let ch = ChannelId::from("C1");
    let mut store = Store::open_in_memory().expect("in-memory store");

    // Seed 10 messages (ts 1000..=1009); mark the newest 5 (1005..=1009) as deleted.
    let msgs: Vec<Message> = (0..10)
        .map(|i| Message {
            ts: Ts::from(format!("{}.000000", 1_700_001_000 + i)),
            text: format!("msg {i}"),
            ..Message::default()
        })
        .collect();
    store.upsert_messages(&ws, &ch, &msgs).expect("seed");
    for i in 5..10 {
        store
            .record_deletion(
                &ws,
                &ch,
                &Ts::from(format!("{}.000000", 1_700_001_000 + i)),
                None,
                None,
            )
            .expect("delete");
    }

    // Requesting limit=3 excluding deleted must return msg 2, 3, 4 (not 0 messages!).
    let live = read_channel_messages(&store, &ws, &ch, 3, None, false).expect("read");
    let texts: Vec<&str> = live.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(texts, vec!["msg 2", "msg 3", "msg 4"]);

    // With `--since` at 1003 and limit=5 excluding deleted -> returns msg 3, 4.
    let since_only =
        read_channel_messages(&store, &ws, &ch, 5, Some(1_700_001_003), false).expect("since");
    let texts: Vec<&str> = since_only.iter().map(|m| m.message.text.as_str()).collect();
    assert_eq!(texts, vec!["msg 3", "msg 4"]);

    // Including deleted with limit=3 -> returns msg 7, 8, 9.
    let with_deleted = read_channel_messages(&store, &ws, &ch, 3, None, true).expect("all");
    let texts: Vec<&str> = with_deleted
        .iter()
        .map(|m| m.message.text.as_str())
        .collect();
    assert_eq!(texts, vec!["msg 7", "msg 8", "msg 9"]);
}

#[test]
fn cmd_search_finds_message_by_spoken_word_from_video_transcript() {
    use std::process::ExitCode;

    use slackless_core::store::Store;
    use slackless_core::types::{ChannelId, File, FileId, Message, TeamId, Ts};

    use crate::archive::Archive;

    let tmp_dir = std::env::temp_dir().join(format!(
        "slackless-cli-video-search-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
    let db_path = tmp_dir.join("T1.sqlite");
    let hit_out = tmp_dir.join("hit.json");
    let miss_out = tmp_dir.join("miss.json");

    let ws = TeamId::from("T1");
    let ch = ChannelId::from("C1");
    let file_id = FileId::from("FVIDEO42");
    let msg_ts = Ts::from("1700002000.000100");

    let mut store = Store::open(&db_path).expect("open store");
    store
        .upsert_messages(
            &ws,
            &ch,
            &[Message {
                ts: msg_ts.clone(),
                text: "Sprint demo recording attached".into(),
                files: vec![File {
                    id: file_id.clone(),
                    name: Some("sprint-demo.mp4".into()),
                    mimetype: Some("video/mp4".into()),
                    ..File::default()
                }],
                ..Message::default()
            }],
        )
        .expect("seed video message");

    let updated = store
        .upsert_file_transcript(
            &ws,
            &file_id,
            "parakeet-v3",
            "We rolled out the zephyrgate canary deployment yesterday",
        )
        .expect("upsert file transcript");
    assert_eq!(updated, 1);
    drop(store);

    let archive = Archive::open_path(&db_path)
        .expect("open archive")
        .expect("archive exists");
    let ctx = crate::Ctx { json: true };

    let code = cmd_search_in_archive(
        &ctx,
        &ws,
        Some(&archive),
        "zephyrgate",
        SearchCmdOpts {
            limit: 20,
            sort: slackless_core::store::SearchSort::Recent,
            format: Some(ExportFormat::Json),
            out: Some(&hit_out),
            include_deleted: false,
        },
    )
    .expect("cmd_search hit");
    assert_eq!(code, ExitCode::SUCCESS);

    let payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hit_out).expect("read hit json"))
            .expect("parse hit json");
    assert_eq!(payload["count"], 1);
    assert_eq!(payload["messages"][0]["ts"], msg_ts.as_str());
    assert_eq!(payload["messages"][0]["files"][0], "sprint-demo.mp4");

    let miss_code = cmd_search_in_archive(
        &ctx,
        &ws,
        Some(&archive),
        "unrelatedtoken",
        SearchCmdOpts {
            limit: 20,
            sort: slackless_core::store::SearchSort::Recent,
            format: Some(ExportFormat::Json),
            out: Some(&miss_out),
            include_deleted: false,
        },
    )
    .expect("cmd_search miss");
    assert_eq!(miss_code, ExitCode::SUCCESS);

    let miss_payload: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&miss_out).expect("read miss json"))
            .expect("parse miss json");
    assert_eq!(miss_payload["count"], 0);

    drop(archive);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[test]
fn parses_unified_search_export_and_archive_cli_flags() {
    use crate::cli::{ArchiveCommand, SearchSortArg};

    let cli = Cli::try_parse_from([
        "slackless",
        "--json",
        "search",
        "--all-workspaces",
        "--sort",
        "relevant",
        "--limit",
        "15",
        "canary",
    ])
    .expect("parse unified search");
    match cli.command {
        Commands::Search(args) => {
            assert!(args.all_workspaces);
            assert_eq!(args.sort, SearchSortArg::Relevant);
            assert_eq!(args.limit, 15);
            assert_eq!(args.query, "canary");
        }
        _ => panic!("expected Search"),
    }

    let cli_thread = Cli::try_parse_from([
        "slackless",
        "export",
        "#eng",
        "--thread",
        "1700000001.000100",
        "--format",
        "json",
        "--exclude-deleted",
    ])
    .expect("parse thread export");
    match cli_thread.command {
        Commands::Export(args) => {
            assert_eq!(args.channel.as_deref(), Some("#eng"));
            assert_eq!(args.thread.as_deref(), Some("1700000001.000100"));
            assert_eq!(args.format, ExportFormat::Json);
            assert!(args.exclude_deleted);
        }
        _ => panic!("expected Export"),
    }

    let cli_transcript = Cli::try_parse_from([
        "slackless",
        "export",
        "--transcript",
        "FCLIP9",
        "--format",
        "md",
    ])
    .expect("parse transcript export");
    match cli_transcript.command {
        Commands::Export(args) => {
            assert!(args.channel.is_none());
            assert_eq!(args.transcript.as_deref(), Some("FCLIP9"));
        }
        _ => panic!("expected Export"),
    }

    let cli_huddle = Cli::try_parse_from(["slackless", "export", "--huddle", "R123"])
        .expect("parse huddle export");
    match cli_huddle.command {
        Commands::Export(args) => {
            assert_eq!(args.huddle.as_deref(), Some("R123"));
        }
        _ => panic!("expected Export"),
    }

    let cli_bf = Cli::try_parse_from([
        "slackless",
        "archive",
        "backfill",
        "--all",
        "--threads",
        "--pages",
        "3",
        "--page-size",
        "50",
    ])
    .expect("parse archive backfill");
    match cli_bf.command {
        Commands::Archive { command } => match *command {
            ArchiveCommand::Backfill(args) => {
                assert!(args.all);
                assert!(args.threads);
                assert_eq!(args.pages, 3);
                assert_eq!(args.page_size, 50);
            }
            _ => panic!("expected ArchiveCommand::Backfill"),
        },
        _ => panic!("expected Archive"),
    }

    let cli_status =
        Cli::try_parse_from(["slackless", "--json", "archive", "status"]).expect("parse status");
    assert!(matches!(
        cli_status.command,
        Commands::Archive { command } if matches!(*command, ArchiveCommand::Status)
    ));
}

#[test]
fn cmd_search_all_workspaces_and_export_and_archive_status_end_to_end() {
    use std::collections::HashMap;
    use std::process::ExitCode;

    use slackless_core::store::{SearchSort, Store};
    use slackless_core::types::{
        ChannelId, Conversation, File, FileId, FileMode, Message, Team, TeamId, Ts, User, UserId,
    };
    use slackless_core::{Synced, write_synced_spans};

    use crate::archive::Archive;
    use crate::commands::maintenance::build_archive_status_report;
    use crate::commands::messages::{
        ExportCmdOpts, SearchAllOpts, cmd_export_in_archive, cmd_search_all_in_archives,
    };

    let tmp_dir = std::env::temp_dir().join(format!(
        "slackless-cli-unified-export-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).expect("create temp dir");

    let ws1 = TeamId::from("T_ALPHA");
    let ws2 = TeamId::from("T_BETA");
    let db1 = tmp_dir.join("T_ALPHA.sqlite");
    let db2 = tmp_dir.join("T_BETA.sqlite");

    // Seed workspace 1: channel, thread, snippet, file transcript, huddle transcript, synced span.
    {
        let mut s1 = Store::open(&db1).expect("open db1");
        s1.upsert_team(
            &ws1,
            &Team {
                id: ws1.clone(),
                name: "Alpha Corp".into(),
                ..Team::default()
            },
        )
        .unwrap();
        s1.upsert_user(
            &ws1,
            &User {
                id: UserId::from("U1"),
                name: "alice".into(),
                ..User::default()
            },
        )
        .unwrap();
        s1.upsert_conversations(
            &ws1,
            &[Conversation {
                id: ChannelId::from("C1"),
                name: Some("ops".into()),
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            }],
        )
        .unwrap();
        s1.upsert_messages(
            &ws1,
            &ChannelId::from("C1"),
            &[
                Message {
                    ts: Ts::from("1700000100.000000"),
                    user: Some(UserId::from("U1")),
                    text: "Deploying quantumbridge v1".into(),
                    reply_count: Some(1),
                    ..Message::default()
                },
                Message {
                    ts: Ts::from("1700000105.000000"),
                    thread_ts: Some(Ts::from("1700000100.000000")),
                    user: Some(UserId::from("U1")),
                    text: "Thread reply confirmed".into(),
                    ..Message::default()
                },
            ],
        )
        .unwrap();
        s1.upsert_file_transcript(
            &ws1,
            &FileId::from("F_CLIP"),
            "whisper",
            "Spoken quantumbridge walkthrough notes",
        )
        .unwrap();
        s1.insert_huddle_transcript_segment(
            &ws1,
            &slackless_core::store::NewHuddleTranscriptSegment {
                channel: &ChannelId::from("C1"),
                call_id: "R_HUDDLE_1",
                thread_root_ts: None,
                speaker_id: Some(&UserId::from("U1")),
                lang: Some("en"),
                source: "local",
                text: "Discussed quantumbridge rollout",
                at_ms: 1_700_000_200_000,
            },
        )
        .unwrap();
        let mut spans = HashMap::new();
        spans.insert(
            ChannelId::from("C1"),
            Synced {
                newest: Ts::from("1700000105.000000"),
                oldest: Ts::from("1700000100.000000"),
                complete: true,
            },
        );
        write_synced_spans(&mut s1, &ws1, &spans).unwrap();
    }

    // Seed workspace 2: snippet attachment matching the same term.
    {
        let mut s2 = Store::open(&db2).expect("open db2");
        s2.upsert_team(
            &ws2,
            &Team {
                id: ws2.clone(),
                name: "Beta Labs".into(),
                ..Team::default()
            },
        )
        .unwrap();
        s2.upsert_conversations(
            &ws2,
            &[Conversation {
                id: ChannelId::from("C2"),
                name: Some("release".into()),
                is_channel: true,
                is_member: true,
                ..Conversation::default()
            }],
        )
        .unwrap();
        s2.upsert_messages(
            &ws2,
            &ChannelId::from("C2"),
            &[Message {
                ts: Ts::from("1700000300.000000"),
                text: "Config snippet".into(),
                files: vec![File {
                    id: FileId::from("F_SNIP"),
                    name: Some("bridge.toml".into()),
                    mode: Some(FileMode::Snippet),
                    preview: Some("quantumbridge = true".into()),
                    ..File::default()
                }],
                ..Message::default()
            }],
        )
        .unwrap();
    }

    let ctx_json = crate::Ctx { json: true };
    let search_out = tmp_dir.join("all_search.json");
    let code = cmd_search_all_in_archives(
        &ctx_json,
        &[(ws1.clone(), db1.clone()), (ws2.clone(), db2.clone())],
        "quantumbridge",
        SearchAllOpts {
            limit: 20,
            sort: SearchSort::Recent,
            format: Some(ExportFormat::Json),
            out: Some(&search_out),
            include_deleted: false,
        },
    )
    .expect("cross-workspace search");
    assert_eq!(code, ExitCode::SUCCESS);
    let search_json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&search_out).unwrap()).unwrap();
    assert_eq!(search_json["all_workspaces"], true);
    assert_eq!(search_json["count"], 3); // T_BETA snippet + T_ALPHA msg + T_ALPHA standalone transcript
    let msgs = search_json["messages"].as_array().unwrap();
    assert!(
        msgs.iter()
            .any(|m| m["workspace"] == "T_BETA" && m["kind"] == "snippet")
    );
    assert!(
        msgs.iter()
            .any(|m| m["workspace"] == "T_ALPHA" && m["kind"] == "message")
    );
    assert!(
        msgs.iter()
            .any(|m| m["workspace"] == "T_ALPHA" && m["kind"] == "file_transcript")
    );

    // Export channel (Markdown with threads), thread (JSON), file transcript (MD), huddle (JSON).
    let archive1 = Archive::open_path(&db1).unwrap().unwrap();
    let ch_md_path = tmp_dir.join("ops.md");
    assert_eq!(
        cmd_export_in_archive(
            &crate::Ctx { json: false },
            &ws1,
            Some(&archive1),
            ExportCmdOpts {
                channel: Some("#ops"),
                thread_ts: None,
                transcript_file_id: None,
                huddle_call_id: None,
                include_threads: true,
                include_deleted: true,
                format: ExportFormat::Md,
                limit: 100,
                out: Some(&ch_md_path),
            },
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    let ch_md = std::fs::read_to_string(&ch_md_path).unwrap();
    assert!(ch_md.contains("# #ops"));
    assert!(ch_md.contains("Deploying quantumbridge v1"));
    assert!(ch_md.contains("Thread reply confirmed"));

    let thread_json_path = tmp_dir.join("thread.json");
    assert_eq!(
        cmd_export_in_archive(
            &ctx_json,
            &ws1,
            Some(&archive1),
            ExportCmdOpts {
                channel: Some("C1"),
                thread_ts: Some("1700000100.000000"),
                transcript_file_id: None,
                huddle_call_id: None,
                include_threads: false,
                include_deleted: true,
                format: ExportFormat::Json,
                limit: 100,
                out: Some(&thread_json_path),
            },
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    let thread_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&thread_json_path).unwrap()).unwrap();
    assert_eq!(thread_val["kind"], "thread");
    assert_eq!(thread_val["count"], 2);

    let clip_md_path = tmp_dir.join("clip.md");
    assert_eq!(
        cmd_export_in_archive(
            &crate::Ctx { json: false },
            &ws1,
            Some(&archive1),
            ExportCmdOpts {
                channel: None,
                thread_ts: None,
                transcript_file_id: Some("F_CLIP"),
                huddle_call_id: None,
                include_threads: false,
                include_deleted: true,
                format: ExportFormat::Md,
                limit: 100,
                out: Some(&clip_md_path),
            },
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    assert!(
        std::fs::read_to_string(&clip_md_path)
            .unwrap()
            .contains("Spoken quantumbridge walkthrough notes")
    );

    let huddle_json_path = tmp_dir.join("huddle.json");
    assert_eq!(
        cmd_export_in_archive(
            &ctx_json,
            &ws1,
            Some(&archive1),
            ExportCmdOpts {
                channel: None,
                thread_ts: None,
                transcript_file_id: None,
                huddle_call_id: Some("R_HUDDLE_1"),
                include_threads: false,
                include_deleted: true,
                format: ExportFormat::Json,
                limit: 100,
                out: Some(&huddle_json_path),
            },
        )
        .unwrap(),
        ExitCode::SUCCESS
    );
    let huddle_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&huddle_json_path).unwrap()).unwrap();
    assert_eq!(huddle_val["kind"], "huddle_transcript");
    assert_eq!(huddle_val["count"], 1);

    // Archive status report verifies synced spans + counts.
    let (status_json, status_human) = build_archive_status_report(&ws1, &archive1).unwrap();
    assert_eq!(status_json["completed_channels"], 1);
    assert_eq!(status_json["total_channels"], 1);
    assert!(status_human.contains("#ops"));
    assert!(status_human.contains("complete"));

    drop(archive1);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
