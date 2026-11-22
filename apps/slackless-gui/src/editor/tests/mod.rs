//! Behavior of the Obsidian Live Preview markdown editing model, driven the way the composer
//! drives it: typing arrives as the input engine's edits ([`type_text`]), everything else
//! through [`Editor`]'s methods.

mod blocks;
mod editing;
mod lookup;
mod text;

use slackless_core::types::rich_text::{RichTextSection, TextElement};
use slackless_core::types::{ChannelId, RichTextContainer, RichTextElement, UserId};

use super::marks::LineKind;
use super::{Directory, Editor};

/// Type `text` as the input engine reports it: one edit per character at the selection.
fn type_text(editor: &mut Editor, text: &str) {
    for ch in text.chars() {
        let range = editor.selection();
        let caret = range.start + ch.len_utf8();
        editor.user_edit(range, ch.encode_utf8(&mut [0; 4]), caret..caret);
    }
}

fn typed(text: &str) -> Editor {
    let mut editor = Editor::default();
    type_text(&mut editor, text);
    editor
}

fn kinds(editor: &Editor) -> Vec<Option<LineKind>> {
    editor
        .doc()
        .parse()
        .lines
        .into_iter()
        .map(|line| line.kind)
        .collect()
}

/// The directory the drafts in these tests are named from.
struct Names;

impl Directory for Names {
    fn mention(&self, id: &UserId) -> String {
        match id.as_str() {
            "U1" => "Alice".to_owned(),
            other => other.to_owned(),
        }
    }

    fn channel_name(&self, id: &ChannelId) -> String {
        match id.as_str() {
            "C1" => "general".to_owned(),
            other => other.to_owned(),
        }
    }

    fn emoji_text(&self, shortcode: &str) -> Option<String> {
        match shortcode {
            "smile" => Some("\u{1F604}".to_owned()),
            "partyparrot" => Some(":partyparrot:".to_owned()),
            _ => None,
        }
    }
}

fn plain(text: &str) -> RichTextElement {
    RichTextElement::Text(TextElement {
        text: text.to_owned(),
        style: None,
    })
}

fn section(elements: Vec<RichTextElement>) -> RichTextContainer {
    RichTextContainer::Section(RichTextSection { elements })
}
