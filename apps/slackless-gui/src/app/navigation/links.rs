//! Message links, deep links, external URL allowlist, focus mode, and calendar sync.

use super::super::*;

impl App {
    /// A link in message text was activated. Slack resolves its own targets in-app:
    /// profiles, channels and threads open their panes, and Slack permalinks open the
    /// conversation (or thread) instead of the browser; everything else is a web URL.
    pub(in crate::app) fn open_link(
        &mut self,
        target: LinkTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            LinkTarget::Profile(user) => self.open_profile(user, cx),
            LinkTarget::Channel(channel) if self.fixtures.conversation(&channel).is_some() => {
                self.select(channel, cx);
            }
            LinkTarget::Thread { channel, thread_ts }
                if self.fixtures.conversation(&channel).is_some() =>
            {
                self.open_thread(channel, thread_ts, cx);
            }
            LinkTarget::Url(url) => {
                if self.handle_deep_link(url.as_ref(), window, cx) {
                    return;
                }
                if is_workflow_trigger_url(url.as_ref()) {
                    if self.fixtures.sample {
                        notify(
                            "Workflow trigger",
                            format!("Triggered {}", url.as_ref()),
                            window,
                            cx,
                        );
                    } else {
                        self.bridge.send(CollabCommand::TripWorkflowTrigger {
                            url: url.to_string(),
                            context: crate::blockkit::serde_json::json!({
                                "location": "link",
                                "channel_id": self.channel.as_str(),
                            }),
                        });
                    }
                    return;
                }
                match parse_permalink(&url) {
                    Some(permalink) if self.fixtures.conversation(&permalink.channel).is_some() => {
                        self.select(permalink.channel.clone(), cx);
                        match permalink.thread_ts {
                            // A reply's link opens its thread scrolled to the reply, as in Slack.
                            Some(thread_ts) => {
                                self.open_thread(permalink.channel.clone(), thread_ts, cx);
                                self.thread
                                    .update(cx, |panel, cx| panel.reveal(permalink.ts, cx));
                            }
                            None => {
                                self.messages
                                    .update(cx, |list, cx| list.reveal(&permalink.ts, cx));
                            }
                        }
                    }
                    _ => self.open_external_url(url.as_ref(), window, cx),
                }
            }
            // An in-app target this workspace does not know: nothing to open.
            LinkTarget::Channel(_) | LinkTarget::Thread { .. } => {}
        }
    }

    /// Open an external browser URL, prompting first when the URL scheme is outside
    /// `Settings.protocol_allowlist` (`F-SHELL-193`) or when `Settings.warn_external` is enabled
    /// and the URL looks suspicious (non-HTTPS, punycode `xn--`, credential `@`, or IP host).
    pub(in crate::app) fn open_external_url(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = self.preferences.read(cx).settings();
        let browser_choice = settings.open_links;
        let blocked_scheme = !is_protocol_allowed(url, &settings.protocol_allowlist);
        let warn = blocked_scheme || (settings.warn_external && is_suspicious_url(url));
        if !warn {
            launch_external_url(url, browser_choice, cx);
            return;
        }
        let target = url.to_owned();
        let prompt_msg = if blocked_scheme {
            "This link uses a URL protocol outside your allowed protocol list (Preferences \u{2192} Advanced). Verify the destination before opening it:"
        } else {
            "This link uses an unencrypted or unusual address. Verify the destination before opening it:"
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let open_target = target.clone();
            dialog.title("Open external link?").w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .child(div().text_sm().text_color(muted).child(prompt_msg))
                    .child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(cx.theme().secondary)
                            .text_xs()
                            .child(target.clone()),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                preferences::form_button(
                                    Button::new("ext-url-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("ext-url-open").primary().label("Open link"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    launch_external_url(&open_target, browser_choice, cx);
                                }),
                            ),
                    ),
            )
        });
    }

    /// Route a `slack://`, `https://app.slack.com/client/...`, or `https://*.slack.com/archives/...`
    /// deep link into the matching workspace, conversation, thread, message, file, or user profile
    /// (`F-SHELL-182..187`). Returns `true` when the URL was handled in-app (including when a clear
    /// "Workspace not signed in" toast was surfaced for an unrecognized target workspace).
    pub fn handle_deep_link(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use slackless_platform::{DeepLink, SlackLinkKind, parse_deep_link};
        let Ok(link) = parse_deep_link(url) else {
            return false;
        };
        match link {
            DeepLink::Slack(slack) => match slack.kind {
                SlackLinkKind::Team { team_id } => {
                    self.resolve_deep_link_workspace(Some(&team_id), None, None, window, cx);
                    true
                }
                SlackLinkKind::Channel {
                    channel_id,
                    team_id,
                    message_ts,
                    thread_ts,
                } => {
                    let ch = ChannelId::from(channel_id.as_str());
                    if !self.resolve_deep_link_workspace(
                        team_id.as_deref(),
                        None,
                        Some(&ch),
                        window,
                        cx,
                    ) {
                        return true;
                    }
                    self.navigate_channel_target(
                        ch,
                        thread_ts.map(Ts::new),
                        message_ts.map(Ts::new),
                        cx,
                    )
                }
                SlackLinkKind::User { user_id, team_id } => {
                    if !self.resolve_deep_link_workspace(team_id.as_deref(), None, None, window, cx)
                    {
                        return true;
                    }
                    let uid = UserId::from(user_id.as_str());
                    self.open_profile(uid, cx);
                    true
                }
                SlackLinkKind::File { file_id, team_id } => {
                    if !self.resolve_deep_link_workspace(team_id.as_deref(), None, None, window, cx)
                    {
                        return true;
                    }
                    self.navigate_file_target(&file_id, window, cx);
                    true
                }
                SlackLinkKind::Open { team_id, target_id } => {
                    let ch_hint = target_id
                        .as_deref()
                        .filter(|t| {
                            !t.starts_with('U') && !t.starts_with('W') && !t.starts_with('F')
                        })
                        .map(ChannelId::from);
                    if !self.resolve_deep_link_workspace(
                        team_id.as_deref(),
                        None,
                        ch_hint.as_ref(),
                        window,
                        cx,
                    ) {
                        return true;
                    }
                    if let Some(target) = target_id {
                        if target.starts_with('U') || target.starts_with('W') {
                            self.open_profile(UserId::from(target.as_str()), cx);
                            return true;
                        }
                        if target.starts_with('F') {
                            self.navigate_file_target(&target, window, cx);
                            return true;
                        }
                        let ch = ChannelId::from(target.as_str());
                        let extra_thread = slack
                            .extra
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("thread_ts"))
                            .map(|(_, v)| Ts::new(v.clone()));
                        let extra_msg = slack
                            .extra
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("message"))
                            .map(|(_, v)| Ts::new(v.clone()));
                        return self.navigate_channel_target(ch, extra_thread, extra_msg, cx);
                    }
                    team_id.is_some()
                }
            },
            DeepLink::WebClient(web) => {
                let ch_opt = web.channel_id.as_deref().map(ChannelId::from);
                if !self.resolve_deep_link_workspace(
                    web.team_id.as_deref(),
                    web.workspace_domain.as_deref(),
                    ch_opt.as_ref(),
                    window,
                    cx,
                ) {
                    return true;
                }
                let Some(ch) = ch_opt else {
                    return web.team_id.is_some() || web.workspace_domain.is_some();
                };
                self.navigate_channel_target(
                    ch,
                    web.thread_ts.map(Ts::new),
                    web.message_ts.map(Ts::new),
                    cx,
                )
            }
            DeepLink::Huddle {
                team_id,
                channel_id,
                ..
            } => {
                let ch = ChannelId::from(channel_id.as_str());
                if !self.resolve_deep_link_workspace(
                    team_id.as_deref(),
                    None,
                    Some(&ch),
                    window,
                    cx,
                ) {
                    return true;
                }
                if self.fixtures.conversation(&ch).is_none() {
                    return false;
                }
                self.select(ch.clone(), cx);
                self.join_huddle_in(ch, window, cx);
                true
            }
            DeepLink::AppHome {
                team_id,
                app_id,
                tab,
                ..
            } => {
                if !self.resolve_deep_link_workspace(team_id.as_deref(), None, None, window, cx) {
                    return true;
                }
                let target_tab = if tab.as_deref() == Some("messages") {
                    ConversationTab::Messages
                } else {
                    ConversationTab::Home
                };
                if let Some(dm) = self.fixtures.dms.iter().find(|c| {
                    c.user
                        .as_ref()
                        .is_some_and(|u| u.as_str() == app_id.as_str())
                        || c.id.as_str() == app_id.as_str()
                }) {
                    let ch = dm.id.clone();
                    self.select(ch, cx);
                    self.select_tab(target_tab, cx);
                    return true;
                }
                if !self.fixtures.sample
                    && let Some(resolved_team) = team_id.or_else(|| {
                        self.fixtures
                            .workspaces
                            .get(self.workspace)
                            .map(|t| t.id.to_string())
                    })
                {
                    self.bridge.send(CollabCommand::OpenAppHome {
                        app_id,
                        team_id: resolved_team,
                    });
                }
                true
            }
            DeepLink::Other { .. } => false,
        }
    }

    /// Resolve the target workspace for a deep link by `team_id` or `<domain>.slack.com` subdomain.
    /// Switches to the matching workspace when signed in; returns `false` and surfaces a clear
    /// toast when the target workspace is not signed in.
    fn resolve_deep_link_workspace(
        &mut self,
        team_id: Option<&str>,
        workspace_domain: Option<&str>,
        fallback_channel: Option<&ChannelId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(tid) = team_id {
            let idx = self
                .live_teams
                .iter()
                .position(|t| t.id.as_str().eq_ignore_ascii_case(tid))
                .or_else(|| {
                    self.fixtures
                        .workspaces
                        .iter()
                        .position(|t| t.id.as_str().eq_ignore_ascii_case(tid))
                });
            if let Some(ix) = idx {
                if ix != self.workspace {
                    self.switch_workspace(ix, window, cx);
                }
                return true;
            }
            notify(
                "Workspace not signed in",
                format!("Sign in to workspace {tid} to open this Slack link."),
                window,
                cx,
            );
            return false;
        }
        if let Some(domain) = workspace_domain {
            let idx =
                self.live_teams
                    .iter()
                    .position(|t| !t.domain.is_empty() && t.domain.eq_ignore_ascii_case(domain))
                    .or_else(|| {
                        self.fixtures.workspaces.iter().position(|t| {
                            !t.domain.is_empty() && t.domain.eq_ignore_ascii_case(domain)
                        })
                    });
            if let Some(ix) = idx {
                if ix != self.workspace {
                    self.switch_workspace(ix, window, cx);
                }
                return true;
            }
            // If no workspace domain matched, still allow permalinks whose channel exists in the
            // active workspace (e.g. fixture/offline mode or workspaces with unpopulated domains).
            if fallback_channel.is_some_and(|ch| self.fixtures.conversation(ch).is_some()) {
                return true;
            }
            notify(
                "Workspace not signed in",
                format!("Sign in to {domain}.slack.com to open this Slack link."),
                window,
                cx,
            );
            return false;
        }
        true
    }

    /// Select `channel` and jump to an exact thread reply, thread root, or message timestamp.
    fn navigate_channel_target(
        &mut self,
        channel: ChannelId,
        thread_ts: Option<Ts>,
        message_ts: Option<Ts>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.fixtures.conversation(&channel).is_none() {
            return false;
        }
        self.select(channel.clone(), cx);
        match (thread_ts, message_ts) {
            (Some(t_ts), Some(m_ts)) => {
                self.open_thread(channel, t_ts, cx);
                self.thread.update(cx, |panel, cx| panel.reveal(m_ts, cx));
            }
            (Some(t_ts), None) => {
                self.open_thread(channel, t_ts.clone(), cx);
                self.thread.update(cx, |panel, cx| panel.reveal(t_ts, cx));
            }
            (None, Some(m_ts)) => {
                self.messages.update(cx, |list, cx| list.reveal(&m_ts, cx));
            }
            (None, None) => {}
        }
        true
    }

    /// Jump to the message carrying `file_id` in the active workspace, or open the Files page.
    fn navigate_file_target(&mut self, file_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut channel_hit: Option<(ChannelId, Ts)> = None;
        for (ch, msgs) in &self.fixtures.messages {
            if let Some(msg) = msgs
                .iter()
                .find(|m| m.files.iter().any(|f| f.id.as_str() == file_id))
            {
                channel_hit = Some((ch.clone(), msg.ts.clone()));
                break;
            }
        }
        if let Some((ch, ts)) = channel_hit {
            let _ = self.navigate_channel_target(ch, None, Some(ts), cx);
            return;
        }
        let mut thread_hit: Option<(ChannelId, Ts, Ts)> = None;
        for ((ch, root_ts), replies) in &self.fixtures.threads {
            if let Some(msg) = replies
                .iter()
                .find(|m| m.files.iter().any(|f| f.id.as_str() == file_id))
            {
                thread_hit = Some((ch.clone(), root_ts.clone(), msg.ts.clone()));
                break;
            }
        }
        if let Some((ch, root_ts, msg_ts)) = thread_hit {
            let _ = self.navigate_channel_target(ch, Some(root_ts), Some(msg_ts), cx);
            return;
        }
        self.open_files(cx);
        notify(
            "Opened Files",
            format!("Looking up file {file_id} in the active workspace."),
            window,
            cx,
        );
    }

    /// Slack's focus mode (the rail's moon): on with Slack's default "Don't turn off", or off.
    /// Syncs DND snooze & automatic focus status with Slack in live mode (`F-SHELL-036`).
    pub(in crate::app) fn toggle_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let auto_status = self.preferences.read(cx).settings().focus_auto_status;
        if self.self_state.focused() {
            self.self_state.end_focus();
            if !self.fixtures.sample {
                self.bridge.send(WorkspaceCommand::EndDndSnooze);
                self.send_status_to_slack(self.self_state.status.as_ref());
            }
        } else {
            self.self_state
                .start_focus_with_status(FOCUS_UNTIL_OFF, auto_status);
            if !self.fixtures.sample {
                // Default 8-hour DND snooze for indefinite focus mode.
                self.bridge
                    .send(WorkspaceCommand::SetDndSnooze { minutes: 480 });
                if auto_status {
                    self.send_status_to_slack(self.self_state.status.as_ref());
                }
            }
        }
        self.apply_preferences(window, cx);
    }

    /// End a timed focus mode or expired custom status once its Duration ran out, and activate
    /// any scheduled statuses whose start time has arrived (checked on the reminder poll).
    pub(in crate::app) fn expire_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.self_state.focus_until.is_some() && !self.self_state.focused() {
            self.self_state.end_focus();
            self.apply_preferences(window, cx);
        }
        if self.self_state.expire_status() {
            self.push_self_state(cx);
        }
        if let Some(activated) = self.self_state.activate_due_scheduled() {
            if !self.fixtures.sample {
                self.send_status_to_slack(Some(&activated));
            }
            self.push_self_state(cx);
            notify(
                "Scheduled status active",
                format!("{} {}", activated.glyph(), activated.text),
                window,
                cx,
            );
        }
    }

    /// Opt-in native macOS Calendar busy-event status sync (`F-SHELL-024`): probes Calendar.app
    /// once per minute off the UI thread when `Settings.calendar_status_sync` is enabled, setting
    /// `:spiral_calendar_pad:` `"In a meeting"` while a non-all-day event is active and clearing
    /// it once the meeting ends (never overwriting a manual user status or active Focus mode).
    pub(in crate::app) fn poll_calendar_status(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = self.preferences.read(cx).settings().calendar_status_sync;
        if !enabled {
            if self.calendar_status_active {
                self.calendar_status_active = false;
                if self
                    .self_state
                    .status
                    .as_ref()
                    .is_some_and(|s| s.text == "In a meeting" && s.emoji == "spiral_calendar_pad")
                {
                    self.self_state.set_status(None);
                    if !self.fixtures.sample {
                        self.send_status_to_slack(None);
                    }
                    self.push_self_state(cx);
                }
            }
            return;
        }
        let tick = self.calendar_sync_ticks;
        self.calendar_sync_ticks = self.calendar_sync_ticks.wrapping_add(1);
        // Probe immediately on first enabled tick, then once every 60s (6 × 10s).
        if !tick.is_multiple_of(6) || self.self_state.focused() {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            cx.spawn_in(window, async move |this, cx| {
                let busy = cx
                    .background_executor()
                    .spawn(async move { probe_macos_calendar_busy() })
                    .await;
                let Some(is_busy) = busy else {
                    return;
                };
                let _ = this.update(cx, |app, cx| {
                    if app.self_state.focused()
                        || !app.preferences.read(cx).settings().calendar_status_sync
                    {
                        return;
                    }
                    if is_busy {
                        if app.self_state.status.is_none() {
                            let status = crate::status::UserStatus {
                                emoji: "spiral_calendar_pad".to_owned(),
                                text: "In a meeting".to_owned(),
                                clear_after: 0,
                                pause: false,
                                expiration: None,
                                source: crate::status::StatusSource::Calendar,
                            };
                            app.self_state.set_status(Some(status.clone()));
                            app.calendar_status_active = true;
                            if !app.fixtures.sample {
                                app.send_status_to_slack(Some(&status));
                            }
                            app.push_self_state(cx);
                        }
                    } else if app.calendar_status_active {
                        app.calendar_status_active = false;
                        if app.self_state.status.as_ref().is_some_and(|s| {
                            s.text == "In a meeting" && s.emoji == "spiral_calendar_pad"
                        }) {
                            app.self_state.set_status(None);
                            if !app.fixtures.sample {
                                app.send_status_to_slack(None);
                            }
                            app.push_self_state(cx);
                        }
                    }
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (window, cx);
    }
}

/// Open `url` in the browser selected in Preferences -> Advanced (`0 = Default`, `1 = Safari`,
/// `2 = Firefox`). Falls back to the system default handler on non-macOS or launch errors.
fn launch_external_url(url: &str, browser_choice: usize, cx: &mut gpui::App) {
    #[cfg(target_os = "macos")]
    {
        let app_name = match browser_choice {
            1 => Some("Safari"),
            2 => Some("Firefox"),
            _ => None,
        };
        if let Some(app_name) = app_name
            && let Ok(mut child) = std::process::Command::new("open")
                .args(["-a", app_name, "--", url])
                .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = browser_choice;
    cx.open_url(url);
}

/// Whether `url`'s scheme is permitted by the comma-separated `allowlist` (`F-SHELL-193`).
pub(in crate::app) fn is_protocol_allowed(url: &str, allowlist: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.trim().to_ascii_lowercase();
    if scheme.is_empty() {
        return false;
    }
    allowlist
        .split([',', ' ', ';'])
        .map(|s| s.trim().trim_end_matches(':').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .any(|allowed| allowed == scheme)
}

/// Whether `url` is a Slack Workflow Builder link trigger (`https://slack.com/shortcuts/Ft...`,
/// `F-COLLAB-319`, `F-COLLAB-376`).
pub(in crate::app) fn is_workflow_trigger_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let Some((host, path)) = rest.split_once('/') else {
        return false;
    };
    (host == "slack.com" || host.ends_with(".slack.com")) && path.starts_with("shortcuts/Ft")
}

/// Read-only macOS Calendar.app probe: returns `Some(true)` if a non-all-day event is active
/// right now, `Some(false)` if free, or `None` if Calendar is unavailable or permission denied.
#[cfg(target_os = "macos")]
fn probe_macos_calendar_busy() -> Option<bool> {
    const SCRIPT: &str = r#"
tell application "System Events"
    if not (exists process "Calendar") then return "FREE"
end tell
tell application "Calendar"
    set nowDate to current date
    repeat with c in calendars
        try
            set evs to (every event of c whose start date <= nowDate and end date >= nowDate)
            repeat with e in evs
                if (allday event of e) is false then
                    return "BUSY"
                end if
            end repeat
        end try
    end repeat
end tell
return "FREE"
"#;
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(SCRIPT)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.trim() {
        "BUSY" => Some(true),
        "FREE" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{is_protocol_allowed, is_workflow_trigger_url};

    #[test]
    fn is_protocol_allowed_parses_separators_colons_case_and_rejects_invalid() {
        let list = "https:, http; mailto  slack: , zoommtg";
        assert!(is_protocol_allowed("https://example.com", list));
        assert!(is_protocol_allowed("HTTPS://EXAMPLE.COM/path", list));
        assert!(is_protocol_allowed("http://example.com", list));
        assert!(is_protocol_allowed("mailto:alice@example.com", list));
        assert!(is_protocol_allowed("slack://channel?id=C1", list));
        assert!(is_protocol_allowed("zoommtg://zoom.us/join", list));

        // Disallowed schemes
        assert!(!is_protocol_allowed("javascript:alert(1)", list));
        assert!(!is_protocol_allowed("file:///etc/passwd", list));
        assert!(!is_protocol_allowed("ftp://example.com", list));

        // Missing colon, empty scheme, empty allowlist
        assert!(!is_protocol_allowed("example.com/no-scheme", list));
        assert!(!is_protocol_allowed(":no-scheme", list));
        assert!(!is_protocol_allowed("   :leading-space", list));
        assert!(!is_protocol_allowed("https://example.com", ""));
        assert!(!is_protocol_allowed("https://example.com", " , ; "));
    }

    #[test]
    fn is_workflow_trigger_url_matches_slack_shortcuts_ft_only() {
        assert!(is_workflow_trigger_url(
            "https://slack.com/shortcuts/Ft0123ABC"
        ));
        assert!(is_workflow_trigger_url(
            "https://acme.slack.com/shortcuts/Ft999/run"
        ));

        // Reject insecure scheme, lookalike host, wrong path prefix, or non-Ft ID.
        assert!(!is_workflow_trigger_url(
            "http://slack.com/shortcuts/Ft0123ABC"
        ));
        assert!(!is_workflow_trigger_url(
            "https://evilslack.com/shortcuts/Ft0123ABC"
        ));
        assert!(!is_workflow_trigger_url(
            "https://slack.com/shortcuts/Ab0123"
        ));
        assert!(!is_workflow_trigger_url(
            "https://acme.slack.com/archives/C0ENG/p1234567890123456"
        ));
        assert!(!is_workflow_trigger_url("https://slack.com"));
    }
}
