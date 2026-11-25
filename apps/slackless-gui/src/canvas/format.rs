//! Pure canvas text formatting, selection wrap/unwrap, table editing, slash command detection,
//! emoji quick-insert, and file-upload markdown helpers (`F-COLLAB-133..136`, `F-COLLAB-139..140`,
//! `F-COLLAB-143`).

use std::ops::Range;

use crate::canvas::model::serialize_markdown_table;
use crate::canvas::parse::{
    is_markdown_table_separator, split_markdown_table_row, strip_numbered_prefix,
};

/// Inline markdown formatting actions for the canvas selection bar (`F-COLLAB-133`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineFormat {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
}

/// Block / line-level formatting actions for the canvas toolbar (`F-COLLAB-133..136`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockFormat {
    CycleHeading,
    Quote,
    Bullet,
    Numbered,
    Checklist,
    Table,
    CodeBlock,
    Divider,
}

/// Slash command entries available via `/` at line start or the `+ Insert` palette (`F-COLLAB-143`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlashCommand {
    Heading1,
    Heading2,
    Heading3,
    BulletList,
    NumberedList,
    Checklist,
    Quote,
    CodeBlock,
    Table,
    Divider,
    Emoji,
    File,
}

/// Display metadata for one slash-command entry.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SlashCommandSpec {
    pub(crate) cmd: SlashCommand,
    pub(crate) trigger: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
}

pub(crate) const SLASH_COMMANDS: &[SlashCommandSpec] = &[
    SlashCommandSpec {
        cmd: SlashCommand::Heading1,
        trigger: "h1",
        label: "Heading 1",
        description: "Large section heading (#)",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Heading2,
        trigger: "h2",
        label: "Heading 2",
        description: "Medium section heading (##)",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Heading3,
        trigger: "h3",
        label: "Heading 3",
        description: "Small section heading (###)",
    },
    SlashCommandSpec {
        cmd: SlashCommand::BulletList,
        trigger: "bullet",
        label: "Bullet list",
        description: "Unordered list (- )",
    },
    SlashCommandSpec {
        cmd: SlashCommand::NumberedList,
        trigger: "numbered",
        label: "Numbered list",
        description: "Ordered list (1. )",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Checklist,
        trigger: "checklist",
        label: "Checklist",
        description: "Interactive task checklist (- [ ] )",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Quote,
        trigger: "quote",
        label: "Quote",
        description: "Blockquote callout (> )",
    },
    SlashCommandSpec {
        cmd: SlashCommand::CodeBlock,
        trigger: "code",
        label: "Code block",
        description: "Fenced code snippet (```)",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Table,
        trigger: "table",
        label: "Table",
        description: "3-column markdown table",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Divider,
        trigger: "divider",
        label: "Divider",
        description: "Horizontal rule (---)",
    },
    SlashCommandSpec {
        cmd: SlashCommand::Emoji,
        trigger: "emoji",
        label: "Emoji",
        description: "Open quick emoji picker",
    },
    SlashCommandSpec {
        cmd: SlashCommand::File,
        trigger: "file",
        label: "Upload file",
        description: "Attach or embed a file/image",
    },
];

/// Curated frequently-used shortcodes for the canvas editor emoji quick-insert popover (`F-COLLAB-139`).
pub(crate) const CANVAS_QUICK_EMOJIS: &[&str] = &[
    "white_check_mark",
    "rocket",
    "tada",
    "fire",
    "bulb",
    "eyes",
    "thumbsup",
    "warning",
    "star",
    "sparkles",
    "heart",
    "memo",
    "calendar",
    "pushpin",
    "link",
    "100",
    "hammer_and_wrench",
    "chart_with_upwards_trend",
];

/// Clamp a byte index to the nearest valid UTF-8 char boundary <= `idx` within `text`.
fn clamp_boundary(text: &str, idx: usize) -> usize {
    let mut i = idx.min(text.len());
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn clamp_range(text: &str, range: Range<usize>) -> Range<usize> {
    let a = clamp_boundary(text, range.start);
    let b = clamp_boundary(text, range.end);
    if a <= b { a..b } else { b..a }
}

/// Insert `snippet` replacing `selection`, returning `(new_text, caret_range_after_snippet)`.
pub(crate) fn insert_text_at(
    text: &str,
    selection: Range<usize>,
    snippet: &str,
) -> (String, Range<usize>) {
    let sel = clamp_range(text, selection);
    let mut out = String::with_capacity(text.len() + snippet.len());
    out.push_str(&text[..sel.start]);
    out.push_str(snippet);
    out.push_str(&text[sel.end..]);
    let end = sel.start + snippet.len();
    (out, end..end)
}

/// Toggle or wrap the active selection with inline markdown formatting (`F-COLLAB-133`).
/// Falls back cleanly to placeholder insertion at the cursor/end when no text is selected.
pub(crate) fn apply_inline_format(
    text: &str,
    selection: Range<usize>,
    fmt: InlineFormat,
) -> (String, Range<usize>) {
    let sel = clamp_range(text, selection);
    if fmt == InlineFormat::Link {
        return apply_link_format(text, sel);
    }
    let (marker, placeholder) = match fmt {
        InlineFormat::Bold => ("**", "bold"),
        InlineFormat::Italic => ("*", "italic"),
        InlineFormat::Strike => ("~~", "strikethrough"),
        InlineFormat::Code => ("`", "code"),
        InlineFormat::Link => unreachable!(),
    };
    let mlen = marker.len();

    if sel.is_empty() {
        let wrapped = format!("{marker}{placeholder}{marker}");
        let (out, _) = insert_text_at(text, sel.clone(), &wrapped);
        let inner_start = sel.start + mlen;
        let inner_end = inner_start + placeholder.len();
        return (out, inner_start..inner_end);
    }

    let selected = &text[sel.clone()];
    // Case 1: markers are inside the selection (`**word**` selected).
    if selected.len() >= mlen * 2 && selected.starts_with(marker) && selected.ends_with(marker) {
        // Avoid stripping `**` when toggling single `*` italic on bold text.
        let is_bold_not_italic =
            fmt == InlineFormat::Italic && selected.starts_with("**") && selected.ends_with("**");
        if !is_bold_not_italic {
            let inner = &selected[mlen..selected.len() - mlen];
            let mut out = String::with_capacity(text.len() - mlen * 2);
            out.push_str(&text[..sel.start]);
            out.push_str(inner);
            out.push_str(&text[sel.end..]);
            return (out, sel.start..(sel.start + inner.len()));
        }
    }

    // Case 2: markers immediately surround the selection (`**` before, `**` after).
    let surrounded = sel.start >= mlen
        && sel.end + mlen <= text.len()
        && &text[sel.start - mlen..sel.start] == marker
        && &text[sel.end..sel.end + mlen] == marker;
    let bold_guard = fmt == InlineFormat::Italic
        && sel.start >= 2
        && sel.end + 2 <= text.len()
        && &text[sel.start - 2..sel.start] == "**"
        && &text[sel.end..sel.end + 2] == "**";
    if surrounded && !bold_guard {
        let mut out = String::with_capacity(text.len() - mlen * 2);
        out.push_str(&text[..sel.start - mlen]);
        out.push_str(selected);
        out.push_str(&text[sel.end + mlen..]);
        let new_start = sel.start - mlen;
        return (out, new_start..(new_start + selected.len()));
    }

    // Case 3: wrap selection.
    let mut out = String::with_capacity(text.len() + mlen * 2);
    out.push_str(&text[..sel.start]);
    out.push_str(marker);
    out.push_str(selected);
    out.push_str(marker);
    out.push_str(&text[sel.end..]);
    let new_start = sel.start + mlen;
    (out, new_start..(new_start + selected.len()))
}

fn apply_link_format(text: &str, sel: Range<usize>) -> (String, Range<usize>) {
    if sel.is_empty() {
        let snippet = "[label](https://)";
        let (out, _) = insert_text_at(text, sel.clone(), snippet);
        let url_start = sel.start + "[label](".len();
        let url_end = url_start + "https://".len();
        return (out, url_start..url_end);
    }
    let selected = &text[sel.clone()];
    // If already `[label](url)`, unwrap to `label`.
    if let Some(rest) = selected.strip_prefix('[')
        && let Some(mid) = rest.find("](")
        && selected.ends_with(')')
    {
        let label = &rest[..mid];
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..sel.start]);
        out.push_str(label);
        out.push_str(&text[sel.end..]);
        return (out, sel.start..(sel.start + label.len()));
    }
    let snippet = format!("[{selected}](https://)");
    let mut out = String::with_capacity(text.len() + 11);
    out.push_str(&text[..sel.start]);
    out.push_str(&snippet);
    out.push_str(&text[sel.end..]);
    let url_start = sel.start + 1 + selected.len() + 2;
    let url_end = url_start + "https://".len();
    (out, url_start..url_end)
}

/// Compute the byte range covering all complete lines intersecting `sel`.
fn line_span_for_selection(text: &str, sel: Range<usize>) -> Range<usize> {
    let start = text[..sel.start].rfind('\n').map_or(0, |i| i + 1);
    let end_probe = if sel.end > sel.start && text.as_bytes().get(sel.end - 1) == Some(&b'\n') {
        sel.end - 1
    } else {
        sel.end
    };
    let end = text[end_probe..]
        .find('\n')
        .map_or(text.len(), |off| end_probe + off);
    start..end
}

/// Apply block / line-level formatting to the active selection or current line (`F-COLLAB-133..136`).
pub(crate) fn apply_block_format(
    text: &str,
    selection: Range<usize>,
    fmt: BlockFormat,
) -> (String, Range<usize>) {
    let sel = clamp_range(text, selection);
    match fmt {
        BlockFormat::Table => return insert_markdown_table(text, sel),
        BlockFormat::Divider => {
            let snippet = if text.trim().is_empty() {
                "---"
            } else if sel.start == text.len() {
                "\n\n---"
            } else {
                "\n---\n"
            };
            return insert_text_at(text, sel, snippet);
        }
        BlockFormat::CodeBlock => {
            if !sel.is_empty() {
                let selected = &text[sel.clone()];
                let wrapped = format!("```\n{selected}\n```");
                let (out, _) = insert_text_at(text, sel.clone(), &wrapped);
                let inner_start = sel.start + 4;
                return (out, inner_start..(inner_start + selected.len()));
            }
            let snippet = if text.trim().is_empty() {
                "```\ncode\n```"
            } else {
                "\n\n```\ncode\n```"
            };
            let prefix_len = snippet.len() - "code\n```".len();
            let (out, _) = insert_text_at(text, sel.clone(), snippet);
            let start = sel.start + prefix_len;
            return (out, start..(start + 4));
        }
        _ => {}
    }

    if text.trim().is_empty() {
        let placeholder = match fmt {
            BlockFormat::CycleHeading => "# Heading",
            BlockFormat::Quote => "> Quoted note",
            BlockFormat::Bullet => "- List item",
            BlockFormat::Numbered => "1. First item",
            BlockFormat::Checklist => "- [ ] Task item",
            BlockFormat::Table | BlockFormat::CodeBlock | BlockFormat::Divider => unreachable!(),
        };
        let end = placeholder.len();
        return (placeholder.to_owned(), end..end);
    }

    let span = line_span_for_selection(text, sel);
    let segment = &text[span.clone()];
    let lines: Vec<&str> = segment.split('\n').collect();

    let transformed: Vec<String> = match fmt {
        BlockFormat::CycleHeading => lines.iter().map(|l| cycle_heading_line(l)).collect(),
        BlockFormat::Quote => {
            let all_quoted = lines
                .iter()
                .all(|l| l.trim_start().starts_with("> ") || l.trim() == ">");
            lines
                .iter()
                .map(|l| {
                    let t = l.trim_start();
                    if all_quoted {
                        t.strip_prefix("> ")
                            .or_else(|| t.strip_prefix('>'))
                            .unwrap_or(t)
                            .to_owned()
                    } else if t.is_empty() {
                        "> ".to_owned()
                    } else {
                        format!("> {t}")
                    }
                })
                .collect()
        }
        BlockFormat::Bullet => {
            let all_bullets = lines
                .iter()
                .all(|l| l.trim_start().starts_with("- ") || l.trim_start().starts_with("* "));
            lines
                .iter()
                .map(|l| {
                    let t = l.trim_start();
                    if all_bullets {
                        t.strip_prefix("- ")
                            .or_else(|| t.strip_prefix("* "))
                            .unwrap_or(t)
                            .to_owned()
                    } else {
                        format!("- {t}")
                    }
                })
                .collect()
        }
        BlockFormat::Numbered => {
            let all_numbered = lines
                .iter()
                .all(|l| strip_numbered_prefix(l.trim_start()).is_some());
            lines
                .iter()
                .enumerate()
                .map(|(ix, l)| {
                    let t = l.trim_start();
                    if all_numbered {
                        strip_numbered_prefix(t).unwrap_or(t).to_owned()
                    } else {
                        format!("{}. {t}", ix + 1)
                    }
                })
                .collect()
        }
        BlockFormat::Checklist => {
            let all_checked_list = lines.iter().all(|l| {
                let t = l.trim_start();
                t.starts_with("- [ ] ") || t.starts_with("- [x] ") || t.starts_with("- [X] ")
            });
            lines
                .iter()
                .map(|l| {
                    let t = l.trim_start();
                    if all_checked_list {
                        t.strip_prefix("- [ ] ")
                            .or_else(|| t.strip_prefix("- [x] "))
                            .or_else(|| t.strip_prefix("- [X] "))
                            .unwrap_or(t)
                            .to_owned()
                    } else {
                        format!("- [ ] {t}")
                    }
                })
                .collect()
        }
        BlockFormat::Table | BlockFormat::CodeBlock | BlockFormat::Divider => unreachable!(),
    };

    let replacement = transformed.join("\n");
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str(&text[..span.start]);
    out.push_str(&replacement);
    out.push_str(&text[span.end..]);
    let new_end = span.start + replacement.len();
    (out, span.start..new_end)
}

fn cycle_heading_line(line: &str) -> String {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("### ") {
        rest.to_owned()
    } else if let Some(rest) = trimmed.strip_prefix("## ") {
        format!("### {rest}")
    } else if let Some(rest) = trimmed.strip_prefix("# ") {
        format!("## {rest}")
    } else {
        format!("# {trimmed}")
    }
}

/// Insert a default 3-column markdown pipe table at `selection` (`F-COLLAB-136`).
pub(crate) fn insert_markdown_table(text: &str, selection: Range<usize>) -> (String, Range<usize>) {
    let template =
        "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n| Value 1 | Value 2 | Value 3 |";
    let sel = clamp_range(text, selection);
    let snippet = if text.trim().is_empty() || sel.start == 0 || text[..sel.start].ends_with("\n\n")
    {
        template.to_owned()
    } else if text[..sel.start].ends_with('\n') {
        format!("\n{template}")
    } else {
        format!("\n\n{template}")
    };
    insert_text_at(text, sel, &snippet)
}

/// Locate the markdown pipe table covering `cursor` (or the last table in `text`), returning
/// `(start_line_ix, end_line_ix_exclusive)`.
fn find_markdown_table_lines(lines: &[&str], cursor_line: usize) -> Option<(usize, usize)> {
    let is_table_line = |l: &str| {
        let t = l.trim();
        t.contains('|') && (t.starts_with('|') || is_markdown_table_separator(t))
    };
    let mut tables = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_table_line(lines[i]) {
            let start = i;
            while i < lines.len() && is_table_line(lines[i]) {
                i += 1;
            }
            if i - start >= 2 {
                tables.push((start, i));
            }
        } else {
            i += 1;
        }
    }
    tables
        .iter()
        .copied()
        .find(|&(s, e)| cursor_line >= s && cursor_line < e)
        .or_else(|| tables.last().copied())
}

fn cursor_line_index(text: &str, cursor: usize) -> usize {
    let clamped = clamp_boundary(text, cursor);
    text[..clamped].bytes().filter(|&b| b == b'\n').count()
}

/// Append a new empty row to the markdown table at `cursor` (or insert a new table if none exists)
/// (`F-COLLAB-136`).
pub(crate) fn add_table_row_at(text: &str, cursor: usize) -> (String, Range<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let cur_line = cursor_line_index(text, cursor);
    let Some((start, end)) = find_markdown_table_lines(&lines, cur_line) else {
        let c = clamp_boundary(text, cursor);
        return insert_markdown_table(text, c..c);
    };
    let header = split_markdown_table_row(lines[start]);
    let col_count = header.len().max(1);
    let mut rows = Vec::new();
    for &line in &lines[start + 1..end] {
        if is_markdown_table_separator(line) {
            continue;
        }
        let mut cells = split_markdown_table_row(line);
        cells.resize(col_count, String::new());
        rows.push(cells);
    }
    rows.push(vec![String::new(); col_count]);
    let rebuilt = serialize_markdown_table(&header, &rows);
    replace_line_range(&lines, start, end, &rebuilt)
}

/// Append a new column to the markdown table at `cursor` (or insert a new table if none exists)
/// (`F-COLLAB-136`).
pub(crate) fn add_table_column_at(text: &str, cursor: usize) -> (String, Range<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let cur_line = cursor_line_index(text, cursor);
    let Some((start, end)) = find_markdown_table_lines(&lines, cur_line) else {
        let c = clamp_boundary(text, cursor);
        return insert_markdown_table(text, c..c);
    };
    let mut header = split_markdown_table_row(lines[start]);
    header.push(format!("Column {}", header.len() + 1));
    let col_count = header.len();
    let mut rows = Vec::new();
    for &line in &lines[start + 1..end] {
        if is_markdown_table_separator(line) {
            continue;
        }
        let mut cells = split_markdown_table_row(line);
        cells.resize(col_count, String::new());
        rows.push(cells);
    }
    let rebuilt = serialize_markdown_table(&header, &rows);
    replace_line_range(&lines, start, end, &rebuilt)
}

fn replace_line_range(
    lines: &[&str],
    start: usize,
    end: usize,
    replacement: &str,
) -> (String, Range<usize>) {
    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len() + 2);
    for &l in &lines[..start] {
        out_lines.push(l.to_owned());
    }
    let prefix_bytes: usize = out_lines.iter().map(|l| l.len() + 1).sum();
    for l in replacement.lines() {
        out_lines.push(l.to_owned());
    }
    let end_bytes = prefix_bytes + replacement.len();
    for &l in &lines[end..] {
        out_lines.push(l.to_owned());
    }
    (out_lines.join("\n"), end_bytes..end_bytes)
}

/// Detect a `/query` slash-command trigger at the start of the active line right before `cursor`
/// (`F-COLLAB-143`). Returns `(byte_range_of_slash_query, lowercase_query)`.
pub(crate) fn detect_slash_query(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let cur = clamp_boundary(text, cursor);
    let line_start = text[..cur].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..cur];
    let trimmed_start = before.trim_start();
    let leading_ws = before.len() - trimmed_start.len();
    let after_slash = trimmed_start.strip_prefix('/')?;
    if after_slash
        .chars()
        .any(|c| c.is_whitespace() || c == '/' || c == '`')
    {
        return None;
    }
    let slash_pos = line_start + leading_ws;
    Some((slash_pos..cur, after_slash.to_ascii_lowercase()))
}

/// Filter slash commands matching `query` by trigger, label, or description (`F-COLLAB-143`).
pub(crate) fn filter_slash_commands(query: &str) -> Vec<&'static SlashCommandSpec> {
    let q = query.trim().to_ascii_lowercase();
    SLASH_COMMANDS
        .iter()
        .filter(|spec| {
            q.is_empty()
                || spec.trigger.contains(&q)
                || spec.label.to_ascii_lowercase().contains(&q)
                || spec.description.to_ascii_lowercase().contains(&q)
        })
        .collect()
}

/// Apply a slash command by replacing `replace_range` (the `/query` or active selection) with its
/// markdown snippet (`F-COLLAB-143`).
pub(crate) fn apply_slash_command(
    text: &str,
    replace_range: Range<usize>,
    cmd: SlashCommand,
) -> (String, Range<usize>) {
    let range = clamp_range(text, replace_range);
    let snippet = match cmd {
        SlashCommand::Heading1 => "# Heading",
        SlashCommand::Heading2 => "## Section",
        SlashCommand::Heading3 => "### Subheading",
        SlashCommand::BulletList => "- List item",
        SlashCommand::NumberedList => "1. First item",
        SlashCommand::Checklist => "- [ ] Task item",
        SlashCommand::Quote => "> Quoted note",
        SlashCommand::CodeBlock => "```\ncode\n```",
        SlashCommand::Table => {
            "| Column 1 | Column 2 | Column 3 |\n| --- | --- | --- |\n| Value 1 | Value 2 | Value 3 |"
        }
        SlashCommand::Divider => "---",
        SlashCommand::Emoji | SlashCommand::File => "",
    };
    insert_text_at(text, range, snippet)
}

/// Whether `filename` has an image file extension that should render as `![name](url)`.
pub(crate) fn is_image_filename(filename: &str) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp"
    )
}

/// Format an uploaded file as Slack canvas markdown (`![name](url)` for images, `[name](url)`
/// for other files; `F-COLLAB-140`).
pub(crate) fn canvas_file_markdown(filename: &str, url: &str) -> String {
    if is_image_filename(filename) {
        format!("![{filename}]({url})")
    } else {
        format!("[{filename}]({url})")
    }
}

/// Cycle the last non-empty line through plain -> `# ` -> `## ` -> `### ` -> plain (`F-SHELL-262`).
#[cfg(test)]
pub(crate) fn cycle_last_line_heading(markdown: &str) -> String {
    let mut lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let Some(ix) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return "# Heading".to_owned();
    };
    lines[ix] = cycle_heading_line(&lines[ix]);
    lines.join("\n")
}

/// Swap the last non-empty line with its neighbor (`delta = -1` moves up, `1` moves down, `F-SHELL-269`).
pub(crate) fn move_last_line(markdown: &str, delta: isize) -> String {
    let mut lines: Vec<String> = markdown.lines().map(str::to_owned).collect();
    let Some(ix) = lines.iter().rposition(|l| !l.trim().is_empty()) else {
        return markdown.to_owned();
    };
    let target = ix as isize + delta;
    if target < 0 || target as usize >= lines.len() {
        return markdown.to_owned();
    }
    lines.swap(ix, target as usize);
    lines.join("\n")
}
