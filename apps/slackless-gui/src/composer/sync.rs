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
        let diverged = match event.edits() {
            [(edit, _)] if lines_up(text, edit, after.len()) => {
                let typed = after.slice(edit.start_byte..edit.new_end_byte).to_string();
                closed_colon = typed == ":";
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
        let converted = closed_colon && self.convert_typed_emoji();
        if diverged || converted {
            self.push(window, cx);
        } else {
            self.sync_presentation(cx);
        }
        self.changed(cx);
    }

    /// Slack turns a typed `:shortcode:` into its emoji as the closing colon is typed (a
    /// glyph, or the workspace's image), as a pick from the emoji menu would. Returns
    /// whether it did. Skipped inside code blocks, inline code, and markdown link URLs.
    fn convert_typed_emoji(&mut self) -> bool {
        let caret = self.editor.selection().end;
        let doc = self.editor.doc();
        let before = &doc.text()[..caret];
        let Some(open) = before[..before.len().saturating_sub(1)].rfind(':') else {
            return false;
        };
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
            self.sync_presentation(cx);
            self.changed(cx);
        }
    }

    /// Focus gained or lost: re-project Live Preview so active syntax reveals on focus and
    /// collapses cleanly on blur.
    pub(super) fn sync_focus(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.was_focused = focused;
        self.sync_presentation(cx);
    }

    /// The draft or the caret changed: reopen or close the completion menu.
    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        self.menu = match Menu::at_caret(&self.editor, &self.fixtures) {
            Some(menu) if self.dismissed == Some(menu.range.start) => None,
            menu => {
                self.dismissed = None;
                menu
            }
        };
        cx.notify();
    }

    /// Show the editor's document in the textarea: its text (one replacement over what
    /// differs), Live Preview tokens, decorations, and selection.
    pub(super) fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(doc, &selection, self.was_focused, palette);
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
            if !content.tokens().is_empty() {
                if let Err(InlineTokenError::SplitsSelection) = state.set_tokens(content, cx) {
                    report(state.set_tokens(tokens::atoms_only(doc), cx));
                }
            }
            state.set_decorations(decorations, cx);
        });
        self.palette = Some(palette);
    }

    /// Refill the textarea from the editor wholesale (a loaded or cleared draft): its own
    /// edit history starts over.
    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(doc, &selection, self.was_focused, palette);
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
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        let (content, decorations) = tokens::project(doc, &selection, self.was_focused, palette);
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
        eprintln!("slackless: composer tokens not applied: {error}");
    }
}
