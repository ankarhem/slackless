//! The composer's document: plain text plus the inline formatting and the atomic units laid
//! over it, and the edits that keep the three consistent. Offsets are UTF-8 bytes into
//! [`Doc::text`], the coordinates the input engine uses.

use std::ops::Range;

use super::marks::{Atom, Attrs, PLAIN, Placed, Span};
use super::read::Piece;

/// Text with formatting spans and atoms. Spans are sorted, disjoint, non-empty and merged;
/// atoms are sorted and disjoint; a marker atom sits at the start of its line.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Doc {
    pub(super) text: String,
    pub(super) spans: Vec<Span>,
    pub(super) atoms: Vec<Placed>,
}

impl From<&str> for Doc {
    /// Unformatted text.
    fn from(text: &str) -> Self {
        Doc {
            text: text.to_owned(),
            ..Doc::default()
        }
    }
}

impl Doc {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    pub fn atoms(&self) -> &[Placed] {
        &self.atoms
    }

    /// Whether there is nothing to send: only whitespace and block markers.
    pub fn is_blank(&self) -> bool {
        if self.atoms.is_empty() {
            return self.text.trim().is_empty();
        }
        self.lines().all(|line| {
            self.pieces(line.content).iter().all(|piece| match piece {
                Piece::Text { text, .. } => text.trim().is_empty(),
                Piece::Atom { .. } => false,
            })
        })
    }

    /// A copy without leading and trailing whitespace, and without the empty list items or
    /// block lines a draft ends with: what is sent.
    pub fn trimmed(&self) -> Doc {
        let mut doc = self.clone();
        loop {
            let end = doc.text.len();
            let cut = match doc.atoms.last().filter(|placed| placed.range.end == end) {
                Some(Placed {
                    range,
                    atom: Atom::Marker(_),
                }) => range.start,
                Some(_) => break,
                None => match doc.text.chars().next_back() {
                    Some(last) if last.is_whitespace() => end - last.len_utf8(),
                    _ => break,
                },
            };
            doc.replace(cut..end, "", &PLAIN);
        }
        let lead = doc.atoms.first().map_or(doc.text.len(), |a| a.range.start);
        let blank = lead - doc.text[..lead].trim_start().len();
        doc.replace(0..blank, "", &PLAIN);
        doc
    }

    /// Replace `range` with `text` carrying `attrs`. Formatting outside the range is kept;
    /// an atom the range touches is dissolved (its remaining text stays as plain text).
    pub fn replace(&mut self, range: Range<usize>, text: &str, attrs: &Attrs) {
        let added = text.len();
        let moved = |offset: usize| offset - range.end + range.start + added;
        self.text.replace_range(range.clone(), text);

        let mut inserted = (added > 0 && !attrs.is_plain()).then(|| Span {
            range: range.start..range.start + added,
            attrs: attrs.clone(),
        });
        let mut spans = Vec::with_capacity(self.spans.len() + 2);
        for span in self.spans.drain(..) {
            let head = span.range.start..span.range.end.min(range.start);
            let tail = span.range.start.max(range.end)..span.range.end;
            match (head.is_empty(), tail.is_empty()) {
                (true, true) => {}
                (false, true) => spans.push(Span {
                    range: head,
                    ..span
                }),
                (headless, false) => {
                    if !headless {
                        spans.push(Span {
                            range: head,
                            attrs: span.attrs.clone(),
                        });
                    }
                    spans.extend(inserted.take());
                    spans.push(Span {
                        range: moved(tail.start)..moved(tail.end),
                        attrs: span.attrs,
                    });
                }
            }
        }
        spans.extend(inserted);
        self.spans = spans;
        self.merge_spans();

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
        self.replace(range.clone(), text, &PLAIN);
        let placed = Placed {
            range: range.start..range.start + text.len(),
            atom,
        };
        let ix = self
            .atoms
            .partition_point(|other| other.range.start < placed.range.start);
        self.atoms.insert(ix, placed);
    }

    /// Add `text` carrying `attrs` at the end.
    pub fn append(&mut self, text: &str, attrs: &Attrs) {
        let start = self.text.len();
        self.text.push_str(text);
        if !text.is_empty() && !attrs.is_plain() {
            self.spans.push(Span {
                range: start..self.text.len(),
                attrs: attrs.clone(),
            });
            self.merge_spans();
        }
    }

    /// Add `atom`, shown as `text`, at the end.
    pub fn append_atom(&mut self, atom: Atom, text: &str) {
        let start = self.text.len();
        self.text.push_str(text);
        self.atoms.push(Placed {
            range: start..self.text.len(),
            atom,
        });
    }

    /// Rewrite the attributes of every character in `range`, plain ones included.
    pub fn map_attrs(&mut self, range: Range<usize>, map: impl Fn(&Attrs) -> Attrs) {
        if range.is_empty() {
            return;
        }
        let mut spans: Vec<Span> = Vec::with_capacity(self.spans.len() + 2);
        let push = |spans: &mut Vec<Span>, range: Range<usize>, attrs: Attrs| {
            if !range.is_empty() && !attrs.is_plain() {
                spans.push(Span { range, attrs });
            }
        };
        // First position inside `range` not yet rewritten.
        let mut cursor = range.start;
        for span in std::mem::take(&mut self.spans) {
            if span.range.end <= range.start {
                spans.push(span);
                continue;
            }
            let inner = span.range.start.max(range.start)..span.range.end.min(range.end);
            if inner.is_empty() {
                push(&mut spans, cursor..range.end, map(&PLAIN));
                cursor = range.end;
                spans.push(span);
                continue;
            }
            push(
                &mut spans,
                span.range.start..inner.start,
                span.attrs.clone(),
            );
            push(&mut spans, cursor..inner.start, map(&PLAIN));
            push(&mut spans, inner.clone(), map(&span.attrs));
            cursor = inner.end;
            push(&mut spans, inner.end..span.range.end, span.attrs);
        }
        push(&mut spans, cursor..range.end, map(&PLAIN));
        self.spans = spans;
        self.merge_spans();
    }

    fn merge_spans(&mut self) {
        self.spans.dedup_by(|next, prev| {
            let joins = prev.range.end == next.range.start && prev.attrs == next.attrs;
            if joins {
                prev.range.end = next.range.end;
            }
            joins
        });
    }
}
