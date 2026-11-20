//! Reading a document: the formatting at an offset, its lines, and the uniform pieces a
//! range is made of.

use std::ops::Range;

use super::doc::Doc;
use super::marks::{Atom, Attrs, LineKind, PLAIN, Placed, Span, Style};

/// One line: where it starts, its block kind and the range after its marker (newline excluded).
#[derive(Clone, PartialEq, Debug)]
pub struct Line {
    pub start: usize,
    pub kind: Option<LineKind>,
    pub content: Range<usize>,
}

/// A stretch of a line that is uniform: formatted text or one atom.
#[derive(Clone, PartialEq, Debug)]
pub enum Piece<'a> {
    Text {
        text: &'a str,
        attrs: &'a Attrs,
    },
    Atom {
        atom: &'a Atom,
        text: &'a str,
        style: Style,
    },
}

impl Doc {
    /// The attributes of the character starting at `offset`.
    pub fn attrs_at(&self, offset: usize) -> &Attrs {
        let ix = self.spans.partition_point(|span| span.range.end <= offset);
        match self.spans.get(ix) {
            Some(span) if span.range.start <= offset => &span.attrs,
            _ => &PLAIN,
        }
    }

    /// The atom covering the character starting at `offset`.
    pub fn atom_at(&self, offset: usize) -> Option<&Placed> {
        let ix = self
            .atoms
            .partition_point(|placed| placed.range.end <= offset);
        self.atoms
            .get(ix)
            .filter(|placed| placed.range.start <= offset)
    }

    /// The formats all the text in `range` carries. Atoms and bare line breaks do not count:
    /// a selection across list items is bold when every item's text is.
    pub fn common_style(&self, range: Range<usize>) -> Style {
        self.pieces(range)
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text { text, .. } if text.trim_matches('\n').is_empty() => None,
                Piece::Text { attrs, .. } => Some(attrs.style),
                Piece::Atom { .. } => None,
            })
            .reduce(Style::common)
            .unwrap_or(Style::NONE)
    }

    /// The whole linked run around `offset`, when the character there links somewhere.
    pub fn link_range(&self, offset: usize) -> Option<Range<usize>> {
        let link = self.attrs_at(offset).link.as_ref()?;
        let ix = self.spans.partition_point(|span| span.range.end <= offset);
        let same = |span: &&Span| span.attrs.link.as_ref() == Some(link);
        let mut range = self.spans[ix].range.clone();
        for span in self.spans[..ix].iter().rev().take_while(same) {
            if span.range.end != range.start {
                break;
            }
            range.start = span.range.start;
        }
        for span in self.spans[ix + 1..].iter().take_while(same) {
            if span.range.start != range.end {
                break;
            }
            range.end = span.range.end;
        }
        Some(range)
    }

    pub fn line_start(&self, offset: usize) -> usize {
        self.text[..offset].rfind('\n').map_or(0, |ix| ix + 1)
    }

    pub fn line_end(&self, offset: usize) -> usize {
        self.text[offset..]
            .find('\n')
            .map_or(self.text.len(), |ix| offset + ix)
    }

    /// The marker atom starting at `offset`, with its text range.
    pub fn marker(&self, offset: usize) -> Option<(LineKind, Range<usize>)> {
        let ix = self
            .atoms
            .partition_point(|placed| placed.range.start < offset);
        match self.atoms.get(ix) {
            Some(Placed {
                range,
                atom: Atom::Marker(kind),
            }) if range.start == offset => Some((*kind, range.clone())),
            _ => None,
        }
    }

    /// The line containing `offset`.
    pub fn line(&self, offset: usize) -> Line {
        let start = self.line_start(offset);
        let end = self.line_end(offset);
        match self.marker(start) {
            Some((kind, marker)) => Line {
                start,
                kind: Some(kind),
                content: marker.end..end,
            },
            None => Line {
                start,
                kind: None,
                content: start..end,
            },
        }
    }

    /// Every line, top to bottom.
    pub fn lines(&self) -> impl Iterator<Item = Line> + '_ {
        let mut next = Some(0);
        std::iter::from_fn(move || {
            let line = self.line(next?);
            next = (line.content.end < self.text.len()).then_some(line.content.end + 1);
            Some(line)
        })
    }

    /// `range` cut into uniform pieces, in order.
    pub fn pieces(&self, range: Range<usize>) -> Vec<Piece<'_>> {
        let mut pieces = Vec::new();
        let mut at = range.start;
        let mut atoms = self.atoms[self.atoms.partition_point(|a| a.range.end <= at)..].iter();
        let mut atom = atoms.next();
        while at < range.end {
            if let Some(placed) = atom.filter(|placed| placed.range.start <= at) {
                let end = placed.range.end.min(range.end);
                pieces.push(Piece::Atom {
                    atom: &placed.atom,
                    text: &self.text[at..end],
                    style: self.attrs_at(at).style,
                });
                at = end;
                atom = atoms.next();
                continue;
            }
            let mut end = atom.map_or(range.end, |placed| placed.range.start.min(range.end));
            let ix = self.spans.partition_point(|span| span.range.end <= at);
            let attrs = match self.spans.get(ix) {
                Some(span) if span.range.start <= at => {
                    end = end.min(span.range.end);
                    &span.attrs
                }
                Some(span) => {
                    end = end.min(span.range.start);
                    &PLAIN
                }
                None => &PLAIN,
            };
            pieces.push(Piece::Text {
                text: &self.text[at..end],
                attrs,
            });
            at = end;
        }
        pieces
    }
}
