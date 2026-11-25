//! Block commands in Obsidian Live Preview: headings (`#`/`##`/`###`), lists (`- `, `1. `),
//! quotes (`> `), fenced code blocks (```` ``` ````), dividers (`---`), and Enter/Shift+Enter
//! block continuation. Every block lives as real editable markdown text in [`Doc`].

use super::marks::LineKind;
use super::parse::is_fence_line;
use super::state::Editor;

impl Editor {
    /// The block kind of the line the selection starts on (`Fence` reports as `Code` so the
    /// Code block toolbar button lights up on fence lines too).
    pub fn active_block(&self) -> Option<LineKind> {
        let parsed = self.now.doc.parse();
        match parsed.line_at(self.now.selection.start).kind {
            Some(LineKind::Fence) => Some(LineKind::Code),
            other => other,
        }
    }

    /// The dropdown's Paragraph: strip `# ` / `## ` / `### ` from every selected heading line
    /// (lists, quotes and code blocks stay).
    pub fn paragraph(&mut self) {
        let parsed = self.now.doc.parse();
        let selection = self.now.selection.clone();
        let prefixes: Vec<_> = parsed
            .lines
            .iter()
            .filter(|line| line.end >= selection.start && line.start <= selection.end)
            .filter(|line| matches!(line.kind, Some(LineKind::Heading(_))))
            .map(|line| line.prefix.clone())
            .collect();
        if prefixes.is_empty() {
            return;
        }
        self.history.checkpoint(&self.now);
        for prefix in prefixes.into_iter().rev() {
            self.replace(prefix, "");
        }
        self.renumber();
    }

    /// Put every selected non-code line in a `kind` block, or strip it back to paragraphs
    /// when all selected lines already carry `kind`.
    pub fn toggle_block(&mut self, kind: LineKind) {
        if kind == LineKind::Code {
            self.toggle_code_block();
            return;
        }
        let parsed = self.now.doc.parse();
        let selection = self.now.selection.clone();
        let selected: Vec<_> = parsed
            .lines
            .iter()
            .filter(|line| line.end >= selection.start && line.start <= selection.end)
            .filter(|line| !matches!(line.kind, Some(LineKind::Code | LineKind::Fence)))
            .map(|line| (line.start, line.kind, line.prefix.clone()))
            .collect();
        if selected.is_empty() {
            return;
        }
        let all = selected.iter().all(|(_, found, _)| *found == Some(kind));
        let new_prefix = if all { "" } else { kind.markdown_prefix() };
        self.history.checkpoint(&self.now);
        for (_, _, old_prefix) in selected.into_iter().rev() {
            self.replace(old_prefix, new_prefix);
        }
        self.renumber();
    }

    fn toggle_code_block(&mut self) {
        let parsed = self.now.doc.parse();
        let selection = self.now.selection.clone();
        let first_ix = parsed
            .lines
            .partition_point(|line| line.end < selection.start)
            .min(parsed.lines.len().saturating_sub(1));
        let last_ix = parsed
            .lines
            .partition_point(|line| line.end < selection.end)
            .min(parsed.lines.len().saturating_sub(1));

        let in_code = parsed.lines[first_ix..=last_ix]
            .iter()
            .all(|line| matches!(line.kind, Some(LineKind::Code | LineKind::Fence)));
        if in_code {
            // Pair opening and closing fences by parity from line 0 so toggling on a closing
            // fence always finds its matching opening fence and never merges two blocks.
            let mut open_fence: Option<usize> = None;
            let mut pair: Option<(usize, Option<usize>)> = None;
            for (ix, line) in parsed.lines.iter().enumerate() {
                if line.kind == Some(LineKind::Fence) {
                    if let Some(open) = open_fence.take() {
                        if first_ix >= open && last_ix <= ix {
                            pair = Some((open, Some(ix)));
                            break;
                        }
                    } else {
                        open_fence = Some(ix);
                    }
                }
            }
            if pair.is_none()
                && let Some(open) = open_fence
                && first_ix >= open
            {
                pair = Some((open, None));
            }
            if let Some((open_ix, close_ix)) = pair {
                self.history.checkpoint(&self.now);
                if let Some(ix) = close_ix {
                    let line = &parsed.lines[ix];
                    let cut = if line.start > 0 {
                        line.start - 1..line.end
                    } else {
                        line.start..line.end
                    };
                    self.replace(cut, "");
                }
                let line = &parsed.lines[open_ix];
                let cut = if line.end < self.now.doc.text().len() {
                    line.start..line.end + 1
                } else {
                    line.start..line.end
                };
                self.replace(cut, "");
                return;
            }
        }

        // Wrap selected lines in ```\n...\n```, leaving caret on the code line inside.
        let start = parsed.lines[first_ix].start;
        let end = parsed.lines[last_ix].end;
        let caret = self.now.selection.start;
        self.history.checkpoint(&self.now);
        self.replace(end..end, "\n```");
        self.replace(start..start, "```\n");
        let moved = caret + 4;
        self.now.selection = moved..moved;
    }

    /// Enter inside a block: an empty item leaves the block, anything else continues the
    /// block (headings and dividers break to a plain paragraph). False on a paragraph line,
    /// which the composer handles (send, or a newline).
    pub fn enter(&mut self) -> bool {
        let parsed = self.now.doc.parse();
        let selection = self.now.selection.clone();
        let line = parsed.line_at(selection.start);
        let Some(kind) = line.kind else {
            return false;
        };
        let is_empty = line.content.is_empty() && selection.is_empty();
        let prefix = line.prefix.clone();
        let line_start = line.start;
        let line_end = line.end;
        let content_start = line.content.start;
        let line_ix = parsed
            .lines
            .iter()
            .position(|l| l.start == line_start)
            .unwrap_or(0);
        self.history.checkpoint(&self.now);

        // Caret inside a block prefix: never split `# `, `- `, `---`, or ```` ``` ```` in half.
        if selection.is_empty() && selection.start < content_start {
            match kind {
                LineKind::Divider | LineKind::Fence => {
                    self.replace(line_end..line_end, "\n");
                }
                _ => {
                    self.replace(line_start..line_start, "\n");
                }
            }
            self.renumber();
            return true;
        }

        match (kind, is_empty) {
            (LineKind::Code | LineKind::Fence | LineKind::Divider, _) => {
                self.replace(selection, "\n");
            }
            (LineKind::Soft, true) => {
                let parent = self.parent_list_kind(line_ix);
                self.replace(prefix, parent.markdown_prefix());
            }
            (_, true) => {
                self.replace(prefix, "");
            }
            (LineKind::Soft, false) => {
                let parent = self.parent_list_kind(line_ix);
                let next = format!("\n{}", parent.markdown_prefix());
                self.replace(selection, &next);
            }
            (LineKind::Heading(_), false) => {
                self.replace(selection, "\n");
            }
            (LineKind::Bullet, false) => {
                self.replace(selection, "\n- ");
            }
            (LineKind::Ordered, false) => {
                self.replace(selection, "\n1. ");
            }
            (LineKind::Quote, false) => {
                self.replace(selection, "\n> ");
            }
        }
        self.renumber();
        true
    }

    /// Shift+Enter inside a block: a new line that stays in the same list item (`    `),
    /// quote (`> `), or code block. False on a plain paragraph line, where the textarea
    /// inserts the newline.
    pub fn soft_break(&mut self) -> bool {
        let parsed = self.now.doc.parse();
        let selection = self.now.selection.clone();
        let line = parsed.line_at(selection.start);
        let Some(kind) = line.kind else {
            return false;
        };
        let line_start = line.start;
        let line_end = line.end;
        let content_start = line.content.start;
        self.history.checkpoint(&self.now);
        if selection.is_empty() && selection.start < content_start {
            match kind {
                LineKind::Divider | LineKind::Fence => self.replace(line_end..line_end, "\n"),
                _ => self.replace(line_start..line_start, "\n"),
            }
            return true;
        }
        let insert = match kind {
            LineKind::Bullet | LineKind::Ordered | LineKind::Soft => "\n    ",
            LineKind::Quote => "\n> ",
            LineKind::Code | LineKind::Fence | LineKind::Heading(_) | LineKind::Divider => "\n",
        };
        self.replace(selection, insert);
        true
    }

    /// Slack's toolbar Divider: `---` on its own line, then a fresh paragraph line below it
    /// with the caret so the divider immediately collapses into a horizontal rule.
    pub fn insert_divider(&mut self) {
        let parsed = self.now.doc.parse();
        let line = parsed.line_at(self.now.selection.start);
        if matches!(line.kind, Some(LineKind::Code | LineKind::Fence)) {
            return;
        }
        let empty =
            line.kind.is_none() && self.now.doc.text()[line.content.clone()].trim().is_empty();
        let (start, end) = (line.start, line.end);
        self.history.checkpoint(&self.now);
        if empty {
            self.replace(start..end, "---\n");
            let caret = start + 4;
            self.now.selection = caret..caret;
        } else {
            self.replace(end..end, "\n---\n");
            let caret = end + 5;
            self.now.selection = caret..caret;
        }
    }

    fn parent_list_kind(&self, mut line_ix: usize) -> LineKind {
        let parsed = self.now.doc.parse();
        while line_ix > 0 {
            line_ix -= 1;
            match parsed.lines[line_ix].kind {
                Some(LineKind::Soft) => {}
                Some(kind @ (LineKind::Bullet | LineKind::Ordered)) => return kind,
                _ => break,
            }
        }
        LineKind::Bullet
    }

    /// Renumber every contiguous run of ordered list items (`1. `, `2. `, ...) in place.
    pub(super) fn renumber(&mut self) {
        let text = self.now.doc.text();
        let atoms = self.now.doc.atoms();
        let mut updates: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        let mut in_code = false;
        let mut number = 0u32;
        let mut offset = 0usize;
        for raw in text.split('\n') {
            let start = offset;
            let end = start + raw.len();
            offset = end + 1;
            if is_fence_line(raw, start, atoms) {
                in_code = !in_code;
                number = 0;
                continue;
            }
            if in_code {
                continue;
            }
            if number > 0 && raw.starts_with("    ") {
                continue;
            }
            let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0
                && raw[digits..].starts_with(". ")
                && let Ok(existing) = raw[..digits].parse::<u32>()
                && (number > 0 || existing == 1)
            {
                number += 1;
                if existing != number {
                    updates.push((start..start + digits + 2, format!("{number}. ")));
                }
                continue;
            }
            number = 0;
        }
        for (range, replacement) in updates.into_iter().rev() {
            self.replace(range, &replacement);
        }
    }
}
