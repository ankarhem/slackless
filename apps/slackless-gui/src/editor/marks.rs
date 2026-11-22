//! What is laid over a document's text: inline formats and links on runs of it, and the
//! atomic units (mentions, emoji, block markers) that occupy stretches of it.

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

    /// Every inline format at once (clear formatting).
    pub const INLINE: Style =
        Style(Self::BOLD.0 | Self::ITALIC.0 | Self::STRIKE.0 | Self::CODE.0 | Self::UNDERLINE.0);

    pub const fn contains(self, other: Style) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn with(self, other: Style) -> Style {
        Style(self.0 | other.0)
    }

    pub const fn without(self, other: Style) -> Style {
        Style(self.0 & !other.0)
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

/// A run of formatted text. Plain text has no span.
#[derive(Clone, PartialEq, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub attrs: Attrs,
}

/// How a line is laid out as a block; a plain paragraph line has none.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineKind {
    Bullet,
    Ordered,
    Quote,
    Code,
    /// A further line of the list item above it (Shift+Enter inside an item).
    Soft,
    /// A Big / Medium / Small heading line, `level` 1 to 3.
    Heading(u8),
}

impl LineKind {
    /// The text a marker of this kind occupies at the start of its line. An ordered marker
    /// is renumbered in place, so its text varies.
    pub(super) const fn marker_text(self) -> &'static str {
        match self {
            LineKind::Bullet => "\u{2022} ",
            LineKind::Ordered => "1. ",
            LineKind::Quote => "> ",
            LineKind::Code | LineKind::Soft | LineKind::Heading(_) => "  ",
        }
    }
}

/// A unit the caret cannot enter and that is deleted whole: a mention, an emoji, or the
/// marker that opens a list, quote or code line.
#[derive(Clone, PartialEq, Debug)]
pub enum Atom {
    User(UserId),
    Channel(ChannelId),
    Broadcast(BroadcastRange),
    /// Shortcode without colons, in Slack's `name::skin-tone-N` form when toned.
    Emoji(Arc<str>),
    Marker(LineKind),
}

/// An atom and the text range it occupies.
#[derive(Clone, PartialEq, Debug)]
pub struct Placed {
    pub range: Range<usize>,
    pub atom: Atom,
}
