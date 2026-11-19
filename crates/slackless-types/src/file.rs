//! The file object (message `files[]`, `files.info`, `file_*` events).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::de::{lenient_i64, null_default};
use crate::ids::{FileId, UserId};
use crate::macros::string_enum;

string_enum! {
    /// `file.mode`.
    pub enum FileMode {
        Hosted = "hosted",
        External = "external",
        Snippet = "snippet",
        Post = "post",
        /// Deleted file still referenced by a message.
        Tombstone = "tombstone",
        /// Hidden by the free-plan history limit.
        HiddenByLimit = "hidden_by_limit",
    }
}

/// A Slack file. A message can carry a stub (no mimetype, size 0) before processing finishes, or
/// a tombstone with only `id` + `mode`, so nearly everything is optional.
///
/// All URLs (`url_private*`, `thumb_*`) need the session cookie/token to fetch.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct File {
    pub id: FileId,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub mimetype: Option<String>,
    #[serde(default)]
    pub filetype: Option<String>,
    /// Human label for generic file cards, e.g. `PDF`.
    #[serde(default)]
    pub pretty_type: Option<String>,
    #[serde(default)]
    pub user: Option<UserId>,
    #[serde(default)]
    pub mode: Option<FileMode>,
    /// Bytes.
    #[serde(default, deserialize_with = "lenient_i64")]
    pub size: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub created: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub timestamp: Option<i64>,
    #[serde(default, deserialize_with = "null_default")]
    pub is_external: bool,
    #[serde(default)]
    pub external_type: Option<String>,
    #[serde(default)]
    pub url_private: Option<String>,
    #[serde(default)]
    pub url_private_download: Option<String>,
    #[serde(default)]
    pub permalink: Option<String>,
    #[serde(default)]
    pub permalink_public: Option<String>,
    #[serde(default)]
    pub thumb_64: Option<String>,
    #[serde(default)]
    pub thumb_80: Option<String>,
    #[serde(default)]
    pub thumb_160: Option<String>,
    #[serde(default)]
    pub thumb_360: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub thumb_360_w: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub thumb_360_h: Option<i64>,
    #[serde(default)]
    pub thumb_360_gif: Option<String>,
    #[serde(default)]
    pub thumb_480: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub thumb_480_w: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub thumb_480_h: Option<i64>,
    #[serde(default)]
    pub thumb_720: Option<String>,
    #[serde(default)]
    pub thumb_800: Option<String>,
    #[serde(default)]
    pub thumb_960: Option<String>,
    #[serde(default)]
    pub thumb_1024: Option<String>,
    /// Base64 blur placeholder.
    #[serde(default)]
    pub thumb_tiny: Option<String>,
    #[serde(default)]
    pub thumb_pdf: Option<String>,
    #[serde(default)]
    pub thumb_video: Option<String>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub original_w: Option<i64>,
    #[serde(default, deserialize_with = "lenient_i64")]
    pub original_h: Option<i64>,
    /// Text preview for snippets/posts.
    #[serde(default)]
    pub preview: Option<String>,
    #[serde(default)]
    pub media_display_type: Option<String>,
    /// Fields this crate does not model (`mp4`, `hls`, `shares`, ...), preserved verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
