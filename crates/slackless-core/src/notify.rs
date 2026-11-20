//! Per-conversation notification levels, as Slack's header bell offers them, read from and
//! written into the `all_notifications_prefs` pref: a JSON object stored as a JSON string,
//! `{"channels": {"C123": {"desktop": "mention", "muted": true, …}}, "global": {…}}`.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::types::ChannelId;

const KEY: &str = "all_notifications_prefs";

/// The bell menu's choices: "All new posts", "Just mentions", "Mute and hide".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyLevel {
    Everything,
    Mentions,
    Muted,
}

impl NotifyLevel {
    /// The `users.prefs.setNotifications` writes (`name`, `value`) that select this level.
    pub(crate) fn writes(self) -> &'static [(&'static str, &'static str)] {
        match self {
            NotifyLevel::Everything => &[("muted", "false"), ("desktop", "everything")],
            NotifyLevel::Mentions => &[("muted", "false"), ("desktop", "mention")],
            NotifyLevel::Muted => &[("muted", "true")],
        }
    }

    fn of(entry: &Value) -> Self {
        if entry.get("muted").and_then(Value::as_bool).unwrap_or(false) {
            NotifyLevel::Muted
        } else if entry.get("desktop").and_then(Value::as_str) == Some("mention") {
            NotifyLevel::Mentions
        } else {
            NotifyLevel::Everything
        }
    }
}

/// Every conversation whose level is not the default ("All new posts").
pub fn notify_levels(prefs: &Value) -> HashMap<ChannelId, NotifyLevel> {
    let Some(all) = parsed(prefs) else {
        return HashMap::new();
    };
    all.get("channels")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(id, entry)| (ChannelId::from(id.as_str()), NotifyLevel::of(entry)))
        .filter(|(_, level)| *level != NotifyLevel::Everything)
        .collect()
}

/// Mirror a level Slack accepted into `prefs`, so the next snapshot shows it.
pub(crate) fn set_notify_level(prefs: &mut Value, channel: &ChannelId, level: NotifyLevel) {
    let mut all = parsed(prefs)
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    if !all["channels"].is_object() {
        all["channels"] = Value::Object(Map::new());
    }
    let entry = &mut all["channels"][channel.as_str()];
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    for &(name, value) in level.writes() {
        entry[name] = match value {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            text => Value::from(text),
        };
    }
    if !prefs.is_object() {
        *prefs = Value::Object(Map::new());
    }
    prefs[KEY] = Value::String(all.to_string());
}

fn parsed(prefs: &Value) -> Option<Value> {
    serde_json::from_str(prefs.get(KEY)?.as_str()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prefs(channels: Value) -> Value {
        json!({ KEY: json!({ "channels": channels, "global": {} }).to_string() })
    }

    #[test]
    fn reads_only_non_default_levels() {
        let prefs = prefs(json!({
            "C1": { "desktop": "everything", "muted": true },
            "C2": { "desktop": "mention", "muted": false },
            "C3": { "desktop": "everything", "muted": false },
        }));
        let levels = notify_levels(&prefs);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[&ChannelId::from("C1")], NotifyLevel::Muted);
        assert_eq!(levels[&ChannelId::from("C2")], NotifyLevel::Mentions);
    }

    #[test]
    fn set_level_round_trips_and_keeps_other_fields() {
        let mut prefs = prefs(json!({ "C1": { "desktop": "mention", "mobile": "mentions_dms" } }));
        set_notify_level(&mut prefs, &ChannelId::from("C1"), NotifyLevel::Muted);
        set_notify_level(&mut prefs, &ChannelId::from("C2"), NotifyLevel::Mentions);
        let levels = notify_levels(&prefs);
        assert_eq!(levels[&ChannelId::from("C1")], NotifyLevel::Muted);
        assert_eq!(levels[&ChannelId::from("C2")], NotifyLevel::Mentions);
        let all = parsed(&prefs).expect("still a JSON string");
        assert_eq!(all["channels"]["C1"]["mobile"], "mentions_dms");

        set_notify_level(&mut prefs, &ChannelId::from("C1"), NotifyLevel::Everything);
        assert!(!notify_levels(&prefs).contains_key(&ChannelId::from("C1")));
    }

    #[test]
    fn set_level_on_missing_prefs_creates_them() {
        let mut prefs = Value::Null;
        set_notify_level(&mut prefs, &ChannelId::from("C9"), NotifyLevel::Mentions);
        assert_eq!(
            notify_levels(&prefs)[&ChannelId::from("C9")],
            NotifyLevel::Mentions
        );
    }
}
