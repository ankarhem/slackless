//! Pure, GPUI-free Slack Canvas document model, Quip/Canvas HTML & Markdown parsers,
//! inline mention tokenizer, and media/embed classifiers (`F-COLLAB-125..150`).

use std::rc::Rc;

use html5ever::ParseOpts;
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

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
///
/// The markup goes through html5ever, so attribute names match exactly (`id` is never
/// `data-id`), `>` inside quoted values or comments cannot truncate a tag, entities are decoded
/// once and unclosed tags are closed the way a browser would.
pub fn parse_canvas_html_with_ids_and_emoji(
    html: &str,
    resolve_emoji: &dyn Fn(&str) -> Option<String>,
) -> (Vec<CanvasBlock>, Vec<Option<String>>) {
    let dom = html5ever::parse_document(RcDom::default(), ParseOpts::default()).one(html);
    let mut parser = HtmlParser::new(resolve_emoji);
    parser.walk_children(&body_of(&dom.document));
    parser.flush_text();
    (parser.blocks, parser.ids)
}

/// `<body>` of a parsed document (the HTML5 tree builder always synthesizes `html > body`).
fn body_of(document: &Handle) -> Handle {
    let element = |parent: &Handle, name: &str| {
        parent
            .children
            .borrow()
            .iter()
            .find(|node| is_element(node, name))
            .cloned()
    };
    element(document, "html")
        .and_then(|html| element(&html, "body"))
        .unwrap_or_else(|| document.clone())
}

/// Tag name of an element node.
fn tag(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(&name.local),
        _ => None,
    }
}

fn is_element(node: &Handle, name: &str) -> bool {
    tag(node) == Some(name)
}

/// Value of attribute `name` (the tokenizer lowercases names and decodes entities in values).
fn attr(node: &Handle, name: &str) -> Option<String> {
    let NodeData::Element { attrs, .. } = &node.data else {
        return None;
    };
    attrs
        .borrow()
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| a.value.to_string())
}

/// Whether an element carries attribute `name` (whatever its value).
fn has_attr(node: &Handle, name: &str) -> bool {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs.borrow().iter().any(|a| &*a.name.local == name),
        _ => false,
    }
}

/// Attribute `name` of `link` when present, else of its `node` container.
fn attr_of(link: Option<&Handle>, node: &Handle, name: &str) -> Option<String> {
    link.and_then(|link| attr(link, name))
        .or_else(|| attr(node, name))
}

/// Lowercased attribute names and values: the haystack for Quip's class / `data-*` markers.
fn marker_haystack(node: &Handle) -> String {
    let NodeData::Element { attrs, .. } = &node.data else {
        return String::new();
    };
    attrs
        .borrow()
        .iter()
        .flat_map(|a| [&*a.name.local, " ", &*a.value, " "])
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Descendant nodes in document order.
fn descendants(node: &Handle) -> impl Iterator<Item = Handle> {
    let mut stack = node
        .children
        .borrow()
        .iter()
        .rev()
        .cloned()
        .collect::<Vec<_>>();
    std::iter::from_fn(move || {
        let next = stack.pop()?;
        stack.extend(next.children.borrow().iter().rev().cloned());
        Some(next)
    })
}

/// First element descendant (document order) with one of `names`.
fn first_descendant(node: &Handle, names: &[&str]) -> Option<Handle> {
    descendants(node).find(|n| names.iter().any(|name| is_element(n, name)))
}

/// Whether a `<p>` holds a block element: quirks-mode HTML (no doctype) lets a `<table>` nest
/// inside an open `<p>`, which then has to be walked instead of flattened to text.
fn has_block_child(node: &Handle) -> bool {
    node.children.borrow().iter().any(|child| {
        matches!(
            tag(child),
            Some(
                "table"
                    | "ul"
                    | "ol"
                    | "div"
                    | "blockquote"
                    | "pre"
                    | "hr"
                    | "p"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "audio"
                    | "video"
                    | "section"
            )
        )
    })
}

/// Tags whose boundaries break lines when inline text is collected.
const BLOCK_BOUNDARY_TAGS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "figure",
    "figcaption",
    "blockquote",
    "pre",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "tr",
    "td",
    "th",
];

fn is_block_boundary(tag: &str) -> bool {
    BLOCK_BOUNDARY_TAGS.contains(&tag)
}

/// Whether an `<li>` is a done checklist item (`data-checked="true"`, `class="checked"` or a
/// checked `<input>`).
fn list_item_checked(li: &Handle) -> bool {
    let has_checked_class = |node: &Handle| {
        attr(node, "class").is_some_and(|class| class.split_whitespace().any(|c| c == "checked"))
    };
    attr(li, "data-checked").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || has_attr(li, "checked")
        || has_checked_class(li)
        || descendants(li).any(|n| has_attr(&n, "checked") || has_checked_class(&n))
}

/// A comment node that is really a preserved mrkdwn token (`<!subteam^S01DES|@design>` is
/// tokenized as a bogus comment), rendered back as text.
fn comment_token(contents: &str) -> Option<String> {
    is_preserved_slack_mention_token(&format!("!{contents}")).then(|| format!("<!{contents}>"))
}

/// DOM walker producing blocks from the `<body>`. Text nodes and inline elements accumulate in
/// `text` until the next block boundary flushes them as a paragraph.
struct HtmlParser<'a> {
    resolve_emoji: &'a dyn Fn(&str) -> Option<String>,
    blocks: Vec<CanvasBlock>,
    ids: Vec<Option<String>>,
    text: String,
}

impl<'a> HtmlParser<'a> {
    fn new(resolve_emoji: &'a dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            resolve_emoji,
            blocks: Vec::new(),
            ids: Vec::new(),
            text: String::new(),
        }
    }

    fn walk_children(&mut self, parent: &Handle) {
        for child in &*parent.children.borrow() {
            self.walk(child);
        }
    }

    fn walk(&mut self, node: &Handle) {
        match &node.data {
            NodeData::Text { contents } => self.text.push_str(&contents.borrow()),
            NodeData::Comment { contents } => {
                if let Some(token) = comment_token(contents) {
                    self.text.push_str(&token);
                }
            }
            NodeData::Element { name, .. } => match &*name.local {
                heading @ ("h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                    self.flush_text();
                    let text = self.line_text(node, None);
                    if !text.is_empty() {
                        let level = match heading {
                            "h1" => 1,
                            "h2" => 2,
                            _ => 3,
                        };
                        self.push(CanvasBlock::Heading { level, text }, node);
                    }
                }
                "pre" => {
                    self.flush_text();
                    let code = self.raw_text(node);
                    if !code.trim().is_empty() {
                        self.push(CanvasBlock::CodeBlock(code.trim().to_owned()), node);
                    }
                }
                "blockquote" => {
                    self.flush_text();
                    let lines = self.line_texts(node).join("\n");
                    if !lines.is_empty() {
                        self.push(CanvasBlock::Quote(lines), node);
                    }
                }
                "ul" | "ol" => self.list(node),
                "table" => self.table(node),
                "hr" => {
                    self.flush_text();
                    self.push(CanvasBlock::Divider, node);
                }
                "img" => {
                    if let Some(glyph) = self.emoji_from_img(node) {
                        self.text.push_str(&glyph);
                    } else {
                        self.flush_text();
                        if let Some(block) = self.image_block(node) {
                            self.push(block, node);
                        }
                    }
                }
                media @ ("audio" | "video") => {
                    self.flush_text();
                    let kind = if media == "video" {
                        CanvasMediaKind::Video
                    } else {
                        CanvasMediaKind::Audio
                    };
                    if let Some(block) = self.media_tag(kind, node) {
                        self.push(block, node);
                    }
                }
                "control" => {
                    self.flush_text();
                    if let Some(card) = self.control_profile_card(node) {
                        self.push(card, node);
                    } else {
                        let mut text = std::mem::take(&mut self.text);
                        self.collect_text(node, false, None, &mut text);
                        self.text = text;
                    }
                }
                "p" | "div" | "section" => self.container(node),
                "head" | "script" | "style" | "title" | "template" | "noscript" => {}
                // Inline or unknown wrappers are transparent; the separators keep adjacent
                // blocks' words from gluing together.
                _ => {
                    self.text.push('\n');
                    self.walk_children(node);
                    self.text.push('\n');
                }
            },
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::ProcessingInstruction { .. } => {}
        }
    }

    fn push(&mut self, block: CanvasBlock, node: &Handle) {
        self.blocks.push(block);
        self.ids.push(attr(node, "id").filter(|id| !id.is_empty()));
    }

    /// Emits the accumulated inline text as a paragraph.
    fn flush_text(&mut self) {
        let text = std::mem::take(&mut self.text);
        let text = self.resolve_line(&text);
        if !text.is_empty() {
            self.blocks.push(CanvasBlock::Paragraph(text));
            self.ids.push(None);
        }
    }

    /// `<p>`, `<div>` and `<section>`: a Quip unfurl/embed card when they carry its markers or
    /// wrap nothing but a single embed; otherwise the paragraph text or, for containers, their
    /// child blocks.
    fn container(&mut self, node: &Handle) {
        self.flush_text();
        if let Some(block) = self.embedded_block(node) {
            self.push(block, node);
        } else if is_element(node, "p") && !has_block_child(node) {
            let text = self.line_text(node, None);
            if !text.is_empty() {
                self.push(CanvasBlock::Paragraph(text), node);
            }
        } else {
            self.walk_children(node);
        }
    }

    fn embedded_block(&self, node: &Handle) -> Option<CanvasBlock> {
        let markers = marker_haystack(node);
        let has_marker = |names: &[&str]| names.iter().any(|name| markers.contains(*name));
        let file_marker = has_marker(&["embedded-file", "media-embed", "data-slack-file-id"])
            || descendants(node).any(|n| has_attr(&n, "data-slack-file-id"));
        if (has_marker(&["user_unfurl", "user-unfurl", "profile-card", "profile_card"])
            || attr(node, "data-user-id").is_some_and(|id| is_slack_user_id(&id)))
            && let Some(card) = self.profile_card(node)
        {
            return Some(card);
        }
        if file_marker {
            if let Some(media) = self.embedded_file_media(node) {
                return Some(media);
            }
            if let Some(doc) = self.doc_embed(node) {
                return Some(doc);
            }
        }
        if has_marker(&[
            "canvas_unfurl",
            "canvas-unfurl",
            "list_unfurl",
            "list-unfurl",
            "embedded-link",
        ]) && let Some(doc) = self.doc_embed(node)
        {
            return Some(doc);
        }
        let sole = first_descendant(node, &["control", "a", "lnk", "img"])?;
        if !self.line_text(node, Some(&sole)).is_empty() {
            return None;
        }
        match tag(&sole) {
            Some("control") => self.control_profile_card(&sole),
            Some("img") => self.image_block(&sole),
            _ => self.link_embed(&sole),
        }
    }

    fn list(&mut self, list: &Handle) {
        self.flush_text();
        let style = attr(list, "data-section-style");
        let checklist = marker_haystack(list).contains("checklist")
            || style.as_deref() == Some("7")
            || descendants(list).any(|n| {
                has_attr(&n, "data-checked")
                    || (is_element(&n, "input") && attr(&n, "type").as_deref() == Some("checkbox"))
            });
        let items: Vec<(bool, String)> = descendants(list)
            .filter(|n| is_element(n, "li"))
            .map(|li| (list_item_checked(&li), self.item_text(&li)))
            .filter(|(_, text)| !text.is_empty())
            .collect();
        if items.is_empty() {
            return;
        }
        let numbered = is_element(list, "ol") || style.as_deref() == Some("6");
        let block = if checklist {
            CanvasBlock::Checklist(items)
        } else {
            let texts = items.into_iter().map(|(_, text)| text).collect();
            if numbered {
                CanvasBlock::NumberedList(texts)
            } else {
                CanvasBlock::Bullets(texts)
            }
        };
        self.push(block, list);
    }

    /// Rows are every `<tr>` descendant (so `thead`/`tbody` groupings do not matter) padded to
    /// the widest row; the first row is the header.
    fn table(&mut self, table: &Handle) {
        self.flush_text();
        let mut rows: Vec<Vec<String>> = descendants(table)
            .filter(|n| is_element(n, "tr"))
            .map(|tr| {
                tr.children
                    .borrow()
                    .iter()
                    .filter(|cell| matches!(tag(cell), Some("td" | "th")))
                    .map(|cell| self.line_text(cell, None))
                    .collect::<Vec<String>>()
            })
            .filter(|cells| !cells.is_empty())
            .collect();
        if rows.is_empty() {
            return;
        }
        let width = rows.iter().map(Vec::len).max().unwrap_or(1);
        for row in &mut rows {
            row.resize(width, String::new());
        }
        let header = rows.remove(0);
        self.push(CanvasBlock::Table { header, rows }, table);
    }

    /// Image block for an `<img>` with an `http(s)` source.
    fn image_block(&self, img: &Handle) -> Option<CanvasBlock> {
        let url = attr(img, "src")
            .filter(|src| src.starts_with("https://") || src.starts_with("http://"))?;
        let size =
            |name: &str| attr(img, name).and_then(|v| v.trim_end_matches("px").parse::<u32>().ok());
        Some(CanvasBlock::Image {
            url,
            alt: attr(img, "alt")
                .map(|alt| self.resolve_line(&alt))
                .unwrap_or_default(),
            width: size("width"),
            height: size("height"),
        })
    }

    /// Glyph for an emoji `<img>`: `data-emoji`, `alt=":shortcode:"`, or an emoji-class image.
    fn emoji_from_img(&self, img: &Handle) -> Option<String> {
        if let Some(code) = attr(img, "data-emoji") {
            let bare = code.trim_matches(':');
            return Some(self.resolve(bare).unwrap_or_else(|| format!(":{bare}:")));
        }
        let alt = attr(img, "alt")?;
        let trimmed = alt.trim();
        if let Some(bare) = trimmed
            .strip_prefix(':')
            .and_then(|b| b.strip_suffix(':'))
            .filter(|b| !b.is_empty())
        {
            return Some(self.resolve(bare).unwrap_or_else(|| trimmed.to_owned()));
        }
        if marker_haystack(img).contains("emoji") && !trimmed.is_empty() {
            return self.resolve(trimmed.trim_matches(':'));
        }
        None
    }

    /// `<audio>` / `<video>` block: source from `src`, a nested `<source>` or a fallback link;
    /// title from `title` / `data-title` / `aria-label`, else the inner text or file name.
    fn media_tag(&self, kind: CanvasMediaKind, node: &Handle) -> Option<CanvasBlock> {
        let source = first_descendant(node, &["source"]);
        let link = first_descendant(node, &["a"]).or_else(|| first_descendant(node, &["lnk"]));
        let url = attr(node, "src")
            .or_else(|| source.as_ref().and_then(|s| attr(s, "src")))
            .or_else(|| link.as_ref().and_then(|l| attr(l, "href")))
            .filter(|src| !src.is_empty())?;
        let title = attr(node, "title")
            .or_else(|| attr(node, "data-title"))
            .or_else(|| attr(node, "aria-label"))
            .or_else(|| {
                let text = self.line_text(node, None);
                (!text.is_empty()).then_some(text)
            })
            .or_else(|| filename_from_url(&url))
            .map(|title| self.resolve_line(&title))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| format!("{} clip", kind.label()));
        let mime = attr(node, "data-mime")
            .or_else(|| attr(node, "type"))
            .or_else(|| source.as_ref().and_then(|s| attr(s, "type")))
            .or_else(|| classify_media_url(&url).map(|(_, mime)| mime.to_owned()));
        Some(CanvasBlock::MediaEmbed {
            kind,
            title,
            url,
            file_id: attr(node, "data-slack-file-id")
                .or_else(|| attr(node, "data-file-id"))
                .map(|id| strip_slack_file_prefix(&id))
                .filter(|id| !id.is_empty()),
            duration: attr(node, "data-duration")
                .or_else(|| attr(node, "duration"))
                .filter(|d| !d.is_empty()),
            mime,
        })
    }

    /// Profile card for `<control><a href=".../team/U...">@U...</a></control>`.
    fn control_profile_card(&self, control: &Handle) -> Option<CanvasBlock> {
        let link =
            first_descendant(control, &["a"]).or_else(|| first_descendant(control, &["lnk"]))?;
        let body = self.line_text(&link, None);
        let user_id = attr(&link, "data-user-id")
            .filter(|id| is_slack_user_id(id))
            .or_else(|| attr(&link, "href").and_then(|href| parse_slack_profile_url(&href)))
            .or_else(|| {
                let bare = body.trim().trim_start_matches('@');
                is_slack_user_id(bare).then(|| bare.to_owned())
            })?;
        let name = body.trim();
        let display_name =
            (!name.is_empty() && name.trim_start_matches('@') != user_id).then(|| name.to_owned());
        Some(CanvasBlock::ProfileCard {
            user_id,
            display_name,
            title: attr(&link, "data-title").filter(|t| !t.is_empty()),
            handle: attr(&link, "data-handle").filter(|h| !h.is_empty()),
        })
    }

    /// Profile card for a `user_unfurl` container: id from `data-user-id`, the link or the text.
    fn profile_card(&self, node: &Handle) -> Option<CanvasBlock> {
        let link = first_descendant(node, &["a"]).or_else(|| first_descendant(node, &["lnk"]));
        let link = link.as_ref();
        let link_text = link.map(|l| self.line_text(l, None)).unwrap_or_default();
        let inner_text = self.line_text(node, None);
        let user_id = attr(node, "data-user-id")
            .filter(|id| is_slack_user_id(id))
            .or_else(|| attr_of(link, node, "data-user-id").filter(|id| is_slack_user_id(id)))
            .or_else(|| attr_of(link, node, "href").and_then(|href| parse_slack_profile_url(&href)))
            .or_else(|| {
                inner_text.split_whitespace().find_map(|token| {
                    let bare = token
                        .trim_start_matches('@')
                        .trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
                    is_slack_user_id(bare).then(|| bare.to_owned())
                })
            })?;
        let title = attr(node, "data-title")
            .or_else(|| attr(node, "data-role"))
            .or_else(|| {
                let span = first_descendant(node, &["span"])?;
                let markers = marker_haystack(&span);
                let text = self.line_text(&span, None);
                ((markers.contains("title") || markers.contains("role")) && !text.is_empty())
                    .then_some(text)
            })
            .filter(|title| !title.is_empty());
        let display_name = attr(node, "data-display-name")
            .or_else(|| attr(node, "data-name"))
            .or_else(|| attr(node, "data-user-name"))
            .or_else(|| {
                (!link_text.is_empty() && link_text.trim_start_matches('@') != user_id)
                    .then_some(link_text)
            })
            .or_else(|| {
                (!inner_text.is_empty()
                    && inner_text.trim_start_matches('@') != user_id
                    && Some(inner_text.as_str()) != title.as_deref())
                .then_some(inner_text)
            });
        Some(CanvasBlock::ProfileCard {
            user_id,
            display_name,
            title,
            handle: attr(node, "data-handle")
                .or_else(|| attr(node, "data-username"))
                .map(|handle| handle.trim_start_matches('@').to_owned())
                .filter(|handle| !handle.is_empty()),
        })
    }

    /// Canvas / List card for an `embedded-link` / `*_unfurl` container.
    fn doc_embed(&self, node: &Handle) -> Option<CanvasBlock> {
        let link = first_descendant(node, &["a"]).or_else(|| first_descendant(node, &["lnk"]));
        let link = link.as_ref();
        let link_text = link.map(|l| self.line_text(l, None)).unwrap_or_default();
        let url = attr_of(link, node, "href")
            .or_else(|| attr(node, "data-url"))
            .unwrap_or_default();
        let markers = marker_haystack(node);
        let doc_kind = attr(node, "data-doc-kind");
        let (kind, file_id) = match parse_slack_doc_url(&url) {
            Some((kind, file_id)) => (kind, file_id),
            None => {
                let is_list = markers.contains("list_unfurl")
                    || markers.contains("list-unfurl")
                    || doc_kind
                        .as_deref()
                        .is_some_and(|k| k.eq_ignore_ascii_case("list"));
                let is_canvas = markers.contains("canvas_unfurl")
                    || markers.contains("canvas-unfurl")
                    || doc_kind
                        .as_deref()
                        .is_some_and(|k| k.eq_ignore_ascii_case("canvas"));
                let kind = if is_list {
                    CanvasEmbedKind::List
                } else if is_canvas {
                    CanvasEmbedKind::Canvas
                } else {
                    return None;
                };
                let file_id = attr_of(link, node, "data-slack-file-id")
                    .or_else(|| attr_of(link, node, "data-file-id"))
                    .map(|id| strip_slack_file_prefix(&id))
                    .filter(|id| is_slack_file_id(id))?;
                (kind, file_id)
            }
        };
        let inner_text = self.line_text(node, None);
        let title = attr(node, "data-title")
            .or_else(|| attr_of(link, node, "data-title"))
            .or_else(|| (!link_text.is_empty() && link_text != url).then_some(link_text))
            .or_else(|| (!inner_text.is_empty() && inner_text != url).then_some(inner_text))
            .unwrap_or_else(|| format!("{} {file_id}", kind.label()));
        let url = if url.is_empty() {
            let segment = match kind {
                CanvasEmbedKind::Canvas => "docs",
                CanvasEmbedKind::List => "lists",
            };
            format!("https://slack.com/{segment}/T0/{file_id}")
        } else {
            url
        };
        Some(CanvasBlock::DocEmbed {
            kind,
            file_id,
            title,
            url,
        })
    }

    /// Media clip for an `embedded-file` container: `<lnk>` or `<a>` carrying the file id, URL,
    /// MIME type and duration.
    fn embedded_file_media(&self, node: &Handle) -> Option<CanvasBlock> {
        let link = first_descendant(node, &["lnk"]).or_else(|| first_descendant(node, &["a"]));
        let link = link.as_ref();
        let link_text = link.map(|l| self.line_text(l, None)).unwrap_or_default();
        let inner_text = self.line_text(node, None);
        let url = link
            .and_then(|l| attr(l, "href"))
            .or_else(|| link.and_then(|l| attr(l, "src")))
            .or_else(|| attr(node, "href"))
            .or_else(|| attr(node, "data-url"))
            .unwrap_or_default();
        let duration = attr_of(link, node, "data-duration").filter(|d| !d.is_empty());
        let file_id = attr_of(link, node, "data-slack-file-id")
            .or_else(|| attr_of(link, node, "data-file-id"))
            .map(|id| strip_slack_file_prefix(&id))
            .filter(|id| !id.is_empty());
        let raw_title = attr_of(link, node, "data-title")
            .or_else(|| attr_of(link, node, "title"))
            .or_else(|| (!link_text.is_empty()).then_some(link_text))
            .or_else(|| (!inner_text.is_empty()).then_some(inner_text))
            .or_else(|| filename_from_url(&url))
            .unwrap_or_default();
        let (kind, mime) = attr_of(link, node, "data-mime")
            .or_else(|| attr_of(link, node, "type"))
            .or_else(|| attr_of(link, node, "data-filetype"))
            .and_then(|mime| classify_media_mime(&mime))
            .or_else(|| classify_media_url(&url).map(|(k, m)| (k, m.to_owned())))
            .or_else(|| classify_media_url(&raw_title).map(|(k, m)| (k, m.to_owned())))?;
        let title = if raw_title.is_empty() {
            format!("{} clip", kind.label())
        } else {
            self.resolve_line(&raw_title)
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

    /// Embed for a paragraph holding nothing but one link: a canvas/list, a profile or a media
    /// clip according to the href.
    fn link_embed(&self, link: &Handle) -> Option<CanvasBlock> {
        let href = attr(link, "href")?;
        let label = self.line_text(link, None);
        let bare_label = label.trim_start_matches('@');
        if let Some((kind, file_id)) = parse_slack_doc_url(&href) {
            let title = if label.is_empty() || label == href {
                format!("{} {file_id}", kind.label())
            } else {
                label
            };
            return Some(CanvasBlock::DocEmbed {
                kind,
                file_id,
                title,
                url: href,
            });
        }
        if let Some(user_id) = parse_slack_profile_url(&href) {
            let display_name =
                (!label.is_empty() && bare_label != user_id && label != href).then_some(label);
            return Some(CanvasBlock::ProfileCard {
                user_id,
                display_name,
                title: attr(link, "data-title").filter(|t| !t.is_empty()),
                handle: attr(link, "data-handle").filter(|h| !h.is_empty()),
            });
        }
        if let Some((kind, mime)) = classify_media_url(&href).or_else(|| classify_media_url(&label))
        {
            let title = if label.is_empty() {
                filename_from_url(&href).unwrap_or_else(|| format!("{} clip", kind.label()))
            } else {
                label
            };
            return Some(CanvasBlock::MediaEmbed {
                kind,
                title,
                url: href,
                file_id: attr(link, "data-slack-file-id")
                    .map(|id| strip_slack_file_prefix(&id))
                    .filter(|id| !id.is_empty()),
                duration: attr(link, "data-duration").filter(|d| !d.is_empty()),
                mime: Some(mime.to_owned()),
            });
        }
        None
    }

    /// Glyph for `<control><img alt="rocket" .../></control>`-style emoji.
    fn emoji_from_control(&self, control: &Handle) -> Option<String> {
        let img = first_descendant(control, &["img"])?;
        if let Some(glyph) = self.emoji_from_img(&img) {
            return Some(glyph);
        }
        let alt = attr(&img, "alt")?;
        let bare = alt.trim().trim_matches(':');
        if bare.is_empty() {
            return None;
        }
        self.resolve(bare)
            .or_else(|| (!bare.is_ascii()).then(|| bare.to_owned()))
    }

    /// Canonical mrkdwn mention token for `<control><a ...>@U... / #C...</a></control>`.
    fn control_mention_token(&self, control: &Handle) -> Option<String> {
        let anchor = first_descendant(control, &["a"])?;
        let text = self.line_text(&anchor, None);
        if let Some(uid) = text
            .strip_prefix('@')
            .map(str::trim)
            .filter(|uid| is_slack_user_id(uid))
        {
            return Some(format!("<@{uid}>"));
        }
        if let Some(cid) = text
            .strip_prefix('#')
            .map(str::trim)
            .filter(|cid| is_upper_alnum_id(cid, b"CG"))
        {
            return Some(format!("<#{cid}>"));
        }
        let href = attr(&anchor, "href")?;
        let label = text.trim_start_matches(['@', '#']).trim();
        if let Some(uid) = parse_slack_profile_url(&href) {
            return Some(if label.is_empty() || label == uid {
                format!("<@{uid}>")
            } else {
                format!("<@{uid}|{label}>")
            });
        }
        let cid: String = href
            .split("/archives/")
            .nth(1)?
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        if is_upper_alnum_id(&cid, b"CG") {
            return Some(if label.is_empty() || label == cid {
                format!("<#{cid}>")
            } else {
                format!("<#{cid}|{label}>")
            });
        }
        None
    }

    /// Appends the inline text of `node`'s subtree to `out`: tags vanish, `<br>` and block
    /// boundaries become line breaks, emoji `<img>` / `<control>` become glyphs and mention
    /// controls become mrkdwn tokens. The subtree rooted at `skip` is ignored. `raw` keeps the
    /// parsed whitespace verbatim (`<pre>`).
    fn collect_text(&self, node: &Handle, raw: bool, skip: Option<&Handle>, out: &mut String) {
        if skip.is_some_and(|skip| Rc::ptr_eq(skip, node)) {
            return;
        }
        match &node.data {
            NodeData::Text { contents } => out.push_str(&contents.borrow()),
            NodeData::Comment { contents } => {
                if let Some(token) = comment_token(contents) {
                    out.push_str(&token);
                }
            }
            NodeData::Element { name, .. } => {
                let tag_name = &*name.local;
                if raw {
                    if tag_name == "br" {
                        out.push('\n');
                    } else {
                        self.collect_children(node, true, skip, out);
                    }
                    return;
                }
                match tag_name {
                    "br" => out.push('\n'),
                    "img" => {
                        if let Some(glyph) = self.emoji_from_img(node) {
                            out.push_str(&glyph);
                        }
                    }
                    "control" => {
                        if let Some(token) = self
                            .emoji_from_control(node)
                            .or_else(|| self.control_mention_token(node))
                        {
                            out.push_str(&token);
                        } else {
                            self.collect_children(node, false, skip, out);
                        }
                    }
                    "head" | "script" | "style" | "title" | "template" | "noscript" => {}
                    _ => {
                        if is_block_boundary(tag_name) {
                            out.push('\n');
                        }
                        self.collect_children(node, false, skip, out);
                        if is_block_boundary(tag_name) {
                            out.push('\n');
                        }
                    }
                }
            }
            NodeData::Document
            | NodeData::Doctype { .. }
            | NodeData::ProcessingInstruction { .. } => {}
        }
    }

    fn collect_children(&self, node: &Handle, raw: bool, skip: Option<&Handle>, out: &mut String) {
        for child in &*node.children.borrow() {
            self.collect_text(child, raw, skip, out);
        }
    }

    /// Inline text collapsed onto one line, `:shortcode:` emoji resolved.
    fn line_text(&self, node: &Handle, skip: Option<&Handle>) -> String {
        let mut out = String::new();
        self.collect_text(node, false, skip, &mut out);
        self.resolve_line(&out)
    }

    /// Inline text split at `<br>` / block boundaries into non-empty lines, emoji resolved.
    fn line_texts(&self, node: &Handle) -> Vec<String> {
        let mut out = String::new();
        self.collect_text(node, false, None, &mut out);
        out.split('\n')
            .map(|line| self.resolve_line(line))
            .filter(|line| !line.is_empty())
            .collect()
    }

    /// Verbatim text of a `<pre>` (entities already decoded by the parser, `<br>` as newlines).
    fn raw_text(&self, node: &Handle) -> String {
        let mut out = String::new();
        self.collect_text(node, true, None, &mut out);
        out
    }

    /// Text of an `<li>` without its nested sub-lists (whose items are flattened into the list).
    fn item_text(&self, li: &Handle) -> String {
        let mut out = String::new();
        for child in &*li.children.borrow() {
            if !matches!(tag(child), Some("ul" | "ol")) {
                self.collect_text(child, false, None, &mut out);
            }
        }
        self.resolve_line(&out)
    }

    /// Collapses whitespace and resolves `:shortcode:` emoji.
    fn resolve_line(&self, text: &str) -> String {
        replace_shortcodes_with(&collapse_whitespace(text), self.resolve_emoji)
    }

    fn resolve(&self, code: &str) -> Option<String> {
        (self.resolve_emoji)(code)
    }
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

/// Collapses runs of whitespace into single spaces.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
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

    #[test]
    fn html_parser_handles_markup_edge_cases() {
        use CanvasBlock::{Bullets, Divider, Heading, NumberedList, Paragraph, Table};
        let image = |alt: &str, width: Option<u32>| CanvasBlock::Image {
            url: "https://files.slack.com/a.png".into(),
            alt: alt.into(),
            width,
            height: None,
        };
        type Case = (&'static str, Vec<CanvasBlock>, Vec<Option<String>>);
        let cases: &[Case] = &[
            // Only an exact `id` attribute is a section id.
            (
                r#"<h1 data-id="x" aria-rowid="y" id="temp:C:h1">Title</h1><p gridid="z">Body</p>"#,
                vec![
                    Heading {
                        level: 1,
                        text: "Title".into(),
                    },
                    Paragraph("Body".into()),
                ],
                vec![Some("temp:C:h1".into()), None],
            ),
            // `>` inside a quoted attribute value neither truncates the tag nor leaks text.
            (
                r#"<img src="https://files.slack.com/a.png" alt="a > b" width="10"><p title="1 > 0">After</p>"#,
                vec![image("a > b", Some(10)), Paragraph("After".into())],
                vec![None, None],
            ),
            // Comments, including ones containing `>`, are dropped.
            (
                "<p>Before <!-- note: a > b --> <b>After</b></p><!-- top > level --><p>Next</p>",
                vec![Paragraph("Before After".into()), Paragraph("Next".into())],
                vec![None, None],
            ),
            // Uppercase tag/attribute names and unquoted attribute values.
            (
                r#"<H1 ID="temp:C:up">UP</H1><IMG SRC=https://files.slack.com/a.png ALT=diagram WIDTH=800><UL CLASS=checklist><LI DATA-CHECKED=true>A</LI></UL>"#,
                vec![
                    Heading {
                        level: 1,
                        text: "UP".into(),
                    },
                    image("diagram", Some(800)),
                    CanvasBlock::Checklist(vec![(true, "A".into())]),
                ],
                vec![Some("temp:C:up".into()), None, None],
            ),
            // Entities are decoded exactly once, in text and in attribute values.
            (
                r#"<p>Fish &amp; chips &lt;3 &quot;q&quot; &#39;s&apos; &nbsp;tail &amp;lt;literal&amp;gt;</p><p class=embedded-link><a href="https://acme.slack.com/docs/T01/F01SPEC?a=1&amp;b=2">Spec</a></p>"#,
                vec![
                    Paragraph("Fish & chips <3 \"q\" 's' tail &lt;literal&gt;".into()),
                    CanvasBlock::DocEmbed {
                        kind: CanvasEmbedKind::Canvas,
                        file_id: "F01SPEC".into(),
                        title: "Spec".into(),
                        url: "https://acme.slack.com/docs/T01/F01SPEC?a=1&b=2".into(),
                    },
                ],
                vec![None, None],
            ),
            // Malformed / unclosed tags: auto-closed paragraphs, list items and table cells, a
            // stray `<` in text.
            (
                "<ul><li>One<li>Two</ul><p>b < c<p>next<table><tr><th>A<th>B<tr><td>1<td>2</table>",
                vec![
                    Bullets(vec!["One".into(), "Two".into()]),
                    Paragraph("b < c".into()),
                    Paragraph("next".into()),
                    Table {
                        header: vec!["A".into(), "B".into()],
                        rows: vec![vec!["1".into(), "2".into()]],
                    },
                ],
                vec![None; 4],
            ),
            // Nested lists are flattened in document order; a list inside a cell joins as text.
            (
                "<ol><li>Step one<ol><li>Sub</ol></ol><table><thead><tr><th>Owner</th><th>Tasks</th></tr></thead><tbody><tr><td>Ana</td><td><ul><li>Ship<li>Test</ul></td></tr></tbody></table>",
                vec![
                    NumberedList(vec!["Step one".into(), "Sub".into()]),
                    Table {
                        header: vec!["Owner".into(), "Tasks".into()],
                        rows: vec![vec!["Ana".into(), "Ship Test".into()]],
                    },
                ],
                vec![None, None],
            ),
            // Body-level inline runs merge into one paragraph; `<br>` separates words.
            (
                "Hello <b>bold</b> world<hr id=\"s1\"><span>Line 1<br>Line 2</span>",
                vec![
                    Paragraph("Hello bold world".into()),
                    Divider,
                    Paragraph("Line 1 Line 2".into()),
                ],
                vec![None, Some("s1".into()), None],
            ),
            // Quip `<control>` mentions and mrkdwn tokens that look like tags survive as text.
            (
                r#"<p>Discuss in <control><a href="https://acme.slack.com/archives/C01ENG">#eng</a></control> with <!subteam^S01DES|@design> and <@U01ALICE> and @here.</p>"#,
                vec![Paragraph(
                    "Discuss in <#C01ENG|eng> with <!subteam^S01DES|@design> and <@U01ALICE> and @here."
                        .into(),
                )],
                vec![None],
            ),
        ];
        for (html, blocks, ids) in cases {
            let (got_blocks, got_ids) = parse_canvas_html_with_ids(html);
            assert_eq!(&got_blocks, blocks, "{html}");
            assert_eq!(&got_ids, ids, "{html}");
        }
    }
}
