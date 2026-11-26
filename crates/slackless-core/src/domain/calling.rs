//! Third-party calling app discovery, slash-command invocation, and call-link recognition
//! (`F-COLLAB-078..094`).

use crate::api::AppCommand;

/// Whether the user asked to launch an audio-only or video third-party call (`F-COLLAB-084`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallMode {
    Video,
    Audio,
}

/// An installed (or sample) third-party calling app launchable from the headphones/call menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallingApp {
    /// Stable provider identifier (e.g. `"zoom"`, `"meet"`, `"teams"`).
    pub id: String,
    /// Human-readable app name shown in the call launcher dropdown.
    pub name: String,
    /// Slash command with leading `/` (e.g. `"/zoom"`, `"/meet"`, `"/teams-calls"`).
    pub command: String,
    /// Owning Slack app ID when known (`A...`).
    pub app_id: Option<String>,
    /// Whether the app accepts an audio-only launch mode distinct from default video.
    pub supports_audio_only: bool,
    /// Optional disconnect/logout command (e.g. `"/teams-calls logout"` for Microsoft Teams).
    pub logout_command: Option<String>,
}

struct KnownProvider {
    id: &'static str,
    name: &'static str,
    commands: &'static [&'static str],
    bot_tokens: &'static [&'static str],
    supports_audio_only: bool,
    logout_command: Option<&'static str>,
}

const KNOWN_PROVIDERS: &[KnownProvider] = &[
    KnownProvider {
        id: "zoom",
        name: "Zoom",
        commands: &["zoom", "zoom-call"],
        bot_tokens: &["zoom"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "meet",
        name: "Google Meet",
        commands: &["meet", "gmeet", "google-meet"],
        bot_tokens: &["google meet", "hangouts meet"],
        supports_audio_only: false,
        logout_command: None,
    },
    KnownProvider {
        id: "teams",
        name: "Microsoft Teams",
        commands: &["teams-calls", "msteams", "teams"],
        bot_tokens: &["microsoft teams", "teams calls"],
        supports_audio_only: true,
        logout_command: Some("/teams-calls logout"),
    },
    KnownProvider {
        id: "webex",
        name: "Cisco Webex",
        commands: &["webex", "webex-meetings"],
        bot_tokens: &["webex"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "jitsi",
        name: "Jitsi Meet",
        commands: &["jitsi"],
        bot_tokens: &["jitsi"],
        supports_audio_only: false,
        logout_command: None,
    },
    KnownProvider {
        id: "whereby",
        name: "Whereby",
        commands: &["whereby"],
        bot_tokens: &["whereby"],
        supports_audio_only: false,
        logout_command: None,
    },
    KnownProvider {
        id: "ringcentral",
        name: "RingCentral",
        commands: &["ringcentral", "rc"],
        bot_tokens: &["ringcentral"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "dialpad",
        name: "Dialpad",
        commands: &["dialpad"],
        bot_tokens: &["dialpad"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "bluejeans",
        name: "BlueJeans",
        commands: &["bluejeans"],
        bot_tokens: &["bluejeans"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "gotomeeting",
        name: "GoToMeeting",
        commands: &["gotomeeting", "goto"],
        bot_tokens: &["gotomeeting"],
        supports_audio_only: true,
        logout_command: None,
    },
    KnownProvider {
        id: "around",
        name: "Around",
        commands: &["around"],
        bot_tokens: &["around"],
        supports_audio_only: false,
        logout_command: None,
    },
];

/// Sample workspace calling apps used in fixture/demo mode when no live `client.appCommands`
/// catalog exists.
pub fn sample_calling_apps() -> Vec<CallingApp> {
    vec![
        CallingApp {
            id: "zoom".to_owned(),
            name: "Zoom".to_owned(),
            command: "/zoom".to_owned(),
            app_id: Some("A0ZOOMAPP".to_owned()),
            supports_audio_only: true,
            logout_command: None,
        },
        CallingApp {
            id: "meet".to_owned(),
            name: "Google Meet".to_owned(),
            command: "/meet".to_owned(),
            app_id: Some("A0GMEETAPP".to_owned()),
            supports_audio_only: false,
            logout_command: None,
        },
        CallingApp {
            id: "teams".to_owned(),
            name: "Microsoft Teams".to_owned(),
            command: "/teams-calls".to_owned(),
            app_id: Some("A0TEAMSAPP".to_owned()),
            supports_audio_only: true,
            logout_command: Some("/teams-calls logout".to_owned()),
        },
    ]
}

/// Discover installed third-party calling apps from the workspace's `client.appCommands` catalog
/// and installed bot names (`F-COLLAB-079`, `F-COLLAB-082`).
pub fn detect_calling_apps(app_commands: &[AppCommand], bot_names: &[&str]) -> Vec<CallingApp> {
    let mut apps = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    let mut seen_commands = std::collections::HashSet::new();

    // 1. Match known calling providers from installed slash commands first (most accurate).
    for cmd in app_commands {
        let name = cmd.name.trim().trim_start_matches('/').to_ascii_lowercase();
        if let Some(provider) = KNOWN_PROVIDERS
            .iter()
            .find(|p| p.commands.contains(&name.as_str()))
        {
            seen_commands.insert(name.clone());
            if seen_ids.insert(provider.id.to_owned()) {
                apps.push(CallingApp {
                    id: provider.id.to_owned(),
                    name: cmd
                        .app_name
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .unwrap_or(provider.name)
                        .to_owned(),
                    command: format!("/{name}"),
                    app_id: cmd.app_id.clone(),
                    supports_audio_only: provider.supports_audio_only,
                    logout_command: provider.logout_command.map(str::to_owned),
                });
            }
        }
    }

    // 2. Match any additional installed app command whose description clearly advertises calling.
    for cmd in app_commands {
        let name = cmd.name.trim().trim_start_matches('/').to_ascii_lowercase();
        if name.is_empty() || seen_ids.contains(&name) || seen_commands.contains(&name) {
            continue;
        }
        let desc = cmd.description.to_ascii_lowercase();
        let looks_like_call = desc.contains("start a call")
            || desc.contains("video call")
            || desc.contains("audio call")
            || desc.contains("start a meeting")
            || desc.contains("video meeting")
            || name == "call";
        if looks_like_call && seen_ids.insert(name.clone()) {
            seen_commands.insert(name.clone());
            apps.push(CallingApp {
                id: name.clone(),
                name: cmd
                    .app_name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&name)
                    .to_owned(),
                command: format!("/{name}"),
                app_id: cmd.app_id.clone(),
                supports_audio_only: desc.contains("audio"),
                logout_command: None,
            });
        }
    }

    // 3. Fall back to matching installed workspace bots when `appCommands` hasn't hydrated yet.
    for bot_name in bot_names {
        let lower = bot_name.to_ascii_lowercase();
        if let Some(provider) = KNOWN_PROVIDERS
            .iter()
            .find(|p| p.bot_tokens.iter().any(|tok| lower.contains(tok)))
            && seen_ids.insert(provider.id.to_owned())
        {
            apps.push(CallingApp {
                id: provider.id.to_owned(),
                name: provider.name.to_owned(),
                command: format!("/{}", provider.commands[0]),
                app_id: None,
                supports_audio_only: provider.supports_audio_only,
                logout_command: provider.logout_command.map(str::to_owned),
            });
        }
    }

    apps
}

/// Build the `(slash_command_without_slash, args)` pair for launching `app` in `mode` with an
/// optional meeting `topic` (`F-COLLAB-083..085`, `F-COLLAB-092`).
pub fn build_calling_command(app: &CallingApp, mode: CallMode, topic: &str) -> (String, String) {
    let command = app.command.trim().trim_start_matches('/').to_owned();
    let topic = topic.trim();
    let args = match (app.id.as_str(), mode, topic.is_empty()) {
        ("zoom", CallMode::Audio, true) => "audio".to_owned(),
        ("zoom", CallMode::Audio, false) => format!("audio {topic}"),
        ("teams", CallMode::Audio, true) => "audio".to_owned(),
        ("teams", CallMode::Audio, false) => format!("audio {topic}"),
        (_, CallMode::Audio, true) if app.supports_audio_only => "audio".to_owned(),
        (_, CallMode::Audio, false) if app.supports_audio_only => format!("audio {topic}"),
        (_, _, true) => String::new(),
        (_, _, false) => topic.to_owned(),
    };
    (command, args)
}

/// Recognize third-party meeting/call URLs for call cards and link unfurls (`F-COLLAB-086`,
/// `F-COLLAB-089`, `F-COLLAB-093`).
pub fn call_provider_for_url(url: &str) -> Option<&'static str> {
    let parsed = url::Url::parse(url).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = parsed.path().to_ascii_lowercase();

    if scheme == "zoommtg" || scheme == "zoomus" {
        return Some("Zoom");
    }
    if scheme == "msteams" {
        return Some("Microsoft Teams");
    }
    if scheme != "http" && scheme != "https" {
        return None;
    }

    if host == "zoom.us"
        || host.ends_with(".zoom.us")
        || host == "zoom.com"
        || host.ends_with(".zoom.com")
    {
        return Some("Zoom");
    }
    if host == "meet.google.com" {
        return Some("Google Meet");
    }
    if (host == "teams.microsoft.com" || host == "teams.live.com")
        && (path.contains("/meetup-join/") || path.contains("/meet/"))
    {
        return Some("Microsoft Teams");
    }
    if host == "webex.com" || host.ends_with(".webex.com") {
        return Some("Cisco Webex");
    }
    if host == "meet.jit.si" || host.ends_with(".jit.si") {
        return Some("Jitsi Meet");
    }
    if host == "whereby.com" || host.ends_with(".whereby.com") {
        return Some("Whereby");
    }
    if host == "meetings.ringcentral.com" || host == "v.ringcentral.com" {
        return Some("RingCentral");
    }
    if host == "bluejeans.com" || host.ends_with(".bluejeans.com") {
        return Some("BlueJeans");
    }
    if host == "meet.goto.com" || host == "gotomeet.me" || host.ends_with(".gotomeeting.com") {
        return Some("GoToMeeting");
    }
    if host == "meetings.dialpad.com" || host == "dialpad.com" {
        return Some("Dialpad");
    }
    if host == "around.co" || host.ends_with(".around.co") {
        return Some("Around");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, app_name: Option<&str>, desc: &str) -> AppCommand {
        AppCommand {
            name: name.to_owned(),
            usage: String::new(),
            description: desc.to_owned(),
            app_name: app_name.map(str::to_owned),
            app_id: Some(format!("A_{}", name.to_uppercase())),
        }
    }

    #[test]
    fn detects_calling_apps_from_commands_and_bots_without_duplicates() {
        let commands = vec![
            cmd("zoom", Some("Zoom"), "Start a Zoom meeting"),
            cmd(
                "teams-calls",
                Some("Microsoft Teams Calls"),
                "Start a video call in Microsoft Teams",
            ),
            cmd("gmeet", Some("Google Meet"), "Start a video meeting"),
            cmd("customcall", Some("Acme Call"), "Start a video call"),
            cmd("giphy", Some("Giphy"), "Post a random gif"),
        ];
        let detected = detect_calling_apps(&commands, &["Zoom", "Google Meet", "Linear"]);
        let ids: Vec<&str> = detected.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["zoom", "teams", "meet", "customcall"]);
        assert_eq!(
            detected[1].logout_command.as_deref(),
            Some("/teams-calls logout")
        );
    }

    #[test]
    fn builds_audio_and_video_calling_commands() {
        let apps = sample_calling_apps();
        let zoom = &apps[0];
        let meet = &apps[1];
        assert_eq!(
            build_calling_command(zoom, CallMode::Video, ""),
            ("zoom".to_owned(), String::new())
        );
        assert_eq!(
            build_calling_command(zoom, CallMode::Audio, "Incident"),
            ("zoom".to_owned(), "audio Incident".to_owned())
        );
        assert_eq!(
            build_calling_command(meet, CallMode::Audio, "Design review"),
            ("meet".to_owned(), "Design review".to_owned())
        );
    }

    #[test]
    fn recognizes_third_party_call_urls() {
        assert_eq!(
            call_provider_for_url("https://acme.zoom.us/j/123456789"),
            Some("Zoom")
        );
        assert_eq!(
            call_provider_for_url("zoommtg://zoom.us/join?confno=123"),
            Some("Zoom")
        );
        assert_eq!(
            call_provider_for_url("https://meet.google.com/abc-defg-hij"),
            Some("Google Meet")
        );
        assert_eq!(
            call_provider_for_url("https://teams.microsoft.com/l/meetup-join/19%3ameeting"),
            Some("Microsoft Teams")
        );
        assert_eq!(
            call_provider_for_url("https://acme.webex.com/meet/alice"),
            Some("Cisco Webex")
        );
        assert_eq!(
            call_provider_for_url("javascript://zoom.us/j/123456789"),
            None
        );
        assert_eq!(
            call_provider_for_url("file://meet.google.com/abc-defg-hij"),
            None
        );
        assert_eq!(call_provider_for_url("https://example.com/docs"), None);
    }
}
