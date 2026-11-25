//! Document buffer, inline styles, block line kinds, and semantic inline atoms.

use std::ops::Range;
use std::sync::Arc;

use crate::types::rich_text::BroadcastRange;
use crate::types::{ChannelId, UserId, UsergroupId};

use super::parse::Piece;

/// A set of inline character formats.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Style(u8);

impl Style {
    pub const NONE: Style = Style(0);
    pub const BOLD: Style = Style(1);
    pub const ITALIC: Style = Style(1 << 1);
    pub const STRIKE: Style = Style(1 << 2);
    pub const CODE: Style = Style(1 << 3);
    pub const UNDERLINE: Style = Style(1 << 4);

    pub const fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }

    /// The formats both sets carry.
    pub const fn common(self, other: Style) -> Style {
        Style(self.0 & other.0)
    }

    pub const fn is_plain(self) -> bool {
        self.0 == 0
    }
}

/// What a run of text carries: its formats and the URL it links to.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct Attrs {
    pub style: Style,
    pub link: Option<Arc<str>>,
}

impl Attrs {
    pub fn is_plain(&self) -> bool {
        self.style.is_plain() && self.link.is_none()
    }
}

/// Unformatted text.
pub static PLAIN: Attrs = Attrs {
    style: Style::NONE,
    link: None,
};

/// How a line is laid out as a block; a plain paragraph line has none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Bullet,
    Ordered,
    Quote,
    /// A line inside a fenced code block.
    Code,
    /// An opening or closing ```` ``` ```` code fence line.
    Fence,
    /// A further line of the list item above it (Shift+Enter inside an item).
    Soft,
    /// A Big / Medium / Small heading line, `level` 1 to 3.
    Heading(u8),
    /// A horizontal rule (`---`); the line has no text of its own.
    Divider,
}

impl LineKind {
    /// The canonical markdown prefix inserted for this block kind.
    pub const fn markdown_prefix(self) -> &'static str {
        match self {
            LineKind::Bullet => "- ",
            LineKind::Ordered => "1. ",
            LineKind::Quote => "> ",
            LineKind::Soft => "    ",
            LineKind::Heading(1) => "# ",
            LineKind::Heading(2) => "## ",
            LineKind::Heading(_) => "### ",
            LineKind::Divider => "---",
            LineKind::Code | LineKind::Fence => "",
        }
    }
}

/// A semantic inline unit: user mention, channel reference, usergroup, broadcast, or emoji.
#[derive(Clone, PartialEq, Debug)]
pub enum Atom {
    User(UserId),
    Channel(ChannelId),
    Usergroup(UsergroupId),
    Broadcast(BroadcastRange),
    /// Shortcode without colons, in Slack's `name::skin-tone-N` form when toned.
    Emoji(Arc<str>),
}

/// An atom and the UTF-8 byte range it occupies in [`Doc::text`].
#[derive(Clone, PartialEq, Debug)]
pub struct Placed {
    pub range: Range<usize>,
    pub atom: Atom,
}

/// Editable markdown text with sorted, disjoint semantic atoms.
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

    /// Whether nothing at all is typed.
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

    /// A copy without leading/trailing blank lines or empty list/quote/heading prefixes.
    pub fn trimmed(&self) -> Doc {
        let mut doc = self.clone();
        loop {
            let parsed = doc.parse();
            let Some(last) = parsed.lines.last() else {
                break;
            };
            let line_empty = match last.kind {
                Some(LineKind::Divider | LineKind::Fence) => false,
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
        let floor = doc.atoms.last().map_or(0, |a| a.range.end);
        let trimmed_end = floor + doc.text[floor..].trim_end().len();
        if trimmed_end < doc.text.len() {
            let end = doc.text.len();
            doc.replace(trimmed_end..end, "");
        }
        let lead = doc.atoms.first().map_or(doc.text.len(), |a| a.range.start);
        let blank = lead - doc.text[..lead].trim_start().len();
        if blank > 0 {
            doc.replace(0..blank, "");
        }
        doc
    }

    /// Replace `range` with `text`. Any atom touched by `range` is dissolved.
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
