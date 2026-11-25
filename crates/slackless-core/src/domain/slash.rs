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
    fn format_shrug_appends_suffix_cleanly() {
        assert_eq!(format_shrug(""), SHRUG);
        assert_eq!(format_shrug("   "), SHRUG);
        assert_eq!(format_shrug("dunno"), format!("dunno {SHRUG}"));
    }
}
