//! Inline formatting: toggling bold, italic, underline, strikethrough and code, clearing it
//! all, and linking text.

use std::ops::Range;
use std::sync::Arc;

use super::marks::{Attrs, Style};
use super::state::Editor;

impl Editor {
    /// The formats in effect: those of the whole selection, or with nothing selected those
    /// the next typed character will take. Toolbar buttons light up from this.
    pub fn active_style(&self) -> Style {
        let selection = &self.now.selection;
        if selection.is_empty() {
            self.typing_attrs(selection).style
        } else {
            self.now.doc.common_style(selection.clone())
        }
    }

    /// Toggle `style` on the selection, or for what is typed next when nothing is selected.
    pub fn toggle_style(&mut self, style: Style) {
        let range = self.now.selection.clone();
        let active = self.active_style();
        let toggled = |current: Style| {
            if active.contains(style) {
                current.without(style)
            } else {
                current.with(style)
            }
        };
        if range.is_empty() {
            self.pending = Some(toggled(active));
            return;
        }
        self.history.checkpoint(&self.now);
        self.now.doc.map_attrs(range, |attrs| Attrs {
            style: toggled(attrs.style),
            link: attrs.link.clone(),
        });
    }

    /// Clear every inline format from the selection (Slack's Clear formatting), or from what
    /// is typed next when nothing is selected. Links and line kinds are kept.
    pub fn clear_styles(&mut self) {
        let range = self.now.selection.clone();
        if range.is_empty() {
            if let Some(style) = self.pending {
                self.pending = Some(style.without(Style::INLINE));
            }
            return;
        }
        self.history.checkpoint(&self.now);
        self.now.doc.map_attrs(range, |attrs| Attrs {
            style: attrs.style.without(Style::INLINE),
            link: attrs.link.clone(),
        });
    }

    /// The linked text the selection is on: the selection itself when it starts on a link,
    /// or with nothing selected the whole link the caret is in or right after.
    pub fn active_link(&self) -> Option<Range<usize>> {
        let doc = &self.now.doc;
        let selection = &self.now.selection;
        if !selection.is_empty() {
            return doc
                .attrs_at(selection.start)
                .link
                .is_some()
                .then(|| selection.clone());
        }
        let before = doc.text()[..selection.start]
            .char_indices()
            .next_back()
            .map(|(ix, _)| ix);
        doc.link_range(selection.start)
            .or_else(|| doc.link_range(before?))
    }

    /// Link the selection to `url`; with nothing selected, insert the URL as its own link.
    pub fn set_link(&mut self, url: &str) {
        self.history.checkpoint(&self.now);
        let link: Arc<str> = Arc::from(url);
        let range = self.now.selection.clone();
        if range.is_empty() {
            let attrs = Attrs {
                style: self.typing_attrs(&range).style,
                link: Some(link),
            };
            self.replace(range, url, &attrs);
            self.pending = None;
            return;
        }
        self.now.doc.map_attrs(range, |attrs| Attrs {
            style: attrs.style,
            link: Some(link.clone()),
        });
    }

    /// Remove the link from [`Editor::active_link`]; the text stays.
    pub fn unlink(&mut self) {
        let Some(range) = self.active_link() else {
            return;
        };
        self.history.checkpoint(&self.now);
        self.now.doc.map_attrs(range, |attrs| Attrs {
            style: attrs.style,
            link: None,
        });
    }
}
