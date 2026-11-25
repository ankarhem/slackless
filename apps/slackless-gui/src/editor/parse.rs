//! Markdown block and inline syntax parser re-exported from `slackless_core::domain::mrkdwn`,
//! plus the small line-level fence check used by [`super::blocks`].

use std::ops::Range;

use slackless_core::domain::mrkdwn::Placed;

pub use slackless_core::domain::mrkdwn::{InlineKind, ParsedDoc, delimiter_ranges, empty_pair_at};

fn overlaps_atom(atoms: &[Placed], range: Range<usize>) -> bool {
    let ix = atoms.partition_point(|a| a.range.end <= range.start);
    atoms.get(ix).is_some_and(|a| a.range.start < range.end)
}

pub(super) fn is_fence_line(raw: &str, start: usize, atoms: &[Placed]) -> bool {
    if !raw.starts_with("```") || overlaps_atom(atoms, start..start + 3) {
        return false;
    }
    let rest = raw[3..].trim();
    // A fence line is ``` alone or ```lang (word characters), never ```code``` inline text.
    !rest.contains('`') && !rest.contains(char::is_whitespace)
}
