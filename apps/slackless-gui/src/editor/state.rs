//! The composer's editing state: the markdown [`Doc`], the current selection, and undo.
//! The input engine owns typing; [`Editor::user_edit`] mirrors each of its edits here
//! without mutating what the user typed, while toolbar buttons and shortcuts edit the
//! markdown buffer and push the change back to the engine.

use std::ops::Range;

use super::doc::Doc;
use super::history::{Change, History, Snapshot};
use super::marks::Atom;

#[derive(Default)]
pub struct Editor {
    pub(super) now: Snapshot,
    pub(super) history: History,
}

impl Editor {
    pub fn doc(&self) -> &Doc {
        &self.now.doc
    }

    pub fn selection(&self) -> Range<usize> {
        self.now.selection.clone()
    }

    /// Start over from `doc` (a loaded draft, sent message, or empty), caret at the end.
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

    /// The caret or selection moved without an edit. Returns whether the selection changed.
    pub fn select(&mut self, selection: Range<usize>) -> bool {
        let len = self.now.doc.text().len();
        let selection = selection.start.min(len)..selection.end.min(len);
        if selection == self.now.selection {
            return false;
        }
        self.now.selection = selection;
        self.history.break_run();
        true
    }

    /// Mirror an edit the input engine made: `range` of the old text became `text`, and the
    /// engine's selection is now `selection`. Returns whether the document ended up different
    /// from the engine's text (so the caller should push it back).
    pub fn user_edit(&mut self, range: Range<usize>, text: &str, selection: Range<usize>) -> bool {
        let (range, text) = narrowed(self.now.doc.text(), range, text);
        if range.is_empty() && text.is_empty() {
            self.now.selection = selection;
            return false;
        }
        self.history.record(&self.now, self.change(&range, text));
        self.now.doc.replace(range, text);
        let len = self.now.doc.text().len();
        self.now.selection = selection.start.min(len)..selection.end.min(len);
        false
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

    /// Replace the selection with plain `text`.
    pub fn insert_text(&mut self, text: &str) {
        self.history.checkpoint(&self.now);
        let range = self.now.selection.clone();
        self.replace(range, text);
    }

    /// Replace `range` with `atom`, shown as `text`, and leave the caret after a space
    /// following it, as Slack does after a mention.
    pub fn insert_atom(&mut self, range: Range<usize>, atom: Atom, text: &str) {
        self.place_atom(range.clone(), atom, text);
        let mut caret = range.start + text.len();
        if !self.now.doc.text()[caret..].starts_with(' ') {
            self.replace(caret..caret, " ");
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
    }

    /// Replace the whole draft with unformatted `text`, caret at the end.
    pub fn replace_all(&mut self, text: &str) {
        self.history.checkpoint(&self.now);
        self.now = Snapshot {
            doc: Doc::from(text),
            selection: text.len()..text.len(),
        };
    }

    /// Step back; false when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.history.undo(&mut self.now)
    }

    /// Step forward again; false when nothing was undone.
    pub fn redo(&mut self) -> bool {
        self.history.redo(&mut self.now)
    }

    /// [`Doc::replace`], with the selection following the edit.
    pub(super) fn replace(&mut self, range: Range<usize>, text: &str) {
        self.follow(&range, text.len());
        self.now.doc.replace(range, text);
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
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((ix, ch), _)| ix + ch.len_utf8());
    let old_tail = &old[prefix..];
    let text_tail = &text[prefix..];
    let suffix = old_tail
        .chars()
        .rev()
        .zip(text_tail.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    (
        range.start + prefix..range.end - suffix,
        &text[prefix..text.len() - suffix],
    )
}

fn sole_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}
