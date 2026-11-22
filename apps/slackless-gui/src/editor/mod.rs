//! The composer's Obsidian-style Live Preview Markdown editing model.
//!
//! # Architecture
//!
//! The composer keeps gpui-kit's `TextareaState` as its editing engine (preserving native
//! IME, caret movement, selection, soft wrapping, auto-grow, clipboard, and accessibility)
//! and stores **real editable Markdown** in [`Doc`] plus atomic semantic references
//! (`@mentions`, `#channels`, `@here/@channel`, `:emoji:`).
//!
//! [`Doc::parse`] derives all block structure (`#`/`##`/`###` headings, `- ` bullets,
//! `1. ` ordered lists, `> ` quotes, ```` ``` ```` code fences, `---` dividers) and inline
//! formatting (`*bold*`, `**bold**`, `_italic_`, `~strike~`, `~~strike~~`, `` `code` ``,
//! `<u>underline</u>`, `[label](url)`) purely from the markdown text. The composer then
//! projects Live Preview onto the textarea:
//! - Lines and inline spans touched by the caret reveal their raw markdown syntax in muted
//!   color while keeping live styling applied.
//! - Inactive inline delimiters collapse to 0px tokens so formatted text renders cleanly
//!   without raw markers.
//! - Inactive block prefixes render clean WYSIWYG indicators (subtle `H1`/`H2`/`H3` badges
//!   on headings, bullet discs, numbered markers, quote bars, horizontal rules).

mod blocks;
mod complete;
mod doc;
mod format;
mod history;
mod marks;
mod mrkdwn;
mod mrkdwn_read;
mod parse;
mod rich_text;
mod state;
#[cfg(test)]
mod tests;

pub use complete::{Lookup, rank, trigger};
pub use doc::Doc;
pub use marks::{Atom, Attrs, LineKind, Style};
pub use mrkdwn::Directory;
pub use mrkdwn_read::from_mrkdwn;
pub(crate) use parse::delimiter_ranges;
pub use parse::empty_pair_at;
pub use rich_text::from_rich_text;
pub use state::{Editor, narrowed};
