use clap::Parser as _;

use crate::cli::{
    AuthCommand, CanvasCommand, Cli, Commands, ExportFormat, HuddleCommand, ListCommand,
    ListReadFormat, READ_LIMIT, SEARCH_LIMIT, SavedCommand, StatusCommand,
};
use crate::commands::maintenance::format_stats_human;
use crate::commands::messages::format_markdown_export;
use crate::commands::{days_from_civil, parse_since_cutoff};
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

    let stats = format_stats_human(
        "T1",
        Some(std::path::Path::new("/tmp/T1.sqlite")),
        2 * 1024 * 1024,
        42,
        50,
        7,
        12,
        1,
    );
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
    let bytes = format_stats_human("T1", None, 512, 1, 2, 3, 4, 0);
    assert!(bytes.contains("Database:\t(unknown) (512 B)"));

    let kib = format_stats_human(
        "T1",
        Some(std::path::Path::new("/db/T1.sqlite")),
        1536,
        10,
        12,
        2,
        5,
        0,
    );
    assert!(kib.contains("Database:\t/db/T1.sqlite (1.5 KiB)"));

    let mib = format_stats_human(
        "T1",
        Some(std::path::Path::new("/db/T1.sqlite")),
        3 * 1024 * 1024,
        100,
        150,
        8,
        20,
        2,
    );
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
        Commands::Search {
            query,
            limit,
            all_workspaces,
            format,
            out,
        } => {
            assert_eq!(query, "launch in:#eng");
            assert_eq!(limit, SEARCH_LIMIT);
            assert!(!all_workspaces);
            assert_eq!(format, None);
            assert_eq!(out, None);
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
        Commands::Read {
            channel,
            limit,
            since,
        } => {
            assert_eq!(channel, "#general");
            assert_eq!(limit, READ_LIMIT);
            assert_eq!(since.as_deref(), Some("24h"));
        }
        _ => panic!("expected Read"),
    }

    // Thread live, digest defaults, upload multi-file, export defaults, nested subcommands.
    assert!(matches!(
        Cli::try_parse_from(["slackless", "thread", "#eng", "1700.1", "--live"])
            .unwrap()
            .command,
        Commands::Thread {
            live: true,
            limit: 200,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "digest"]).unwrap().command,
        Commands::Digest { ref since, all_workspaces: false, limit: 25 } if since == "24h"
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
        ])
        .unwrap()
        .command,
        Commands::Upload { ref files, comment: Some(_), thread_ts: Some(_), .. } if files.len() == 2
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "export", "#eng"])
            .unwrap()
            .command,
        Commands::Export {
            format: ExportFormat::Md,
            limit: 10_000,
            out: None,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "mark-thread", "#eng", "1700.1", "--unread"])
            .unwrap()
            .command,
        Commands::MarkThread {
            unread: true,
            ts: None,
            ..
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "saved", "list"]).unwrap().command,
        Commands::Saved { command: SavedCommand::List { ref filter, limit: 50 } } if filter == "in_progress"
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "canvas", "read", "F1", "--html"])
            .unwrap()
            .command,
        Commands::Canvas {
            command: CanvasCommand::Read { html: true, .. }
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "list", "read", "F2", "--format", "csv"])
            .unwrap()
            .command,
        Commands::List {
            command: ListCommand::Read {
                format: ListReadFormat::Csv,
                ..
            }
        }
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
        Commands::Status {
            command: StatusCommand::Set {
                expires_in: Some(1800),
                ..
            }
        }
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
        Commands::Huddle {
            command: HuddleCommand::Join {
                muted: true,
                duration: Some(5),
                ..
            }
        }
    ));
    assert!(matches!(
        Cli::try_parse_from(["slackless", "auth", "login", "acme"])
            .unwrap()
            .command,
        Commands::Auth {
            command: AuthCommand::Login { domain: Some(_) }
        }
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
    assert!(Cli::try_parse_from(["slackless", "bogus-cmd"]).is_err());
}
