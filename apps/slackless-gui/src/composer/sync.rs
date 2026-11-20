//! Keeping the textarea and the editor in step. The textarea is where text is typed, the
//! editor is what the draft is: each edit the textarea makes is mirrored into the editor
//! ([`Composer::on_edits`]), and each change the editor makes on its own is shown in the
//! textarea ([`Composer::push`]).

use std::ops::Range;

use gpui_kit::component::input::{
    InlineTokenError, InputContent, InputEdit, InputEditsEvent, TextareaState,
};
use gpui_kit::gpui::{Context, Entity, EntityInputHandler as _, Window};

use super::Composer;
use super::menu::Menu;
use super::tokens::{self, Palette};
use crate::editor::narrowed;

impl Composer {
    /// The textarea edited its text: mirror the edit into the editor, then show whatever the
    /// editor made of it.
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
        let selection = self.input.read(cx).selected_range();
        let diverged = match event.edits() {
            [(edit, _)] if lines_up(text, edit, after.len()) => {
                let typed = after.slice(edit.start_byte..edit.new_end_byte).to_string();
                self.editor
                    .user_edit(edit.start_byte..edit.old_end_byte, &typed, selection)
            }
            // Several cursors edited at once, or an edit that does not line up with the
            // document: follow by difference.
            _ => self.editor.user_edit(0..len, &after.to_string(), selection),
        };
        let tokens = self.input.read(cx).tokens().len();
        if diverged || tokens != self.editor.doc().atoms().len() {
            self.push(window, cx);
        } else {
            self.restyle(cx);
        }
        self.changed(cx);
    }

    /// The textarea repainted; when only the caret or selection moved, follow it.
    pub(super) fn on_moved(&mut self, input: &Entity<TextareaState>, cx: &mut Context<Self>) {
        let state = input.read(cx);
        // A different length means an edit is on its way through `on_edits`; a stale
        // textarea still shows the previous draft, whose offsets mean nothing here.
        if self.is_stale || state.text().len() != self.editor.doc().text().len() {
            return;
        }
        let shown = state.selected_range();
        let moved = self.editor.select(shown.clone());
        // The editor keeps the caret out from in front of a line's marker.
        let selection = self.editor.selection();
        if selection != shown {
            input.update(cx, |state, cx| state.set_selected_range(selection, cx));
        }
        if moved {
            self.changed(cx);
        }
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
    /// differs), tokens, formatting and selection.
    pub(super) fn push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        self.input.update(cx, |state, cx| {
            let shown = state.text().to_string();
            let content = tokens::content(doc);
            let is_new_text = shown != doc.text();
            if !is_new_text && state.tokens() == content.tokens() {
                state.set_selected_range(selection, cx);
                state.set_decorations(tokens::decorations(doc, palette), cx);
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
                report(state.set_tokens(content, cx));
            }
            state.set_decorations(tokens::decorations(doc, palette), cx);
        });
        self.palette = Some(palette);
    }

    /// Refill the textarea from the editor wholesale (a loaded or cleared draft): its own
    /// edit history starts over.
    pub(super) fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.editor.doc();
        let selection = self.editor.selection();
        let palette = Palette::of(cx);
        self.input.update(cx, |state, cx| {
            state.set_value(tokens::content(doc), window, cx);
            state.set_decorations(tokens::decorations(doc, palette), cx);
            state.set_selected_range(selection, cx);
        });
        self.palette = Some(palette);
        self.is_stale = false;
    }

    /// Redraw the draft's formatting (after typing, or a theme change).
    pub(super) fn restyle(&mut self, cx: &mut Context<Self>) {
        let doc = self.editor.doc();
        let palette = Palette::of(cx);
        self.input.update(cx, |state, cx| {
            state.set_decorations(tokens::decorations(doc, palette), cx);
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
