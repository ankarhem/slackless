//! Parsers for Slack Quip/Canvas HTML exports and Canvas markdown, including emoji resolution
//! (`F-COLLAB-134`, `F-COLLAB-135`, `F-COLLAB-136`, `F-COLLAB-139`).

use crate::canvas::model::CanvasBlock;

/// Resolve `:shortcode:` tokens inside canvas text into Unicode emoji glyphs (`F-COLLAB-139`).
pub(crate) fn resolve_canvas_emoji(text: &str) -> String {
    crate::emoji::replace_shortcodes(text)
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
                    let text = clean_inline_html(inner);
                    if !text.is_empty() {
                        blocks.push(CanvasBlock::Paragraph(text));
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

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
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
