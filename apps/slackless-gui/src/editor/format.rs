//! Inline formatting in Obsidian Live Preview: toggling bold (`*...*`), italic (`_..._`),
//! underline (`<u>...</u>`), strikethrough (`~...~`), code (`` `...` ``), clearing inline
//! markers, and linking text (`[label](url)`).

use std::ops::Range;

use super::marks::{LineKind, Style};
use super::parse::{InlineKind, empty_pair_at};
use super::state::Editor;

/// Canonical markdown delimiters inserted by toolbar buttons and keyboard shortcuts.
fn delimiters(style: Style) -> (&'static str, &'static str) {
    if style == Style::BOLD {
        ("*", "*")
    } else if style == Style::ITALIC {
        ("_", "_")
    } else if style == Style::STRIKE {
        ("~", "~")
    } else if style == Style::CODE {
        ("`", "`")
    } else if style == Style::UNDERLINE {
        ("<u>", "</u>")
    } else {
        ("", "")
    }
}

fn is_non_formatted_line(kind: Option<LineKind>) -> bool {
    matches!(
        kind,
        Some(LineKind::Code | LineKind::Fence | LineKind::Divider)
    )
}

impl Editor {
    /// The formats in effect at the selection or caret. Toolbar buttons light up from this.
    pub fn active_style(&self) -> Style {
        let parsed = self.now.doc.parse();
        let selection = &self.now.selection;
        if selection.is_empty() {
            parsed.attrs_at(&self.now.doc, selection.start).style
        } else {
            parsed.common_style(&self.now.doc, selection.clone())
        }
    }

    /// Toggle `style` on the selection (per-line within non-code content, keeping surrounding
    /// whitespace outside the delimiters), or insert/exit an empty delimiter pair at the caret
    /// when nothing is selected.
    pub fn toggle_style(&mut self, style: Style) {
        let (open, close) = delimiters(style);
        if open.is_empty() {
            return;
        }
        let range = self.now.selection.clone();
        let parsed = self.now.doc.parse();

        if range.is_empty() {
            let caret = range.start;
            let line = parsed.line_at(caret);
            if is_non_formatted_line(line.kind) {
                return;
            }
            // Toggle off an armed empty pair directly around the caret (`*|*`, `**|**`, etc.).
            if let Some((pair_range, pair_style)) = empty_pair_at(self.now.doc.text(), line, caret)
            {
                if pair_style == style {
                    self.history.checkpoint(&self.now);
                    self.replace(pair_range, "");
                    return;
                }
            }
            // If caret is inside an existing matching construct, step out if at the end or
            // unwrap the construct.
            if let Some(found) = line.inlines.iter().rev().find(|c| {
                c.kind == InlineKind::Style(style) && caret >= c.inner.start && caret <= c.inner.end
            }) {
                let (open_r, close_r, inner_end) =
                    (found.open.clone(), found.close.clone(), found.inner.end);
                if caret == inner_end {
                    self.now.selection = close_r.end..close_r.end;
                    self.history.break_run();
                } else {
                    self.history.checkpoint(&self.now);
                    self.replace(close_r, "");
                    self.replace(open_r, "");
                }
                return;
            }
            // Insert empty delimiter pair and place caret inside so typed text is styled.
            self.history.checkpoint(&self.now);
            let inserted = format!("{open}{close}");
            self.replace(caret..caret, &inserted);
            let inside = caret + open.len();
            self.now.selection = inside..inside;
            return;
        }

        // Non-empty selection: collect per-line slices clamped to non-code `line.content`,
        // trimming leading/trailing whitespace so markers never hug spaces.
        enum LineOp {
            Unwrap {
                open_r: Range<usize>,
                close_r: Range<usize>,
                inner_len: usize,
            },
            Wrap(Range<usize>),
        }

        let text = self.now.doc.text();
        let mut ops: Vec<LineOp> = Vec::new();
        for line in &parsed.lines {
            if line.end < range.start || line.start > range.end || is_non_formatted_line(line.kind)
            {
                continue;
            }
            let slice_start = line.content.start.max(range.start);
            let slice_end = line.content.end.min(range.end);
            if slice_start >= slice_end {
                continue;
            }
            let raw = &text[slice_start..slice_end];
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lead = raw.len() - raw.trim_start().len();
            let target = slice_start + lead..slice_start + lead + trimmed.len();

            if let Some(found) = line.inlines.iter().rev().find(|c| {
                c.kind == InlineKind::Style(style)
                    && target.start >= c.outer.start
                    && target.end <= c.outer.end
            }) {
                ops.push(LineOp::Unwrap {
                    open_r: found.open.clone(),
                    close_r: found.close.clone(),
                    inner_len: found.inner.len(),
                });
            } else {
                ops.push(LineOp::Wrap(target));
            }
        }
        if ops.is_empty() {
            return;
        }

        // If every line is already wrapped, unwrap them all; otherwise wrap the unwrapped ones.
        let all_wrapped = ops.iter().all(|op| matches!(op, LineOp::Unwrap { .. }));
        self.history.checkpoint(&self.now);
        let single_line = ops.len() == 1;
        if all_wrapped {
            let mut single_sel = None;
            for op in ops.into_iter().rev() {
                if let LineOp::Unwrap {
                    open_r,
                    close_r,
                    inner_len,
                } = op
                {
                    let new_start = open_r.start;
                    self.replace(close_r, "");
                    self.replace(open_r, "");
                    if single_line {
                        single_sel = Some(new_start..new_start + inner_len);
                    }
                }
            }
            if let Some(sel) = single_sel {
                self.now.selection = sel;
            }
        } else {
            let mut single_sel = None;
            for op in ops.into_iter().rev() {
                if let LineOp::Wrap(target) = op {
                    // Use `**` / `~~` if directly abutting word characters so single-marker
                    // word-boundary rules don't reject it.
                    let (o, c) = if style == Style::BOLD || style == Style::STRIKE {
                        let before_word = self.now.doc.text()[..target.start]
                            .chars()
                            .next_back()
                            .is_some_and(char::is_alphanumeric);
                        let after_word = self.now.doc.text()[target.end..]
                            .chars()
                            .next()
                            .is_some_and(char::is_alphanumeric);
                        if before_word || after_word {
                            if style == Style::BOLD {
                                ("**", "**")
                            } else {
                                ("~~", "~~")
                            }
                        } else {
                            (open, close)
                        }
                    } else {
                        (open, close)
                    };
                    self.replace(target.end..target.end, c);
                    self.replace(target.start..target.start, o);
                    if single_line {
                        single_sel = Some(target.start + o.len()..target.end + o.len());
                    }
                }
            }
            if let Some(sel) = single_sel {
                self.now.selection = sel;
            }
        }
    }

    /// Clear every inline style delimiter overlapping the selection (or enclosing the caret).
    /// Links and block prefixes are kept.
    pub fn clear_styles(&mut self) {
        let range = self.now.selection.clone();
        let parsed = self.now.doc.parse();
        let mut removals: Vec<Range<usize>> = Vec::new();
        for line in &parsed.lines {
            if line.end < range.start || line.start > range.end {
                continue;
            }
            for c in &line.inlines {
                if !matches!(c.kind, InlineKind::Style(_)) {
                    continue;
                }
                let overlaps = if range.is_empty() {
                    range.start >= c.outer.start && range.start <= c.outer.end
                } else {
                    c.outer.end > range.start && c.outer.start < range.end
                };
                if overlaps {
                    removals.push(c.open.clone());
                    removals.push(c.close.clone());
                }
            }
        }
        if removals.is_empty() {
            return;
        }
        removals.sort_by_key(|r| r.start);
        removals.dedup();
        self.history.checkpoint(&self.now);
        for rem in removals.into_iter().rev() {
            self.replace(rem, "");
        }
    }

    /// The outer range of the markdown link `[label](url)` touched by the selection, if any.
    pub fn active_link(&self) -> Option<Range<usize>> {
        let parsed = self.now.doc.parse();
        parsed.link_at(&self.now.selection).map(|c| c.outer.clone())
    }

    /// Link the selection to `url` as `[label](url)`, preserving any atoms inside the
    /// selection (or updating the URL if already on a link). With nothing selected, inserts
    /// `url`.
    pub fn set_link(&mut self, url: &str) {
        let range = self.now.selection.clone();
        let parsed = self.now.doc.parse();
        if let Some(existing) = parsed.link_at(&range) {
            let close_r = existing.close.clone();
            let replacement = format!("]({url})");
            let end = close_r.start + replacement.len();
            self.history.checkpoint(&self.now);
            self.replace(close_r, &replacement);
            self.now.selection = end..end;
            return;
        }
        let line = parsed.line_at(range.start);
        if is_non_formatted_line(line.kind) {
            return;
        }
        if range.is_empty() {
            self.history.checkpoint(&self.now);
            self.replace(range, url);
            return;
        }
        // Clamp to a single line's non-empty content and wrap via two edge insertions so any
        // @mention or :emoji: atoms inside `range` stay intact.
        let start = range.start.max(line.content.start);
        let end = range.end.min(line.content.end);
        if start >= end || self.now.doc.text()[start..end].trim().is_empty() {
            return;
        }
        let suffix = format!("]({url})");
        self.history.checkpoint(&self.now);
        self.replace(end..end, &suffix);
        self.replace(start..start, "[");
        let after = end + 1 + suffix.len();
        self.now.selection = after..after;
    }

    /// Remove the markdown link wrapper around [`Editor::active_link`]; the label stays.
    pub fn unlink(&mut self) {
        let parsed = self.now.doc.parse();
        let Some(link) = parsed.link_at(&self.now.selection) else {
            return;
        };
        let (open_r, close_r) = (link.open.clone(), link.close.clone());
        let label_len = link.inner.len();
        let start = open_r.start;
        self.history.checkpoint(&self.now);
        self.replace(close_r, "");
        self.replace(open_r, "");
        self.now.selection = start..start + label_len;
    }
}
