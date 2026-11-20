//! Slack's as-you-type conversions: `*bold*`, `_italic_`, `~strike~` and `` `code` `` turn
//! into formatting when the closing marker is typed, and `- `, `* `, `1. `, `> ` or three
//! backticks at the start of a line open a list, quote or code block. Each conversion is its
//! own undo step, so undoing it leaves the literal characters.

use super::marks::{Attrs, LineKind, PLAIN, Style};
use super::read::Line;
use super::state::Editor;

impl Editor {
    /// Convert what `typed`, the character just before the caret, completed. Returns whether
    /// anything was converted.
    pub(super) fn autoformat(&mut self, typed: char) -> bool {
        let caret = self.now.selection.start;
        let line = self.now.doc.line(caret);
        if line.kind == Some(LineKind::Code) {
            return false;
        }
        match typed {
            ' ' => self.block_shortcut(&line, caret),
            '`' => self.code_fence(&line, caret) || self.inline_pair(&line, caret, Style::CODE),
            '*' => self.inline_pair(&line, caret, Style::BOLD),
            '_' => self.inline_pair(&line, caret, Style::ITALIC),
            '~' => self.inline_pair(&line, caret, Style::STRIKE),
            _ => false,
        }
    }

    /// `- `, `* `, `1. ` or `> ` typed at the start of a paragraph line opens a block.
    fn block_shortcut(&mut self, line: &Line, caret: usize) -> bool {
        if line.kind.is_some() {
            return false;
        }
        let kind = match &self.now.doc.text()[line.start..caret] {
            "- " | "* " => LineKind::Bullet,
            "1. " => LineKind::Ordered,
            "> " => LineKind::Quote,
            _ => return false,
        };
        self.history.checkpoint(&self.now);
        self.replace(line.start..caret, "", &PLAIN);
        self.set_kind(line.start, Some(kind));
        self.renumber();
        true
    }

    /// Three backticks in a paragraph line start a code block there: the line itself when
    /// they open it, otherwise a new line after the text before them.
    fn code_fence(&mut self, line: &Line, caret: usize) -> bool {
        let text = &self.now.doc.text()[line.start..caret];
        if line.kind.is_some() || !text.ends_with("```") {
            return false;
        }
        let fence = caret - 3;
        self.history.checkpoint(&self.now);
        self.replace(fence..caret, "", &PLAIN);
        if fence == line.start {
            self.set_kind(line.start, Some(LineKind::Code));
        } else {
            self.replace(fence..fence, "\n", &PLAIN);
            self.set_kind(fence + 1, Some(LineKind::Code));
        }
        true
    }

    /// The marker just typed closes a pair: drop both markers and format what is between.
    /// The opening marker must start a word and the content must not begin or end with
    /// whitespace, so `2 * 3 * 4` and `snake_case_name` stay as typed.
    fn inline_pair(&mut self, line: &Line, caret: usize, style: Style) -> bool {
        let doc = &self.now.doc;
        let close = caret - 1;
        let marker = &doc.text()[close..caret];
        let Some(open) = doc.text()[line.content.start..close]
            .rfind(marker)
            .map(|ix| line.content.start + ix)
        else {
            return false;
        };
        let content = &doc.text()[open + 1..close];
        let starts_word = doc.text()[line.content.start..open]
            .chars()
            .next_back()
            .is_none_or(|before| !before.is_alphanumeric());
        let in_code = |offset| doc.attrs_at(offset).style.contains(Style::CODE);
        let converts = !content.is_empty()
            && !content.starts_with(char::is_whitespace)
            && !content.ends_with(char::is_whitespace)
            && starts_word
            && doc.atom_at(open).is_none()
            && !in_code(open)
            && !in_code(close);
        if !converts {
            return false;
        }
        self.history.checkpoint(&self.now);
        self.replace(close..caret, "", &PLAIN);
        self.replace(open..open + 1, "", &PLAIN);
        self.now.doc.map_attrs(open..close - 1, |attrs| Attrs {
            style: attrs.style.with(style),
            link: attrs.link.clone(),
        });
        // What is typed next is outside the pair.
        self.pending = Some(self.active_style().without(style));
        true
    }
}
