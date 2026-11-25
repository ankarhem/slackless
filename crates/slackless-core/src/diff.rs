//! Lightweight word-level diffing for message edit revision inspection.

/// Classification of one contiguous segment in a word-level diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Equal,
    Removed,
    Added,
}

/// One contiguous run of unchanged, removed, or added text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSegment {
    pub kind: DiffKind,
    pub text: String,
}

/// Split `input` into alternating whitespace and non-whitespace tokens so word diffs preserve
/// original spacing and newlines verbatim.
fn tokenize(input: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0usize;
    let mut in_ws: Option<bool> = None;
    for (idx, ch) in input.char_indices() {
        let ws = ch.is_whitespace();
        match in_ws {
            None => {
                in_ws = Some(ws);
                start = idx;
            }
            Some(prev) if prev != ws => {
                tokens.push(&input[start..idx]);
                start = idx;
                in_ws = Some(ws);
            }
            Some(_) => {}
        }
    }
    if start < input.len() {
        tokens.push(&input[start..]);
    }
    tokens
}

/// Compute a clean word-level diff from `old` to `new` using LCS over whitespace-preserving
/// tokens. Adjacent segments of the same [`DiffKind`] are coalesced.
///
/// For very large messages (>512 tokens on either side), falls back to a prefix/suffix trim
/// plus single replaced span so latency stays O(n) with bounded memory.
pub fn diff_words(old: &str, new: &str) -> Vec<DiffSegment> {
    if old == new {
        return if old.is_empty() {
            Vec::new()
        } else {
            vec![DiffSegment {
                kind: DiffKind::Equal,
                text: old.to_owned(),
            }]
        };
    }

    let a = tokenize(old);
    let b = tokenize(new);

    // Strip common prefix and suffix first (fast path for typical typo edits).
    let mut prefix = 0usize;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < a.len()
        && suffix + prefix < b.len()
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let mut out: Vec<DiffSegment> = Vec::new();
    let push = |out: &mut Vec<DiffSegment>, kind: DiffKind, piece: &str| {
        if piece.is_empty() {
            return;
        }
        if let Some(last) = out.last_mut()
            && last.kind == kind
        {
            last.text.push_str(piece);
        } else {
            out.push(DiffSegment {
                kind,
                text: piece.to_owned(),
            });
        }
    };

    for tok in &a[..prefix] {
        push(&mut out, DiffKind::Equal, tok);
    }

    let mid_a = &a[prefix..a.len() - suffix];
    let mid_b = &b[prefix..b.len() - suffix];

    const MAX_LCS_TOKENS: usize = 512;
    if mid_a.len() > MAX_LCS_TOKENS || mid_b.len() > MAX_LCS_TOKENS {
        for tok in mid_a {
            push(&mut out, DiffKind::Removed, tok);
        }
        for tok in mid_b {
            push(&mut out, DiffKind::Added, tok);
        }
    } else if !mid_a.is_empty() || !mid_b.is_empty() {
        let m = mid_a.len();
        let n = mid_b.len();
        let mut dp = vec![vec![0u16; n + 1]; m + 1];
        for i in (0..m).rev() {
            for j in (0..n).rev() {
                dp[i][j] = if mid_a[i] == mid_b[j] {
                    dp[i + 1][j + 1].saturating_add(1)
                } else {
                    dp[i + 1][j].max(dp[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < m && j < n {
            if mid_a[i] == mid_b[j] {
                push(&mut out, DiffKind::Equal, mid_a[i]);
                i += 1;
                j += 1;
            } else if dp[i + 1][j] >= dp[i][j + 1] {
                push(&mut out, DiffKind::Removed, mid_a[i]);
                i += 1;
            } else {
                push(&mut out, DiffKind::Added, mid_b[j]);
                j += 1;
            }
        }
        while i < m {
            push(&mut out, DiffKind::Removed, mid_a[i]);
            i += 1;
        }
        while j < n {
            push(&mut out, DiffKind::Added, mid_b[j]);
            j += 1;
        }
    }

    for tok in &a[a.len() - suffix..] {
        push(&mut out, DiffKind::Equal, tok);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_diff_highlights_replacements_insertions_and_unicode() {
        let segs = diff_words("Deploy at 10:00 tomorrow", "Deploy at 14:00 today");
        assert_eq!(
            segs,
            vec![
                DiffSegment {
                    kind: DiffKind::Equal,
                    text: "Deploy at ".into()
                },
                DiffSegment {
                    kind: DiffKind::Removed,
                    text: "10:00".into()
                },
                DiffSegment {
                    kind: DiffKind::Added,
                    text: "14:00".into()
                },
                DiffSegment {
                    kind: DiffKind::Equal,
                    text: " ".into()
                },
                DiffSegment {
                    kind: DiffKind::Removed,
                    text: "tomorrow".into()
                },
                DiffSegment {
                    kind: DiffKind::Added,
                    text: "today".into()
                },
            ]
        );

        // Identical text collapses to a single Equal segment (or empty for empty strings).
        assert!(diff_words("", "").is_empty());
        assert_eq!(
            diff_words("Hej världen", "Hej världen"),
            vec![DiffSegment {
                kind: DiffKind::Equal,
                text: "Hej världen".into()
            }]
        );

        // Pure insertion with Swedish characters.
        let ins = diff_words("Möte kl 10", "Möte om budget kl 10");
        assert!(
            ins.iter()
                .any(|s| s.kind == DiffKind::Added && s.text.contains("om budget"))
        );
        assert!(ins.iter().all(|s| s.kind != DiffKind::Removed));
    }
}
