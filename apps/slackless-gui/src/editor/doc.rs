//! The composer's document: real editable Markdown text plus the semantic inline atoms
//! (mentions, channels, broadcasts, emoji) laid over it. Formatting and block structure are
//! parsed deterministically from the markdown text in [`super::parse`], so there is no
//! hidden formatting side-table that can drift out of sync with what the user edits.

use std::ops::Range;

use super::marks::{Atom, LineKind, Placed};
use super::parse::Piece;

/// Markdown text with sorted, disjoint semantic atoms.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Doc {
    pub(super) text: String,
    pub(super) atoms: Vec<Placed>,
}

impl From<&str> for Doc {
    /// Plain or markdown text with no semantic atoms.
    fn from(text: &str) -> Self {
        Doc {
            text: text.to_owned(),
            atoms: Vec::new(),
        }
    }
}

impl Doc {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn atoms(&self) -> &[Placed] {
        &self.atoms
    }

    /// Whether nothing at all is typed (the textarea shows its placeholder).
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Whether there is nothing to send: only whitespace and empty block prefixes.
    pub fn is_blank(&self) -> bool {
        let parsed = self.parse();
        parsed.lines.iter().all(|line| match line.kind {
            Some(LineKind::Divider) => false,
            Some(LineKind::Fence) => true,
            Some(LineKind::Code) => self.text[line.content.clone()].trim().is_empty(),
            _ => parsed
                .pieces(self, line.content.clone())
                .iter()
                .all(|piece| match piece {
                    Piece::Text { text, .. } => text.trim().is_empty(),
                    Piece::Atom { .. } => false,
                }),
        })
    }

    /// A copy without leading and trailing blank lines / whitespace, and without trailing
    /// empty list/quote/heading prefixes: what is sent.
    pub fn trimmed(&self) -> Doc {
        let mut doc = self.clone();
        // Drop trailing blank lines and trailing empty block prefixes (e.g. `- ` left after
        // pressing Enter on a list).
        loop {
            let parsed = doc.parse();
            let Some(last) = parsed.lines.last() else {
                break;
            };
            let line_empty = match last.kind {
                Some(LineKind::Divider) => false,
                Some(LineKind::Fence) => false,
                _ => {
                    doc.text[last.content.clone()].trim().is_empty()
                        && !doc
                            .atoms
                            .iter()
                            .any(|a| a.range.start >= last.start && a.range.end <= last.end)
                }
            };
            if line_empty {
                let cut = if last.start > 0 { last.start - 1 } else { 0 };
                let end = doc.text.len();
                doc.replace(cut..end, "");
                if cut == 0 {
                    break;
                }
            } else {
                break;
            }
        }
        // Trim trailing inline whitespace after the last atom.
        let floor = doc.atoms.last().map_or(0, |a| a.range.end);
        let trimmed_end = floor + doc.text[floor..].trim_end().len();
        if trimmed_end < doc.text.len() {
            let end = doc.text.len();
            doc.replace(trimmed_end..end, "");
        }
        // Trim leading whitespace before the first atom.
        let lead = doc.atoms.first().map_or(doc.text.len(), |a| a.range.start);
        let blank = lead - doc.text[..lead].trim_start().len();
        if blank > 0 {
            doc.replace(0..blank, "");
        }
        doc
    }

    /// Replace `range` with `text`. An atom the range touches is dissolved (its remaining
    /// text stays as plain text).
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let added = text.len();
        let moved = |offset: usize| offset - range.end + range.start + added;
        self.text.replace_range(range.clone(), text);

        self.atoms.retain_mut(|placed| {
            if placed.range.end <= range.start {
                return true;
            }
            if placed.range.start >= range.end {
                placed.range = moved(placed.range.start)..moved(placed.range.end);
                return true;
            }
            false
        });
    }

    /// Replace `range` with `atom`, shown as `text`.
    pub fn place(&mut self, range: Range<usize>, atom: Atom, text: &str) {
        self.replace(range.clone(), text);
        let placed = Placed {
            range: range.start..range.start + text.len(),
            atom,
        };
        let ix = self
            .atoms
            .partition_point(|other| other.range.start < placed.range.start);
        self.atoms.insert(ix, placed);
    }

    /// Add `text` at the end.
    pub fn append(&mut self, text: &str) {
        self.text.push_str(text);
    }

    /// Add `atom`, shown as `text`, at the end.
    pub fn append_atom(&mut self, atom: Atom, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        let end = self.text.len();
        self.atoms.push(Placed {
            range: start..end,
            atom,
        });
    }
}
