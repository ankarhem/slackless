//! Built-in slash commands (docs/research/api-apps-blocks.md): the composer's autocomplete
//! menu lists [`BUILTINS`], and a draft starting with `/name` is parsed into an
//! [`Invocation`] instead of being sent as a message. Dispatch lives in the root (`app.rs`);
//! fixture mode runs the commands that only touch local state and logs the rest.

/// One built-in command, as the autocomplete menu shows it.
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
}

pub const BUILTINS: [SlashCommand; 11] = [
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

/// A parsed `/name args` draft.
#[derive(Clone)]
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

/// Built-ins whose name starts with `query` (case-insensitive), in menu order.
pub fn matches(query: &str) -> Vec<&'static SlashCommand> {
    let query = query.to_lowercase();
    BUILTINS
        .iter()
        .filter(|c| c.name.starts_with(&query))
        .collect()
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
