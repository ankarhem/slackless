//! The composer's rich-text editing model: a WYSIWYG document (formatted runs, mention and
//! emoji atoms, list / quote / code lines), the edits Slack's composer makes to one, undo,
//! and its two serializations (mrkdwn text and the `rich_text` block). Nothing here touches
//! GPUI, so all of it is unit-tested.
//!
//! # Architecture decision
//!
//! The composer keeps gpui-kit's `TextareaState` as its editing engine and drives it from
//! this model, instead of replacing it with a custom GPUI text element.
//!
//! The engine already does the parts of text editing that are hard to get right and
//! impossible to unit-test: IME composition, caret movement and selection by keyboard,
//! mouse and touch, soft wrapping, auto-grow and scrolling, clipboard, accessibility. It
//! also has exactly the three hooks true WYSIWYG needs. `set_decorations` draws bold,
//! italic, strikethrough, code and link styling over plain text, so no marker character is
//! ever visible. Inline tokens are atomic units the caret cannot enter, which is what a
//! mention chip is, and what a list bullet or quote bar is. `InputEditsEvent` reports each
//! edit as an exact byte range, so this model follows typing in O(edit) with no diffing.
//!
//! The division of work: the engine owns typing; [`Editor::user_edit`] mirrors each edit
//! into the [`Doc`]. Everything Slack-specific is decided here (formats armed for the next
//! character, markup that converts as you type, lists that continue on Enter, undo) and the
//! composer pushes the resulting document back to the engine as one text replacement plus
//! its tokens and decorations.
//!
//! The rejected alternative, a custom element over its own layout, would render block
//! chrome more faithfully (see the limits below) but would mean re-implementing and
//! hand-verifying all of the above, with every caret and IME bug landing in the one control
//! the user types into all day.
//!
//! Limits that follow from the engine: decorations cannot change the font, so code is shown
//! by color and background rather than monospace, and a wrapped list item continues under
//! its bullet instead of hanging. Undo is this model's (the engine's own history does not
//! know about formatting), so the composer routes Undo and Redo here.

mod autoformat;
mod blocks;
mod complete;
mod doc;
mod format;
mod history;
mod marks;
mod mrkdwn;
mod mrkdwn_read;
mod read;
mod rich_text;
mod state;
#[cfg(test)]
mod tests;

pub use complete::{Lookup, rank, trigger};
pub use doc::Doc;
pub use marks::{Atom, Attrs, LineKind, Style};
pub use mrkdwn::Directory;
pub use mrkdwn_read::from_mrkdwn;
pub use state::{Editor, narrowed};
