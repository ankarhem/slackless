//! The composer's commands: formatting tools (toolbar buttons and their shortcuts), the keys
//! it takes before the textarea sees them (Enter, the menu keys, Undo), and the link prompt.
//!
//! Enter, the arrows, Tab, Esc and Undo are key *bindings* of the textarea, and GPUI runs a
//! binding's action before any key-down listener, so those are intercepted as actions in the
//! capture phase. The formatting shortcuts have no binding and arrive as plain key-downs.

use gpui_kit::component::input::{Enter, Escape, IndentInline, MoveDown, MoveUp, Redo, Undo};
use gpui_kit::gpui::{ClipboardEntry, ClipboardItem, Context, KeyDownEvent, Window};

use super::{Composer, ComposerEvent};
use crate::editor::{LineKind, Style};

/// A formatting command.
#[derive(Clone, Copy)]
pub(super) enum Tool {
    Style(Style),
    Block(LineKind),
    Link,
    /// Start an @mention at the caret.
    Mention,
    /// Start a slash command.
    Slash,
    /// Clear every inline format from the selection.
    Clear,
    /// A heading level 1 to 3 (big, medium, small).
    Heading(u8),
    /// Back to plain paragraphs (clears headings).
    Paragraph,
    /// Insert a horizontal divider line.
    Divider,
}

impl Composer {
    /// Whether `tool` is in effect at the selection (its button is lit).
    pub(super) fn is_active(&self, tool: Tool) -> bool {
        match tool {
            Tool::Style(style) => self.editor.active_style().contains(style),
            Tool::Block(kind) => self.editor.active_block() == Some(kind),
            Tool::Link => self.editor.active_link().is_some(),
            Tool::Mention | Tool::Slash | Tool::Clear | Tool::Paragraph | Tool::Divider => false,
            Tool::Heading(level) => self.editor.active_block() == Some(LineKind::Heading(level)),
        }
    }

    pub(super) fn run(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
        match tool {
            Tool::Style(style) => self.editor.toggle_style(style),
            Tool::Block(kind) => self.editor.toggle_block(kind),
            Tool::Link if self.editor.active_link().is_some() => self.editor.unlink(),
            Tool::Link => return self.open_link(window, cx),
            Tool::Mention => {
                let caret = self.editor.selection().start;
                let follows_text = self.editor.doc().text()[..caret]
                    .chars()
                    .next_back()
                    .is_some_and(|before| !before.is_whitespace());
                self.editor
                    .insert_text(if follows_text { " @" } else { "@" });
            }
            Tool::Slash => self.editor.insert_text("/"),
            Tool::Clear => self.editor.clear_styles(),
            Tool::Heading(level) => self.editor.toggle_block(LineKind::Heading(level)),
            Tool::Paragraph => self.editor.paragraph(),
            Tool::Divider => self.editor.insert_divider(),
        }
        self.was_focused = true;
        self.focus_input(window, cx);
        self.push(window, cx);
        self.changed(cx);
    }

    fn open_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.link_open = true;
        self.menu = None;
        self.link_input
            .update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    fn close_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.link_open = false;
        self.link_input
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Link the selection to the URL in the prompt; a bare domain gets `https://`.
    fn apply_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.link_input.read(cx).value().trim().to_owned();
        self.close_link(window, cx);
        if typed.is_empty() {
            return;
        }
        let url = if typed.contains("://") || typed.starts_with("mailto:") {
            typed
        } else {
            format!("https://{typed}")
        };
        self.was_focused = true;
        self.editor.set_link(&url);
        self.push(window, cx);
        self.changed(cx);
    }

    /// Handles clipboard image/file paste (attaching them to the composer) and URL-over-selection
    /// paste (linking the selected text). Returns whether the paste was taken; otherwise the
    /// textarea pastes text as usual.
    pub(super) fn on_paste(
        &mut self,
        item: &ClipboardItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.edit_mode {
            for entry in &item.entries {
                match entry {
                    ClipboardEntry::Image(image) => {
                        self.attach_clipboard_image(image.clone(), cx);
                        return true;
                    }
                    ClipboardEntry::ExternalPaths(paths) => {
                        self.attach_paths(paths.paths().to_vec(), cx);
                        return true;
                    }
                    ClipboardEntry::String(_) => {}
                }
            }
        }
        let Some(pasted) = item.text() else {
            return false;
        };
        let url = pasted.trim();
        let is_url = (url.starts_with("https://") || url.starts_with("http://"))
            && !url.contains(char::is_whitespace);
        if !is_url || self.editor.selection().is_empty() {
            return false;
        }
        self.editor.set_link(url);
        self.push(window, cx);
        self.changed(cx);
        true
    }

    pub(super) fn on_enter(&mut self, action: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_open {
            self.apply_link(window, cx);
        } else if action.secondary && action.shift && !self.edit_mode {
            self.open_text_snippet(window, cx);
        } else if action.secondary {
            self.submit(window, cx);
        } else if let Some(ix) = self.menu_pick(action) {
            self.pick(ix, window, cx);
        } else {
            let in_block = if action.shift {
                self.editor.soft_break()
            } else {
                self.editor.enter()
            };
            if in_block {
                self.push(window, cx);
                self.changed(cx);
            } else if self.enter_sends && !action.shift {
                self.submit(window, cx);
            } else {
                // The textarea breaks the line.
                return;
            }
        }
        cx.stop_propagation();
    }

    /// The menu row a plain Enter picks. A slash command typed in full is sent instead.
    fn menu_pick(&self, action: &Enter) -> Option<usize> {
        let menu = self.menu.as_ref().filter(|_| !action.shift)?;
        (!menu.is_typed_command(self.editor.doc().text())).then_some(menu.selected)
    }

    pub(super) fn on_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.step(false);
            cx.notify();
        } else if !self.edit_mode && self.editor.doc().text().is_empty() && !self.link_open {
            cx.emit(ComposerEvent::EditLast);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    pub(super) fn on_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.step(true);
            cx.notify();
            cx.stop_propagation();
        }
    }

    pub(super) fn on_tab(&mut self, _: &IndentInline, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.menu.as_ref().map(|menu| menu.selected) {
            self.pick(ix, window, cx);
            cx.stop_propagation();
        }
    }

    pub(super) fn on_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_open {
            self.close_link(window, cx);
        } else if let Some(menu) = self.menu.take() {
            self.dismissed = Some(menu.range.start);
            cx.notify();
        } else if self.edit_mode {
            cx.emit(ComposerEvent::Cancel);
        } else {
            return;
        }
        cx.stop_propagation();
    }

    /// Undo is the editor's: the textarea's own history knows nothing of formatting.
    pub(super) fn on_undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_open {
            return;
        }
        if self.editor.undo() {
            self.push(window, cx);
            self.changed(cx);
        }
        cx.stop_propagation();
    }

    pub(super) fn on_redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        if self.link_open {
            return;
        }
        if self.editor.redo() {
            self.push(window, cx);
            self.changed(cx);
        }
        cx.stop_propagation();
    }

    /// Slack's formatting shortcuts: Cmd+B, Cmd+I, Cmd+U (underline), Cmd+Shift+X
    /// (strikethrough), Cmd+Shift+C (code), Cmd+Shift+U (link), Cmd+Shift+7/8/9 (ordered list,
    /// bullets, quote), Cmd+Alt+Shift+C (code block) and Cmd+\ (clear formatting).
    pub(super) fn on_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let modifiers = event.keystroke.modifiers;
        if self.link_open || !modifiers.secondary(cx) {
            return;
        }
        if !self.edit_mode
            && matches!(
                (event.keystroke.key.as_str(), modifiers.shift, modifiers.alt),
                ("o", false, false)
            )
        {
            cx.stop_propagation();
            self.pick_files(cx);
            return;
        }
        // With Shift (and Alt) held the platform may report the shifted character.
        let tool = match (event.keystroke.key.as_str(), modifiers.shift, modifiers.alt) {
            ("b", false, false) => Tool::Style(Style::BOLD),
            ("i", false, false) => Tool::Style(Style::ITALIC),
            ("u", false, false) => Tool::Style(Style::UNDERLINE),
            ("\\", false, false) => Tool::Clear,
            ("x", true, false) => Tool::Style(Style::STRIKE),
            ("c", true, false) => Tool::Style(Style::CODE),
            ("u", true, false) => Tool::Link,
            ("7" | "&", true, false) => Tool::Block(LineKind::Ordered),
            ("8" | "*", true, false) => Tool::Block(LineKind::Bullet),
            ("9" | "(", true, false) => Tool::Block(LineKind::Quote),
            ("c" | "\u{e7}" | "\u{c7}", true, true) => Tool::Block(LineKind::Code),
            _ => return,
        };
        cx.stop_propagation();
        self.run(tool, window, cx);
    }
}
