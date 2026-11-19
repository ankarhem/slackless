//! Sidebar channel sections (`users.channelSections.list`): the user's custom grouping and
//! ordering of the sidebar.

use serde::{Deserialize, Serialize};

use crate::ids::ChannelId;
use crate::macros::string_enum;

/// A sidebar section: a named, ordered group of conversations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSection {
    /// Section id (`L…`).
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Built-in buckets have a fixed type; user-created sections are `Standard`.
    #[serde(default = "standard_kind")]
    pub kind: SectionKind,
    /// Bare emoji name for the section header, empty when none.
    #[serde(default)]
    pub emoji: String,
    /// The section displayed directly below this one; `None` at the end of the chain.
    #[serde(default)]
    pub next_id: Option<String>,
    #[serde(default)]
    pub channel_ids: Vec<ChannelId>,
}

string_enum! {
    /// The `type` of a sidebar section.
    pub enum SectionKind {
        /// A user-created section.
        Standard = "standard",
        /// The default `Channels` bucket.
        Channels = "channels",
        /// The default `Direct messages` bucket.
        DirectMessages = "direct_messages",
        /// The default `Apps` bucket.
        RecentApps = "recent_apps",
        /// The `Starred` bucket.
        Stars = "stars",
        /// The `External connections` bucket (shared channels).
        SlackConnect = "slack_connect",
        /// AI agent conversations.
        Agents = "agents",
        /// A user-group driven section (`usergroup_id`).
        UserGroup = "user_group",
    }
}

/// The kind assumed when `type` is missing: user sections carry no distinct marker.
fn standard_kind() -> SectionKind {
    SectionKind::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_kind_round_trips_observed_wire_types() {
        // Known wire strings stay known (and map back to themselves); an unmodeled one is kept
        // verbatim instead of failing the parse.
        for wire in [
            "standard",
            "channels",
            "direct_messages",
            "recent_apps",
            "stars",
            "slack_connect",
            "agents",
            "user_group",
            "salesforce_records",
        ] {
            assert_eq!(SectionKind::from(wire).as_str(), wire);
        }
    }

    #[test]
    fn channel_section_defaults_missing_fields() {
        let section: ChannelSection = serde_json::from_str(r#"{"id":"L1"}"#).unwrap();
        assert_eq!(section.name, "");
        assert_eq!(
            section.kind,
            SectionKind::Standard,
            "missing type is Standard"
        );
        assert_eq!(section.emoji, "");
        assert_eq!(section.next_id, None);
        assert!(section.channel_ids.is_empty());
        let round: ChannelSection =
            serde_json::from_str(&serde_json::to_string(&section).unwrap()).unwrap();
        assert_eq!(round, section);
    }
}
