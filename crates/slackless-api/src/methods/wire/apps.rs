//! Block Kit action containers, App Home, and slash-command catalog wire types.

use serde::Deserialize;
use serde_json::Value;

use crate::de::lenient_vec;
use crate::types::{BotId, ChannelId, TeamId, Ts};

/// Request parameters for `blocks.suggestions` (`external_select` / `multi_external_select`).
#[derive(Debug, Clone)]
pub struct BlockSuggestionsRequest<'a> {
    pub bot: &'a BotId,
    pub app_id: &'a str,
    pub team: &'a TeamId,
    pub action_id: &'a str,
    pub block_id: &'a str,
    pub value: &'a str,
    pub container: &'a ActionContainer,
}

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
    let mut seen_names = std::collections::HashSet::new();
    let mut seen_attributed = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cmd in wire.commands {
        let name = cmd.name.trim().trim_start_matches('/').to_lowercase();
        if name.is_empty() {
            continue;
        }
        let app_id = cmd.app.filter(|s| !s.is_empty());
        let app_name = cmd
            .app_name
            .filter(|s| !s.is_empty())
            .or_else(|| cmd.service_name.filter(|s| !s.is_empty()));
        // Preserve duplicate `/command` registrations across distinct apps (`F-COLLAB-686`),
        // while deduplicating identical app attributions and unattributed duplicate rows.
        if app_id.is_none() && app_name.is_none() {
            if !seen_names.insert(name.clone()) {
                continue;
            }
        } else {
            let dedup_key = (
                name.clone(),
                app_id.clone().unwrap_or_default(),
                app_name.clone().unwrap_or_default().to_ascii_lowercase(),
            );
            if !seen_attributed.insert(dedup_key) {
                continue;
            }
            seen_names.insert(name.clone());
        }
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
            app_id,
        });
    }
    out
}

/// Optional immediate response payload returned by `chat.command` (`F-COLLAB-684`).
/// Most slash commands reply asynchronously over the websocket, so both fields default to `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CommandResponse {
    /// `"in_channel"` or `"ephemeral"`, when returned inline.
    #[serde(default)]
    pub response_type: Option<String>,
    /// Immediate response text, if any.
    #[serde(default, alias = "response")]
    pub text: Option<String>,
}

/// Workspace custom/standard profile field metadata from `team.profile.get`, including
/// SCIM / admin lock indicators (`F-SHELL-005`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamProfileField {
    pub id: String,
    pub label: String,
    pub field_name: Option<String>,
    pub is_hidden: bool,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
}

#[derive(Deserialize)]
pub(in crate::methods) struct TeamProfileFieldWire {
    pub(in crate::methods) id: String,
    #[serde(default)]
    pub(in crate::methods) label: String,
    #[serde(default)]
    pub(in crate::methods) field_name: Option<String>,
    #[serde(default)]
    pub(in crate::methods) is_hidden: bool,
    #[serde(default)]
    pub(in crate::methods) is_scim: bool,
    #[serde(default)]
    pub(in crate::methods) is_locked: bool,
    #[serde(default)]
    pub(in crate::methods) options: Option<Value>,
    #[serde(default)]
    pub(in crate::methods) permissions: Option<Value>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct TeamProfileDefWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) fields: Vec<TeamProfileFieldWire>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct TeamProfileWire {
    #[serde(default)]
    pub(in crate::methods) profile: TeamProfileDefWire,
}

pub(in crate::methods) fn parse_team_profile_fields(
    wire: TeamProfileWire,
) -> Vec<TeamProfileField> {
    wire.profile
        .fields
        .into_iter()
        .map(|f| {
            let opt_scim = f
                .options
                .as_ref()
                .and_then(|o| o.get("is_scim").and_then(Value::as_bool))
                .unwrap_or(false);
            let opt_protected = f
                .options
                .as_ref()
                .and_then(|o| o.get("is_protected").and_then(Value::as_bool))
                .unwrap_or(false);
            let perm_scim = f
                .permissions
                .as_ref()
                .and_then(|p| p.get("scim").and_then(Value::as_bool))
                .unwrap_or(false);
            let perm_ui_locked = f
                .permissions
                .as_ref()
                .and_then(|p| p.get("ui").and_then(Value::as_bool))
                == Some(false);
            let is_scim = f.is_scim || opt_scim || perm_scim;
            let is_locked = f.is_locked || is_scim || opt_protected || perm_ui_locked;
            let lock_reason = if is_scim {
                Some("Managed by SCIM / identity provider".to_owned())
            } else if is_locked {
                Some("Locked by workspace admin".to_owned())
            } else {
                None
            };
            TeamProfileField {
                id: f.id,
                label: f.label,
                field_name: f.field_name.filter(|s| !s.trim().is_empty()),
                is_hidden: f.is_hidden,
                is_locked,
                lock_reason,
            }
        })
        .collect()
}

/// Structured outcome of `views.submit` (`F-COLLAB-605..612`).
///
/// Slack responds with `response_action`:
/// - absent / empty -> close the submitted view
/// - `"clear"` -> close the entire modal stack
/// - `"push"` -> push a new modal view onto the stack (carrying `previous_view_id` & `root_view_id`)
/// - `"update"` -> replace the current modal view in place
/// - `"errors"` -> keep the modal open and render per-`block_id` validation errors inline
#[derive(Debug, Clone, PartialEq)]
pub enum ViewSubmitResponse {
    Close,
    Clear,
    Push(Value),
    Update(Value),
    Errors(std::collections::HashMap<String, String>),
}

/// Extract a map of `block_id -> error message` from a `views.submit` response payload.
fn extract_validation_errors(raw: &Value) -> std::collections::HashMap<String, String> {
    let mut errors = std::collections::HashMap::new();
    let Some(map) = raw.get("errors").and_then(Value::as_object) else {
        return errors;
    };
    for (block_id, msg) in map {
        let text = msg
            .as_str()
            .map(str::to_owned)
            .or_else(|| {
                msg.as_object()
                    .and_then(|o| o.get("message").or_else(|| o.get("text")))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| msg.to_string());
        if !block_id.is_empty() && !text.trim().is_empty() {
            errors.insert(block_id.clone(), text);
        }
    }
    errors
}

/// Parse the raw JSON response of `views.submit`, supporting both `ok: true` and `ok: false`
/// validation error shapes.
pub(in crate::methods) fn parse_view_submit_response(
    raw: &Value,
) -> crate::Result<ViewSubmitResponse> {
    let response_action = raw
        .get("response_action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let errors = extract_validation_errors(raw);
    if response_action == "errors" || !errors.is_empty() {
        return Ok(ViewSubmitResponse::Errors(errors));
    }
    let ok = raw.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if !ok {
        let code = raw
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error")
            .to_owned();
        return Err(crate::ApiError::from_slack_error(code));
    }
    match response_action {
        "clear" => Ok(ViewSubmitResponse::Clear),
        "push" => match raw.get("view").filter(|v| v.is_object()).cloned() {
            Some(view) => Ok(ViewSubmitResponse::Push(view)),
            None => Ok(ViewSubmitResponse::Close),
        },
        "update" => match raw.get("view").filter(|v| v.is_object()).cloned() {
            Some(view) => Ok(ViewSubmitResponse::Update(view)),
            None => Ok(ViewSubmitResponse::Close),
        },
        _ => Ok(ViewSubmitResponse::Close),
    }
}

/// Metadata returned by `workflows.triggers.trip`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct WorkflowTriggerTripResponse {
    #[serde(default)]
    pub is_slow_workflow: bool,
    #[serde(default)]
    pub contains_ai_steps: bool,
}

/// Summary of one workflow step returned inside `workflows.triggers.info`
/// (`include_workflow_details=true`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowStepSummary {
    pub title: String,
    pub description: Option<String>,
    pub function_id: Option<String>,
}

/// Detailed workflow trigger metadata returned by `workflows.triggers.info` (`F-COLLAB-432`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowTriggerInfo {
    pub trigger_id: String,
    pub name: String,
    pub description: String,
    pub trigger_type: String,
    pub url: Option<String>,
    pub workflow_id: Option<String>,
    pub app_id: Option<String>,
    pub steps: Vec<WorkflowStepSummary>,
    pub collaborators: Vec<String>,
    pub is_published: Option<bool>,
    pub date_updated: Option<i64>,
}

/// Extract a Slack workflow trigger ID (`Ft...`) from a workflow link trigger URL such as
/// `https://slack.com/shortcuts/Ft0123456789/abcdef` or `https://acme.slack.com/shortcuts/Ft0123456789`.
pub fn extract_workflow_trigger_id(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    if host != "slack.com" && !host.ends_with(".slack.com") {
        return None;
    }
    let mut segments = parsed.path_segments()?;
    while let Some(seg) = segments.next() {
        if seg == "shortcuts" {
            let id = segments.next()?.trim();
            if id.starts_with("Ft") && id.len() > 2 && id.chars().all(|c| c.is_ascii_alphanumeric())
            {
                return Some(id.to_owned());
            }
            return None;
        }
    }
    None
}

pub(in crate::methods) fn parse_workflow_trigger_info(
    fallback_trigger_id: &str,
    raw: &Value,
) -> WorkflowTriggerInfo {
    let trigger = raw.get("trigger").filter(|v| v.is_object()).unwrap_or(raw);
    let workflow = trigger
        .get("workflow")
        .or_else(|| raw.get("workflow"))
        .filter(|v| v.is_object());

    let str_at = |obj: &Value, keys: &[&str]| -> Option<String> {
        keys.iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };

    let trigger_id =
        str_at(trigger, &["id", "trigger_id"]).unwrap_or_else(|| fallback_trigger_id.to_owned());
    let name = str_at(trigger, &["name", "title"])
        .or_else(|| workflow.and_then(|w| str_at(w, &["title", "name"])))
        .unwrap_or_else(|| "Workflow".to_owned());
    let description = str_at(trigger, &["description"])
        .or_else(|| workflow.and_then(|w| str_at(w, &["description"])))
        .unwrap_or_default();
    let trigger_type =
        str_at(trigger, &["type", "trigger_type"]).unwrap_or_else(|| "shortcut".to_owned());
    let url = str_at(trigger, &["shortcut_url", "url", "trigger_url"]);
    let workflow_id = workflow
        .and_then(|w| str_at(w, &["id", "workflow_id"]))
        .or_else(|| str_at(trigger, &["workflow_id"]));
    let app_id = workflow
        .and_then(|w| str_at(w, &["app_id"]))
        .or_else(|| str_at(trigger, &["app_id"]));

    let steps = workflow
        .and_then(|w| w.get("steps"))
        .or_else(|| trigger.get("steps"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|step| {
                    let title = str_at(step, &["title", "name", "function_title"])
                        .or_else(|| {
                            step.get("function")
                                .and_then(|f| str_at(f, &["title", "name"]))
                        })
                        .or_else(|| str_at(step, &["function_id", "type"]))?;
                    let description = str_at(step, &["description"]).or_else(|| {
                        step.get("function")
                            .and_then(|f| str_at(f, &["description"]))
                    });
                    let function_id = str_at(step, &["function_id"])
                        .or_else(|| step.get("function").and_then(|f| str_at(f, &["id"])));
                    Some(WorkflowStepSummary {
                        title,
                        description,
                        function_id,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let collaborators = workflow
        .and_then(|w| w.get("collaborators"))
        .or_else(|| trigger.get("collaborators"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| str_at(v, &["user_id", "id"]))
                })
                .collect()
        })
        .unwrap_or_default();

    let is_published = workflow
        .and_then(|w| w.get("is_published").and_then(Value::as_bool))
        .or_else(|| trigger.get("is_published").and_then(Value::as_bool));
    let date_updated = workflow
        .and_then(|w| w.get("date_updated").and_then(Value::as_i64))
        .or_else(|| trigger.get("date_updated").and_then(Value::as_i64));

    WorkflowTriggerInfo {
        trigger_id,
        name,
        description,
        trigger_type,
        url,
        workflow_id,
        app_id,
        steps,
        collaborators,
        is_published,
        date_updated,
    }
}
