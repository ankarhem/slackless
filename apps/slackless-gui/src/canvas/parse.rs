//! Parsers for Slack Quip/Canvas HTML exports and Canvas markdown, including emoji resolution,
//! inline audio/video embeds, user profile cards, and embedded canvases & lists
//! (`F-COLLAB-134..136`, `F-COLLAB-139`, `F-COLLAB-141`, `F-COLLAB-147`, `F-COLLAB-148`).

use crate::canvas::model::{CanvasBlock, CanvasEmbedKind, MediaKind};

/// Resolve `:shortcode:` tokens inside canvas text into Unicode emoji glyphs (`F-COLLAB-139`).
pub(crate) fn resolve_canvas_emoji(text: &str) -> String {
    crate::emoji::replace_shortcodes(text)
}

/// Classify a URL or filename by media file extension (`F-COLLAB-141`).
/// Returns `(MediaKind, canonical_mime)`.
pub(crate) fn classify_media_url(url_or_name: &str) -> Option<(MediaKind, &'static str)> {
    let trimmed = url_or_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_frag = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let segment = without_query.rsplit('/').next().unwrap_or(without_query);
    let dot = segment.rfind('.')?;
    let ext = segment[dot + 1..].to_ascii_lowercase();
    match ext.as_str() {
        "mp3" => Some((MediaKind::Audio, "audio/mpeg")),
        "m4a" => Some((MediaKind::Audio, "audio/mp4")),
        "wav" => Some((MediaKind::Audio, "audio/wav")),
        "ogg" | "oga" => Some((MediaKind::Audio, "audio/ogg")),
        "aac" => Some((MediaKind::Audio, "audio/aac")),
        "flac" => Some((MediaKind::Audio, "audio/flac")),
        "opus" => Some((MediaKind::Audio, "audio/opus")),
        "mp4" | "m4v" => Some((MediaKind::Video, "video/mp4")),
        "mov" => Some((MediaKind::Video, "video/quicktime")),
        "webm" => Some((MediaKind::Video, "video/webm")),
        "mkv" => Some((MediaKind::Video, "video/x-matroska")),
        _ => None,
    }
}

/// Classify a MIME type or Slack filetype string (`audio/*`, `video/*`, `mp3`, `mp4`, etc.)
/// (`F-COLLAB-141`).
pub(crate) fn classify_media_mime(mime: &str) -> Option<(MediaKind, String)> {
    let lower = mime.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if lower.starts_with("audio/") {
        return Some((MediaKind::Audio, lower));
    }
    if lower.starts_with("video/") {
        return Some((MediaKind::Video, lower));
    }
    match lower.as_str() {
        "mp3" | "m4a" | "wav" | "ogg" | "aac" | "flac" | "opus" | "slack_audio" => {
            let fake = format!("clip.{lower}");
            let mime_str = classify_media_url(&fake)
                .map(|(_, m)| m.to_owned())
                .unwrap_or_else(|| "audio/mpeg".to_owned());
            Some((MediaKind::Audio, mime_str))
        }
        "mp4" | "mov" | "webm" | "m4v" | "mkv" | "slack_video" => {
            let fake = format!("clip.{lower}");
            let mime_str = classify_media_url(&fake)
                .map(|(_, m)| m.to_owned())
                .unwrap_or_else(|| "video/mp4".to_owned());
            Some((MediaKind::Video, mime_str))
        }
        _ => None,
    }
}

/// Short uppercase format pill label (e.g. `"MP4"`, `"MP3"`, `"WEBM"`) derived from MIME or URL.
pub(crate) fn media_format_pill(mime: Option<&str>, url: &str) -> Option<String> {
    if let Some(m) = mime {
        let lower = m.trim().to_ascii_lowercase();
        let sub = lower.split('/').nth(1).unwrap_or(lower.as_str());
        let clean = sub.split(';').next().unwrap_or(sub).trim();
        let pill = match clean {
            "mpeg" => "MP3",
            "quicktime" => "MOV",
            "x-matroska" => "MKV",
            "x-wav" | "wave" => "WAV",
            "" => "",
            other => return Some(other.to_ascii_uppercase()),
        };
        if !pill.is_empty() {
            return Some(pill.to_owned());
        }
    }
    let without_frag = url.split('#').next().unwrap_or(url);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let segment = without_query.rsplit('/').next().unwrap_or(without_query);
    let dot = segment.rfind('.')?;
    let ext = &segment[dot + 1..];
    if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(ext.to_ascii_uppercase())
    } else {
        None
    }
}

/// Whether `s` looks like a Slack user ID (`U...` or `W...`, uppercase ASCII alphanumeric).
fn is_slack_user_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && matches!(bytes[0], b'U' | b'W')
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Whether `s` looks like a Slack file/canvas/list ID (`F...`, uppercase ASCII alphanumeric).
fn is_slack_file_id(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0] == b'F'
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// Parse a Slack profile URL (`https://<ws>.slack.com/team/U...` or `/team/U...`) into its
/// `user_id` (`F-COLLAB-147`).
pub(crate) fn parse_slack_profile_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let without_frag = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let clean = without_query.trim_end_matches('/');
    let marker = "/team/";
    let pos = clean.find(marker)?;
    let after = &clean[pos + marker.len()..];
    let candidate = after.split('/').next()?;
    if is_slack_user_id(candidate) {
        Some(candidate.to_owned())
    } else {
        None
    }
}

/// Parse a Slack Canvas (`/docs/<T>/<F>`) or List (`/lists/<T>/<F>`) URL (`F-COLLAB-148`).
pub(crate) fn parse_slack_doc_url(url: &str) -> Option<(CanvasEmbedKind, String)> {
    let trimmed = url.trim();
    let without_frag = trimmed.split('#').next().unwrap_or(trimmed);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let clean = without_query.trim_end_matches('/');

    for (marker, kind) in [
        ("/docs/", CanvasEmbedKind::Canvas),
        ("/lists/", CanvasEmbedKind::List),
    ] {
        if let Some(pos) = clean.find(marker) {
            let after = &clean[pos + marker.len()..];
            let mut parts = after.split('/').filter(|s| !s.is_empty());
            let first = parts.next()?;
            let second = parts.next();
            let candidate = match second {
                Some(fid) if is_slack_file_id(fid) => fid,
                None if is_slack_file_id(first) => first,
                _ => continue,
            };
            return Some((kind, candidate.to_owned()));
        }
    }
    None
}

fn strip_slack_file_prefix(raw: &str) -> String {
    raw.trim()
        .strip_prefix("sf:")
        .unwrap_or(raw.trim())
        .to_owned()
}

fn filename_from_url(url: &str) -> Option<String> {
    let without_frag = url.split('#').next().unwrap_or(url);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let seg = without_query.trim_end_matches('/').rsplit('/').next()?;
    if seg.is_empty() {
        None
    } else {
        Some(seg.to_owned())
    }
}

/// Lightweight parser converting Slack's Quip/Canvas HTML export into structured [`CanvasBlock`]s.
#[cfg(test)]
pub fn parse_canvas_html(html: &str) -> Vec<CanvasBlock> {
    parse_canvas_html_with_ids(html).0
}

/// Like [`parse_canvas_html`], also returning each block's Quip `id` attribute (`temp:C:...`)
/// when present so `canvases.edit` can target individual sections.
pub fn parse_canvas_html_with_ids(html: &str) -> (Vec<CanvasBlock>, Vec<Option<String>>) {
    let body = extract_tag_inner(html, "body").unwrap_or(html);
    let mut blocks = Vec::new();
    let mut ids = Vec::new();
    let mut rest = body;
    while let Some(open_pos) = rest.find('<') {
        let before = clean_inline_html(&rest[..open_pos]);
        if !before.is_empty() {
            blocks.push(CanvasBlock::Paragraph(before));
            ids.push(None);
        }
        rest = &rest[open_pos..];
        let Some(gt) = rest.find('>') else {
            break;
        };
        let raw_tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let trimmed = raw_tag.trim();
        if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.starts_with('!') {
            continue;
        }
        let tag_name = trimmed
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let section_id = extract_attr(trimmed, "id").filter(|s| !s.is_empty());
        if tag_name == "hr" {
            blocks.push(CanvasBlock::Divider);
            ids.push(section_id);
            continue;
        }
        if tag_name == "img" {
            if let Some(emoji_glyph) = extract_emoji_from_img_tag(trimmed) {
                blocks.push(CanvasBlock::Paragraph(emoji_glyph));
                ids.push(section_id);
                continue;
            }
            if let Some(src) = extract_attr(trimmed, "src")
                && (src.starts_with("https://") || src.starts_with("http://"))
            {
                let alt = extract_attr(trimmed, "alt").unwrap_or_default();
                let width = extract_attr(trimmed, "width")
                    .and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
                let height = extract_attr(trimmed, "height")
                    .and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
                blocks.push(CanvasBlock::Image {
                    url: decode_entities(&src),
                    alt: resolve_canvas_emoji(&clean_text(&alt)),
                    width,
                    height,
                });
                ids.push(section_id);
            }
            continue;
        }
        if matches!(tag_name.as_str(), "audio" | "video") {
            let kind = if tag_name == "audio" {
                MediaKind::Audio
            } else {
                MediaKind::Video
            };
            let inner = if trimmed.ends_with('/') {
                ""
            } else {
                let close = format!("</{tag_name}>");
                if let Some(end) = find_case_insensitive(rest, &close) {
                    let slice = &rest[..end];
                    rest = &rest[end + close.len()..];
                    slice
                } else {
                    ""
                }
            };
            if let Some(block) = parse_html_media_tag(kind, trimmed, inner) {
                blocks.push(block);
                ids.push(section_id);
            }
            continue;
        }
        if tag_name == "control" {
            if let Some(end) = find_case_insensitive(rest, "</control>") {
                let inner = &rest[..end];
                if let Some(card) = parse_control_profile_card(inner) {
                    rest = &rest[end + "</control>".len()..];
                    blocks.push(card);
                    ids.push(section_id);
                    continue;
                }
                if let Some(emoji) = extract_control_emoji(inner) {
                    rest = &rest[end + "</control>".len()..];
                    blocks.push(CanvasBlock::Paragraph(emoji));
                    ids.push(section_id);
                    continue;
                }
            }
            continue;
        }
        if matches!(tag_name.as_str(), "div" | "section") {
            let is_profile_card = has_profile_card_marker(trimmed);
            let is_doc_embed = has_doc_embed_marker(trimmed);
            let is_media_container = has_embedded_file_marker(trimmed);
            if (is_profile_card || is_doc_embed || is_media_container)
                && let Some((inner, next_rest)) = take_balanced_inner(rest, &tag_name)
            {
                let parsed = if is_profile_card {
                    parse_html_profile_card(trimmed, inner)
                } else if is_doc_embed {
                    parse_html_doc_embed(trimmed, inner)
                } else {
                    parse_html_embedded_file_media(trimmed, inner)
                        .or_else(|| parse_html_doc_embed(trimmed, inner))
                };
                if let Some(block) = parsed {
                    rest = next_rest;
                    blocks.push(block);
                    ids.push(section_id);
                    continue;
                }
            }
            continue;
        }
        if matches!(tag_name.as_str(), "style" | "script" | "head") {
            let close = format!("</{tag_name}>");
            if let Some(end) = find_case_insensitive(rest, &close) {
                rest = &rest[end + close.len()..];
            }
            continue;
        }
        if matches!(
            tag_name.as_str(),
            "h1" | "h2" | "h3" | "h4" | "p" | "pre" | "blockquote" | "ul" | "ol" | "table"
        ) {
            let close = format!("</{tag_name}>");
            let Some(end) = find_case_insensitive(rest, &close) else {
                continue;
            };
            let inner = &rest[..end];
            rest = &rest[end + close.len()..];
            let prev_len = blocks.len();
            match tag_name.as_str() {
                "h1" | "h2" | "h3" | "h4" => {
                    let text = clean_inline_html(inner);
                    if !text.is_empty() {
                        let level = match tag_name.as_str() {
                            "h1" => 1,
                            "h2" => 2,
                            _ => 3,
                        };
                        blocks.push(CanvasBlock::Heading { level, text });
                    }
                }
                "p" => {
                    if let Some(embed_block) = parse_html_paragraph_embed(trimmed, inner) {
                        blocks.push(embed_block);
                    } else {
                        let text = clean_inline_html(inner);
                        if !text.is_empty() {
                            blocks.push(CanvasBlock::Paragraph(text));
                        }
                    }
                }
                "pre" => {
                    let code = decode_entities(&strip_tags(inner)).trim().to_owned();
                    if !code.is_empty() {
                        blocks.push(CanvasBlock::CodeBlock(code));
                    }
                }
                "blockquote" => {
                    let text = clean_multiline_html(inner);
                    if !text.is_empty() {
                        blocks.push(CanvasBlock::Quote(text));
                    }
                }
                "ul" | "ol" => {
                    let section_style = extract_attr(trimmed, "data-section-style");
                    let is_checklist = trimmed.contains("checklist")
                        || section_style.as_deref() == Some("7")
                        || inner.contains("data-checked")
                        || inner.contains("type=\"checkbox\"");
                    let is_numbered = !is_checklist
                        && (tag_name == "ol" || section_style.as_deref() == Some("6"));
                    let items = extract_list_items(inner);
                    if !items.is_empty() {
                        if is_checklist {
                            blocks.push(CanvasBlock::Checklist(items));
                        } else if is_numbered {
                            blocks.push(CanvasBlock::NumberedList(
                                items.into_iter().map(|(_, t)| t).collect(),
                            ));
                        } else {
                            blocks.push(CanvasBlock::Bullets(
                                items.into_iter().map(|(_, t)| t).collect(),
                            ));
                        }
                    }
                }
                "table" => {
                    if let Some((header, rows)) = parse_html_table(inner) {
                        blocks.push(CanvasBlock::Table { header, rows });
                    }
                }
                _ => {}
            }
            if blocks.len() > prev_len {
                ids.push(section_id);
            }
        }
    }
    let tail = clean_inline_html(rest);
    if !tail.is_empty() {
        blocks.push(CanvasBlock::Paragraph(tail));
        ids.push(None);
    }
    (blocks, ids)
}

/// Lightweight Slack canvas markdown parser (headings, bullets, numbered lists, checklists,
/// code fences, multi-line blockquotes, horizontal rules, pipe tables, images, paragraphs).
pub fn parse_canvas_markdown(markdown: &str) -> Vec<CanvasBlock> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = markdown.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }
        if trimmed.starts_with("```") {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim().starts_with("```") {
                code.push(lines[i]);
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            blocks.push(CanvasBlock::CodeBlock(code.join("\n")));
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            blocks.push(CanvasBlock::Divider);
            i += 1;
            continue;
        }
        if let Some((alt, url)) = parse_markdown_image_line(trimmed) {
            blocks.push(CanvasBlock::Image {
                url,
                alt,
                width: None,
                height: None,
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            blocks.push(CanvasBlock::Heading {
                level: 3,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            blocks.push(CanvasBlock::Heading {
                level: 2,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            blocks.push(CanvasBlock::Heading {
                level: 1,
                text: rest.trim().to_owned(),
            });
            i += 1;
            continue;
        }
        if trimmed == ">" || trimmed.starts_with("> ") {
            let mut quote_lines = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = t.strip_prefix("> ") {
                    quote_lines.push(rest.trim_end().to_owned());
                    i += 1;
                } else if t == ">" {
                    quote_lines.push(String::new());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Quote(quote_lines.join("\n")));
            continue;
        }
        if trimmed.starts_with("- [ ] ")
            || trimmed.starts_with("- [x] ")
            || trimmed.starts_with("- [X] ")
        {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = t.strip_prefix("- [ ] ") {
                    items.push((false, rest.trim().to_owned()));
                    i += 1;
                } else if let Some(rest) = t
                    .strip_prefix("- [x] ")
                    .or_else(|| t.strip_prefix("- [X] "))
                {
                    items.push((true, rest.trim().to_owned()));
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Checklist(items));
            continue;
        }
        if strip_numbered_prefix(trimmed).is_some() {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = strip_numbered_prefix(t) {
                    items.push(rest.trim().to_owned());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::NumberedList(items));
            continue;
        }
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let mut items = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if let Some(rest) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                    items.push(rest.trim().to_owned());
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Bullets(items));
            continue;
        }
        if is_markdown_table_start(&lines, i) {
            let header = split_markdown_table_row(trimmed);
            let col_count = header.len().max(1);
            i += 1;
            if i < lines.len() && is_markdown_table_separator(lines[i].trim()) {
                i += 1;
            }
            let mut rows = Vec::new();
            while i < lines.len() {
                let t = lines[i].trim();
                if is_markdown_table_row(t) && !is_markdown_table_separator(t) {
                    let mut cells = split_markdown_table_row(t);
                    cells.resize(col_count, String::new());
                    cells.truncate(col_count);
                    rows.push(cells);
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(CanvasBlock::Table { header, rows });
            continue;
        }
        if let Some(embed_block) = parse_markdown_embed_line(trimmed) {
            blocks.push(embed_block);
            i += 1;
            continue;
        }
        blocks.push(CanvasBlock::Paragraph(trimmed.to_owned()));
        i += 1;
    }
    blocks
}

/// Strip a numbered list prefix like `"1. item"` or `"12) item"`.
pub(crate) fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let digits_len = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let after_digits = &line[digits_len..];
    after_digits
        .strip_prefix(". ")
        .or_else(|| after_digits.strip_prefix(") "))
}

fn parse_markdown_image_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix("![")?;
    let close_bracket = rest.find("](")?;
    let alt = &rest[..close_bracket];
    let after = &rest[close_bracket + 2..];
    let url = after.strip_suffix(')')?;
    if url.is_empty() || url.contains(')') {
        return None;
    }
    Some((alt.to_owned(), url.to_owned()))
}

fn parse_markdown_link_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('[')?;
    let close_bracket = rest.find("](")?;
    let label = &rest[..close_bracket];
    let after = &rest[close_bracket + 2..];
    let url = after.strip_suffix(')')?;
    if url.is_empty() || url.contains(')') || url.contains(char::is_whitespace) {
        return None;
    }
    Some((label.trim().to_owned(), url.trim().to_owned()))
}

fn parse_standalone_user_mention(line: &str) -> Option<(String, Option<String>)> {
    let inner = line.strip_prefix("<@")?.strip_suffix('>')?;
    let mut parts = inner.splitn(2, '|');
    let uid = parts.next()?.trim();
    if !is_slack_user_id(uid) {
        return None;
    }
    let handle = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some((uid.to_owned(), handle))
}

/// Classify a standalone markdown line as a doc embed (`F-COLLAB-148`), profile card
/// (`F-COLLAB-147`), or inline audio/video embed (`F-COLLAB-141`).
fn parse_markdown_embed_line(line: &str) -> Option<CanvasBlock> {
    if let Some((user_id, handle)) = parse_standalone_user_mention(line) {
        return Some(CanvasBlock::ProfileCard {
            user_id,
            display_name: None,
            title: None,
            handle,
        });
    }

    let (label, url) = if let Some((l, u)) = parse_markdown_link_line(line) {
        (Some(l), u)
    } else if (line.starts_with("https://") || line.starts_with("http://"))
        && !line.contains(char::is_whitespace)
    {
        (None, line.to_owned())
    } else {
        return None;
    };

    if let Some((kind, file_id)) = parse_slack_doc_url(&url) {
        let title = label
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{} {file_id}", kind.label()));
        return Some(CanvasBlock::DocEmbed {
            kind,
            file_id,
            title,
            url,
        });
    }

    if let Some(user_id) = parse_slack_profile_url(&url) {
        let display_name = label.filter(|s| !s.is_empty() && s != &user_id && s != &url);
        return Some(CanvasBlock::ProfileCard {
            user_id,
            display_name,
            title: None,
            handle: None,
        });
    }

    if let Some((kind, mime)) =
        classify_media_url(&url).or_else(|| label.as_deref().and_then(classify_media_url))
    {
        let title = label
            .filter(|s| !s.is_empty())
            .or_else(|| filename_from_url(&url))
            .unwrap_or_else(|| format!("{} clip", kind.label()));
        return Some(CanvasBlock::MediaEmbed {
            kind,
            title,
            url,
            file_id: None,
            duration: None,
            mime: Some(mime.to_owned()),
        });
    }

    None
}

fn has_profile_card_marker(open_tag: &str) -> bool {
    let lower = open_tag.to_ascii_lowercase();
    lower.contains("user_unfurl")
        || lower.contains("user-unfurl")
        || lower.contains("profile-card")
        || lower.contains("profile_card")
        || extract_attr(open_tag, "data-user-id").is_some_and(|id| is_slack_user_id(&id))
}

fn has_doc_embed_marker(open_tag: &str) -> bool {
    let lower = open_tag.to_ascii_lowercase();
    lower.contains("canvas_unfurl")
        || lower.contains("canvas-unfurl")
        || lower.contains("list_unfurl")
        || lower.contains("list-unfurl")
        || lower.contains("embedded-link")
}

fn has_embedded_file_marker(open_tag: &str) -> bool {
    let lower = open_tag.to_ascii_lowercase();
    lower.contains("embedded-file")
        || lower.contains("media-embed")
        || lower.contains("data-slack-file-id")
}

/// Extract the balanced inner HTML for `tag_name` (tracking nested `<tag_name>` opens/closes),
/// returning `(inner, remaining_after_close_tag)`.
fn take_balanced_inner<'a>(rest: &'a str, tag_name: &str) -> Option<(&'a str, &'a str)> {
    let open_needle = format!("<{tag_name}");
    let close_needle = format!("</{tag_name}>");
    let lower = rest.to_ascii_lowercase();
    let mut depth: usize = 1;
    let mut cursor: usize = 0;

    while cursor < lower.len() {
        let next_open = lower[cursor..].find(&open_needle).map(|o| cursor + o);
        let next_close = lower[cursor..].find(&close_needle).map(|c| cursor + c);

        match (next_open, next_close) {
            (_, None) => return None,
            (Some(o), Some(c)) if o < c => {
                let after = lower.as_bytes().get(o + open_needle.len()).copied();
                let is_tag_boundary =
                    after.is_some_and(|b| b.is_ascii_whitespace() || b == b'>' || b == b'/');
                if is_tag_boundary {
                    let gt_rel = lower[o..].find('>')?;
                    let is_self_closing = lower[o..o + gt_rel].trim_end().ends_with('/');
                    if !is_self_closing {
                        depth += 1;
                    }
                    cursor = o + gt_rel + 1;
                } else {
                    cursor = o + open_needle.len();
                }
            }
            (_, Some(c)) => {
                depth -= 1;
                if depth == 0 {
                    return Some((&rest[..c], &rest[c + close_needle.len()..]));
                }
                cursor = c + close_needle.len();
            }
        }
    }
    None
}

/// Find the first opening tag matching `tag_name` inside `html`, returning `(raw_open_tag, inner)`.
fn find_first_tag<'a>(html: &'a str, tag_name: &str) -> Option<(&'a str, &'a str)> {
    let open = format!("<{tag_name}");
    let lower = html.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(rel) = lower[offset..].find(&open) {
        let pos = offset + rel;
        let next_byte = lower.as_bytes().get(pos + open.len()).copied();
        if !next_byte.is_some_and(|b| b.is_ascii_whitespace() || b == b'>' || b == b'/') {
            offset = pos + open.len();
            continue;
        }
        let after = &html[pos..];
        let gt = after.find('>')?;
        let raw = after[1..gt].trim();
        let body = &after[gt + 1..];
        if raw.ends_with('/') {
            return Some((raw, ""));
        }
        let close = format!("</{tag_name}>");
        let end = find_case_insensitive(body, &close).unwrap_or(body.len());
        return Some((raw, &body[..end]));
    }
    None
}

/// Parse `<audio>` or `<video>` tags (including nested `<source src="..." type="...">`) into
/// [`CanvasBlock::MediaEmbed`] (`F-COLLAB-141`).
fn parse_html_media_tag(kind: MediaKind, open_tag: &str, inner: &str) -> Option<CanvasBlock> {
    let source_tag = find_first_tag(inner, "source").map(|(t, _)| t);
    let link_tag = find_first_tag(inner, "a").or_else(|| find_first_tag(inner, "lnk"));

    let raw_url = extract_attr(open_tag, "src")
        .or_else(|| source_tag.and_then(|t| extract_attr(t, "src")))
        .or_else(|| link_tag.and_then(|(t, _)| extract_attr(t, "href")))?;
    let url = decode_entities(&raw_url);
    if url.is_empty() {
        return None;
    }

    let mime = extract_attr(open_tag, "data-mime")
        .or_else(|| extract_attr(open_tag, "type"))
        .or_else(|| source_tag.and_then(|t| extract_attr(t, "type")))
        .or_else(|| classify_media_url(&url).map(|(_, m)| m.to_owned()));
    let duration = extract_attr(open_tag, "data-duration")
        .or_else(|| extract_attr(open_tag, "duration"))
        .filter(|s| !s.is_empty());
    let file_id = extract_attr(open_tag, "data-slack-file-id")
        .or_else(|| extract_attr(open_tag, "data-file-id"))
        .map(|s| strip_slack_file_prefix(&s))
        .filter(|s| !s.is_empty());
    let title = extract_attr(open_tag, "title")
        .or_else(|| extract_attr(open_tag, "data-title"))
        .or_else(|| extract_attr(open_tag, "aria-label"))
        .or_else(|| {
            let txt = clean_inline_html(inner);
            (!txt.is_empty()).then_some(txt)
        })
        .or_else(|| filename_from_url(&url))
        .map(|s| resolve_canvas_emoji(&clean_text(&s)))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{} clip", kind.label()));

    Some(CanvasBlock::MediaEmbed {
        kind,
        title,
        url,
        file_id,
        duration,
        mime,
    })
}

/// Parse Quip `p.embedded-file` or `lnk[data-slack-file-id]` audio/video embeds (`F-COLLAB-141`).
fn parse_html_embedded_file_media(open_tag: &str, inner: &str) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "lnk").or_else(|| find_first_tag(inner, "a"));
    let link_tag = link.map(|(t, _)| t).unwrap_or("");
    let link_text = link
        .map(|(_, body)| clean_inline_html(body))
        .unwrap_or_default();

    let file_id = extract_attr(link_tag, "data-slack-file-id")
        .or_else(|| extract_attr(open_tag, "data-slack-file-id"))
        .or_else(|| extract_attr(link_tag, "data-file-id"))
        .or_else(|| extract_attr(open_tag, "data-file-id"))
        .map(|s| strip_slack_file_prefix(&s))
        .filter(|s| !s.is_empty());
    let url = extract_attr(link_tag, "href")
        .or_else(|| extract_attr(link_tag, "src"))
        .or_else(|| extract_attr(open_tag, "href"))
        .or_else(|| extract_attr(open_tag, "data-url"))
        .map(|u| decode_entities(&u))
        .unwrap_or_default();
    let raw_mime = extract_attr(link_tag, "data-mime")
        .or_else(|| extract_attr(open_tag, "data-mime"))
        .or_else(|| extract_attr(link_tag, "type"))
        .or_else(|| extract_attr(open_tag, "type"))
        .or_else(|| extract_attr(link_tag, "data-filetype"))
        .or_else(|| extract_attr(open_tag, "data-filetype"));
    let duration = extract_attr(link_tag, "data-duration")
        .or_else(|| extract_attr(open_tag, "data-duration"))
        .filter(|s| !s.is_empty());

    let raw_title = extract_attr(link_tag, "data-title")
        .or_else(|| extract_attr(open_tag, "data-title"))
        .or_else(|| extract_attr(link_tag, "title"))
        .or_else(|| (!link_text.is_empty()).then_some(link_text))
        .or_else(|| {
            let txt = clean_inline_html(inner);
            (!txt.is_empty()).then_some(txt)
        })
        .or_else(|| filename_from_url(&url))
        .unwrap_or_default();

    let (kind, mime) = raw_mime
        .as_deref()
        .and_then(classify_media_mime)
        .or_else(|| classify_media_url(&url).map(|(k, m)| (k, m.to_owned())))
        .or_else(|| classify_media_url(&raw_title).map(|(k, m)| (k, m.to_owned())))?;

    let title = if raw_title.is_empty() {
        format!("{} clip", kind.label())
    } else {
        resolve_canvas_emoji(&clean_text(&raw_title))
    };

    Some(CanvasBlock::MediaEmbed {
        kind,
        title,
        url,
        file_id,
        duration,
        mime: Some(mime),
    })
}

/// Extract a Slack `user_id` from a `<control><a ...>@U...</a></control>` block (`F-COLLAB-147`).
fn parse_control_profile_card(control_inner: &str) -> Option<CanvasBlock> {
    let (link_tag, link_body) =
        find_first_tag(control_inner, "a").or_else(|| find_first_tag(control_inner, "lnk"))?;
    let body_text = clean_inline_html(link_body);
    let href = extract_attr(link_tag, "href").map(|h| decode_entities(&h));

    let user_id = extract_attr(link_tag, "data-user-id")
        .filter(|s| is_slack_user_id(s))
        .or_else(|| href.as_deref().and_then(parse_slack_profile_url))
        .or_else(|| {
            let bare = body_text.trim().trim_start_matches('@');
            is_slack_user_id(bare).then(|| bare.to_owned())
        })?;

    let display_name = {
        let trimmed = body_text.trim();
        let bare = trimmed.trim_start_matches('@');
        if trimmed.is_empty() || bare == user_id {
            None
        } else {
            Some(trimmed.to_owned())
        }
    };
    let title = extract_attr(link_tag, "data-title").filter(|s| !s.is_empty());
    let handle = extract_attr(link_tag, "data-handle").filter(|s| !s.is_empty());

    Some(CanvasBlock::ProfileCard {
        user_id,
        display_name,
        title,
        handle,
    })
}

/// Parse an HTML `user_unfurl` / `profile-card` / `data-user-id` container (`F-COLLAB-147`).
fn parse_html_profile_card(open_tag: &str, inner: &str) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "a").or_else(|| find_first_tag(inner, "lnk"));
    let href = extract_attr(open_tag, "href")
        .or_else(|| link.and_then(|(t, _)| extract_attr(t, "href")))
        .map(|h| decode_entities(&h));
    let inner_text = clean_inline_html(inner);

    let user_id = extract_attr(open_tag, "data-user-id")
        .filter(|s| is_slack_user_id(s))
        .or_else(|| {
            link.and_then(|(t, _)| extract_attr(t, "data-user-id"))
                .filter(|s| is_slack_user_id(s))
        })
        .or_else(|| href.as_deref().and_then(parse_slack_profile_url))
        .or_else(|| {
            inner_text.split_whitespace().find_map(|tok| {
                let bare = tok
                    .trim_start_matches('@')
                    .trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                is_slack_user_id(bare).then(|| bare.to_owned())
            })
        })?;

    let title = extract_attr(open_tag, "data-title")
        .or_else(|| extract_attr(open_tag, "data-role"))
        .or_else(|| {
            find_first_tag(inner, "span").and_then(|(t, body)| {
                let lower = t.to_ascii_lowercase();
                if lower.contains("title") || lower.contains("role") {
                    let s = clean_inline_html(body);
                    (!s.is_empty()).then_some(s)
                } else {
                    None
                }
            })
        })
        .filter(|s| !s.is_empty());

    let handle = extract_attr(open_tag, "data-handle")
        .or_else(|| extract_attr(open_tag, "data-username"))
        .map(|h| h.trim_start_matches('@').to_owned())
        .filter(|s| !s.is_empty());

    let display_name = extract_attr(open_tag, "data-display-name")
        .or_else(|| extract_attr(open_tag, "data-name"))
        .or_else(|| extract_attr(open_tag, "data-user-name"))
        .or_else(|| {
            link.and_then(|(_, body)| {
                let t = clean_inline_html(body);
                let bare = t.trim_start_matches('@');
                (!t.is_empty() && bare != user_id).then_some(t)
            })
        })
        .or_else(|| {
            let bare = inner_text.trim().trim_start_matches('@');
            (!inner_text.is_empty()
                && bare != user_id
                && Some(inner_text.as_str()) != title.as_deref())
            .then_some(inner_text)
        });

    Some(CanvasBlock::ProfileCard {
        user_id,
        display_name,
        title,
        handle,
    })
}

/// Parse an HTML `p.embedded-link` / `canvas_unfurl` / list embed container (`F-COLLAB-148`).
fn parse_html_doc_embed(open_tag: &str, inner: &str) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "a").or_else(|| find_first_tag(inner, "lnk"));
    let link_tag = link.map(|(t, _)| t).unwrap_or("");
    let link_text = link
        .map(|(_, body)| clean_inline_html(body))
        .unwrap_or_default();

    let url = extract_attr(link_tag, "href")
        .or_else(|| extract_attr(open_tag, "href"))
        .or_else(|| extract_attr(open_tag, "data-url"))
        .map(|u| decode_entities(&u))
        .unwrap_or_default();

    let from_url = parse_slack_doc_url(&url);
    let lower_tag = open_tag.to_ascii_lowercase();
    let attr_file_id = extract_attr(link_tag, "data-slack-file-id")
        .or_else(|| extract_attr(open_tag, "data-slack-file-id"))
        .or_else(|| extract_attr(link_tag, "data-file-id"))
        .or_else(|| extract_attr(open_tag, "data-file-id"))
        .map(|s| strip_slack_file_prefix(&s))
        .filter(|s| is_slack_file_id(s));

    let (kind, file_id) = match from_url {
        Some((k, fid)) => (k, fid),
        None => {
            let fid = attr_file_id?;
            let is_list = lower_tag.contains("list_unfurl")
                || lower_tag.contains("list-unfurl")
                || extract_attr(open_tag, "data-doc-kind")
                    .is_some_and(|k| k.eq_ignore_ascii_case("list"));
            let is_canvas_marker = lower_tag.contains("canvas_unfurl")
                || lower_tag.contains("canvas-unfurl")
                || extract_attr(open_tag, "data-doc-kind")
                    .is_some_and(|k| k.eq_ignore_ascii_case("canvas"));
            if !is_list && !is_canvas_marker {
                return None;
            }
            (
                if is_list {
                    CanvasEmbedKind::List
                } else {
                    CanvasEmbedKind::Canvas
                },
                fid,
            )
        }
    };

    let title = extract_attr(open_tag, "data-title")
        .or_else(|| extract_attr(link_tag, "data-title"))
        .or_else(|| (!link_text.is_empty() && link_text != url).then_some(link_text))
        .or_else(|| {
            let txt = clean_inline_html(inner);
            (!txt.is_empty() && txt != url).then_some(txt)
        })
        .unwrap_or_else(|| format!("{} {file_id}", kind.label()));

    let final_url = if url.is_empty() {
        let seg = match kind {
            CanvasEmbedKind::Canvas => "docs",
            CanvasEmbedKind::List => "lists",
        };
        format!("https://slack.com/{seg}/T0/{file_id}")
    } else {
        url
    };

    Some(CanvasBlock::DocEmbed {
        kind,
        file_id,
        title,
        url: final_url,
    })
}

/// Check whether a `<p ...>...</p>` block is an embedded media file, profile card, or
/// canvas/list link card (`F-COLLAB-141`, `F-COLLAB-147`, `F-COLLAB-148`).
fn parse_html_paragraph_embed(open_tag: &str, inner: &str) -> Option<CanvasBlock> {
    if has_profile_card_marker(open_tag)
        && let Some(card) = parse_html_profile_card(open_tag, inner)
    {
        return Some(card);
    }

    if has_embedded_file_marker(open_tag)
        || inner.to_ascii_lowercase().contains("data-slack-file-id")
    {
        if let Some(media) = parse_html_embedded_file_media(open_tag, inner) {
            return Some(media);
        }
        if let Some(doc) = parse_html_doc_embed(open_tag, inner) {
            return Some(doc);
        }
    }

    if has_doc_embed_marker(open_tag)
        && let Some(doc) = parse_html_doc_embed(open_tag, inner)
    {
        return Some(doc);
    }

    // Standalone `<p><control><a ...>@U...</a></control></p>` (no surrounding prose).
    if let Some((control_inner, remaining_text)) = extract_sole_tag(inner, "control")
        && remaining_text.is_empty()
        && let Some(card) = parse_control_profile_card(control_inner)
    {
        return Some(card);
    }

    // Standalone `<p><a href="...">...</a></p>` or `<p><lnk href="...">...</lnk></p>`.
    let sole_link = extract_sole_tag_with_attrs(inner, "a")
        .or_else(|| extract_sole_tag_with_attrs(inner, "lnk"));
    if let Some((link_tag, link_body, remaining_text)) = sole_link
        && remaining_text.is_empty()
        && let Some(raw_href) = extract_attr(link_tag, "href")
    {
        let href = decode_entities(&raw_href);
        let label = clean_inline_html(link_body);
        if let Some((kind, file_id)) = parse_slack_doc_url(&href) {
            let title = if !label.is_empty() && label != href {
                label
            } else {
                format!("{} {file_id}", kind.label())
            };
            return Some(CanvasBlock::DocEmbed {
                kind,
                file_id,
                title,
                url: href,
            });
        }
        if let Some(user_id) = parse_slack_profile_url(&href) {
            let display_name = {
                let bare = label.trim_start_matches('@');
                (!label.is_empty() && bare != user_id && label != href).then_some(label)
            };
            return Some(CanvasBlock::ProfileCard {
                user_id,
                display_name,
                title: extract_attr(link_tag, "data-title").filter(|s| !s.is_empty()),
                handle: extract_attr(link_tag, "data-handle").filter(|s| !s.is_empty()),
            });
        }
        if let Some((kind, mime)) = classify_media_url(&href).or_else(|| classify_media_url(&label))
        {
            let title = if !label.is_empty() {
                label
            } else {
                filename_from_url(&href).unwrap_or_else(|| format!("{} clip", kind.label()))
            };
            let file_id = extract_attr(link_tag, "data-slack-file-id")
                .map(|s| strip_slack_file_prefix(&s))
                .filter(|s| !s.is_empty());
            let duration = extract_attr(link_tag, "data-duration").filter(|s| !s.is_empty());
            return Some(CanvasBlock::MediaEmbed {
                kind,
                title,
                url: href,
                file_id,
                duration,
                mime: Some(mime.to_owned()),
            });
        }
    }

    None
}

/// If `html` contains a `<tag>...</tag>`, return `(inner, cleaned_text_outside_tag)`.
fn extract_sole_tag<'a>(html: &'a str, tag: &str) -> Option<(&'a str, String)> {
    let (_, inner, outside) = extract_sole_tag_with_attrs(html, tag)?;
    Some((inner, outside))
}

fn extract_sole_tag_with_attrs<'a>(html: &'a str, tag: &str) -> Option<(&'a str, &'a str, String)> {
    let open = format!("<{tag}");
    let pos = find_case_insensitive(html, &open)?;
    let before = &html[..pos];
    let after_start = &html[pos..];
    let gt = after_start.find('>')?;
    let raw_tag = after_start[1..gt].trim();
    let after_open = &after_start[gt + 1..];
    let close = format!("</{tag}>");
    let end = find_case_insensitive(after_open, &close)?;
    let inner = &after_open[..end];
    let after = &after_open[end + close.len()..];
    let mut outside = clean_inline_html(before);
    outside.push_str(&clean_inline_html(after));
    Some((raw_tag, inner, outside))
}

fn is_markdown_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.contains('|')
}

pub(crate) fn is_markdown_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') && !trimmed.contains('-') {
        return false;
    }
    let inner = trimmed.trim_matches('|');
    if inner.is_empty() {
        return false;
    }
    let parts: Vec<&str> = inner.split('|').collect();
    !parts.is_empty()
        && parts.iter().all(|p| {
            let s = p.trim();
            !s.is_empty() && s.contains('-') && s.chars().all(|c| matches!(c, '-' | ':'))
        })
}

fn is_markdown_table_start(lines: &[&str], ix: usize) -> bool {
    let line = lines[ix].trim();
    if line.starts_with('|') && line.ends_with('|') {
        return true;
    }
    if line.contains('|') && ix + 1 < lines.len() {
        return is_markdown_table_separator(lines[ix + 1].trim());
    }
    false
}

/// Split a markdown pipe-table row, honoring escaped `\|` inside cells.
pub(crate) fn split_markdown_table_row(row: &str) -> Vec<String> {
    let trimmed = row.trim();
    let trimmed = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let trimmed = if trimmed.ends_with('|') && !trimmed.ends_with("\\|") {
        &trimmed[..trimmed.len() - 1]
    } else {
        trimmed
    };
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut chars = trimmed.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'|') {
            let _ = chars.next();
            current.push('|');
        } else if ch == '|' {
            cells.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    cells.push(current.trim().to_owned());
    cells
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{attr}=");
    let pos = lower.find(&needle)?;
    let rest = tag[pos + needle.len()..].trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"')?;
        Some(quoted[..end].to_owned())
    } else if let Some(quoted) = rest.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        Some(quoted[..end].to_owned())
    } else {
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .unwrap_or(rest.len());
        Some(rest[..end].to_owned())
    }
}

fn extract_tag_inner<'a>(html: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = find_case_insensitive(html, &open)?;
    let after_open = &html[start..];
    let gt = after_open.find('>')?;
    let content = &after_open[gt + 1..];
    let close = format!("</{tag}>");
    let end = find_case_insensitive(content, &close)?;
    Some(&content[..end])
}

/// If an `<img>` tag represents a Quip/Slack inline emoji (`alt=":rocket:"`, `data-emoji="rocket"`,
/// or `class="...emoji..."` with `alt="rocket"`), resolve it to a Unicode emoji glyph.
fn extract_emoji_from_img_tag(raw_tag: &str) -> Option<String> {
    if let Some(code) = extract_attr(raw_tag, "data-emoji") {
        let bare = code.trim_matches(':');
        return crate::emoji::display_glyph(bare).or_else(|| Some(format!(":{bare}:")));
    }
    let alt = extract_attr(raw_tag, "alt")?;
    let trimmed = alt.trim();
    if trimmed.starts_with(':') && trimmed.ends_with(':') && trimmed.len() > 2 {
        let bare = &trimmed[1..trimmed.len() - 1];
        return crate::emoji::display_glyph(bare).or_else(|| Some(trimmed.to_owned()));
    }
    let lower_tag = raw_tag.to_ascii_lowercase();
    let looks_like_emoji = lower_tag.contains("emoji")
        || extract_attr(raw_tag, "src")
            .map(|s| s.to_ascii_lowercase().contains("emoji"))
            .unwrap_or(false);
    if looks_like_emoji && !trimmed.is_empty() {
        let bare = trimmed.trim_matches(':');
        if let Some(glyph) = crate::emoji::display_glyph(bare) {
            return Some(glyph);
        }
    }
    None
}

/// Expand Quip `<control><img alt="name" ...></control>` and inline emoji `<img>` tags to text
/// before stripping remaining HTML tags (`F-COLLAB-139`).
fn expand_inline_emoji_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open_pos) = rest.find('<') {
        out.push_str(&rest[..open_pos]);
        rest = &rest[open_pos..];
        let Some(gt) = rest.find('>') else {
            out.push_str(rest);
            return out;
        };
        let raw_tag = &rest[1..gt];
        let trimmed = raw_tag.trim();
        let tag_name = trimmed
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if tag_name == "control" {
            let after_open = &rest[gt + 1..];
            if let Some(end) = find_case_insensitive(after_open, "</control>") {
                let inner = &after_open[..end];
                if let Some(emoji) = extract_control_emoji(inner) {
                    out.push_str(&emoji);
                    rest = &after_open[end + "</control>".len()..];
                    continue;
                }
                if let Some(mention_token) = extract_control_inline_mention(inner) {
                    out.push_str(&mention_token);
                    rest = &after_open[end + "</control>".len()..];
                    continue;
                }
            }
        }
        if tag_name == "img"
            && let Some(emoji) = extract_emoji_from_img_tag(trimmed)
        {
            out.push_str(&emoji);
            rest = &rest[gt + 1..];
            continue;
        }
        out.push_str(&rest[..=gt]);
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out
}

/// Extract emoji from inside a Quip `<control>...</control>` element (which commonly wraps
/// `<img alt="rocket" ...>` even without colons or `class="emoji"`).
fn extract_control_emoji(inner: &str) -> Option<String> {
    let img_pos = find_case_insensitive(inner, "<img")?;
    let after_img = &inner[img_pos + 1..];
    let gt = after_img.find('>')?;
    let raw_img = &after_img[..gt];
    if let Some(glyph) = extract_emoji_from_img_tag(raw_img) {
        return Some(glyph);
    }
    let alt = extract_attr(raw_img, "alt")?;
    let bare = alt.trim().trim_matches(':');
    if bare.is_empty() {
        return None;
    }
    crate::emoji::display_glyph(bare).or_else(|| {
        if !bare.is_ascii() {
            Some(bare.to_owned())
        } else {
            None
        }
    })
}

fn clean_inline_html(inner: &str) -> String {
    let expanded = expand_inline_emoji_tags(inner);
    resolve_canvas_emoji(&clean_text(&strip_tags(&expanded)))
}

/// Clean HTML while preserving `<br>` and paragraph boundaries as `\n` (for multi-line quotes).
fn clean_multiline_html(inner: &str) -> String {
    let expanded = expand_inline_emoji_tags(inner);
    let with_breaks = replace_line_break_tags(&expanded);
    let stripped = strip_tags(&with_breaks);
    let decoded = decode_entities(&stripped);
    decoded
        .lines()
        .map(|l| {
            let collapsed = l.split_whitespace().collect::<Vec<_>>().join(" ");
            resolve_canvas_emoji(&collapsed)
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn replace_line_break_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open_pos) = rest.find('<') {
        out.push_str(&rest[..open_pos]);
        rest = &rest[open_pos..];
        let Some(gt) = rest.find('>') else {
            out.push_str(rest);
            return out;
        };
        let raw = rest[1..gt].trim().to_ascii_lowercase();
        let name = raw
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        if name == "br" || raw.starts_with("/p") || raw.starts_with("/div") {
            out.push('\n');
        } else {
            out.push_str(&rest[..=gt]);
        }
        rest = &rest[gt + 1..];
    }
    out.push_str(rest);
    out
}

fn extract_list_items(inner: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut rest = inner;
    while let Some(pos) = find_case_insensitive(rest, "<li") {
        rest = &rest[pos..];
        let Some(gt) = rest.find('>') else { break };
        let open_tag = &rest[..gt];
        rest = &rest[gt + 1..];
        let end = find_case_insensitive(rest, "</li>").unwrap_or(rest.len());
        let body = &rest[..end];
        let checked = open_tag.contains("data-checked=\"true\"")
            || open_tag.contains("class=\"checked\"")
            || body.contains("checked");
        let text = clean_inline_html(body);
        if !text.is_empty() {
            out.push((checked, text));
        }
        rest = if end + 5 <= rest.len() {
            &rest[end + 5..]
        } else {
            ""
        };
    }
    out
}

fn parse_html_table(inner: &str) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut rest = inner;
    while let Some(pos) = find_case_insensitive(rest, "<tr") {
        rest = &rest[pos..];
        let Some(gt) = rest.find('>') else { break };
        rest = &rest[gt + 1..];
        let end = find_case_insensitive(rest, "</tr>").unwrap_or(rest.len());
        let tr = &rest[..end];
        let mut cells = Vec::new();
        let mut cell_rest = tr;
        while let Some(cpos) = cell_rest.find('<') {
            cell_rest = &cell_rest[cpos..];
            let Some(cgt) = cell_rest.find('>') else {
                break;
            };
            let tag = cell_rest[1..cgt]
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            cell_rest = &cell_rest[cgt + 1..];
            if tag == "th" || tag == "td" {
                let close = format!("</{tag}>");
                let cend = find_case_insensitive(cell_rest, &close).unwrap_or(cell_rest.len());
                cells.push(clean_inline_html(&cell_rest[..cend]));
                cell_rest = if cend + close.len() <= cell_rest.len() {
                    &cell_rest[cend + close.len()..]
                } else {
                    ""
                };
            }
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
        rest = if end + 5 <= rest.len() {
            &rest[end + 5..]
        } else {
            ""
        };
    }
    if rows.is_empty() {
        return None;
    }
    let col_count = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
    for row in &mut rows {
        row.resize(col_count, String::new());
    }
    let header = rows.remove(0);
    Some((header, rows))
}

/// Extract an inline Slack `@user` or `#channel` mention token from a Quip `<control>` element
/// (`F-COLLAB-150`) so `strip_tags` preserves it as `<@U...>` / `<#C...|name>`.
fn extract_control_inline_mention(inner: &str) -> Option<String> {
    let a_pos = find_case_insensitive(inner, "<a")?;
    let a_rest = &inner[a_pos..];
    let gt = a_rest.find('>')?;
    let raw_a = &a_rest[1..gt];
    let after_a = &a_rest[gt + 1..];
    let a_end = find_case_insensitive(after_a, "</a>")?;
    let anchor_text = clean_text(&strip_tags(&after_a[..a_end]));

    if let Some(uid) = anchor_text.strip_prefix('@')
        && is_slack_user_id(uid.trim())
    {
        return Some(format!("<@{}>", uid.trim()));
    }
    if let Some(cid) = anchor_text.strip_prefix('#') {
        let candidate = cid.trim();
        let bytes = candidate.as_bytes();
        if bytes.len() >= 3
            && matches!(bytes[0], b'C' | b'G')
            && bytes
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return Some(format!("<#{candidate}>"));
        }
    }
    if let Some(href) = extract_attr(raw_a, "href") {
        let decoded = decode_entities(&href);
        if let Some(uid) = parse_slack_profile_url(&decoded) {
            let label = anchor_text.trim_start_matches('@').trim();
            if !label.is_empty() && label != uid {
                return Some(format!("<@{uid}|{label}>"));
            }
            return Some(format!("<@{uid}>"));
        }
        if let Some(pos) = decoded.find("/archives/") {
            let tail = &decoded[pos + "/archives/".len()..];
            let cid: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            let bytes = cid.as_bytes();
            if bytes.len() >= 3
                && matches!(bytes[0], b'C' | b'G')
                && bytes
                    .iter()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
            {
                let name = anchor_text.trim_start_matches('#').trim();
                if !name.is_empty() && name != cid {
                    return Some(format!("<#{cid}|{name}>"));
                }
                return Some(format!("<#{cid}>"));
            }
        }
    }
    None
}

fn is_preserved_slack_mention_token(inner: &str) -> bool {
    if let Some(body) = inner.strip_prefix('@') {
        let uid = body.split('|').next().unwrap_or(body).trim();
        return is_slack_user_id(uid);
    }
    if let Some(body) = inner.strip_prefix('#') {
        let cid = body.split('|').next().unwrap_or(body).trim();
        let bytes = cid.as_bytes();
        return bytes.len() >= 3
            && matches!(bytes[0], b'C' | b'G')
            && bytes
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    }
    if let Some(body) = inner.strip_prefix("!subteam^") {
        let gid = body.split('|').next().unwrap_or(body).trim();
        let bytes = gid.as_bytes();
        return bytes.len() >= 3
            && bytes[0] == b'S'
            && bytes
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    }
    if let Some(body) = inner.strip_prefix('!') {
        let kw = body.split('|').next().unwrap_or(body).trim();
        return matches!(kw, "here" | "channel" | "everyone");
    }
    false
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let after = &rest[lt + 1..];
        let Some(gt) = after.find('>') else {
            break;
        };
        let inside = &after[..gt];
        if is_preserved_slack_mention_token(inside) {
            out.push('<');
            out.push_str(inside);
            out.push('>');
        }
        rest = &after[gt + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

fn clean_text(s: &str) -> String {
    decode_entities(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
