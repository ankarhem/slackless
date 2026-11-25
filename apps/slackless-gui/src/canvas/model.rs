//! Canvas document data model, markdown serialization, block mutations, and fixture data.

use crate::canvas::parse::{parse_canvas_html_with_ids, parse_canvas_markdown};

/// One block of a canvas document, in reading order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanvasBlock {
    /// `level` 1-3, like the canvas editor's H1/H2/H3.
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
    Divider,
}

impl CanvasBlock {
    /// Case-insensitive text search across the block's rendered text (`F-SHELL-266`).
    pub(crate) fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let has = |s: &str| s.to_lowercase().contains(needle);
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
            Self::Divider => false,
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
            Self::Divider => "---".to_owned(),
        }
    }
}

/// Escape pipe characters inside a markdown table cell.
fn escape_table_cell(cell: &str) -> String {
    cell.replace('|', "\\|").replace('\n', " ")
}

/// Serialize a table to normalized markdown pipe-table syntax (`F-COLLAB-136`).
pub(crate) fn serialize_markdown_table(header: &[String], rows: &[Vec<String>]) -> String {
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

/// A parsed canvas document ready to render in the native reader/editor.
#[derive(Clone, Debug)]
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

    /// Parse lightweight markdown back into [`CanvasBlock`]s (for local preview & fixture edits).
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

    /// Move block at `index` up (`delta = -1`) or down (`delta = 1`) (`F-SHELL-265`, `F-COLLAB-145`).
    pub fn move_block(&mut self, index: usize, delta: isize) -> bool {
        if index >= self.blocks.len() {
            return false;
        }
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.blocks.len() {
            return false;
        }
        let target = target as usize;
        self.blocks.swap(index, target);
        if self.section_ids.len() < self.blocks.len() {
            self.section_ids.resize(self.blocks.len(), None);
        }
        self.section_ids.swap(index, target);
        true
    }

    /// Delete block at `index` (`F-SHELL-265`, `F-COLLAB-145`).
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

    /// Toggle a checklist item's checked state, returning the updated section markdown (`F-COLLAB-135`).
    pub fn toggle_checklist_item(&mut self, block_ix: usize, item_ix: usize) -> Option<String> {
        let CanvasBlock::Checklist(items) = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        let (done, _) = items.get_mut(item_ix)?;
        *done = !*done;
        Some(CanvasBlock::Checklist(items.clone()).to_markdown())
    }

    /// Append a new unchecked checklist item, returning the updated section markdown (`F-COLLAB-135`).
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
    /// and returns `Ok(None)`; otherwise returns `Ok(Some(section_markdown))` (`F-COLLAB-135`).
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

    /// Append an empty row matching the header width to a table block (`F-COLLAB-136`).
    pub fn table_add_row(&mut self, block_ix: usize) -> Option<String> {
        let CanvasBlock::Table { header, rows } = self.blocks.get_mut(block_ix)? else {
            return None;
        };
        let cols = header.len().max(1);
        rows.push(vec![String::new(); cols]);
        Some(serialize_markdown_table(header, rows))
    }

    /// Append a new column to a table block's header and all data rows (`F-COLLAB-136`).
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
}

/// The sample canvas every conversation shows in fixture mode.
pub(super) fn fixture() -> CanvasDoc {
    let blocks = vec![
        CanvasBlock::Heading {
            level: 1,
            text: "Q4 launch plan :rocket:".into(),
        },
        CanvasBlock::Paragraph(
            "This canvas tracks everything we need to ship the desktop client before the \
             end of the quarter. Owners update their rows; the channel gets a weekly digest."
                .into(),
        ),
        CanvasBlock::Heading {
            level: 2,
            text: "Goals".into(),
        },
        CanvasBlock::Bullets(vec![
            "Native desktop client with feature parity for messaging".into(),
            "Offline search over the local archive".into(),
            "Huddles via native audio and video".into(),
        ]),
        CanvasBlock::Heading {
            level: 2,
            text: "Rollout steps".into(),
        },
        CanvasBlock::NumberedList(vec![
            "Verify release artifacts and notarization".into(),
            "Publish beta build to internal testers".into(),
            "Announce general availability in #announcements".into(),
        ]),
        CanvasBlock::Quote(
            "Keep every milestone owner accountable.\nFlag blockers in the weekly sync.".into(),
        ),
        CanvasBlock::Heading {
            level: 2,
            text: "Launch checklist".into(),
        },
        CanvasBlock::Checklist(vec![
            (true, "Sign the macOS build".into()),
            (true, "Fixture-driven screenshots for the docs".into()),
            (false, "Beta invite list reviewed by support".into()),
            (false, "Release notes drafted".into()),
        ]),
        CanvasBlock::Divider,
        CanvasBlock::Heading {
            level: 2,
            text: "Milestones".into(),
        },
        CanvasBlock::Table {
            header: vec![
                "Milestone".into(),
                "Owner".into(),
                "Due".into(),
                "Status".into(),
            ],
            rows: vec![
                vec![
                    "Private beta".into(),
                    "Maya Chen".into(),
                    "Oct 14".into(),
                    ":white_check_mark: Done".into(),
                ],
                vec![
                    "Public beta".into(),
                    "Leo Park".into(),
                    "Nov 4".into(),
                    "In progress".into(),
                ],
                vec![
                    "General availability".into(),
                    "Sam Rivera".into(),
                    "Dec 2".into(),
                    "Not started".into(),
                ],
            ],
        },
        CanvasBlock::Heading {
            level: 3,
            text: "Open questions".into(),
        },
        CanvasBlock::Paragraph(
            "Do we gate huddles behind a setting while rolling out screen sharing?".into(),
        ),
    ];
    let section_ids = vec![None; blocks.len()];
    CanvasDoc {
        title: "Q4 launch plan".into(),
        meta: "Canvas · last edited by Maya Chen".into(),
        blocks,
        section_ids,
    }
}
