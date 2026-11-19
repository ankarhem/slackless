//! Reactions on messages.

use serde::{Deserialize, Serialize};

use crate::de::null_default;
use crate::ids::UserId;

/// One emoji reaction aggregate on a message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reaction {
    /// Shortcode without colons; may include a skin tone suffix (`thumbsup::skin-tone-2`).
    pub name: String,
    #[serde(default)]
    pub count: u32,
    /// May be truncated relative to `count` on busy messages.
    #[serde(default, deserialize_with = "null_default")]
    pub users: Vec<UserId>,
}
