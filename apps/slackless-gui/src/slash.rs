//! Built-in slash commands: re-exports the shared catalog and parser from
//! `slackless_core::domain::slash`. Dispatch lives in `app/`: live mode hands `/giphy` and
//! commands slackless does not know to Slack; fixture mode runs local commands and logs the rest.

pub use slackless_core::domain::slash::{
    BUILTINS, Invocation, SHRUG, SlashKind, format_command_error, parse, typing,
};
