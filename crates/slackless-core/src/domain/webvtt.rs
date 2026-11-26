//! WebVTT transcript parser and Slack audio/video clip transcript extractors.

use crate::types::{File, TranscriptCue};

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

/// Parse a WebVTT / SRT timestamp token (`HH:MM:SS.mmm`, `MM:SS.mmm`, with `.` or `,`
/// fractional separator) into seconds.
pub fn parse_webvtt_timestamp(token: &str) -> Option<f64> {
    let trimmed = token.trim().replace(',', ".");
    let parts: Vec<&str> = trimmed.split(':').collect();
    let (hours, mins, secs) = match parts.as_slice() {
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [h, m, s] => (
            h.parse::<f64>().ok()?,
            m.parse::<f64>().ok()?,
            s.parse::<f64>().ok()?,
        ),
        _ => return None,
    };
    if !hours.is_finite()
        || !mins.is_finite()
        || !secs.is_finite()
        || hours < 0.0
        || mins < 0.0
        || secs < 0.0
    {
        return None;
    }
    Some(hours * 3600.0 + mins * 60.0 + secs)
}

/// Strip inline WebVTT cue markup (`<v Speaker>`, `<i>`, `<b>`, `<c>`, `<00:00:01.000>`) and
/// decode common HTML entities into clean display text.
fn clean_webvtt_cue_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&quot;", "\"");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a WebVTT document into timed [`TranscriptCue`] entries (`start_secs`, `end_secs`, `text`),
/// skipping `WEBVTT` headers, `NOTE`/`STYLE`/`REGION` blocks, cue identifiers, and cue settings.
pub fn parse_webvtt_cues(raw: &str) -> Vec<TranscriptCue> {
    let normalized = raw.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    let lines: Vec<&str> = normalized.lines().collect();
    let mut i = 0usize;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        // Skip WEBVTT header / NOTE / STYLE / REGION blocks until the next blank line.
        if trimmed.starts_with("WEBVTT")
            || trimmed.starts_with("NOTE")
            || trimmed.starts_with("STYLE")
            || trimmed.starts_with("REGION")
        {
            i += 1;
            while i < lines.len() && !lines[i].trim().is_empty() {
                i += 1;
            }
            continue;
        }

        // Optional cue identifier line before the timestamp line.
        let timing_line = if trimmed.contains("-->") {
            trimmed
        } else if i + 1 < lines.len() && lines[i + 1].contains("-->") {
            i += 1;
            lines[i].trim()
        } else {
            i += 1;
            continue;
        };

        let Some((lhs, rhs)) = timing_line.split_once("-->") else {
            i += 1;
            continue;
        };
        let start_token = lhs.split_whitespace().next().unwrap_or("");
        let end_token = rhs.split_whitespace().next().unwrap_or("");
        let (Some(start_secs), Some(end_secs)) = (
            parse_webvtt_timestamp(start_token),
            parse_webvtt_timestamp(end_token),
        ) else {
            i += 1;
            continue;
        };

        i += 1;
        let mut payload = Vec::new();
        while i < lines.len() {
            let line = lines[i].trim();
            if line.is_empty() {
                break;
            }
            payload.push(line);
            i += 1;
        }

        let text = clean_webvtt_cue_text(&payload.join(" "));
        if !text.is_empty() {
            cues.push(TranscriptCue {
                start_secs,
                end_secs: end_secs.max(start_secs),
                text,
            });
        }
    }

    cues
}

/// Strip WebVTT headers, cue numbers, and timestamp arrows into clean readable transcript text.
pub fn parse_webvtt(raw: &str) -> String {
    let cues = parse_webvtt_cues(raw);
    if !cues.is_empty() {
        return TranscriptCue::plain_text(&cues);
    }
    let mut lines = Vec::new();
    for line in raw.trim_start_matches('\u{feff}').lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("WEBVTT")
            || trimmed.starts_with("NOTE")
            || trimmed.starts_with("STYLE")
            || trimmed.starts_with("REGION")
            || trimmed.contains("-->")
            || trimmed.chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let cleaned = clean_webvtt_cue_text(trimmed);
        if !cleaned.is_empty() {
            lines.push(cleaned);
        }
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
    fn parse_webvtt_cues_preserves_timestamps_settings_multiline_and_tags() {
        let vtt = "\u{feff}WEBVTT
Kind: captions
Language: en

STYLE
::cue { color: white; }

NOTE
multi-line comment
still ignored

cue-1
00:01.250 --> 00:04.500 align:start position:0%
<v Alice>Hello <i>everyone</i> &amp; welcome,</v>
let's check the <c.hi>metrics</c>.

2
01:02:03,500 --> 01:02:07,000
Second cue after an hour.
";
        let cues = parse_webvtt_cues(vtt);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start_secs - 1.25).abs() < 1e-6);
        assert!((cues[0].end_secs - 4.5).abs() < 1e-6);
        assert_eq!(
            cues[0].text,
            "Hello everyone & welcome, let's check the metrics."
        );

        assert!((cues[1].start_secs - 3723.5).abs() < 1e-6);
        assert!((cues[1].end_secs - 3727.0).abs() < 1e-6);
        assert_eq!(cues[1].text, "Second cue after an hour.");

        assert_eq!(
            parse_webvtt(vtt),
            "Hello everyone & welcome, let's check the metrics. Second cue after an hour."
        );
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
