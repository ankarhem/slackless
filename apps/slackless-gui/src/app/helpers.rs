//! Pure stateless helpers: permalink/tag parsing, emoticons, link safety, and demo data.

use super::*;

/// The conversation a workspace opens on: its first DM, else its first channel.
/// The conversation to open: the one most recently read in Slack (live), else the first DM or
/// channel.
/// The in-app target of a Slack permalink (`https://<team>.slack.com/archives/...`).
pub(super) struct Permalink {
    pub(super) channel: ChannelId,
    /// The message the link points at (the `p<ts>` path segment).
    pub(super) ts: Ts,
    /// The thread the link opens into, when it is a thread link (`?thread_ts=`).
    pub(super) thread_ts: Option<Ts>,
}

/// Parse a Slack permalink; `None` for anything else (or a malformed one).
pub(super) fn parse_permalink(url: &str) -> Option<Permalink> {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut path = base.strip_prefix("https://")?.split('/');
    let host = path.next()?;
    if !host.ends_with(".slack.com") {
        return None;
    }
    if path.next()? != "archives" {
        return None;
    }
    let channel = path.next()?.trim();
    if channel.is_empty() {
        return None;
    }
    let ts = ts_from_permalink(path.next()?.trim())?;
    let mut permalink = Permalink {
        channel: ChannelId::from(channel),
        ts,
        thread_ts: None,
    };
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        match key {
            "thread_ts" => permalink.thread_ts = Some(Ts::new(value)),
            // A thread link's channel rides the `cid` query parameter (DMs).
            "cid" => permalink.channel = ChannelId::from(value),
            _ => {}
        }
    }
    Some(permalink)
}

/// `p1234567890123456` back to Slack's `ts` (`1234567890.123456`): the permalink form drops
/// the dot before the six-digit fraction.
pub(super) fn ts_from_permalink(digits: &str) -> Option<Ts> {
    let digits = digits.strip_prefix('p')?;
    if digits.len() < 7 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (secs, fraction) = digits.split_at(digits.len() - 6);
    Some(Ts::new(format!("{secs}.{fraction}")))
}

pub(super) fn first_conversation(fixtures: &Fixtures) -> ChannelId {
    let last_read = fixtures
        .counts
        .values()
        .filter(|counts| counts.last_read.is_some() && fixtures.conversation(&counts.id).is_some())
        .max_by(|a, b| a.last_read.cmp(&b.last_read));
    last_read
        .map(|counts| counts.id.clone())
        .or_else(|| {
            fixtures
                .dms
                .first()
                .or(fixtures.channels.first())
                .map(|c| c.id.clone())
        })
        .unwrap_or_default()
}

/// Derive a workspace switcher tile's badge from a background workspace's `client.counts`
/// and Activity unread count (`F-CORE-006`).
pub(super) fn workspace_badge_from_counts(
    counts: &HashMap<ChannelId, ConversationCounts>,
    activity_unread: u32,
) -> WorkspaceBadge {
    let mut mentions = activity_unread;
    let mut has_unreads = activity_unread > 0;
    for entry in counts.values() {
        mentions = mentions.saturating_add(entry.effective_mentions());
        if entry.is_unread() {
            has_unreads = true;
        }
    }
    WorkspaceBadge {
        mentions,
        has_unreads,
    }
}

/// Decode a system notification tag (`msg:<channel>:<ts>`, `later:<channel>:<ts>`, or
/// `bg:<workspace_index>:<channel>:<ts>`) into `(optional_workspace_index, channel, ts)` (`F-SHELL-171`).
pub(super) fn parse_notification_target(tag: &str) -> Option<(Option<usize>, ChannelId, Ts)> {
    if let Some(rest) = tag.strip_prefix("bg:") {
        let (ws_str, tail) = rest.split_once(':')?;
        let ws: usize = ws_str.parse().ok()?;
        let (channel, ts) = tail.split_once(':')?;
        return (!channel.is_empty() && !ts.is_empty())
            .then(|| (Some(ws), ChannelId::from(channel), Ts::from(ts)));
    }
    let (channel, ts) = parse_notification_tag(tag)?;
    Some((None, channel, ts))
}

/// Decode a system notification tag (`msg:<channel>:<ts>` or `later:<channel>:<ts>`).
pub(super) fn parse_notification_tag(tag: &str) -> Option<(ChannelId, Ts)> {
    let rest = tag
        .strip_prefix("msg:")
        .or_else(|| tag.strip_prefix("later:"))?;
    let (channel, ts) = rest.split_once(':')?;
    (!channel.is_empty() && !ts.is_empty()).then(|| (ChannelId::from(channel), Ts::from(ts)))
}

/// First `@here` / `@channel` / `@everyone` broadcast inside `content`, if any.
pub(super) fn broadcast_kind(content: &MessageContent) -> Option<BroadcastRange> {
    fn scan_elements(elements: &[RichTextElement]) -> Option<BroadcastRange> {
        elements.iter().find_map(|el| match el {
            RichTextElement::Broadcast(b) => Some(b.range.clone()),
            _ => None,
        })
    }
    fn scan_containers(containers: &[RichTextContainer]) -> Option<BroadcastRange> {
        containers.iter().find_map(|c| match c {
            RichTextContainer::Section(s) => scan_elements(&s.elements),
            RichTextContainer::Header(h) => scan_elements(&h.elements),
            RichTextContainer::Quote(q) => scan_elements(&q.elements),
            RichTextContainer::List(l) => scan_containers(&l.elements),
            RichTextContainer::Preformatted(_)
            | RichTextContainer::Divider(_)
            | RichTextContainer::Unknown { .. } => None,
        })
    }
    content.blocks.iter().find_map(|block| match block {
        Block::RichText(rt) => scan_containers(&rt.elements),
        Block::Unknown { .. } => None,
    })
}

/// Screenshot/demo hook: `SLACKLESS_GUI_DEMO=browser` opens the channel browser at startup,
/// `SLACKLESS_GUI_DEMO=search:<query>` starts on the results of `<query>`,
/// `SLACKLESS_GUI_DEMO=canvas` / `=list` start in the huddle channel with the huddle bar open
/// on the Canvas / List tab. `=prefs` opens Preferences on Appearance & Themes, `=profile` the
/// first DM peer's profile, `=status` the status picker, `=switcher` the Cmd+K switcher and
/// `=menu` the title-bar user menu. `=blockkit` starts in #engineering on the deploy bot's
/// Block Kit message; `=emoji` additionally opens the composer's emoji picker. `=shortcuts`
/// opens the shortcuts sheet, `=drafts` Drafts & sent, `=side` the second workspace and
/// `=slash` the composer's slash menu. `=later` opens Later, `=pins` the pins panel,
/// `=activity` the Activity page, `=unreads` the All Unreads view, `=files` the Files view
/// (`=files:canvases` / `=files:lists` on that page), `=notify` posts a test system
/// notification, `=typing` shows two people typing in the open conversation and `=message`
/// delivers a demo direct message.
pub(super) fn demo(results: &Entity<SearchView>, window: &mut Window, cx: &mut Context<App>) {
    let Ok(demo) = std::env::var("SLACKLESS_GUI_DEMO") else {
        return;
    };
    if matches!(demo.as_str(), "blockkit" | "emoji") {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.select("C0ENG".into(), cx);
                    if demo == "emoji" {
                        // Where the composer's emoji button sits in the default 1200x800 window.
                        let anchor = point(px(340.), px(730.));
                        app.open_picker(PickerTarget::Composer, anchor, window, cx);
                    }
                });
            }
        });
        return;
    }
    if matches!(
        demo.as_str(),
        "prefs"
            | "prefs:nav"
            | "huddle:popout"
            | "profile"
            | "status"
            | "switcher"
            | "menu"
            | "shortcuts"
            | "drafts"
            | "side"
            | "slash"
            | "later"
            | "pins"
            | "activity"
            | "unreads"
            | "files"
            | "files:canvases"
            | "files:lists"
            | "threads"
            | "huddles"
            | "directory"
            | "tools"
            | "cleanup"
            | "self-profile"
            | "compose"
            | "details"
            | "wsmenu"
            | "notify"
            | "typing"
            | "message"
    ) {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| match demo.as_str() {
                    "prefs" => app.open_preferences(Some(Section::Appearance), window, cx),
                    "prefs:nav" => app.open_preferences(Some(Section::Navigation), window, cx),
                    "huddle:popout" => {
                        let ch = app.channel.clone();
                        app.join_huddle_in(ch, window, cx);
                        app.pop_out_huddle(window, cx);
                    }
                    "profile" => {
                        if let Some(peer) = app.fixtures.dms.first().and_then(|c| c.user.clone()) {
                            app.open_profile(peer, cx);
                        }
                    }
                    "status" => app.open_status_picker(window, cx),
                    "switcher" => app.open_switcher(window, cx),
                    "shortcuts" => app.open_shortcuts(window, cx),
                    "drafts" => app.open_drafts(cx),
                    "side" => app.switch_workspace(1, window, cx),
                    "slash" => app
                        .composer
                        .update(cx, |composer, cx| composer.load_draft("/", cx)),
                    "later" => app.open_later(cx),
                    "pins" => app.toggle_pins(cx),
                    "activity" => app.show_nav(NavItem::Activity, cx),
                    "unreads" => app.open_unreads(window, cx),
                    "files" => app.show_nav(NavItem::Files, cx),
                    "files:canvases" | "files:lists" => {
                        app.show_nav(NavItem::Files, cx);
                        let section = if demo == "files:lists" {
                            FilesSection::Lists
                        } else {
                            FilesSection::Canvases
                        };
                        app.sidebar
                            .update(cx, |sidebar, cx| sidebar.show_files_section(section, cx));
                    }
                    "threads" => app.open_threads(cx),
                    "huddles" => app.open_huddles(cx),
                    "directory" => app.open_directory(cx),
                    "tools" => app.open_tools(cx),
                    "cleanup" => app.open_inactive_channels_dialog(window, cx),
                    "self-profile" => {
                        let me = app.fixtures.self_user.clone();
                        app.open_profile(me, cx);
                    }
                    "compose" => app.open_compose("", window, cx),
                    "details" => {
                        // The Slack Connect channel: the dialog and banner together.
                        app.select("C0PARTNER".into(), cx);
                        app.open_channel_details(
                            &"C0PARTNER".into(),
                            channel_details::DetailsTab::About,
                            window,
                            cx,
                        );
                    }
                    "wsmenu" => app
                        .sidebar
                        .update(cx, |sidebar, cx| sidebar.show_settings_menu(cx)),
                    "notify" => app.post_notification(
                        "demo",
                        "slackless",
                        "Notifications are working.",
                        window,
                        cx,
                    ),
                    "typing" => {
                        // Two DM partners typing in the open conversation, re-announced as
                        // Slack's clients do while someone keeps typing.
                        let typists: Vec<UserId> = (app.fixtures.dms.iter())
                            .filter_map(|dm| dm.user.clone())
                            .take(2)
                            .collect();
                        cx.spawn(async move |this, cx| {
                            loop {
                                let typing = this.update(cx, |app, cx| {
                                    let channel = app.channel.clone();
                                    for user in &typists {
                                        app.someone_typing(&channel, user, None, cx);
                                    }
                                });
                                if typing.is_err() {
                                    break;
                                }
                                cx.background_executor().timer(Duration::from_secs(3)).await;
                            }
                        })
                        .detach();
                    }
                    "message" => {
                        // A direct message arriving live: Slack's notification rules, banner
                        // and sound.
                        let dm = (app.fixtures.dms.iter())
                            .find(|dm| {
                                dm.id != app.channel
                                    && dm.user.as_ref() != Some(&app.fixtures.self_user)
                            })
                            .cloned();
                        if let Some(dm) = dm {
                            let message = Message {
                                ts: Ts::from("9999999999.000100"),
                                channel: Some(dm.id.clone()),
                                user: dm.user.clone(),
                                text: "This is a demo direct message.".to_owned(),
                                ..Default::default()
                            };
                            app.message_posted(&dm.id, &message, window, cx);
                        }
                    }
                    _ => {
                        app.user_menu_open = true;
                        cx.notify();
                    }
                });
            }
        });
        return;
    }
    let surface = match demo.as_str() {
        "canvas" => Some(ConversationTab::Canvas),
        "list" => Some(ConversationTab::List),
        _ => None,
    };
    if let Some(tab) = surface {
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    if let Some(channel) = app.huddle.as_ref().map(|(channel, _)| channel.clone()) {
                        app.select(channel, cx);
                        app.huddle_open = true;
                    }
                    app.tab = tab;
                    cx.notify();
                });
            }
        });
    } else if demo == "browser" {
        let this = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| app.open_browser(window, cx));
            }
        });
    } else if let Some(query) = demo.strip_prefix("search:") {
        let query = query.to_owned();
        results.update(cx, |results, cx| results.run(query, cx));
        let this = cx.weak_entity();
        window.defer(cx, move |_, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |app, cx| {
                    app.searching = true;
                    cx.notify();
                });
            }
        });
    }
}

/// Drop a failed lazy tab load so its tab shows loading again while it is re-requested.
pub(super) fn forget_failure<T>(loaded: &mut HashMap<ChannelId, Loaded<T>>, channel: &ChannelId) {
    if loaded.get(channel).is_some_and(Result::is_err) {
        loaded.remove(channel);
    }
}

/// Convert common typed emoticons outside inline/fenced code spans into Slack `:shortcode:`
/// emoji when Preferences > Appearance > Convert emoticons is enabled (`F-SHELL-091`).
pub(super) fn convert_emoticons(text: &str) -> String {
    const MAP: [(&str, &str); 12] = [
        (":-)", ":slightly_smiling_face:"),
        (":)", ":slightly_smiling_face:"),
        (":-D", ":smiley:"),
        (":D", ":smiley:"),
        (":-(", ":disappointed:"),
        (":(", ":disappointed:"),
        (";-)", ":wink:"),
        (";)", ":wink:"),
        (":-P", ":stuck_out_tongue:"),
        (":P", ":stuck_out_tongue:"),
        ("<3", ":heart:"),
        (":/", ":confused:"),
    ];
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`') {
            in_code = !in_code;
            out.push('`');
            rest = after;
            continue;
        }
        if !in_code {
            let at_boundary = out
                .chars()
                .last()
                .is_none_or(|c| c.is_whitespace() || c == '(');
            if at_boundary {
                if let Some((pat, rep)) = MAP.iter().find(|(pat, _)| {
                    rest.strip_prefix(*pat).is_some_and(|after| {
                        after
                            .chars()
                            .next()
                            .is_none_or(|c| c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?'))
                    })
                }) {
                    out.push_str(rep);
                    rest = &rest[pat.len()..];
                    continue;
                }
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Howard Hinnant's `civil_from_days`: convert days since 1970-01-01 into `(year, month, day)`.
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    (year as i32, m as u32, d as u32)
}

/// Default `(YYYY-MM-DD, HH:MM)` local strings ~1 hour from now for custom schedule/reminder dialogs.
pub(crate) fn default_custom_datetime() -> (String, String) {
    let local = now_secs() as i64 + crate::messages::local_offset_now() + 3_600;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        format!("{y:04}-{m:02}-{d:02}"),
        format!("{:02}:{:02}", secs / 3_600, (secs % 3_600) / 60),
    )
}

/// Parse a local `YYYY-MM-DD` date and `HH:MM` (24-hour) time into a future unix timestamp.
pub(crate) fn parse_local_schedule(date_str: &str, time_str: &str) -> Option<u64> {
    let date = slackless_core::store::Date::parse(date_str)?;
    let (h_str, m_str) = time_str.split_once(':')?;
    let hour: i64 = h_str.trim().parse().ok()?;
    let min: i64 = m_str.trim().parse().ok()?;
    if !(0..24).contains(&hour) || !(0..60).contains(&min) {
        return None;
    }
    let offset = crate::messages::local_offset_now();
    let utc = date.unix_start() + hour * 3_600 + min * 60 - offset;
    let now = now_secs() as i64;
    (utc > now + 30).then_some(utc as u64)
}

/// Heuristic for external link warning (`Preferences -> Advanced -> Warn me about potentially
/// malicious links`): flags non-HTTPS links, credentialed URLs (`user@host`), punycode IDN
/// hosts (`xn--`), or raw IPv4 hosts.
pub(super) fn is_suspicious_url(raw: &str) -> bool {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") {
        return true;
    }
    let Some(rest) = lower.strip_prefix("https://") else {
        return !lower.starts_with("mailto:");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return true;
    }
    let host = authority.split(':').next().unwrap_or("");
    if host.contains("xn--") {
        return true;
    }
    !host.is_empty() && host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Enable or disable Launch at login on macOS via a standard user LaunchAgent plist
/// (`~/Library/LaunchAgents/dev.slackless.gui.plist`). No-op on other OSes.
pub(super) fn sync_launch_at_login(enabled: bool) {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let dir = std::path::PathBuf::from(home).join("Library/LaunchAgents");
        let plist = dir.join("dev.slackless.gui.plist");
        if !enabled {
            let _ = std::fs::remove_file(plist);
            return;
        }
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let exe_str = exe.to_string_lossy();
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
\t<key>Label</key>\n\
\t<string>dev.slackless.gui</string>\n\
\t<key>ProgramArguments</key>\n\
\t<array>\n\
\t\t<string>{exe_str}</string>\n\
\t</array>\n\
\t<key>RunAtLoad</key>\n\
\t<true/>\n\
</dict>\n\
</plist>\n"
        );
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(plist, xml);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enabled;
    }
}

/// Whether `text` (Slack's mrkdwn) is for `me`: it mentions them (`<@U…>`), broadcasts
/// (`@here`, `@channel`, `@everyone`), or holds one of the comma-separated notification
/// `keywords` (case-insensitive).
pub(super) fn mentions_me(text: &str, me: &UserId, keywords: &str) -> bool {
    let mention = format!("<@{me}");
    let tagged = text
        .match_indices(&mention)
        .any(|(at, _)| matches!(text[at + mention.len()..].chars().next(), Some('>' | '|')));
    if tagged
        || ["<!here", "<!channel", "<!everyone"]
            .iter()
            .any(|b| text.contains(b))
    {
        return true;
    }
    let lower = text.to_lowercase();
    (keywords.split(',').map(str::trim))
        .filter(|keyword| !keyword.is_empty())
        .any(|keyword| lower.contains(&keyword.to_lowercase()))
}

#[cfg(test)]
mod notification_tag_tests {
    use super::{
        civil_from_days, convert_emoticons, is_suspicious_url, parse_local_schedule,
        parse_notification_target,
    };
    use slackless_core::types::{ChannelId, Ts};

    #[test]
    fn parses_foreground_and_background_workspace_notification_tags() {
        assert_eq!(
            parse_notification_target("bg:2:C0ABC:1700000001.000100"),
            Some((
                Some(2),
                ChannelId::from("C0ABC"),
                Ts::from("1700000001.000100")
            ))
        );
        assert_eq!(
            parse_notification_target("msg:D0XYZ:1700000002.000200"),
            Some((
                None,
                ChannelId::from("D0XYZ"),
                Ts::from("1700000002.000200")
            ))
        );
        assert_eq!(
            parse_notification_target("later:C0ENG:1700000003.000300"),
            Some((
                None,
                ChannelId::from("C0ENG"),
                Ts::from("1700000003.000300")
            ))
        );
        assert_eq!(parse_notification_target("bg:bad:C0:1.0"), None);
    }

    #[test]
    fn is_suspicious_url_flags_insecure_and_deceptive_urls() {
        // Non-HTTPS & dangerous schemes
        assert!(is_suspicious_url("http://example.com"));
        assert!(is_suspicious_url("HTTP://EXAMPLE.COM/path"));
        assert!(is_suspicious_url("javascript:alert(1)"));
        assert!(is_suspicious_url("file:///etc/passwd"));
        assert!(is_suspicious_url("ftp://files.example.com/a"));
        assert!(is_suspicious_url("data:text/html,<h1>hi</h1>"));

        // Credentialed authority (phishing user@host)
        assert!(is_suspicious_url("https://slack.com@evil.example/login"));
        assert!(is_suspicious_url("https://user:pass@example.com"));

        // Punycode IDN homographs
        assert!(is_suspicious_url("https://xn--80ak6aa92e.com/signin"));
        assert!(is_suspicious_url("https://sub.XN--p1ai/path"));

        // Raw IPv4 hosts (with or without port/path)
        assert!(is_suspicious_url("https://127.0.0.1"));
        assert!(is_suspicious_url("https://192.168.1.10:8443/admin"));
        assert!(is_suspicious_url("https://10.0.0.1/path?x=1"));

        // Clean HTTPS domains, ports, queries with `@`, and mailto:
        assert!(!is_suspicious_url("https://example.com"));
        assert!(!is_suspicious_url(
            "  https://docs.slack.com:443/reference?email=a@b.com  "
        ));
        assert!(!is_suspicious_url(
            "https://sub.domain.co.uk/path#user@frag"
        ));
        assert!(!is_suspicious_url("mailto:alice@example.com"));
        assert!(!is_suspicious_url("MAILTO:bob@example.com"));
    }

    #[test]
    fn convert_emoticons_handles_all_twelve_boundaries_and_code_spans() {
        let cases = [
            (":-)", ":slightly_smiling_face:"),
            (":)", ":slightly_smiling_face:"),
            (":-D", ":smiley:"),
            (":D", ":smiley:"),
            (":-(", ":disappointed:"),
            (":(", ":disappointed:"),
            (";-)", ":wink:"),
            (";)", ":wink:"),
            (":-P", ":stuck_out_tongue:"),
            (":P", ":stuck_out_tongue:"),
            ("<3", ":heart:"),
            (":/", ":confused:"),
        ];
        for (raw, expected) in cases {
            assert_eq!(convert_emoticons(raw), expected, "bare {raw}");
            assert_eq!(
                convert_emoticons(&format!("hi {raw}!")),
                format!("hi {expected}!"),
                "punctuated {raw}"
            );
            assert_eq!(
                convert_emoticons(&format!("({raw}.")),
                format!("({expected}."),
                "after paren {raw}"
            );
        }

        // Boundary rules: mid-word or URLs must NOT convert.
        assert_eq!(convert_emoticons("a:)"), "a:)");
        assert_eq!(convert_emoticons(":)x"), ":)x");
        assert_eq!(
            convert_emoticons("https://example.com"),
            "https://example.com"
        );
        assert_eq!(convert_emoticons("foo<3bar"), "foo<3bar");
        assert_eq!(convert_emoticons("a:/b"), "a:/b");

        // Inline code and fenced code blocks are skipped, and conversion resumes afterward.
        assert_eq!(
            convert_emoticons("hi `:)` and :)"),
            "hi `:)` and :slightly_smiling_face:"
        );
        assert_eq!(
            convert_emoticons("before :)\n```\n:) <3 :D\n```\nafter ;)!"),
            "before :slightly_smiling_face:\n```\n:) <3 :D\n```\nafter :wink:!"
        );
    }

    #[test]
    fn civil_from_days_handles_epoch_leap_years_and_negative_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
        // 2100 is not a leap year (divisible by 100, not 400): 2099-12-31 -> +59 days = 2100-02-28, +60 = 2100-03-01.
        assert_eq!(civil_from_days(47_540), (2100, 2, 28));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
    }

    #[test]
    fn parse_local_schedule_validates_bounds_and_rejects_past_or_malformed() {
        // Far-future valid date & time succeeds.
        let future = parse_local_schedule("2099-06-15", "14:30");
        assert!(future.is_some());
        assert!(parse_local_schedule("2099-02-28", "00:00").is_some());
        assert!(parse_local_schedule("2096-02-29", "23:59").is_some());

        // Invalid times
        assert_eq!(parse_local_schedule("2099-06-15", "24:00"), None);
        assert_eq!(parse_local_schedule("2099-06-15", "12:60"), None);
        assert_eq!(parse_local_schedule("2099-06-15", "-1:00"), None);
        assert_eq!(parse_local_schedule("2099-06-15", "12:-5"), None);
        assert_eq!(parse_local_schedule("2099-06-15", "noon"), None);
        assert_eq!(parse_local_schedule("2099-06-15", "12"), None);

        // Invalid / malformed dates
        assert_eq!(parse_local_schedule("not-a-date", "12:00"), None);
        assert_eq!(parse_local_schedule("2099-13-01", "12:00"), None);
        assert_eq!(parse_local_schedule("2099-02-29", "12:00"), None);

        // Past timestamps (<= now + 30) are rejected.
        assert_eq!(parse_local_schedule("2020-01-01", "12:00"), None);
        let now = crate::drafts::now_secs() as i64 + crate::messages::local_offset_now();
        let (y, m, d) = civil_from_days(now.div_euclid(86_400));
        let secs = now.rem_euclid(86_400);
        let date = format!("{y:04}-{m:02}-{d:02}");
        let time = format!("{:02}:{:02}", secs / 3_600, (secs % 3_600) / 60);
        assert_eq!(
            parse_local_schedule(&date, &time),
            None,
            "current minute is <= now + 30 and must be rejected"
        );
    }
}
