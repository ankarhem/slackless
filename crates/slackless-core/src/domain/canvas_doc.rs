//! Pure, GPUI-free Slack Canvas document model, Quip/Canvas HTML & Markdown parsers,
//! inline mention tokenizer, and media/embed classifiers (`F-COLLAB-125..150`).

use crate::api::CanvasEditOp;
use crate::domain::mrkdwn::replace_shortcodes_with;

/// Kind of inline media embed inside a canvas (`F-COLLAB-141`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasMediaKind {
    Audio,
    Video,
}

impl CanvasMediaKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Video => "Video",
        }
    }
}

/// Kind of embedded Slack document card inside a canvas (`F-COLLAB-148`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasEmbedKind {
    Canvas,
    List,
}

impl CanvasEmbedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas",
            Self::List => "List",
        }
    }
}

/// Structured table data extracted from a Canvas HTML or Markdown table (`F-COLLAB-136`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanvasTable {
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl CanvasTable {
    pub fn to_markdown(&self) -> String {
        serialize_markdown_table(&self.header, &self.rows)
    }

    pub fn add_row(&mut self) {
        let cols = self.header.len().max(1);
        self.rows.push(vec![String::new(); cols]);
    }

    pub fn add_column(&mut self) {
        let next_col = self.header.len() + 1;
        self.header.push(format!("Column {next_col}"));
        let cols = self.header.len();
        for row in &mut self.rows {
            row.resize(cols, String::new());
        }
    }
}

impl From<CanvasTable> for CanvasBlock {
    fn from(table: CanvasTable) -> Self {
        Self::Table {
            header: table.header,
            rows: table.rows,
        }
    }
}

/// Resolved display metadata for a canvas user profile card (`F-COLLAB-147`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasUserProfile {
    pub user_id: String,
    pub display_name: String,
    pub title: Option<String>,
    pub handle: Option<String>,
}

impl CanvasUserProfile {
    /// Up to 2 uppercase avatar initials derived from `display_name` (or `user_id` fallback).
    pub fn initials(&self) -> String {
        let words: Vec<&str> = self
            .display_name
            .split_whitespace()
            .map(|w| w.trim_start_matches('@'))
            .filter(|w| !w.is_empty())
            .collect();
        let chars: Vec<char> = match words.as_slice() {
            [] => self
                .user_id
                .chars()
                .take(2)
                .map(|c| c.to_ascii_uppercase())
                .collect(),
            [single] => single
                .chars()
                .take(2)
                .map(|c| c.to_ascii_uppercase())
                .collect(),
            many => many
                .iter()
                .take(2)
                .filter_map(|w| w.chars().next())
                .map(|c| c.to_ascii_uppercase())
                .collect(),
        };
        if chars.is_empty() {
            "?".to_owned()
        } else {
            chars.into_iter().collect()
        }
    }
}

/// Pure resolver merging parsed card fields with an optional host-supplied lookup (`F-COLLAB-147`).
pub fn resolve_profile_card<F>(
    user_id: &str,
    fallback_name: Option<&str>,
    fallback_title: Option<&str>,
    fallback_handle: Option<&str>,
    lookup: Option<F>,
) -> CanvasUserProfile
where
    F: Fn(&str) -> Option<CanvasUserProfile>,
{
    let resolved = lookup.and_then(|f| f(user_id));
    let clean = |opt: Option<&str>| {
        opt.map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let handle = resolved
        .as_ref()
        .and_then(|u| clean(u.handle.as_deref()))
        .or_else(|| clean(fallback_handle));
    let title = resolved
        .as_ref()
        .and_then(|u| clean(u.title.as_deref()))
        .or_else(|| clean(fallback_title));
    let display_name = resolved
        .as_ref()
        .and_then(|u| clean(Some(u.display_name.as_str())))
        .or_else(|| clean(fallback_name))
        .or_else(|| handle.as_ref().map(|h| format!("@{h}")))
        .unwrap_or_else(|| user_id.to_owned());

    CanvasUserProfile {
        user_id: user_id.to_owned(),
        display_name,
        title,
        handle,
    }
}

/// One block of a canvas document, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasBlock {
    /// `level` 1–3, like the canvas editor's H1/H2/H3.
    Heading {
        level: u8,
        text: String,
    },
    Paragraph(String),
    Bullets(Vec<String>),
    /// Ordered / numbered list (`1.`, `2.`, ...; `F-COLLAB-134`).
    NumberedList(Vec<String>),
    /// `(done, item)` pairs (`F-COLLAB-135`).
    Checklist(Vec<(bool, String)>),
    CodeBlock(String),
    /// Single- or multi-line blockquote (`\n`-separated lines; `F-COLLAB-134`).
    Quote(String),
    Table {
        header: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Embedded canvas image (`img[src]` with optional alt caption and reserved dimensions).
    Image {
        url: String,
        alt: String,
        width: Option<u32>,
        height: Option<u32>,
    },
    /// Embedded inline audio or video clip/file (`F-COLLAB-141`).
    MediaEmbed {
        kind: CanvasMediaKind,
        title: String,
        url: String,
        file_id: Option<String>,
        duration: Option<String>,
        mime: Option<String>,
    },
    /// Embedded Slack user profile card / user unfurl (`F-COLLAB-147`).
    ProfileCard {
        user_id: String,
        display_name: Option<String>,
        title: Option<String>,
        handle: Option<String>,
    },
    /// Embedded Slack Canvas or List card by link (`F-COLLAB-148`).
    DocEmbed {
        kind: CanvasEmbedKind,
        file_id: String,
        title: String,
        url: String,
    },
    Divider,
}

impl CanvasBlock {
    /// Case-insensitive text search across the block's rendered text (`F-SHELL-266`).
    pub fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let needle_lower = needle.to_lowercase();
        let has = |s: &str| s.to_lowercase().contains(&needle_lower);
        match self {
            Self::Heading { text, .. }
            | Self::Paragraph(text)
            | Self::CodeBlock(text)
            | Self::Quote(text) => has(text),
            Self::Bullets(items) | Self::NumberedList(items) => items.iter().any(|s| has(s)),
            Self::Checklist(items) => items.iter().any(|(_, s)| has(s)),
            Self::Table { header, rows } => {
                header.iter().any(|s| has(s)) || rows.iter().any(|r| r.iter().any(|s| has(s)))
            }
            Self::Image { alt, url, .. } => has(alt) || has(url),
            Self::MediaEmbed {
                kind,
                title,
                url,
                file_id,
                duration,
                mime,
            } => {
                has(kind.label())
                    || has(title)
                    || has(url)
                    || file_id.as_deref().is_some_and(has)
                    || duration.as_deref().is_some_and(has)
                    || mime.as_deref().is_some_and(has)
            }
            Self::ProfileCard {
                user_id,
                display_name,
                title,
                handle,
            } => {
                has(user_id)
                    || display_name.as_deref().is_some_and(has)
                    || title.as_deref().is_some_and(has)
                    || handle.as_deref().is_some_and(has)
            }
            Self::DocEmbed {
                kind,
                file_id,
                title,
                url,
            } => has(kind.label()) || has(file_id) || has(title) || has(url),
            Self::Divider => false,
        }
    }

    /// Short 1-line label for drag-and-drop preview chips (`F-COLLAB-144`).
    pub fn preview_label(&self) -> String {
        let raw = match self {
            Self::Heading { text, .. } => text.clone(),
            Self::Paragraph(text) => text.clone(),
            Self::Bullets(items) => items
                .first()
                .map(|s| format!("• {s}"))
                .unwrap_or_else(|| "Bullet list".to_owned()),
            Self::NumberedList(items) => items
                .first()
                .map(|s| format!("1. {s}"))
                .unwrap_or_else(|| "Numbered list".to_owned()),
            Self::Checklist(items) => items
                .first()
                .map(|(_, s)| format!("☑ {s}"))
                .unwrap_or_else(|| "Checklist".to_owned()),
            Self::CodeBlock(code) => code
                .lines()
                .next()
                .map(|l| format!("Code: {l}"))
                .unwrap_or_else(|| "Code block".to_owned()),
            Self::Quote(text) => text
                .lines()
                .next()
                .map(|l| format!("“{l}”"))
                .unwrap_or_else(|| "Quote".to_owned()),
            Self::Table { header, .. } => {
                if header.is_empty() {
                    "Table".to_owned()
                } else {
                    format!("Table: {}", header.join(" · "))
                }
            }
            Self::Image { alt, .. } => {
                if alt.is_empty() {
                    "Image".to_owned()
                } else {
                    format!("Image: {alt}")
                }
            }
            Self::MediaEmbed { kind, title, .. } => format!("{}: {title}", kind.label()),
            Self::ProfileCard {
                user_id,
                display_name,
                ..
            } => display_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|n| format!("Profile: {n}"))
                .unwrap_or_else(|| format!("Profile: {user_id}")),
            Self::DocEmbed { kind, title, .. } => format!("{}: {title}", kind.label()),
            Self::Divider => "Divider".to_owned(),
        };
        let mut chars = raw.chars();
        let truncated: String = chars.by_ref().take(42).collect();
        if chars.next().is_some() {
            format!("{truncated}…")
        } else {
            truncated
        }
    }

    /// Serialize this block to Slack canvas markdown (`canvases.edit`).
    pub fn to_markdown(&self) -> String {
        match self {
            Self::Heading { level, text } => {
                let hashes = "#".repeat((*level).clamp(1, 3) as usize);
                format!("{hashes} {text}")
            }
            Self::Paragraph(text) => text.clone(),
            Self::Bullets(items) => items
                .iter()
                .map(|item| format!("- {item}"))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::NumberedList(items) => items
                .iter()
                .enumerate()
                .map(|(ix, item)| format!("{}. {item}", ix + 1))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Checklist(items) => items
                .iter()
                .map(|(done, item)| {
                    if *done {
                        format!("- [x] {item}")
                    } else {
                        format!("- [ ] {item}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::CodeBlock(code) => format!("```\n{code}\n```"),
            Self::Quote(text) => text
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        ">".to_owned()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Table { header, rows } => serialize_markdown_table(header, rows),
            Self::Image { url, alt, .. } => format!("![{alt}]({url})"),
            Self::MediaEmbed { title, url, .. } => {
                if title.is_empty() {
                    url.clone()
                } else {
                    format!("[{title}]({url})")
                }
            }
            Self::ProfileCard {
                user_id,
                display_name,
                ..
            } => {
                let profile_url = format!("https://slack.com/team/{user_id}");
                match display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    Some(name) => format!("[{name}]({profile_url})"),
                    None => profile_url,
                }
            }
            Self::DocEmbed {
                kind,
                file_id,
                title,
                url,
            } => {
                let canonical = if !url.is_empty() {
                    url.clone()
                } else {
                    let seg = match kind {
                        CanvasEmbedKind::Canvas => "docs",
                        CanvasEmbedKind::List => "lists",
                    };
                    format!("https://slack.com/{seg}/T0/{file_id}")
                };
                if title.is_empty() {
                    canonical
                } else {
                    format!("[{title}]({canonical})")
                }
            }
            Self::Divider => "---".to_owned(),
        }
    }
}

/// One embedded file, image, media clip, or document extracted for the "Files in this canvas"
/// section strip (`F-COLLAB-126`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasEmbeddedFile {
    pub block_ix: usize,
    pub kind_label: &'static str,
    pub title: String,
    pub url: String,
    pub file_id: Option<String>,
    pub embed_kind: Option<CanvasEmbedKind>,
    pub media_kind: Option<CanvasMediaKind>,
}

/// A parsed canvas document ready for CLI rendering, export, or native GUI editing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanvasDoc {
    pub title: String,
    pub meta: String,
    pub blocks: Vec<CanvasBlock>,
    /// Optional Quip section IDs (`temp:C:...`) aligned 1:1 with `blocks` when parsed from HTML.
    pub section_ids: Vec<Option<String>>,
}

impl CanvasDoc {
    /// Parse a live Slack canvas HTML export (`url_private`) into structured blocks + section IDs.
    pub fn from_html(title: String, html: &str) -> Self {
        let (blocks, section_ids) = parse_canvas_html_with_ids(html);
        Self {
            title,
            meta: "Live canvas".to_owned(),
            blocks,
            section_ids,
        }
    }

    /// Parse a live Slack canvas HTML export, resolving `:shortcode:` emoji tokens through
    /// `resolve_emoji` (e.g. a front-end Unicode emoji catalog).
    pub fn from_html_with_emoji(
        title: String,
        html: &str,
        resolve_emoji: &dyn Fn(&str) -> Option<String>,
    ) -> Self {
        let (blocks, section_ids) = parse_canvas_html_with_ids_and_emoji(html, resolve_emoji);
        Self {
            title,
            meta: "Live canvas".to_owned(),
            blocks,
            section_ids,
        }
    }

    /// Parse lightweight markdown back into [`CanvasBlock`]s.
    pub fn from_markdown(title: String, meta: String, markdown: &str) -> Self {
        let blocks = parse_canvas_markdown(markdown);
        let section_ids = vec![None; blocks.len()];
        Self {
            title,
            meta,
            blocks,
            section_ids,
        }
    }

    /// Serialize the entire canvas back into Slack canvas markdown (`canvases.edit`).
    pub fn to_markdown(&self) -> String {
        self.blocks
            .iter()
            .map(CanvasBlock::to_markdown)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Headings in document order (`(level, text)`) for the collapsible outline strip.
    pub fn outline_headings(&self) -> Vec<(u8, &str)> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                CanvasBlock::Heading { level, text } if !text.is_empty() => {
                    Some((*level, text.as_str()))
                }
                _ => None,
            })
            .collect()
    }

    /// Plain markdown text for a single block (`F-SHELL-265` / `F-COLLAB-145`).
    pub fn block_plain_text(&self, index: usize) -> Option<String> {
        self.blocks.get(index).map(CanvasBlock::to_markdown)
    }

    /// Duplicate block at `index`, inserting the copy right after it (`F-SHELL-265`, `F-COLLAB-145`).
    pub fn duplicate_block(&mut self, index: usize) -> bool {
        let Some(cloned) = self.blocks.get(index).cloned() else {
            return false;
        };
        self.insert_block_after(index, cloned)
    }

    /// Insert `block` immediately after `index` (`F-COLLAB-145`).
    pub fn insert_block_after(&mut self, index: usize, block: CanvasBlock) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        self.blocks.insert(index + 1, block);
        if self.section_ids.len() < self.blocks.len() - 1 {
            self.section_ids.resize(self.blocks.len() - 1, None);
        }
        self.section_ids.insert(index + 1, None);
        true
    }

    /// Append a block to the end of the canvas (e.g. uploaded file or new section).
    pub fn push_block(&mut self, block: CanvasBlock) {
        self.blocks.push(block);
        if self.section_ids.len() < self.blocks.len() - 1 {
            self.section_ids.resize(self.blocks.len() - 1, None);
        }
        self.section_ids.push(None);
    }

    /// Reorder the block at `from` to land at `to`, keeping `blocks` and `section_ids` aligned.
    pub fn reorder_block(&mut self, from: usize, to: usize) -> bool {
        let len = self.blocks.len();
        if from >= len || to >= len || from == to {
            return false;
        }
        if self.section_ids.len() < len {
            self.section_ids.resize(len, None);
        } else if self.section_ids.len() > len {
            self.section_ids.truncate(len);
        }
        let block = self.blocks.remove(from);
        let sid = self.section_ids.remove(from);
        self.blocks.insert(to, block);
        self.section_ids.insert(to, sid);
        true
    }

    /// Reorder the block at `from` to `to` and return the resulting `canvases.edit` operation.
    pub fn reorder_block_op(&mut self, from: usize, to: usize) -> Option<CanvasEditOp> {
        if !self.reorder_block(from, to) {
            return None;
        }
        Some(CanvasEditOp::Replace {
            section_id: None,
            markdown: self.to_markdown(),
        })
    }

    /// Move block at `index` up (`delta = -1`) or down (`delta = 1`).
    pub fn move_block(&mut self, index: usize, delta: isize) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.blocks.len() {
            return false;
        }
        self.reorder_block(index, target as usize)
    }

    /// Delete block at `index`.
    pub fn delete_block(&mut self, index: usize) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        self.blocks.remove(index);
        if index < self.section_ids.len() {
            self.section_ids.remove(index);
        }
        true
    }

    /// Toggle a checklist item's checked state, returning the updated section markdown.
    pub fn toggle_checklist_item(&mut self, block_ix: usize, item_ix: usize) -> Option<String> {
        let CanvasBlock::Checklist(items) = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        let (done, _) = items.get_mut(item_ix)?;
        *done = !*done;
        Some(CanvasBlock::Checklist(items.clone()).to_markdown())
    }

    /// Append a new unchecked checklist item, returning the updated section markdown.
    pub fn add_checklist_item(&mut self, block_ix: usize, label: String) -> Option<String> {
        let trimmed = label.trim().to_owned();
        if trimmed.is_empty() {
            return None;
        }
        let CanvasBlock::Checklist(items) = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        items.push((false, trimmed));
        Some(CanvasBlock::Checklist(items.clone()).to_markdown())
    }

    /// Remove a checklist item at `item_ix`. If the checklist becomes empty, removes the block
    /// and returns `Some(None)`; otherwise returns `Some(Some(section_markdown))`.
    pub fn delete_checklist_item(
        &mut self,
        block_ix: usize,
        item_ix: usize,
    ) -> Option<Option<String>> {
        let empty = {
            let CanvasBlock::Checklist(items) = self.blocks.get_mut(block_ix)? else {
                return None;
            };
            if item_ix >= items.len() {
                return None;
            }
            items.remove(item_ix);
            items.is_empty()
        };
        if empty {
            self.delete_block(block_ix);
            Some(None)
        } else {
            let md = self.blocks.get(block_ix).map(CanvasBlock::to_markdown);
            Some(md)
        }
    }

    /// Append an empty row matching the header width to a table block.
    pub fn table_add_row(&mut self, block_ix: usize) -> Option<String> {
        let CanvasBlock::Table { header, rows } = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        let cols = header.len().max(1);
        rows.push(vec![String::new(); cols]);
        Some(serialize_markdown_table(header, rows))
    }

    /// Append a new column to a table block's header and all data rows.
    pub fn table_add_column(&mut self, block_ix: usize) -> Option<String> {
        let CanvasBlock::Table { header, rows } = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        let next_col = header.len() + 1;
        header.push(format!("Column {next_col}"));
        let cols = header.len();
        for row in rows.iter_mut() {
            row.resize(cols, String::new());
        }
        Some(serialize_markdown_table(header, rows))
    }

    /// Update the accessible description (`alt` text) of an image block.
    pub fn set_image_alt(&mut self, block_ix: usize, new_alt: String) -> Option<String> {
        let block = self.blocks.get_mut(block_ix)?;
        let CanvasBlock::Image { alt, .. } = block else {
            return None;
        };
        *alt = new_alt.trim().to_owned();
        Some(block.to_markdown())
    }

    /// Extract all embedded images, audio/video media clips, and embedded canvases/lists.
    pub fn embedded_files(&self) -> Vec<CanvasEmbeddedFile> {
        let mut out = Vec::new();
        for (block_ix, block) in self.blocks.iter().enumerate() {
            match block {
                CanvasBlock::Image { url, alt, .. } => {
                    let fallback_name = url
                        .split('?')
                        .next()
                        .unwrap_or(url)
                        .rsplit('/')
                        .next()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("Image");
                    let title = if alt.trim().is_empty() {
                        fallback_name.to_owned()
                    } else {
                        alt.trim().to_owned()
                    };
                    out.push(CanvasEmbeddedFile {
                        block_ix,
                        kind_label: "Image",
                        title,
                        url: url.clone(),
                        file_id: None,
                        embed_kind: None,
                        media_kind: None,
                    });
                }
                CanvasBlock::MediaEmbed {
                    kind,
                    title,
                    url,
                    file_id,
                    ..
                } => {
                    out.push(CanvasEmbeddedFile {
                        block_ix,
                        kind_label: kind.label(),
                        title: title.clone(),
                        url: url.clone(),
                        file_id: file_id.clone(),
                        embed_kind: None,
                        media_kind: Some(*kind),
                    });
                }
                CanvasBlock::DocEmbed {
                    kind,
                    file_id,
                    title,
                    url,
                } => {
                    out.push(CanvasEmbeddedFile {
                        block_ix,
                        kind_label: kind.label(),
                        title: title.clone(),
                        url: url.clone(),
                        file_id: Some(file_id.clone()),
                        embed_kind: Some(*kind),
                        media_kind: None,
                    });
                }
                _ => {}
            }
        }
        out
    }
}

/// One parsed inline segment inside a canvas text block (`F-COLLAB-150`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasInlineSegment {
    Text(String),
    UserMention {
        user_id: String,
        label: Option<String>,
    },
    ChannelMention {
        channel_id: String,
        name: Option<String>,
    },
    UsergroupMention {
        group_id: String,
        handle: String,
    },
    BroadcastMention(String),
}

/// Whether `s` is an uppercase ASCII alphanumeric Slack ID with length >= 3 starting with one of `prefixes`.
pub fn is_upper_alnum_id(s: &str, prefixes: &[u8]) -> bool {
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && prefixes.contains(&bytes[0])
        && bytes
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn is_slack_user_id(s: &str) -> bool {
    is_upper_alnum_id(s, b"UW")
}

fn is_slack_file_id(s: &str) -> bool {
    is_upper_alnum_id(s, b"F")
}

/// Classify a URL or filename by media file extension (`F-COLLAB-141`).
/// Returns `(CanvasMediaKind, canonical_mime)`.
pub fn classify_media_url(url_or_name: &str) -> Option<(CanvasMediaKind, &'static str)> {
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
        "mp3" => Some((CanvasMediaKind::Audio, "audio/mpeg")),
        "m4a" => Some((CanvasMediaKind::Audio, "audio/mp4")),
        "wav" => Some((CanvasMediaKind::Audio, "audio/wav")),
        "ogg" | "oga" => Some((CanvasMediaKind::Audio, "audio/ogg")),
        "aac" => Some((CanvasMediaKind::Audio, "audio/aac")),
        "flac" => Some((CanvasMediaKind::Audio, "audio/flac")),
        "opus" => Some((CanvasMediaKind::Audio, "audio/opus")),
        "mp4" | "m4v" => Some((CanvasMediaKind::Video, "video/mp4")),
        "mov" => Some((CanvasMediaKind::Video, "video/quicktime")),
        "webm" => Some((CanvasMediaKind::Video, "video/webm")),
        "mkv" => Some((CanvasMediaKind::Video, "video/x-matroska")),
        _ => None,
    }
}

/// Classify a MIME type or Slack filetype string (`audio/*`, `video/*`, `mp3`, `mp4`, etc.).
pub fn classify_media_mime(mime: &str) -> Option<(CanvasMediaKind, String)> {
    let lower = mime.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return None;
    }
    if lower.starts_with("audio/") {
        return Some((CanvasMediaKind::Audio, lower));
    }
    if lower.starts_with("video/") {
        return Some((CanvasMediaKind::Video, lower));
    }
    match lower.as_str() {
        "mp3" | "m4a" | "wav" | "ogg" | "aac" | "flac" | "opus" | "slack_audio" => {
            let fake = format!("clip.{lower}");
            let mime_str = classify_media_url(&fake)
                .map(|(_, m)| m.to_owned())
                .unwrap_or_else(|| "audio/mpeg".to_owned());
            Some((CanvasMediaKind::Audio, mime_str))
        }
        "mp4" | "mov" | "webm" | "m4v" | "mkv" | "slack_video" => {
            let fake = format!("clip.{lower}");
            let mime_str = classify_media_url(&fake)
                .map(|(_, m)| m.to_owned())
                .unwrap_or_else(|| "video/mp4".to_owned());
            Some((CanvasMediaKind::Video, mime_str))
        }
        _ => None,
    }
}

/// Short uppercase format pill label (e.g. `"MP4"`, `"MP3"`, `"WEBM"`) derived from MIME or URL.
pub fn media_format_pill(mime: Option<&str>, url: &str) -> Option<String> {
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

/// Parse a Slack profile URL (`https://<ws>.slack.com/team/U...` or `/team/U...`) into its `user_id`.
pub fn parse_slack_profile_url(url: &str) -> Option<String> {
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
pub fn parse_slack_doc_url(url: &str) -> Option<(CanvasEmbedKind, String)> {
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

/// Extract the final path segment (filename) from a URL, stripping query strings and fragments.
pub fn filename_from_url(url: &str) -> Option<String> {
    let without_frag = url.split('#').next().unwrap_or(url);
    let without_query = without_frag.split('?').next().unwrap_or(without_frag);
    let seg = without_query.trim_end_matches('/').rsplit('/').next()?;
    if seg.is_empty() {
        None
    } else {
        Some(seg.to_owned())
    }
}

/// Decode common HTML character entities found in Slack Quip/Canvas HTML exports.
pub fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

/// Escape pipe characters inside a markdown table cell.
fn escape_table_cell(cell: &str) -> String {
    cell.replace('|', "\\|").replace('\n', " ")
}

/// Serialize a table to normalized markdown pipe-table syntax (`F-COLLAB-136`).
pub fn serialize_markdown_table(header: &[String], rows: &[Vec<String>]) -> String {
    let col_count = header
        .len()
        .max(rows.iter().map(Vec::len).max().unwrap_or(0))
        .max(1);
    let fmt_row = |cells: &[String]| {
        let mut padded = Vec::with_capacity(col_count);
        for i in 0..col_count {
            padded.push(
                cells
                    .get(i)
                    .map(|c| escape_table_cell(c))
                    .unwrap_or_default(),
            );
        }
        format!("| {} |", padded.join(" | "))
    };
    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(fmt_row(header));
    lines.push(format!(
        "| {} |",
        (0..col_count)
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | ")
    ));
    for row in rows {
        lines.push(fmt_row(row));
    }
    lines.join("\n")
}

/// Strip a numbered list prefix like `"1. item"` or `"12) item"`.
pub fn strip_numbered_prefix(line: &str) -> Option<&str> {
    let digits_len = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let after_digits = &line[digits_len..];
    after_digits
        .strip_prefix(". ")
        .or_else(|| after_digits.strip_prefix(") "))
}

pub fn is_markdown_table_separator(line: &str) -> bool {
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

/// Split a markdown pipe-table row, honoring escaped `\|` inside cells.
pub fn split_markdown_table_row(row: &str) -> Vec<String> {
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

fn no_emoji(_shortcode: &str) -> Option<String> {
    None
}

/// Lightweight parser converting Slack's Quip/Canvas HTML export into structured [`CanvasBlock`]s.
pub fn parse_canvas_html(html: &str) -> Vec<CanvasBlock> {
    parse_canvas_html_with_ids(html).0
}

/// Like [`parse_canvas_html`], also returning each block's Quip `id` attribute (`temp:C:...`)
/// when present so `canvases.edit` can target individual sections.
pub fn parse_canvas_html_with_ids(html: &str) -> (Vec<CanvasBlock>, Vec<Option<String>>) {
    parse_canvas_html_with_ids_and_emoji(html, &no_emoji)
}

/// Like [`parse_canvas_html_with_ids`], resolving `:shortcode:` emoji tokens and inline emoji
/// `<img>` / `<control>` elements via `resolve_emoji`.
pub fn parse_canvas_html_with_ids_and_emoji(
    html: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> (Vec<CanvasBlock>, Vec<Option<String>>) {
    let body = extract_tag_inner(html, "body").unwrap_or(html);
    let mut blocks = Vec::new();
    let mut ids = Vec::new();
    let mut rest = body;
    while let Some(open_pos) = rest.find('<') {
        let before = clean_inline_html(&rest[..open_pos], resolve_emoji);
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
            if let Some(emoji_glyph) = extract_emoji_from_img_tag(trimmed, resolve_emoji) {
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
                    alt: replace_shortcodes_with(&clean_text(&alt), resolve_emoji),
                    width,
                    height,
                });
                ids.push(section_id);
            }
            continue;
        }
        if matches!(tag_name.as_str(), "audio" | "video") {
            let kind = if tag_name == "audio" {
                CanvasMediaKind::Audio
            } else {
                CanvasMediaKind::Video
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
            if let Some(block) = parse_html_media_tag(kind, trimmed, inner, resolve_emoji) {
                blocks.push(block);
                ids.push(section_id);
            }
            continue;
        }
        if tag_name == "control" {
            if let Some(end) = find_case_insensitive(rest, "</control>") {
                let inner = &rest[..end];
                if let Some(card) = parse_control_profile_card(inner, resolve_emoji) {
                    rest = &rest[end + "</control>".len()..];
                    blocks.push(card);
                    ids.push(section_id);
                    continue;
                }
                if let Some(emoji) = extract_control_emoji(inner, resolve_emoji) {
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
                    parse_html_profile_card(trimmed, inner, resolve_emoji)
                } else if is_doc_embed {
                    parse_html_doc_embed(trimmed, inner, resolve_emoji)
                } else {
                    parse_html_embedded_file_media(trimmed, inner, resolve_emoji)
                        .or_else(|| parse_html_doc_embed(trimmed, inner, resolve_emoji))
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
                    let text = clean_inline_html(inner, resolve_emoji);
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
                    if let Some(embed_block) =
                        parse_html_paragraph_embed(trimmed, inner, resolve_emoji)
                    {
                        blocks.push(embed_block);
                    } else {
                        let text = clean_inline_html(inner, resolve_emoji);
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
                    let text = clean_multiline_html(inner, resolve_emoji);
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
                    let items = extract_list_items(inner, resolve_emoji);
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
                    if let Some((header, rows)) = parse_html_table(inner, resolve_emoji) {
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
    let tail = clean_inline_html(rest, resolve_emoji);
    if !tail.is_empty() {
        blocks.push(CanvasBlock::Paragraph(tail));
        ids.push(None);
    }
    (blocks, ids)
}

/// Lightweight Slack canvas markdown parser (headings, bullets, numbered lists, checklists,
/// code fences, multi-line blockquotes, horizontal rules, pipe tables, images, embeds, paragraphs).
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

/// Pure inline tokenizer splitting canvas text into plain runs and mention tokens (`F-COLLAB-150`).
pub fn parse_inline_canvas_segments(text: &str) -> Vec<CanvasInlineSegment> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cursor = 0;
    let mut plain_start = 0;
    let bytes = text.as_bytes();

    while cursor < bytes.len() {
        let b = bytes[cursor];
        if b == b'<' {
            let rest = &text[cursor..];
            if let Some((seg, consumed)) =
                try_parse_control_mention(rest).or_else(|| try_parse_angle_mention(rest))
            {
                push_inline_text(&mut out, &text[plain_start..cursor]);
                out.push(seg);
                cursor += consumed;
                plain_start = cursor;
                continue;
            }
        } else if b == b'@'
            && let Some((seg, consumed)) = try_parse_bare_broadcast(text, cursor)
        {
            push_inline_text(&mut out, &text[plain_start..cursor]);
            out.push(seg);
            cursor += consumed;
            plain_start = cursor;
            continue;
        }

        let ch_len = text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
        cursor += ch_len;
    }

    push_inline_text(&mut out, &text[plain_start..]);
    out
}

fn push_inline_text(out: &mut Vec<CanvasInlineSegment>, chunk: &str) {
    if chunk.is_empty() {
        return;
    }
    if let Some(CanvasInlineSegment::Text(prev)) = out.last_mut() {
        prev.push_str(chunk);
    } else {
        out.push(CanvasInlineSegment::Text(chunk.to_owned()));
    }
}

fn try_parse_control_mention(rest: &str) -> Option<(CanvasInlineSegment, usize)> {
    let lower = rest.to_ascii_lowercase();
    if !lower.starts_with("<control") {
        return None;
    }
    let close_tag = "</control>";
    let end_rel = lower.find(close_tag)?;
    let consumed = end_rel + close_tag.len();
    let open_gt = rest[..end_rel].find('>')?;
    let inner = &rest[open_gt + 1..end_rel];

    let inner_lower = inner.to_ascii_lowercase();
    let a_start = inner_lower.find("<a")?;
    let a_after = &inner[a_start..];
    let a_gt = a_after.find('>')?;
    let a_open = &a_after[..a_gt];
    let a_body = &a_after[a_gt + 1..];
    let a_end = a_body.to_ascii_lowercase().find("</a>")?;
    let anchor_text = a_body[..a_end].trim();

    if let Some(raw_uid) = anchor_text.strip_prefix('@') {
        let candidate = raw_uid.trim();
        if is_upper_alnum_id(candidate, b"UW") {
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: candidate.to_owned(),
                    label: None,
                },
                consumed,
            ));
        }
    }
    if let Some(raw_cid) = anchor_text.strip_prefix('#') {
        let candidate = raw_cid.trim();
        if is_upper_alnum_id(candidate, b"CG") {
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: candidate.to_owned(),
                    name: None,
                },
                consumed,
            ));
        }
    }
    if let Some(pos) = a_open.find("/team/") {
        let tail = &a_open[pos + "/team/".len()..];
        let uid: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if is_upper_alnum_id(&uid, b"UW") {
            let label = anchor_text.trim_start_matches('@').trim().to_owned();
            let label = (!label.is_empty() && label != uid).then_some(label);
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: uid,
                    label,
                },
                consumed,
            ));
        }
    }
    if let Some(pos) = a_open.find("/archives/") {
        let tail = &a_open[pos + "/archives/".len()..];
        let cid: String = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if is_upper_alnum_id(&cid, b"CG") {
            let name = anchor_text.trim_start_matches('#').trim().to_owned();
            let name = (!name.is_empty() && name != cid).then_some(name);
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: cid,
                    name,
                },
                consumed,
            ));
        }
    }
    None
}

fn try_parse_angle_mention(rest: &str) -> Option<(CanvasInlineSegment, usize)> {
    if !rest.starts_with('<') {
        return None;
    }
    let gt = rest.find('>')?;
    let inner = &rest[1..gt];
    let consumed = gt + 1;

    if let Some(body) = inner.strip_prefix('@') {
        let mut parts = body.splitn(2, '|');
        let uid = parts.next()?.trim();
        if is_upper_alnum_id(uid, b"UW") {
            let label = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_owned());
            return Some((
                CanvasInlineSegment::UserMention {
                    user_id: uid.to_owned(),
                    label,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix('#') {
        let mut parts = body.splitn(2, '|');
        let cid = parts.next()?.trim();
        if is_upper_alnum_id(cid, b"CG") {
            let name = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('#').to_owned());
            return Some((
                CanvasInlineSegment::ChannelMention {
                    channel_id: cid.to_owned(),
                    name,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix("!subteam^") {
        let mut parts = body.splitn(2, '|');
        let gid = parts.next()?.trim();
        if is_upper_alnum_id(gid, b"S") {
            let handle = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.trim_start_matches('@').to_owned())
                .unwrap_or_else(|| gid.to_owned());
            return Some((
                CanvasInlineSegment::UsergroupMention {
                    group_id: gid.to_owned(),
                    handle,
                },
                consumed,
            ));
        }
    } else if let Some(body) = inner.strip_prefix('!') {
        let kind = body.split('|').next().unwrap_or(body).trim();
        if matches!(kind, "here" | "channel" | "everyone") {
            return Some((
                CanvasInlineSegment::BroadcastMention(kind.to_owned()),
                consumed,
            ));
        }
    }

    None
}

fn try_parse_bare_broadcast(text: &str, idx: usize) -> Option<(CanvasInlineSegment, usize)> {
    let rest = &text[idx..];
    if !rest.starts_with('@') {
        return None;
    }
    if idx > 0 {
        let prev = text[..idx].chars().next_back()?;
        if prev.is_ascii_alphanumeric() || prev == '_' || prev == '.' {
            return None;
        }
    }
    for kw in ["channel", "here"] {
        let candidate = &rest[1..];
        if candidate.len() >= kw.len() && candidate[..kw.len()].eq_ignore_ascii_case(kw) {
            let after = &candidate[kw.len()..];
            let boundary = after
                .chars()
                .next()
                .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
            if boundary {
                return Some((
                    CanvasInlineSegment::BroadcastMention(kw.to_owned()),
                    1 + kw.len(),
                ));
            }
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

fn parse_html_media_tag(
    kind: CanvasMediaKind,
    open_tag: &str,
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
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
            let txt = clean_inline_html(inner, resolve_emoji);
            (!txt.is_empty()).then_some(txt)
        })
        .or_else(|| filename_from_url(&url))
        .map(|s| replace_shortcodes_with(&clean_text(&s), resolve_emoji))
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

fn parse_html_embedded_file_media(
    open_tag: &str,
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "lnk").or_else(|| find_first_tag(inner, "a"));
    let link_tag = link.map(|(t, _)| t).unwrap_or("");
    let link_text = link
        .map(|(_, body)| clean_inline_html(body, resolve_emoji))
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
            let txt = clean_inline_html(inner, resolve_emoji);
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
        replace_shortcodes_with(&clean_text(&raw_title), resolve_emoji)
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

fn parse_control_profile_card(
    control_inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
    let (link_tag, link_body) =
        find_first_tag(control_inner, "a").or_else(|| find_first_tag(control_inner, "lnk"))?;
    let body_text = clean_inline_html(link_body, resolve_emoji);
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

fn parse_html_profile_card(
    open_tag: &str,
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "a").or_else(|| find_first_tag(inner, "lnk"));
    let href = extract_attr(open_tag, "href")
        .or_else(|| link.and_then(|(t, _)| extract_attr(t, "href")))
        .map(|h| decode_entities(&h));
    let inner_text = clean_inline_html(inner, resolve_emoji);

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
                    let s = clean_inline_html(body, resolve_emoji);
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
                let t = clean_inline_html(body, resolve_emoji);
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

fn parse_html_doc_embed(
    open_tag: &str,
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
    let link = find_first_tag(inner, "a").or_else(|| find_first_tag(inner, "lnk"));
    let link_tag = link.map(|(t, _)| t).unwrap_or("");
    let link_text = link
        .map(|(_, body)| clean_inline_html(body, resolve_emoji))
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
            let txt = clean_inline_html(inner, resolve_emoji);
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

fn parse_html_paragraph_embed(
    open_tag: &str,
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<CanvasBlock> {
    if has_profile_card_marker(open_tag)
        && let Some(card) = parse_html_profile_card(open_tag, inner, resolve_emoji)
    {
        return Some(card);
    }

    if has_embedded_file_marker(open_tag)
        || inner.to_ascii_lowercase().contains("data-slack-file-id")
    {
        if let Some(media) = parse_html_embedded_file_media(open_tag, inner, resolve_emoji) {
            return Some(media);
        }
        if let Some(doc) = parse_html_doc_embed(open_tag, inner, resolve_emoji) {
            return Some(doc);
        }
    }

    if has_doc_embed_marker(open_tag)
        && let Some(doc) = parse_html_doc_embed(open_tag, inner, resolve_emoji)
    {
        return Some(doc);
    }

    if let Some((control_inner, remaining_text)) = extract_sole_tag(inner, "control", resolve_emoji)
        && remaining_text.is_empty()
        && let Some(card) = parse_control_profile_card(control_inner, resolve_emoji)
    {
        return Some(card);
    }

    let sole_link = extract_sole_tag_with_attrs(inner, "a", resolve_emoji)
        .or_else(|| extract_sole_tag_with_attrs(inner, "lnk", resolve_emoji));
    if let Some((link_tag, link_body, remaining_text)) = sole_link
        && remaining_text.is_empty()
        && let Some(raw_href) = extract_attr(link_tag, "href")
    {
        let href = decode_entities(&raw_href);
        let label = clean_inline_html(link_body, resolve_emoji);
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

fn extract_sole_tag<'a>(
    html: &'a str,
    tag: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<(&'a str, String)> {
    let (_, inner, outside) = extract_sole_tag_with_attrs(html, tag, resolve_emoji)?;
    Some((inner, outside))
}

fn extract_sole_tag_with_attrs<'a>(
    html: &'a str,
    tag: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<(&'a str, &'a str, String)> {
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
    let mut outside = clean_inline_html(before, resolve_emoji);
    outside.push_str(&clean_inline_html(after, resolve_emoji));
    Some((raw_tag, inner, outside))
}

fn is_markdown_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.contains('|')
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

fn extract_emoji_from_img_tag(
    raw_tag: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if let Some(code) = extract_attr(raw_tag, "data-emoji") {
        let bare = code.trim_matches(':');
        return resolve_emoji(bare).or_else(|| Some(format!(":{bare}:")));
    }
    let alt = extract_attr(raw_tag, "alt")?;
    let trimmed = alt.trim();
    if trimmed.starts_with(':') && trimmed.ends_with(':') && trimmed.len() > 2 {
        let bare = &trimmed[1..trimmed.len() - 1];
        return resolve_emoji(bare).or_else(|| Some(trimmed.to_owned()));
    }
    let lower_tag = raw_tag.to_ascii_lowercase();
    let looks_like_emoji = lower_tag.contains("emoji")
        || extract_attr(raw_tag, "src")
            .map(|s| s.to_ascii_lowercase().contains("emoji"))
            .unwrap_or(false);
    if looks_like_emoji && !trimmed.is_empty() {
        let bare = trimmed.trim_matches(':');
        if let Some(glyph) = resolve_emoji(bare) {
            return Some(glyph);
        }
    }
    None
}

fn expand_inline_emoji_tags(html: &str, resolve_emoji: &dyn Fn(&str) -> Option<String>) -> String {
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
                if let Some(emoji) = extract_control_emoji(inner, resolve_emoji) {
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
            && let Some(emoji) = extract_emoji_from_img_tag(trimmed, resolve_emoji)
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

fn extract_control_emoji(
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let img_pos = find_case_insensitive(inner, "<img")?;
    let after_img = &inner[img_pos + 1..];
    let gt = after_img.find('>')?;
    let raw_img = &after_img[..gt];
    if let Some(glyph) = extract_emoji_from_img_tag(raw_img, resolve_emoji) {
        return Some(glyph);
    }
    let alt = extract_attr(raw_img, "alt")?;
    let bare = alt.trim().trim_matches(':');
    if bare.is_empty() {
        return None;
    }
    resolve_emoji(bare).or_else(|| {
        if !bare.is_ascii() {
            Some(bare.to_owned())
        } else {
            None
        }
    })
}

fn clean_inline_html(inner: &str, resolve_emoji: &dyn Fn(&str) -> Option<String>) -> String {
    let expanded = expand_inline_emoji_tags(inner, resolve_emoji);
    replace_shortcodes_with(&clean_text(&strip_tags(&expanded)), resolve_emoji)
}

fn clean_multiline_html(inner: &str, resolve_emoji: &dyn Fn(&str) -> Option<String>) -> String {
    let expanded = expand_inline_emoji_tags(inner, resolve_emoji);
    let with_breaks = replace_line_break_tags(&expanded);
    let stripped = strip_tags(&with_breaks);
    let decoded = decode_entities(&stripped);
    decoded
        .lines()
        .map(|l| {
            let collapsed = l.split_whitespace().collect::<Vec<_>>().join(" ");
            replace_shortcodes_with(&collapsed, resolve_emoji)
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

fn extract_list_items(
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Vec<(bool, String)> {
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
        let text = clean_inline_html(body, resolve_emoji);
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

fn parse_html_table(
    inner: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> Option<(Vec<String>, Vec<Vec<String>>)> {
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
                cells.push(clean_inline_html(&cell_rest[..cend], resolve_emoji));
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
        if is_upper_alnum_id(candidate, b"CG") {
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
            if is_upper_alnum_id(&cid, b"CG") {
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
        return is_upper_alnum_id(cid, b"CG");
    }
    if let Some(body) = inner.strip_prefix("!subteam^") {
        let gid = body.split('|').next().unwrap_or(body).trim();
        return is_upper_alnum_id(gid, b"S");
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

fn clean_text(s: &str) -> String {
    decode_entities(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canvas_html_blocks_and_section_ids() {
        let html = r#"<html><head><style>body{color:red}</style></head><body>
            <h1 id="temp:C:h1">Architecture &amp; Plan</h1>
            <p id="temp:C:p1">Overview of the <b>native</b> reader.</p>
            <ul id="temp:C:chk1" class="checklist">
                <li data-checked="true">Fetch HTML</li>
                <li data-checked="false">Add find bar</li>
            </ul>
            <pre><code>cargo test</code></pre>
            <blockquote>Line 1<br/>Line 2</blockquote>
            <hr/>
            <table><tr><th>Owner</th><th>Task</th></tr><tr><td>Ana</td><td>Ship</td></tr></table>
        </body></html>"#;
        let (blocks, ids) = parse_canvas_html_with_ids(html);
        assert_eq!(
            blocks,
            vec![
                CanvasBlock::Heading {
                    level: 1,
                    text: "Architecture & Plan".into(),
                },
                CanvasBlock::Paragraph("Overview of the native reader.".into()),
                CanvasBlock::Checklist(vec![
                    (true, "Fetch HTML".into()),
                    (false, "Add find bar".into()),
                ]),
                CanvasBlock::CodeBlock("cargo test".into()),
                CanvasBlock::Quote("Line 1\nLine 2".into()),
                CanvasBlock::Divider,
                CanvasBlock::Table {
                    header: vec!["Owner".into(), "Task".into()],
                    rows: vec![vec!["Ana".into(), "Ship".into()]],
                },
            ]
        );
        assert_eq!(ids[0].as_deref(), Some("temp:C:h1"));
        assert_eq!(ids[1].as_deref(), Some("temp:C:p1"));
        assert_eq!(ids[2].as_deref(), Some("temp:C:chk1"));
        assert!(blocks[0].matches("architecture"));
        assert!(!blocks[0].matches("cargo"));
    }

    #[test]
    fn parses_canvas_markdown_and_roundtrips() {
        let md = "# Runbook\n\n- Alpha\n- Beta\n\n1. First\n2. Second\n\n- [x] Done\n- [ ] Todo\n\n| Col A | Col B |\n| --- | --- |\n| 1 | a \\| b |\n\n---\n\n> Quoted\n> line 2\n\n```\nfn main() {}\n```";
        let doc = CanvasDoc::from_markdown("Runbook".into(), "Local".into(), md);
        assert_eq!(doc.blocks.len(), 8);
        assert_eq!(doc.outline_headings(), vec![(1, "Runbook")]);
        assert!(matches!(doc.blocks[4], CanvasBlock::Table { .. }));
        let reserialized = doc.to_markdown();
        let reparsed = parse_canvas_markdown(&reserialized);
        assert_eq!(doc.blocks, reparsed);
    }

    #[test]
    fn parses_urls_media_and_entities() {
        assert_eq!(
            parse_slack_profile_url("https://acme.slack.com/team/U01ALICE?x=1"),
            Some("U01ALICE".into())
        );
        assert_eq!(
            parse_slack_profile_url("https://acme.slack.com/team/bad"),
            None
        );

        assert_eq!(
            parse_slack_doc_url("https://acme.slack.com/docs/T01TEAM/F01CANVAS"),
            Some((CanvasEmbedKind::Canvas, "F01CANVAS".into()))
        );
        assert_eq!(
            parse_slack_doc_url("https://acme.slack.com/lists/T01TEAM/F02LIST"),
            Some((CanvasEmbedKind::List, "F02LIST".into()))
        );

        assert_eq!(
            classify_media_url("https://files.slack.com/demo.mp4?download=1"),
            Some((CanvasMediaKind::Video, "video/mp4"))
        );
        assert_eq!(
            classify_media_mime("audio/mpeg"),
            Some((CanvasMediaKind::Audio, "audio/mpeg".into()))
        );
        assert_eq!(
            filename_from_url("https://example.com/a/b/report.pdf?sig=1#top"),
            Some("report.pdf".into())
        );
        assert_eq!(
            decode_entities("A &amp; B &lt;C&gt; &quot;D&quot; &#39;E&#x27;&nbsp;F"),
            "A & B <C> \"D\" 'E' F"
        );
    }

    #[test]
    fn parses_inline_mentions_and_emoji_resolver() {
        let html = r#"<p>Ping <control><a href="https://acme.slack.com/team/U01ALICE">@Alice</a></control> :rocket:</p>"#;
        let (blocks, _) = parse_canvas_html_with_ids_and_emoji(html, &|code| {
            (code == "rocket").then(|| "🚀".to_owned())
        });
        assert_eq!(
            blocks,
            vec![CanvasBlock::Paragraph("Ping <@U01ALICE|Alice> 🚀".into())]
        );

        let segs = parse_inline_canvas_segments("Hi <@U01ALICE|Alice> in <#C01ENG|eng> @here!");
        assert_eq!(
            segs,
            vec![
                CanvasInlineSegment::Text("Hi ".into()),
                CanvasInlineSegment::UserMention {
                    user_id: "U01ALICE".into(),
                    label: Some("Alice".into()),
                },
                CanvasInlineSegment::Text(" in ".into()),
                CanvasInlineSegment::ChannelMention {
                    channel_id: "C01ENG".into(),
                    name: Some("eng".into()),
                },
                CanvasInlineSegment::Text(" ".into()),
                CanvasInlineSegment::BroadcastMention("here".into()),
                CanvasInlineSegment::Text("!".into()),
            ]
        );
    }

    #[test]
    fn canvas_doc_block_mutations_preserve_alignment() {
        let mut doc = CanvasDoc::from_html(
            "Test".into(),
            r#"<h1 id="s1">One</h1><p id="s2">Two</p><ul id="s3" class="checklist"><li data-checked="false">Item</li></ul>"#,
        );
        assert!(doc.reorder_block(0, 1));
        assert_eq!(
            doc.section_ids,
            vec![Some("s2".into()), Some("s1".into()), Some("s3".into())]
        );
        let toggled = doc.toggle_checklist_item(2, 0).expect("toggle");
        assert_eq!(toggled, "- [x] Item");
        let mut table = CanvasTable {
            header: vec!["A".into()],
            rows: vec![vec!["1".into()]],
        };
        table.add_column();
        table.add_row();
        assert_eq!(table.header, vec!["A", "Column 2"]);
        assert_eq!(table.rows.len(), 2);
    }
}
