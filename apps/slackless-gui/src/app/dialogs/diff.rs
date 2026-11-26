//! Word-level diff of two message revisions for the revision history dialog.

use similar::{ChangeTag, TextDiff};

/// Classification of one contiguous segment in a word-level diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffKind {
    Equal,
    Removed,
    Added,
}

/// One contiguous run of unchanged, removed, or added text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DiffSegment {
    pub(super) kind: DiffKind,
    pub(super) text: String,
}

/// Word-level diff from `old` to `new` over whitespace-preserving tokens
/// (`similar::TextDiff::from_words`), coalesced into one segment per contiguous run of a kind.
pub(super) fn diff_words(old: &str, new: &str) -> Vec<DiffSegment> {
    let mut out: Vec<DiffSegment> = Vec::new();
    for change in TextDiff::from_words(old, new).iter_all_changes() {
        let kind = match change.tag() {
            ChangeTag::Equal => DiffKind::Equal,
            ChangeTag::Delete => DiffKind::Removed,
            ChangeTag::Insert => DiffKind::Added,
        };
        let text = change.value();
        if let Some(last) = out.last_mut()
            && last.kind == kind
        {
            last.text.push_str(text);
        } else {
            out.push(DiffSegment {
                kind,
                text: text.to_owned(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{DiffKind::*, *};

    #[test]
    fn word_diff_segments_cover_replacements_insertions_equality_and_unicode() {
        let seg = |kind, text: &str| DiffSegment {
            kind,
            text: text.to_owned(),
        };
        for (old, new, expected) in [
            (
                "Deploy at 10:00 tomorrow",
                "Deploy at 14:00 today",
                vec![
                    seg(Equal, "Deploy at "),
                    seg(Removed, "10:00"),
                    seg(Added, "14:00"),
                    seg(Equal, " "),
                    seg(Removed, "tomorrow"),
                    seg(Added, "today"),
                ],
            ),
            ("", "", vec![]),
            (
                "Hej världen",
                "Hej världen",
                vec![seg(Equal, "Hej världen")],
            ),
            (
                "Möte kl 10",
                "Möte om budget kl 10",
                vec![
                    seg(Equal, "Möte "),
                    seg(Added, "om budget "),
                    seg(Equal, "kl 10"),
                ],
            ),
        ] {
            assert_eq!(diff_words(old, new), expected, "{old:?} -> {new:?}");
        }
    }
}
