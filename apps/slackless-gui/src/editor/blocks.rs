//! Lists, quotes and code blocks. A line belongs to a block through the marker atom at its
//! start, so deleting the marker (Backspace at the start of an item) returns the line to a
//! paragraph without any special case.

use super::marks::{Atom, LineKind, PLAIN};
use super::state::Editor;

impl Editor {
    /// The block kind of the line the selection starts on.
    pub fn active_block(&self) -> Option<LineKind> {
        self.now.doc.line(self.now.selection.start).kind
    }

    /// Put every selected line in a `kind` block, or back to paragraphs when all are in one.
    pub fn toggle_block(&mut self, kind: LineKind) {
        self.history.checkpoint(&self.now);
        let doc = &self.now.doc;
        let selection = self.now.selection.clone();
        let mut starts = vec![doc.line_start(selection.start)];
        while let Some(end) = starts
            .last()
            .map(|start| doc.line_end(*start))
            .filter(|end| *end < selection.end)
        {
            starts.push(end + 1);
        }
        let all = starts
            .iter()
            .all(|start| doc.marker(*start).is_some_and(|(found, _)| found == kind));
        let target = (!all).then_some(kind);
        for start in starts.into_iter().rev() {
            self.set_kind(start, target);
        }
        self.renumber();
    }

    /// Enter inside a block: an empty item leaves the block, anything else starts the next
    /// item. False on a paragraph line, which the composer handles (send, or a newline).
    pub fn enter(&mut self) -> bool {
        let line = self.now.doc.line(self.now.selection.start);
        let Some(kind) = line.kind else {
            return false;
        };
        self.history.checkpoint(&self.now);
        let is_empty = line.content.is_empty() && self.now.selection.is_empty();
        match (kind, is_empty) {
            (LineKind::Soft, true) => self.set_kind(line.start, Some(self.item_kind(line.start))),
            (_, true) => self.set_kind(line.start, None),
            (LineKind::Soft, false) => self.break_line(self.item_kind(line.start)),
            (_, false) => self.break_line(kind),
        }
        self.renumber();
        true
    }

    /// Shift+Enter inside a block: a new line that stays in the same item, quote or code
    /// block. False on a paragraph line, where the input engine inserts the newline.
    pub fn soft_break(&mut self) -> bool {
        let kind = match self.active_block() {
            None => return false,
            Some(LineKind::Bullet | LineKind::Ordered | LineKind::Soft) => LineKind::Soft,
            Some(kind @ (LineKind::Quote | LineKind::Code)) => kind,
        };
        self.history.checkpoint(&self.now);
        self.break_line(kind);
        true
    }

    /// Left at the start of a block line's text: the caret skips the marker to the end of
    /// the line above. False when the caret is anywhere else.
    pub fn left_over_marker(&mut self) -> bool {
        let caret = self.now.selection.clone();
        let line = self.now.doc.line(caret.start);
        let at_text_start = caret.is_empty() && caret.start == line.content.start;
        if !at_text_start || line.kind.is_none() || line.start == 0 {
            return false;
        }
        self.select(line.start - 1..line.start - 1)
    }

    /// Break the line at the selection; the new line opens with a `kind` marker.
    fn break_line(&mut self, kind: LineKind) {
        let range = self.now.selection.clone();
        let next = range.start + 1;
        self.replace(range, "\n", &PLAIN);
        self.set_kind(next, Some(kind));
    }

    /// The list a soft line continues: the kind of the nearest item above it.
    fn item_kind(&self, mut start: usize) -> LineKind {
        let doc = &self.now.doc;
        while start > 0 {
            start = doc.line_start(start - 1);
            match doc.marker(start) {
                Some((LineKind::Soft, _)) => {}
                Some((kind @ (LineKind::Bullet | LineKind::Ordered), _)) => return kind,
                Some((LineKind::Quote | LineKind::Code, _)) | None => break,
            }
        }
        LineKind::Bullet
    }

    /// Give the line starting at `start` a `kind` marker, replacing or removing its own.
    pub(super) fn set_kind(&mut self, start: usize, kind: Option<LineKind>) {
        let old = self
            .now
            .doc
            .marker(start)
            .map_or(start..start, |(_, range)| range);
        match kind {
            Some(kind) => self.place(old, Atom::Marker(kind), kind.marker_text()),
            None => self.replace(old, "", &PLAIN),
        }
    }

    /// Restore the marker invariants after an edit: a marker that no longer opens its line
    /// (the line was joined onto another) goes away with its text, and ordered items count
    /// up from one. Returns whether anything changed.
    pub(super) fn fix_markers(&mut self) -> bool {
        let doc = &self.now.doc;
        let opens_line = |offset: usize| offset == 0 || doc.text()[..offset].ends_with('\n');
        let stray: Vec<_> = doc
            .atoms()
            .iter()
            .filter(|placed| matches!(placed.atom, Atom::Marker(_)))
            .filter(|placed| !opens_line(placed.range.start))
            .map(|placed| placed.range.clone())
            .collect();
        let dropped = !stray.is_empty();
        for range in stray.into_iter().rev() {
            self.replace(range, "", &PLAIN);
        }
        self.renumber() || dropped
    }

    /// Number each run of ordered items from one. Returns whether a number changed.
    pub(super) fn renumber(&mut self) -> bool {
        let ordered = Atom::Marker(LineKind::Ordered);
        if !self.now.doc.atoms().iter().any(|p| p.atom == ordered) {
            return false;
        }
        let mut changed = false;
        let mut number = 0u32;
        let mut start = 0;
        loop {
            match self.now.doc.marker(start) {
                Some((LineKind::Ordered, range)) => {
                    number += 1;
                    let shown = self.now.doc.text()[range.clone()]
                        .strip_suffix(". ")
                        .and_then(|digits| digits.parse().ok());
                    if shown != Some(number) {
                        self.place(range, ordered.clone(), &format!("{number}. "));
                        changed = true;
                    }
                }
                Some((LineKind::Soft, _)) => {}
                Some((LineKind::Bullet | LineKind::Quote | LineKind::Code, _)) | None => number = 0,
            }
            let end = self.now.doc.line_end(start);
            if end == self.now.doc.text().len() {
                return changed;
            }
            start = end + 1;
        }
    }
}
