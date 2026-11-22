//! The composer's editing state: the document and selection, the formats armed for the next
//! typed character, and undo. The input engine owns typing; [`Editor::user_edit`] mirrors
//! each of its edits here, and every other method is a change this model makes that the
//! composer then pushes back to the engine.

use std::ops::Range;

use super::doc::Doc;
use super::history::{Change, History, Snapshot};
use super::marks::{Atom, Attrs, LineKind, PLAIN, Style};

#[derive(Default)]
pub struct Editor {
    pub(super) now: Snapshot,
    /// Formats the next typed character takes instead of its neighbor's: a format toggled
    /// with nothing selected. Dropped when the caret moves.
    pub(super) pending: Option<Style>,
    pub(super) history: History,
}

impl Editor {
    pub fn doc(&self) -> &Doc {
        &self.now.doc
    }

    pub fn selection(&self) -> Range<usize> {
        self.now.selection.clone()
    }

    /// Start over from `doc` (a loaded draft, or nothing after sending), caret at the end.
    pub fn load(&mut self, doc: Doc) {
        let end = doc.text().len();
        *self = Editor {
            now: Snapshot {
                doc,
                selection: end..end,
            },
            ..Editor::default()
        };
    }

    /// The caret or selection moved without an edit. A caret never rests in front of a
    /// line's marker: it lands after it, so the result can differ from `selection`. Returns
    /// whether the selection changed.
    pub fn select(&mut self, selection: Range<usize>) -> bool {
        let selection = match self.now.doc.marker(selection.start) {
            Some((_, marker)) if selection.is_empty() => marker.end..marker.end,
            _ => selection,
        };
        if selection == self.now.selection {
            return false;
        }
        self.now.selection = selection;
        self.pending = None;
        self.history.break_run();
        true
    }

    /// Mirror an edit the input engine made: `range` of the old text became `text`, and the
    /// engine's selection is now `selection`. Returns whether the document ended up different
    /// from the engine's text (a marker was carried to a pasted line, typed markup turned
    /// into formatting, ...), in which case the caller pushes the document back.
    pub fn user_edit(&mut self, range: Range<usize>, text: &str, selection: Range<usize>) -> bool {
        let (range, text) = narrowed(self.now.doc.text(), range, text);
        if range.is_empty() && text.is_empty() {
            self.now.selection = selection;
            return false;
        }
        let typed = sole_char(text).filter(|_| range.is_empty());
        self.history.record(&self.now, self.change(&range, text));

        // Text typed in front of a line's marker lands after it.
        let (at, selection, mut diverged) = match self.now.doc.marker(range.start) {
            Some((_, marker)) if range.is_empty() => {
                let shift = marker.len();
                let moved = selection.start + shift..selection.end + shift;
                (marker.end..marker.end, moved, true)
            }
            _ => (range, selection, false),
        };
        let attrs = self.typing_attrs(&at);
        let kind = self.now.doc.line(at.start).kind;
        self.now.doc.replace(at.clone(), text, &attrs);
        self.now.selection = selection;
        self.pending = None;

        // Lines broken off inside a list, quote or code block stay in it.
        if let Some(kind) = kind {
            for (ix, _) in text.rmatch_indices('\n') {
                self.set_kind(at.start + ix + 1, Some(kind));
                diverged = true;
            }
        }
        diverged |= self.fix_markers();
        let caret = self.now.selection.clone();
        if let Some(typed) = typed.filter(|_| caret.is_empty()) {
            diverged |= self.autoformat(typed);
        }
        diverged
    }

    fn change(&self, range: &Range<usize>, text: &str) -> Change {
        let doc = self.now.doc.text();
        match (sole_char(text), sole_char(&doc[range.clone()])) {
            (Some(typed), None) if range.is_empty() && typed != '\n' => {
                let after_space = doc[..range.start]
                    .chars()
                    .next_back()
                    .is_none_or(char::is_whitespace);
                Change::Type {
                    at: range.start,
                    len: text.len(),
                    word_start: after_space && !typed.is_whitespace(),
                }
            }
            (None, Some(_)) if text.is_empty() => Change::Erase {
                range: range.clone(),
            },
            _ => Change::Other,
        }
    }

    /// What text replacing `range` carries: the armed formats, else those of the text it
    /// continues. A link is continued only from inside it, never from its edge.
    pub(super) fn typing_attrs(&self, range: &Range<usize>) -> Attrs {
        let doc = &self.now.doc;
        let line = doc.line(range.start);
        if line.kind == Some(LineKind::Code) {
            return Attrs::default();
        }
        // The character the new text continues: the first one replaced, else the one before.
        let lead = if range.is_empty() {
            doc.text()[line.content.start.min(range.start)..range.start]
                .char_indices()
                .next_back()
                .map(|(ix, _)| line.content.start + ix)
        } else {
            Some(range.start)
        };
        let next = (range.end < doc.line_end(range.end) && doc.atom_at(range.end).is_none())
            .then(|| doc.attrs_at(range.end));
        let mut attrs = match lead {
            Some(lead) if doc.atom_at(lead).is_none() => {
                let lead = doc.attrs_at(lead);
                let continues = |link: &std::sync::Arc<str>| {
                    next.is_some_and(|next| next.link.as_ref() == Some(link))
                };
                Attrs {
                    style: lead.style,
                    link: lead.link.clone().filter(continues),
                }
            }
            Some(_) => Attrs::default(),
            None => Attrs {
                style: next.map_or(Style::NONE, |next| next.style),
                link: None,
            },
        };
        if let Some(style) = self.pending {
            attrs.style = style;
        }
        attrs
    }

    /// Replace the selection with `text` (an emoji pick, a pasted link).
    pub fn insert_text(&mut self, text: &str) {
        self.history.checkpoint(&self.now);
        let range = self.now.selection.clone();
        let attrs = self.typing_attrs(&range);
        self.replace(range, text, &attrs);
        self.pending = None;
    }

    /// Replace `range` with `atom`, shown as `text`, and leave the caret after a space
    /// following it, as Slack does after a mention.
    pub fn insert_atom(&mut self, range: Range<usize>, atom: Atom, text: &str) {
        self.place_atom(range.clone(), atom, text);
        let mut caret = range.start + text.len();
        if !self.now.doc.text()[caret..].starts_with(' ') {
            self.replace(caret..caret, " ", &PLAIN);
        }
        caret += 1;
        self.now.selection = caret..caret;
    }

    /// Replace `range` with `atom`, shown as `text`, with the caret right after it; its own
    /// undo step, so undoing leaves the typed characters (a typed `:shortcode:` that became
    /// its emoji).
    pub fn place_atom(&mut self, range: Range<usize>, atom: Atom, text: &str) {
        self.history.checkpoint(&self.now);
        self.place(range.clone(), atom, text);
        let caret = range.start + text.len();
        self.now.selection = caret..caret;
        self.pending = None;
    }

    /// Replace the whole draft with unformatted `text`, caret at the end.
    pub fn replace_all(&mut self, text: &str) {
        self.history.checkpoint(&self.now);
        self.now = Snapshot {
            doc: Doc::from(text),
            selection: text.len()..text.len(),
        };
        self.pending = None;
    }

    /// Step back; false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.pending = None;
        self.history.undo(&mut self.now)
    }

    /// Step forward again; false when nothing was undone.
    pub fn redo(&mut self) -> bool {
        self.pending = None;
        self.history.redo(&mut self.now)
    }

    /// [`Doc::replace`], with the selection following the edit.
    pub(super) fn replace(&mut self, range: Range<usize>, text: &str, attrs: &Attrs) {
        self.follow(&range, text.len());
        self.now.doc.replace(range, text, attrs);
    }

    /// [`Doc::place`], with the selection following the edit.
    pub(super) fn place(&mut self, range: Range<usize>, atom: Atom, text: &str) {
        self.follow(&range, text.len());
        self.now.doc.place(range, atom, text);
    }

    /// Move the selection as replacing `range` with `added` bytes moves the text under it:
    /// offsets at or past the end shift, offsets inside land after the replacement.
    fn follow(&mut self, range: &Range<usize>, added: usize) {
        let moved = |offset: usize| {
            if offset < range.start {
                offset
            } else if offset >= range.end {
                offset - range.len() + added
            } else {
                range.start + added
            }
        };
        let selection = &self.now.selection;
        self.now.selection = moved(selection.start)..moved(selection.end);
    }
}

/// `range` and `text` without what the replaced text and `text` have in common at both ends:
/// the smallest edit with the same effect. An IME commit reports the whole document replaced.
pub fn narrowed<'a>(doc: &str, range: Range<usize>, text: &'a str) -> (Range<usize>, &'a str) {
    let old = &doc[range.clone()];
    let prefix = old
        .char_indices()
        .zip(text.chars())
        .find(|((_, a), b)| a != b)
        .map_or(old.len().min(text.len()), |((ix, _), _)| ix);
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(text[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (
        range.start + prefix..range.end - suffix,
        &text[prefix..text.len() - suffix],
    )
}

/// The character `text` consists of, when it is exactly one.
fn sole_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}
