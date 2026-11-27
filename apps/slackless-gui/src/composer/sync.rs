//! Keeping the textarea and the Obsidian Live Preview markdown editor in step.
//!
//! - Typing arrives through [`Composer::on_edits`], which mirrors the edit into [`Editor`]
//!   and refreshes Live Preview tokens and decorations without rewriting the text buffer.
//! - Caret movement arrives through [`Composer::on_moved`], which re-projects Live Preview
//!   tokens and decorations so markers smoothly reveal at the caret and collapse away from
//!   it — with zero text mutation and zero undo pollution.
//! - Toolbar buttons, shortcuts, undo/redo, and mention/emoji picks edit [`Editor`] and push
//!   the new text back to the textarea through [`Composer::push`].

use std::ops::Range;

use gpui_kit::component::input::{
    InlineTokenError, InputContent, InputEdit, InputEditsEvent, TextareaState,
};
use gpui_kit::gpui::{Context, Entity, EntityInputHandler as _, Window};
use slackless_platform::SpellChecker as _;

use super::Composer;
use super::menu::Menu;
use super::tokens::{self, Palette};
use crate::editor::{Atom, LineKind, Style, narrowed};

impl Composer {
    /// The textarea edited its text: mirror the edit into the editor, convert `:emoji:` if
    /// closed, and refresh Live Preview tokens & decorations.
    pub(super) fn on_edits(
        &mut self,
        event: &InputEditsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, after)) = event.edits().last() else {
            return;
        };
        let text = self.editor.doc().text();
        let len = text.len();
        // Our own push echoes back as an edit that changes nothing here.
        let is_echo = after.len() == len && *after == text;
        if is_echo || self.is_stale {
            return;
        }
        self.was_focused = true;
        let selection = self.input.read(cx).selected_range();
        let mut closed_colon = false;
        let mut typed_backtick = false;
        let diverged = match event.edits() {
            [(edit, _)] if lines_up(text, edit, after.len()) => {
                let typed = after.slice(edit.start_byte..edit.new_end_byte).to_string();
                closed_colon = typed == ":";
                typed_backtick = typed == "`";
                self.editor
                    .user_edit(edit.start_byte..edit.old_end_byte, &typed, selection)
            }
            // Several cursors edited at once, or an edit that does not line up with the
            // document: follow by difference.
            _ => self.editor.user_edit(0..len, &after.to_string(), selection),
        };
        if !self.edit_mode && !self.editor.doc().text().is_empty() {
            self.note_typing(cx);
        }
        let converted = (closed_colon && self.convert_typed_emoji())
            || (typed_backtick && self.auto_open_code_block());
        // Suppress flagging the word currently being typed at the caret until the user hits
        // whitespace/punctuation or moves the caret away.
        let sel = self.editor.selection();
        let after_text = self.editor.doc().text();
        let typing_word = sel.is_empty()
            && after_text
                .get(..sel.end)
                .and_then(|prefix| prefix.chars().next_back())
                .is_some_and(|c| c.is_alphanumeric() || c == '\'');
        self.spell_typing_end = typing_word.then_some(sel.end);
        if diverged || converted {
            self.push(window, cx);
        } else {
            self.sync_presentation(cx);
        }
        self.changed(cx);
    }

    /// Slack turns typing three backticks (` ``` `) at the start of a line into a fenced code
    /// block immediately with the caret placed inside it, ready to type code. Only fires when
    /// the third backtick opens an unclosed fence (never when closing an existing block).
    pub(super) fn auto_open_code_block(&mut self) -> bool {
        let selection = self.editor.selection();
        if !selection.is_empty() {
            return false;
        }
        let caret = selection.start;
        let doc = self.editor.doc();
        let parsed = doc.parse();
        let line = parsed.line_at(caret);
        if line.kind != Some(LineKind::Fence) || caret != line.end {
            return false;
        }
        if &doc.text()[line.start..line.end] != "```" {
            return false;
        }
        // Only auto-wrap when this line is an unclosed opening fence (odd number of fences in
        // the document and even number of fences before this line).
        let fences_before = parsed
            .lines
            .iter()
            .take_while(|other| other.start < line.start)
            .filter(|other| other.kind == Some(LineKind::Fence))
            .count();
        let total_fences = parsed
            .lines
            .iter()
            .filter(|other| other.kind == Some(LineKind::Fence))
            .count();
        if fences_before % 2 != 0 || total_fences % 2 == 0 {
            return false;
        }
        let line_start = line.start;
        self.editor.insert_text("\n\n```");
        // Move caret onto the empty code line right after ` ```\n `.
        let inside = line_start + 4;
        self.editor.select(inside..inside);
        true
    }

    /// Slack turns a typed `:shortcode:` into its emoji as the closing colon is typed (a
    /// glyph, or the workspace's image), as a pick from the emoji menu would. Returns
    /// whether it did. Skipped inside code blocks, inline code, and markdown link URLs.
    pub(super) fn convert_typed_emoji(&mut self) -> bool {
        let caret = self.editor.selection().end;
        let doc = self.editor.doc();
        if caret > doc.text().len() || !doc.text().is_char_boundary(caret) {
            return false;
        }
        let before = &doc.text()[..caret];
        let Some(prefix) = before.strip_suffix(':') else {
            return false;
        };
        // Never scan back into an existing placed atom (e.g. custom emoji `:party:` followed by
        // `smile:` must not steal `:party:`'s closing colon).
        let atom_floor = doc
            .atoms()
            .iter()
            .filter(|a| a.range.end <= caret)
            .map(|a| a.range.end)
            .max()
            .unwrap_or(0)
            .min(prefix.len());
        let Some(rel_open) = prefix[atom_floor..].rfind(':') else {
            return false;
        };
        let open = atom_floor + rel_open;
        let parsed = doc.parse();
        let line = parsed.line_at(open);
        if matches!(
            line.kind,
            Some(LineKind::Code | LineKind::Fence | LineKind::Divider)
        ) || parsed.attrs_at(doc, open).style.contains(Style::CODE)
        {
            return false;
        }
        if parsed
            .link_at(&(open..caret))
            .is_some_and(|link| open >= link.close.start)
        {
            return false;
        }
        let Some((len, shortcode)) = crate::emoji::parse_shortcode(&before[open..]) else {
            return false;
        };
        let shown = match crate::emoji::EmojiArt::of(&shortcode, &self.fixtures) {
            crate::emoji::EmojiArt::Glyph(glyph) => glyph,
            crate::emoji::EmojiArt::Image(_) => format!(":{shortcode}:"),
            crate::emoji::EmojiArt::Text(_) => return false,
        };
        if open + len != caret {
            return false;
        }
        self.editor
            .place_atom(open..caret, Atom::Emoji(shortcode.into()), &shown);
        true
    }

    /// The textarea repainted; when only the caret or selection moved, follow it and refresh
    /// Live Preview tokens/decorations (without touching the text buffer).
    pub(super) fn on_moved(&mut self, input: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let state = input.read(cx);
        // Different text means an edit is on its way through `on_edits`; a stale textarea
        // still shows the previous draft, whose offsets mean nothing here.
        if self.is_stale || *state.text() != self.editor.doc().text() {
            return;
        }
        let shown = state.selected_range();
        let moved = self.editor.select(shown);
        if moved {
            self.spell_typing_end = None;
            self.sync_presentation(cx);
            self.changed(cx);
        }
    }

    /// Focus gained or lost: re-project Live Preview so active syntax reveals on focus and
    /// collapses cleanly on blur.
    pub(super) fn sync_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.was_focused = focused;
        self.spell_typing_end = None;
        self.sync_presentation(cx);
    }

    /// Re-run OS spellchecking on plain prose spans (skipping code blocks, inline code,
    /// semantic atoms, and URLs) and compute replacement suggestions when the caret is on a
    /// misspelled word (`F-SHELL-098`, `099`).
    pub(super) fn refresh_spellcheck(&mut self) {
        if !self.spellcheck {
            self.misspellings.clear();
            self.spell_guesses = None;
            return;
        }
        let doc = self.editor.doc();
        let text = doc.text();
        if text.trim().is_empty() || text.starts_with('/') {
            self.misspellings.clear();
            self.spell_guesses = None;
            return;
        }
        let parsed = doc.parse();
        let mut skip: Vec<Range<usize>> = doc.atoms().iter().map(|a| a.range.clone()).collect();
        for line in &parsed.lines {
            if matches!(line.kind, Some(LineKind::Fence | LineKind::Code)) {
                skip.push(line.start..line.end);
            } else {
                for run in parsed.styled_runs(line) {
                    if run.attrs.style.contains(Style::CODE) || run.attrs.link.is_some() {
                        skip.push(run.range);
                    }
                }
            }
        }
        let checker = slackless_platform::current();
        let raw = checker.check_spelling(text, self.spellcheck_lang);
        self.misspellings = raw
            .into_iter()
            .map(|m| m.range)
            .filter(|r| {
                let word = &text[r.clone()];
                self.spell_typing_end != Some(r.end)
                    && !word.contains("://")
                    && !word.starts_with('@')
                    && !word.starts_with('#')
                    && !word.starts_with(':')
                    && !skip.iter().any(|s| r.start < s.end && s.start < r.end)
            })
            .collect();
        self.update_spell_guesses();
    }

    pub(super) fn update_spell_guesses(&mut self) {
        if !self.spellcheck || self.misspellings.is_empty() {
            self.spell_guesses = None;
            return;
        }
        let caret = self.editor.selection().start;
        let text = self.editor.doc().text();
        let hit = self
            .misspellings
            .iter()
            .find(|r| self.spell_typing_end != Some(r.end) && caret >= r.start && caret <= r.end)
            .cloned();
        self.spell_guesses = hit.and_then(|range| {
            let word = text.get(range.clone())?.to_owned();
            let checker = slackless_platform::current();
            let guesses = checker.spelling_guesses(&word, self.spellcheck_lang);
            Some((range, word, guesses))
        });
    }

    /// Replace a misspelled word range with a chosen spelling suggestion.
    pub(super) fn apply_spelling_fix(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = range.start + replacement.len();
        self.spell_typing_end = None;
        self.editor.user_edit(range, replacement, end..end);
        self.refresh_spellcheck();
        self.push(window, cx);
        self.changed(cx);
    }

    /// Teach the OS spellchecker `word` and clear its underline.
    pub(super) fn learn_spelling_word(&mut self, word: &str, cx: &mut Context<Self>) {
        let checker = slackless_platform::current();
        checker.learn_word(word);
        self.spell_typing_end = None;
        self.refresh_spellcheck();
        self.sync_presentation(cx);
        cx.notify();
    }

    /// The draft or the caret changed: reopen or close the completion menu.
    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        self.menu = match Menu::at_caret_with_slash(&self.editor, &self.fixtures, !self.edit_mode) {
            Some(menu) if self.dismissed == Some(menu.range.start) => None,
            menu => {
                self.dismissed = None;
                menu
            }
        };
        self.update_spell_guesses();
        cx.notify();
    }

    /// Show the editor's document in the textarea: its text (one replacement over what
    /// differs), Live Preview tokens, decorations, and selection.
    pub(super) fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_spellcheck();
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(
            doc,
            &selection,
            self.was_focused,
            self.format_markup,
            palette,
            &self.misspellings,
        );
        self.input.update(cx, |state, cx| {
            let shown = state.text().to_string();
            let is_new_text = shown != doc.text();
            if !is_new_text && state.tokens() == content.tokens() {
                state.set_selected_range(selection, cx);
                state.set_decorations(decorations, cx);
                return;
            }
            // Tokens go first: the engine widens an edit to whole tokens, and the new ones
            // are checked against the new text and selection.
            if !state.tokens().is_empty() {
                report(state.set_tokens(InputContent::new(shown.clone()), cx));
            }
            if is_new_text {
                let (range, text) = narrowed(&shown, 0..shown.len(), doc.text());
                state.replace_text_in_range(Some(utf16(&shown, range)), text, window, cx);
            }
            state.set_selected_range(selection, cx);
            if !content.tokens().is_empty()
                && let Err(InlineTokenError::SplitsSelection) = state.set_tokens(content, cx)
            {
                report(state.set_tokens(tokens::atoms_only(doc), cx));
            }
            state.set_decorations(decorations, cx);
        });
        self.palette = Some(palette);
    }

    /// Refill the textarea from the editor wholesale (a loaded or cleared draft): its own
    /// edit history starts over.
    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_spellcheck();
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(
            doc,
            &selection,
            self.was_focused,
            self.format_markup,
            palette,
            &self.misspellings,
        );
        self.input.update(cx, |state, cx| {
            state.set_value(content, window, cx);
            state.set_decorations(decorations, cx);
            state.set_selected_range(selection, cx);
        });
        self.palette = Some(palette);
        self.is_stale = false;
    }

    /// Refresh Live Preview tokens and decorations in place without touching the text buffer
    /// (after typing, cursor movement, focus change, or theme change).
    pub(super) fn sync_presentation(&mut self, cx: &mut Context<Self>) {
        self.refresh_spellcheck();
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(
            doc,
            &selection,
            self.was_focused,
            self.format_markup,
            palette,
            &self.misspellings,
        );
        self.input.update(cx, |state, cx| {
            if *state.text() == doc.text() {
                if let Err(InlineTokenError::SplitsSelection) = state.set_tokens(content, cx) {
                    let _ = state.set_tokens(tokens::atoms_only(doc), cx);
                }
                state.set_decorations(decorations, cx);
            }
        });
        self.palette = Some(palette);
    }
}

/// Whether `edit` can be applied to `text` as reported: its range lies on characters of
/// `text`, and replacing it yields a document of the engine's length `after`.
fn lines_up(text: &str, edit: &InputEdit, after: usize) -> bool {
    let (start, end) = (edit.start_byte, edit.old_end_byte);
    start <= end
        && end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end)
        && text.len() - (end - start) + (edit.new_end_byte - start) == after
}

/// `range` of `text` in UTF-16 code units, the offsets the platform input API takes.
fn utf16(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text[..range.start].encode_utf16().count();
    start..start + text[range].encode_utf16().count()
}

/// The engine can refuse tokens (an IME composition is in progress); the draft then shows
/// them as plain text until the next push.
fn report(result: Result<(), InlineTokenError>) {
    if let Err(error) = result {
        tracing::warn!(error = %error, "composer tokens not applied");
    }
}

#[cfg(test)]
mod tests {
    use super::{lines_up, utf16};
    use gpui_kit::component::input::{InputEdit, Point};

    fn edit(start_byte: usize, old_end_byte: usize, new_end_byte: usize) -> InputEdit {
        let zero = Point::new(0, 0);
        InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: zero,
            old_end_position: zero,
            new_end_position: zero,
        }
    }

    #[test]
    fn utf16_maps_ascii_bmp_and_surrogate_pair_byte_ranges() {
        // 'a' (1B, 1 u16), 'é' (2B, 1 u16), '🚀' (4B, 2 u16 surrogate pair), 'z' (1B, 1 u16)
        let text = "aé🚀z";
        assert_eq!(text.len(), 8);
        assert_eq!(utf16(text, 0..1), 0..1);
        assert_eq!(utf16(text, 1..3), 1..2);
        assert_eq!(utf16(text, 3..7), 2..4);
        assert_eq!(utf16(text, 7..8), 4..5);
        assert_eq!(utf16(text, 0..8), 0..5);
    }

    #[test]
    fn lines_up_validates_char_boundaries_bounds_and_resulting_length() {
        let text = "aé🚀z"; // len 8

        // Valid replacement of '🚀' (bytes 3..7) with "hi" (2 bytes -> new_end 5, total len 6)
        let ok = edit(3, 7, 5);
        assert!(lines_up(text, &ok, 6));
        // Wrong resulting length rejected
        assert!(!lines_up(text, &ok, 7));

        // Mid-UTF-8 byte offset rejected
        let mid_char = edit(4, 7, 4);
        assert!(!lines_up(text, &mid_char, 5));

        // Out-of-bounds or inverted range rejected
        let oob = edit(0, 99, 1);
        assert!(!lines_up(text, &oob, 1));
        let inverted = edit(7, 3, 7);
        assert!(!lines_up(text, &inverted, 8));
    }
}
