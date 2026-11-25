//! Block Kit action containers, App Home, and slash-command catalog wire types.

use serde::Deserialize;
use serde_json::Value;

use crate::de::lenient_vec;
use crate::types::{ChannelId, Ts};

/// Where a Block Kit action happened, as `blocks.actions`'s `container` names it.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionContainer {
    pub channel: ChannelId,
    pub message_ts: Ts,
    /// The legacy attachment holding the block (Giphy's preview), by its `id`.
    pub attachment_id: Option<i64>,
    pub is_ephemeral: bool,
    /// When set, the action belongs to an App Home or modal view (`{"type":"view","view_id":...}`).
    pub view_id: Option<String>,
}

impl ActionContainer {
    pub(in crate::methods) fn json(&self) -> Value {
        if let Some(view_id) = &self.view_id {
            return serde_json::json!({
                "type": "view",
                "view_id": view_id,
            });
        }
        let (channel, ts) = (self.channel.as_str(), self.message_ts.as_str());
        match self.attachment_id {
            Some(id) => serde_json::json!({
                "type": "message_attachment", "message_ts": ts, "attachment_id": id,
                "channel_id": channel, "is_ephemeral": self.is_ephemeral,
            }),
            None => serde_json::json!({
                "type": "message", "message_ts": ts, "channel_id": channel,
                "is_ephemeral": self.is_ephemeral,
            }),
        }
    }
}

/// App Home metadata and Block Kit view returned by `conversations.info` with
/// `return_app_home=true` (`F-COLLAB-480`, `docs/research/api-apps-blocks.md`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppHomeResponse {
    pub home_tab_enabled: bool,
    pub messages_tab_enabled: bool,
    pub home_view: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct AppHomeFlagsWire {
    #[serde(default)]
    home_tab_enabled: bool,
    #[serde(default)]
    messages_tab_enabled: bool,
    #[serde(default)]
    view: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct AppHomeWire {
    #[serde(default)]
    app_home: Option<AppHomeFlagsWire>,
    #[serde(default)]
    home_view: Option<serde_json::Value>,
}

impl AppHomeWire {
    pub(in crate::methods) fn into_response(self) -> AppHomeResponse {
        let flags = self.app_home.unwrap_or_default();
        let home_view = self.home_view.or(flags.view);
        AppHomeResponse {
            home_tab_enabled: flags.home_tab_enabled || home_view.is_some(),
            messages_tab_enabled: flags.messages_tab_enabled,
            home_view,
        }
    }
}

/// One workspace slash command returned by `client.appCommands` (`core`, `app`, or `service`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCommand {
    /// Name without leading `/`, lowercased (e.g. `"jira"`, `"linear"`, `"giphy"`).
    pub name: String,
    /// Argument hint (e.g. `"[issue key]"`).
    pub usage: String,
    /// Description shown below the command in `/` autocomplete.
    pub description: String,
    /// Owning app or service name (e.g. `"Jira Cloud"`, `"Giphy"`).
    pub app_name: Option<String>,
    /// Owning app id (`A...`) for app-provided commands.
    pub app_id: Option<String>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct WireAppCommand {
    #[serde(default)]
    pub(in crate::methods) name: String,
    #[serde(default)]
    pub(in crate::methods) desc: String,
    #[serde(default)]
    pub(in crate::methods) usage: String,
    #[serde(default)]
    pub(in crate::methods) app: Option<String>,
    #[serde(default)]
    pub(in crate::methods) app_name: Option<String>,
    #[serde(default)]
    pub(in crate::methods) service_name: Option<String>,
    #[serde(default)]
    pub(in crate::methods) alias_of: Option<String>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct AppCommandsWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) commands: Vec<WireAppCommand>,
}

pub(in crate::methods) fn parse_app_commands(wire: AppCommandsWire) -> Vec<AppCommand> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cmd in wire.commands {
        let name = cmd.name.trim().trim_start_matches('/').to_lowercase();
        if name.is_empty() || !seen.insert(name.clone()) {
            continue;
        }
        let app_name = cmd
            .app_name
            .filter(|s| !s.is_empty())
            .or_else(|| cmd.service_name.filter(|s| !s.is_empty()));
        let description = if !cmd.desc.trim().is_empty() {
            cmd.desc.trim().to_owned()
        } else if let Some(alias) = cmd.alias_of.filter(|s| !s.is_empty()) {
            format!("Alias for {alias}")
        } else {
            String::new()
        };
        out.push(AppCommand {
            name,
            usage: cmd.usage.trim().to_owned(),
            description,
            app_name,
            app_id: cmd.app.filter(|s| !s.is_empty()),
        });
    }
    out
}
