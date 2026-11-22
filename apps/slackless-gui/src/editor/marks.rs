//! What is laid over a document's markdown text: inline character styles and the semantic
//! atomic units (mentions, channels, broadcasts, emoji) that stay opaque chips while block
//! and inline formatting live as real editable markdown.

use std::ops::Range;
use std::sync::Arc;

use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{ChannelId, UserId};

/// A set of inline character formats.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Style(u8);

impl Style {
    pub const NONE: Style = Style(0);
    pub const BOLD: Style = Style(1);
    pub const ITALIC: Style = Style(1 << 1);
    pub const STRIKE: Style = Style(1 << 2);
    pub const CODE: Style = Style(1 << 3);
    pub const UNDERLINE: Style = Style(1 << 4);

    pub const fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }

    /// The formats both sets carry.
    pub const fn common(self, other: Style) -> Style {
        Style(self.0 & other.0)
    }

    pub const fn is_plain(self) -> bool {
        self.0 == 0
    }
}

/// What a run of text carries: its formats and the URL it links to.
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct Attrs {
    pub style: Style,
    pub link: Option<Arc<str>>,
}

impl Attrs {
    pub fn is_plain(&self) -> bool {
        self.style.is_plain() && self.link.is_none()
    }
}

/// Unformatted text.
pub static PLAIN: Attrs = Attrs {
    style: Style::NONE,
    link: None,
};

/// How a line is laid out as a block; a plain paragraph line has none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Bullet,
    Ordered,
    Quote,
    /// A line inside a fenced code block.
    Code,
    /// An opening or closing ```` ``` ```` code fence line.
    Fence,
    /// A further line of the list item above it (Shift+Enter inside an item).
    Soft,
    /// A Big / Medium / Small heading line, `level` 1 to 3.
    Heading(u8),
    /// A horizontal rule (`---`); the line has no text of its own.
    Divider,
}

impl LineKind {
    /// The canonical markdown prefix inserted when a toolbar button or shortcut applies
    /// this block kind to a line.
    pub(super) const fn markdown_prefix(self) -> &'static str {
        match self {
            LineKind::Bullet => "- ",
            LineKind::Ordered => "1. ",
            LineKind::Quote => "> ",
            LineKind::Soft => "    ",
            LineKind::Heading(1) => "# ",
            LineKind::Heading(2) => "## ",
            LineKind::Heading(_) => "### ",
            LineKind::Divider => "---",
            LineKind::Code | LineKind::Fence => "",
        }
    }
}

/// A semantic inline unit the caret cannot enter and that is deleted whole: a user mention,
/// a channel reference, a broadcast (`@here`/`@channel`/`@everyone`), or an emoji.
#[derive(Clone, PartialEq, Debug)]
pub enum Atom {
    User(UserId),
    Channel(ChannelId),
    Broadcast(BroadcastRange),
    /// Shortcode without colons, in Slack's `name::skin-tone-N` form when toned.
    Emoji(Arc<str>),
}

/// An atom and the UTF-8 byte range it occupies in [`super::Doc::text`].
#[derive(Clone, PartialEq, Debug)]
pub struct Placed {
    pub range: Range<usize>,
    pub atom: Atom,
}
