//! Slack-style search query parsing: `in:general from:@ana has:link "exact phrase" words`.
//!
//! Recognized modifiers become [`SearchFilters`] (SQL WHERE clauses); everything else, including
//! a modifier whose value does not parse, is free text for the FTS5 MATCH.

/// A parsed search query.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// Free-text terms; a quoted phrase is one term.
    pub terms: Vec<String>,
    pub filters: SearchFilters,
}

/// Structured modifiers. Repeated `in:`/`from:` values are alternatives (OR); different
/// modifiers combine with AND.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchFilters {
    /// Channel ids or names (`#` and `<#C…|name>` wrappers stripped).
    pub in_channels: Vec<String>,
    /// User ids or names (`@` and `<@U…>` wrappers stripped).
    pub from_users: Vec<String>,
    /// Messages strictly before this day (UTC).
    pub before: Option<Date>,
    /// Messages strictly after this day (UTC).
    pub after: Option<Date>,
    pub has: Vec<Has>,
    /// Messages mentioning this user id (`<@U…>` in the text). Not parsed from a
    /// modifier: the search menu's "Includes me" chip sets it programmatically.
    pub includes_user: Option<String>,
    /// Only thread replies (a stored `thread_ts`). Set by the "Threads only" chip.
    pub threads_only: bool,
}

/// `has:` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Has {
    Link,
    File,
    Reaction,
    Pin,
}

impl Has {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "link" | "links" => Self::Link,
            "file" | "files" => Self::File,
            "reaction" | "reactions" => Self::Reaction,
            "pin" | "pins" | "pinned" => Self::Pin,
            _ => return None,
        })
    }
}

/// A calendar day, `YYYY-MM-DD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
}

impl Date {
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.splitn(3, '-');
        let year: i32 = parts.next()?.parse().ok()?;
        let month: u32 = parts.next()?.parse().ok()?;
        let day: u32 = parts.next()?.parse().ok()?;
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days_in_month = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days_in_month)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    /// Unix seconds at 00:00 UTC of this day.
    pub fn unix_start(&self) -> i64 {
        // Howard Hinnant's days_from_civil.
        let (m, d) = (i64::from(self.month), i64::from(self.day));
        let y = i64::from(self.year) - i64::from(m <= 2);
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        (era * 146_097 + doe - 719_468) * 86_400
    }
}

impl SearchQuery {
    /// Parses a Slack-style query string. Never fails.
    pub fn parse(input: &str) -> Self {
        let mut query = Self::default();
        for (token, quoted) in tokenize(input) {
            if token.is_empty() {
                continue;
            }
            let is_modifier = !quoted
                && token
                    .split_once(':')
                    .is_some_and(|(key, value)| query.filters.apply(key, value));
            if !is_modifier {
                query.terms.push(token);
            }
        }
        query
    }

    /// The free text, terms joined by spaces.
    pub fn free_text(&self) -> String {
        self.terms.join(" ")
    }

    /// The FTS5 MATCH expression: every term quoted (so FTS5 syntax in user input is literal),
    /// implicitly ANDed. `None` when there is no free text.
    pub fn fts_match(&self) -> Option<String> {
        (!self.terms.is_empty()).then(|| {
            self.terms
                .iter()
                .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" ")
        })
    }
}

impl SearchFilters {
    /// Applies `key:value`; false when it is not a recognized, well-formed modifier.
    fn apply(&mut self, key: &str, value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        match key.to_ascii_lowercase().as_str() {
            "in" => self.in_channels.push(unwrap_ref(value, "<#", '#')),
            "from" => self.from_users.push(unwrap_ref(value, "<@", '@')),
            "before" => match Date::parse(value) {
                Some(d) => self.before = Some(d),
                None => return false,
            },
            "after" => match Date::parse(value) {
                Some(d) => self.after = Some(d),
                None => return false,
            },
            "has" => match Has::parse(value) {
                Some(h) => self.has.push(h),
                None => return false,
            },
            _ => return false,
        }
        true
    }
}

/// `<#C123|general>` -> `C123`, `#general` -> `general` (same for `<@U1>` / `@ana`).
fn unwrap_ref(value: &str, open: &str, sigil: char) -> String {
    if let Some(inner) = value.strip_prefix(open).and_then(|v| v.strip_suffix('>')) {
        return inner.split('|').next().unwrap_or(inner).to_owned();
    }
    value.strip_prefix(sigil).unwrap_or(value).to_owned()
}

/// Splits on whitespace outside double quotes; quotes are removed. The flag is true when the
/// token began with a quote (a quoted `"in:x"` is free text, not a modifier).
fn tokenize(input: &str) -> Vec<(String, bool)> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let (mut in_quotes, mut started_quoted, mut has_token) = (false, false, false);
    for c in input.chars() {
        match c {
            '"' => {
                if !has_token {
                    started_quoted = true;
                }
                in_quotes = !in_quotes;
                has_token = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    tokens.push((std::mem::take(&mut current), started_quoted));
                }
                has_token = false;
                started_quoted = false;
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push((current, started_quoted));
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_modifiers_from_free_text() {
        let q = SearchQuery::parse("in:general from:U123 hello world");
        assert_eq!(q.filters.in_channels, vec!["general"]);
        assert_eq!(q.filters.from_users, vec!["U123"]);
        assert_eq!(q.free_text(), "hello world");
        assert_eq!(q.fts_match().as_deref(), Some(r#""hello" "world""#));
    }

    #[test]
    fn parses_dates_has_and_wrapped_refs() {
        let q = SearchQuery::parse(
            r#"in:<#C42|random> from:@ana before:2024-02-29 after:2024-01-01 has:link has:reaction "exact phrase""#,
        );
        assert_eq!(q.filters.in_channels, vec!["C42"]);
        assert_eq!(q.filters.from_users, vec!["ana"]);
        assert_eq!(
            q.filters.before,
            Some(Date {
                year: 2024,
                month: 2,
                day: 29
            })
        );
        assert_eq!(q.filters.after.unwrap().unix_start(), 1_704_067_200);
        assert_eq!(q.filters.has, vec![Has::Link, Has::Reaction]);
        assert_eq!(q.terms, vec!["exact phrase"]);
    }

    #[test]
    fn malformed_or_unknown_modifiers_stay_free_text() {
        let q = SearchQuery::parse(r#"before:2023-02-29 has:sparkles to:x "in:general" in:"#);
        assert_eq!(q.filters, SearchFilters::default());
        assert_eq!(
            q.terms,
            vec![
                "before:2023-02-29",
                "has:sparkles",
                "to:x",
                "in:general",
                "in:"
            ]
        );
        assert_eq!(
            Date {
                year: 1970,
                month: 1,
                day: 1
            }
            .unix_start(),
            0
        );
        assert_eq!(SearchQuery::parse("  ").fts_match(), None);
    }

    #[test]
    fn fts_match_escapes_quotes() {
        let q = SearchQuery {
            terms: vec![r#"a"b"#.into()],
            ..Default::default()
        };
        assert_eq!(q.fts_match().as_deref(), Some(r#""a""b""#));
    }
}
