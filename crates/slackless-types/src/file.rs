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
    pub thumb_480_gif: Option<String>,
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

/// One timed speech cue within an audio or video file transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptCue {
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
}

impl TranscriptCue {
    /// Join non-empty cue texts into a single plain-text transcript string suitable for display
    /// and FTS5 indexing.
    pub fn plain_text(cues: &[Self]) -> String {
        cues.iter()
            .map(|c| c.text.trim())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Return the index of the cue currently spoken at `pos_secs`, if any.
    ///
    /// Matches `[start_secs, end_secs)` for positive-duration cues (and inclusive `end_secs` on the
    /// final cue), or `[start_secs, next.start_secs)` when a cue omits a distinct end timestamp.
    pub fn active_index(cues: &[Self], pos_secs: f64) -> Option<usize> {
        if cues.is_empty() || !pos_secs.is_finite() || pos_secs < 0.0 {
            return None;
        }
        let mut matched = None;
        for (idx, cue) in cues.iter().enumerate() {
            if !cue.start_secs.is_finite() || pos_secs < cue.start_secs {
                continue;
            }
            let effective_end = if cue.end_secs.is_finite() && cue.end_secs > cue.start_secs {
                cue.end_secs
            } else if let Some(next) = cues.get(idx + 1)
                && next.start_secs.is_finite()
                && next.start_secs > cue.start_secs
            {
                next.start_secs
            } else {
                f64::INFINITY
            };
            let is_last = idx + 1 == cues.len();
            if pos_secs < effective_end || (is_last && pos_secs <= effective_end) {
                matched = Some(idx);
            }
        }
        matched
    }

    /// Format a cue timestamp in seconds as `M:SS` (or `H:MM:SS` at or above one hour).
    pub fn format_timestamp(secs: f64) -> String {
        let total = if secs.is_finite() && secs > 0.0 {
            secs.floor() as u64
        } else {
            0
        };
        let hours = total / 3600;
        let mins = (total % 3600) / 60;
        let rem = total % 60;
        if hours > 0 {
            format!("{hours}:{mins:02}:{rem:02}")
        } else {
            format!("{mins}:{rem:02}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_cue_active_index_plain_text_and_formatting() {
        let cues = vec![
            TranscriptCue {
                start_secs: 0.5,
                end_secs: 2.5,
                text: "Hello everyone,".into(),
            },
            TranscriptCue {
                start_secs: 3.0,
                end_secs: 6.0,
                text: "let's ship it.".into(),
            },
            TranscriptCue {
                start_secs: 6.0,
                end_secs: 6.0, // zero-duration tail cue
                text: "Thanks!".into(),
            },
        ];

        assert_eq!(
            TranscriptCue::plain_text(&cues),
            "Hello everyone, let's ship it. Thanks!"
        );
        assert_eq!(TranscriptCue::active_index(&[], 1.0), None);
        assert_eq!(TranscriptCue::active_index(&cues, -1.0), None);
        assert_eq!(TranscriptCue::active_index(&cues, f64::NAN), None);
        assert_eq!(TranscriptCue::active_index(&cues, 0.2), None);
        assert_eq!(TranscriptCue::active_index(&cues, 0.5), Some(0));
        assert_eq!(TranscriptCue::active_index(&cues, 2.49), Some(0));
        assert_eq!(TranscriptCue::active_index(&cues, 2.7), None);
        assert_eq!(TranscriptCue::active_index(&cues, 3.0), Some(1));
        assert_eq!(TranscriptCue::active_index(&cues, 6.0), Some(2));
        assert_eq!(TranscriptCue::active_index(&cues, 10.0), Some(2));

        assert_eq!(TranscriptCue::format_timestamp(0.0), "0:00");
        assert_eq!(TranscriptCue::format_timestamp(-5.0), "0:00");
        assert_eq!(TranscriptCue::format_timestamp(5.9), "0:05");
        assert_eq!(TranscriptCue::format_timestamp(125.0), "2:05");
        assert_eq!(TranscriptCue::format_timestamp(3665.2), "1:01:05");
    }
}
