//! WebVTT transcript parser and Slack audio/video clip transcript extractors.

use crate::types::File;

/// Extract inline transcript text from Slack clip metadata (`transcription.preview.content`,
/// `transcription.text`, `transcript`, or `file.preview` on audio/video clips).
pub fn inline_clip_transcript(file: &File) -> Option<String> {
    if let Some(t) = file.extra.get("transcription") {
        if let Some(s) = t
            .get("preview")
            .and_then(|p| p.get("content"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            return Some(s.trim().to_owned());
        }
        if let Some(s) = t
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            return Some(s.trim().to_owned());
        }
    }
    if let Some(s) = file
        .extra
        .get("transcript")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return Some(s.trim().to_owned());
    }
    file.preview
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

/// Optional WebVTT URL (`vtt` or `transcription.vtt`) for lazy transcript loading.
pub fn clip_vtt_url(file: &File) -> Option<String> {
    file.extra
        .get("vtt")
        .and_then(|v| v.as_str())
        .or_else(|| {
            file.extra
                .get("transcription")
                .and_then(|t| t.get("vtt"))
                .and_then(|v| v.as_str())
        })
        .filter(|s| s.starts_with("http"))
        .map(ToOwned::to_owned)
}

/// Strip WebVTT headers, cue numbers, and timestamp arrows into clean readable transcript text.
pub fn parse_webvtt(raw: &str) -> String {
    let mut lines = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("WEBVTT")
            || trimmed.starts_with("NOTE")
            || trimmed.contains("-->")
            || trimmed.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        lines.push(trimmed);
    }
    lines.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_webvtt_strips_headers_notes_cue_ids_and_timestamps() {
        let vtt = "\
WEBVTT - Clip Transcript

NOTE generated automatically

1
00:00:00.000 --> 00:00:02.500
Hello everyone,

2
00:00:02.500 --> 00:00:05.000
let's ship it today.
";
        assert_eq!(parse_webvtt(vtt), "Hello everyone, let's ship it today.");
        assert_eq!(parse_webvtt("WEBVTT\n\n"), "");
    }

    #[test]
    fn inline_clip_transcript_and_vtt_url_follow_precedence() {
        let mut file = File {
            preview: Some("  preview fallback  ".into()),
            ..Default::default()
        };
        assert_eq!(
            inline_clip_transcript(&file).as_deref(),
            Some("preview fallback")
        );
        assert_eq!(clip_vtt_url(&file), None);

        file.extra.insert("transcript".into(), json!("direct text"));
        assert_eq!(
            inline_clip_transcript(&file).as_deref(),
            Some("direct text")
        );

        file.extra.insert(
            "transcription".into(),
            json!({
                "text": "transcription text",
                "vtt": "https://files.slack.com/clip.vtt"
            }),
        );
        assert_eq!(
            inline_clip_transcript(&file).as_deref(),
            Some("transcription text")
        );
        assert_eq!(
            clip_vtt_url(&file).as_deref(),
            Some("https://files.slack.com/clip.vtt")
        );

        file.extra.insert(
            "transcription".into(),
            json!({
                "preview": { "content": "  preview content wins  " },
                "text": "transcription text"
            }),
        );
        assert_eq!(
            inline_clip_transcript(&file).as_deref(),
            Some("preview content wins")
        );

        // Non-http URL is ignored.
        file.extra.insert("vtt".into(), json!("file:///etc/passwd"));
        assert_eq!(clip_vtt_url(&file), None);
    }
}
