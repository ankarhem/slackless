//! Built-in slash commands: autocomplete catalog ([`BUILTINS`]) and draft parser ([`parse`]).

/// One built-in slash command, as shown in the composer's autocomplete menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub kind: SlashKind,
    /// Name without the leading slash.
    pub name: &'static str,
    /// Argument hint shown after the name.
    pub usage: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlashKind {
    Remind,
    Status,
    Away,
    Dnd,
    Mute,
    Invite,
    Leave,
    Topic,
    Shrug,
    Collapse,
    Msg,
    Giphy,
}

pub const BUILTINS: [SlashCommand; 12] = [
    SlashCommand {
        kind: SlashKind::Away,
        name: "away",
        usage: "",
        description: "Toggle your availability to away",
    },
    SlashCommand {
        kind: SlashKind::Collapse,
        name: "collapse",
        usage: "",
        description: "Collapse all inline images and video in the current channel",
    },
    SlashCommand {
        kind: SlashKind::Dnd,
        name: "dnd",
        usage: "[some description of time]",
        description: "Pause notifications or set your do not disturb schedule",
    },
    SlashCommand {
        kind: SlashKind::Giphy,
        name: "giphy",
        usage: "[text]",
        description: "Post a random gif to channel",
    },
    SlashCommand {
        kind: SlashKind::Invite,
        name: "invite",
        usage: "@user [channel]",
        description: "Invite another member to a channel",
    },
    SlashCommand {
        kind: SlashKind::Leave,
        name: "leave",
        usage: "",
        description: "Leave a channel",
    },
    SlashCommand {
        kind: SlashKind::Msg,
        name: "msg",
        usage: "@user [your message]",
        description: "Send a direct message to another member",
    },
    SlashCommand {
        kind: SlashKind::Mute,
        name: "mute",
        usage: "",
        description: "Mute a channel, or unmute a channel that is muted",
    },
    SlashCommand {
        kind: SlashKind::Remind,
        name: "remind",
        usage: "[@someone or #channel] [what] [when]",
        description: "Set a reminder",
    },
    SlashCommand {
        kind: SlashKind::Shrug,
        name: "shrug",
        usage: "[your message]",
        description: "Appends \u{af}\\_(\u{30c4})_/\u{af} to your message",
    },
    SlashCommand {
        kind: SlashKind::Status,
        name: "status",
        usage: "[clear]",
        description: "Set or clear your custom status",
    },
    SlashCommand {
        kind: SlashKind::Topic,
        name: "topic",
        usage: "[text]",
        description: "Set the channel topic",
    },
];

/// The shrug suffix `/shrug` appends.
pub const SHRUG: &str = "\u{af}\\_(\u{30c4})_/\u{af}";

/// Format the outgoing message text for `/shrug <args>`.
pub fn format_shrug(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        SHRUG.to_owned()
    } else {
        format!("{trimmed} {SHRUG}")
    }
}

/// A parsed `/name args` draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The matched built-in; `None` for an unknown command.
    pub command: Option<&'static SlashCommand>,
    pub name: String,
    /// Everything after the name, trimmed.
    pub args: String,
    /// Explicitly chosen Slack app ID when multiple apps register the same `/name` (`F-COLLAB-686`).
    pub app_id: Option<String>,
    /// Display name of the chosen app for feedback/toasts (`F-COLLAB-686`).
    pub app_name: Option<String>,
    /// True when the user explicitly selected the built-in command over an app duplicate.
    pub force_builtin: bool,
}

impl Invocation {
    /// Attach an explicit app choice from autocomplete or the disambiguation picker.
    pub fn with_app(mut self, app_id: Option<String>, app_name: Option<String>) -> Self {
        self.app_id = app_id;
        self.app_name = app_name;
        self.force_builtin = false;
        self
    }

    /// Force dispatching as the built-in command when an app also registers the same name.
    pub fn with_builtin(mut self) -> Self {
        self.app_id = None;
        self.app_name = None;
        self.force_builtin = true;
        self
    }
}

/// Format a user-friendly `(title, detail)` pair for a failed or timed-out slash command
/// (`F-COLLAB-684`, `F-COLLAB-685`).
pub fn format_command_error(command: &str, error: &str) -> (String, String) {
    let cmd = if command.starts_with('/') {
        command.to_owned()
    } else {
        format!("/{command}")
    };
    let lower = error.trim().to_ascii_lowercase();
    if lower == "unknown_command" || lower == "invalid_command" {
        (
            "Unknown command".to_owned(),
            format!("{cmd} is not a valid command in this workspace."),
        )
    } else if lower == "timeout"
        || lower == "command_timeout"
        || lower == "operation_timeout"
        || lower == "dispatch_failed"
        || lower.contains("timed out")
        || lower.contains("timeout")
    {
        (
            format!("{cmd} timed out"),
            format!(
                "{cmd} failed because the app did not respond in time ({}). Try running it again in a moment.",
                error.trim()
            ),
        )
    } else if lower == "not_in_channel" || lower == "channel_not_found" {
        (
            format!("{cmd} unavailable here"),
            format!(
                "{cmd} cannot be run in this conversation ({}).",
                error.trim()
            ),
        )
    } else if lower == "cannot_run_in_thread" || lower == "cant_use_in_thread" {
        (
            format!("{cmd} not supported in threads"),
            format!("{cmd} can only be run in the main channel, not inside a thread."),
        )
    } else if lower == "restricted_action" || lower == "not_allowed" {
        (
            "Command restricted".to_owned(),
            format!("Your workspace permissions do not allow running {cmd} here."),
        )
    } else if lower == "rate_limited" {
        (
            format!("{cmd} rate limited"),
            format!("Too many requests for {cmd}. Wait a few seconds and try again."),
        )
    } else {
        (
            format!("{cmd} failed"),
            format!("Slack could not run {cmd} ({}).", error.trim()),
        )
    }
}

/// The name being typed when the draft is a bare `/prefix` (no whitespace yet): the
/// autocomplete query.
pub fn typing(draft: &str) -> Option<&str> {
    let name = draft.strip_prefix('/')?;
    (!name.contains(char::is_whitespace)).then_some(name)
}

/// Parse a sent draft. `None` when it is not a command (no leading `/`, or `/` followed by
/// something that is not a command name, like a path).
pub fn parse(text: &str) -> Option<Invocation> {
    let rest = text.strip_prefix('/')?;
    let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    let name = name.to_lowercase();
    Some(Invocation {
        command: BUILTINS.iter().find(|c| c.name == name),
        name,
        args: args.trim().to_owned(),
        app_id: None,
        app_name: None,
        force_builtin: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_returns_prefix_only_without_whitespace() {
        assert_eq!(typing("/"), Some(""));
        assert_eq!(typing("/gip"), Some("gip"));
        assert_eq!(typing("/giphy cats"), None);
        assert_eq!(typing("hello"), None);
    }

    #[test]
    fn parse_recognizes_builtins_unknowns_and_rejects_paths() {
        let giphy = parse("/Giphy   dancing cat  ").expect("valid command");
        assert_eq!(giphy.name, "giphy");
        assert_eq!(giphy.args, "dancing cat");
        assert_eq!(giphy.command.map(|c| c.kind), Some(SlashKind::Giphy));

        let custom = parse("/deploy-prod staging").expect("custom command");
        assert_eq!(custom.name, "deploy-prod");
        assert_eq!(custom.args, "staging");
        assert!(custom.command.is_none());

        assert!(parse("/").is_none());
        assert!(parse("/usr/local/bin").is_none());
        assert!(parse("plain text").is_none());
        assert!(parse(" /away").is_none());
    }

    #[test]
    fn invocation_app_and_builtin_builders_and_error_formatting() {
        let inv = parse("/deploy prod")
            .expect("valid")
            .with_app(Some("A01".into()), Some("ReleaseBot".into()));
        assert_eq!(inv.app_id.as_deref(), Some("A01"));
        assert_eq!(inv.app_name.as_deref(), Some("ReleaseBot"));
        assert!(!inv.force_builtin);

        let builtin = parse("/shrug ok").expect("valid").with_builtin();
        assert!(builtin.force_builtin);
        assert_eq!(builtin.app_id, None);

        let (t, d) = format_command_error("/deploy", "dispatch_failed");
        assert!(t.contains("timed out"));
        assert!(d.contains("did not respond in time"));

        let (t, d) = format_command_error("deploy", "operation_timeout");
        assert!(t.contains("/deploy timed out"));
        assert!(d.contains("operation_timeout"));

        let (t, d) = format_command_error("/nope", "unknown_command");
        assert_eq!(t, "Unknown command");
        assert!(d.contains("/nope"));

        let (t, d) = format_command_error("/jira", "cannot_run_in_thread");
        assert!(t.contains("threads"));
        assert!(d.contains("main channel"));

        let (t, _) = format_command_error("/admin", "restricted_action");
        assert_eq!(t, "Command restricted");
    }

    #[test]
    fn format_shrug_appends_suffix_cleanly() {
        assert_eq!(format_shrug(""), SHRUG);
        assert_eq!(format_shrug("   "), SHRUG);
        assert_eq!(format_shrug("dunno"), format!("dunno {SHRUG}"));
    }
}
