//! Users, profiles, presence, DND, and the team object.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{lenient_i64, null_default};
use crate::ids::{BotId, TeamId, UserId};
use crate::macros::string_enum;

/// A user object (`users.info`, `users.list`, `client.userBoot.self`, `user_change`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: UserId,
    #[serde(default)]
    pub team_id: Option<TeamId>,
    /// Legacy username handle.
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default)]
    pub real_name: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub profile: Profile,
    #[serde(default, deserialize_with = "null_default")]
    pub deleted: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_bot: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_app_user: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_admin: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_owner: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_restricted: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_ultra_restricted: bool,
    #[serde(default, deserialize_with = "null_default")]
    pub is_stranger: bool,
    /// Hex color without `#`.
    #[serde(default)]
    pub color: Option<String>,
    /// IANA zone, e.g. `America/Los_Angeles`.
    #[serde(default)]
    pub tz: Option<String>,
    #[serde(default)]
    pub tz_label: Option<String>,
    /// Offset from UTC in seconds.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub tz_offset: Option<i64>,
    /// Unix seconds of the last change; used for cache invalidation.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub updated: Option<i64>,
    /// Fields this crate does not model, preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl User {
    /// What the UI shows: profile display name, then real name, then the handle.
    pub fn display_name(&self) -> &str {
        [
            self.profile.display_name.as_deref(),
            self.profile.real_name.as_deref(),
            self.real_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .unwrap_or(&self.name)
    }
}

/// A user profile. Also used for the trimmed `user_profile` embedded in messages, so every field
/// is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub display_name_normalized: Option<String>,
    #[serde(default)]
    pub real_name: Option<String>,
    #[serde(default)]
    pub real_name_normalized: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    /// Message-embedded profiles carry the handle here.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub phone: Option<String>,
    #[serde(default)]
    pub status_text: Option<String>,
    /// Shortcode with colons, e.g. `:books:`.
    #[serde(default)]
    pub status_emoji: Option<String>,
    /// Unix seconds; 0 means no expiry.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub status_expiration: Option<i64>,
    #[serde(default)]
    pub avatar_hash: Option<String>,
    #[serde(default)]
    pub image_24: Option<String>,
    #[serde(default)]
    pub image_32: Option<String>,
    #[serde(default)]
    pub image_48: Option<String>,
    #[serde(default)]
    pub image_72: Option<String>,
    #[serde(default)]
    pub image_192: Option<String>,
    #[serde(default)]
    pub image_512: Option<String>,
    #[serde(default)]
    pub image_1024: Option<String>,
    #[serde(default)]
    pub image_original: Option<String>,
    #[serde(default)]
    pub team: Option<TeamId>,
    #[serde(default)]
    pub bot_id: Option<BotId>,
    /// Custom profile fields keyed by field id.
    #[serde(default)]
    pub fields: Option<Value>,
}

string_enum! {
    /// `active` / `away`.
    pub enum Presence {
        Active = "active",
        Away = "away",
    }
}

/// `users.getPresence` payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPresence {
    pub presence: Presence,
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default)]
    pub auto_away: Option<bool>,
    #[serde(default)]
    pub manual_away: Option<bool>,
    #[serde(default)]
    pub connection_count: Option<u32>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub last_activity: Option<i64>,
}

/// Do-not-disturb state (`dnd.info`, `dnd_updated`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DndStatus {
    #[serde(default, deserialize_with = "null_default")]
    pub dnd_enabled: bool,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub next_dnd_start_ts: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub next_dnd_end_ts: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
    pub snooze_enabled: bool,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub snooze_endtime: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub snooze_remaining: Option<i64>,
}

impl DndStatus {
    /// True while notifications are silenced: during an active snooze, or inside the enabled
    /// do-not-disturb window. `now` is unix seconds.
    pub fn paused(&self, now: i64) -> bool {
        (self.snooze_enabled && self.snooze_endtime.is_some_and(|end| end > now))
            || (self.dnd_enabled
                && matches!(
                    (self.next_dnd_start_ts, self.next_dnd_end_ts),
                    (Some(start), Some(end)) if now >= start && now < end
                ))
    }
}

/// A workspace (`auth.test`, `client.userBoot.team`, `rtm.connect.team`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: TeamId,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    /// Subdomain: `<domain>.slack.com`.
    #[serde(default, deserialize_with = "null_default")]
    pub domain: String,
    #[serde(default)]
    pub enterprise_id: Option<TeamId>,
    #[serde(default)]
    pub icon: Option<Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::DndStatus;

    #[test]
    fn paused_follows_snooze_and_dnd_window() {
        let off = DndStatus::default();
        assert!(!off.paused(1000));

        let snoozing = DndStatus {
            snooze_enabled: true,
            snooze_endtime: Some(1200),
            ..Default::default()
        };
        assert!(snoozing.paused(1000));
        assert!(
            !snoozing.paused(1200),
            "a snooze that ended no longer pauses"
        );

        let scheduled = DndStatus {
            dnd_enabled: true,
            next_dnd_start_ts: Some(1000),
            next_dnd_end_ts: Some(2000),
            ..Default::default()
        };
        assert!(
            scheduled.paused(1000),
            "the window is inclusive of its start"
        );
        assert!(scheduled.paused(1999));
        assert!(!scheduled.paused(2000), "the window end no longer pauses");
        assert!(!scheduled.paused(999), "before the window nothing pauses");

        // A schedule without the do-not-disturb preference on never pauses.
        let disabled = DndStatus {
            next_dnd_start_ts: Some(1000),
            next_dnd_end_ts: Some(2000),
            ..Default::default()
        };
        assert!(!disabled.paused(1500));
    }
}
