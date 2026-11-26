//! Pure, fast local smart notification rules engine and daily Catch-Up reminder scheduler.
//!
//! Rules are persisted directly in the GUI `Settings` JSON (no SQLite schema changes required)
//! and evaluated locally against incoming Slack messages in sub-millisecond time.
//!
//! # Priority & conflict resolution
//!
//! When multiple enabled rules match the same incoming message, conflicts are resolved in strict
//! precedence order:
//!
//! 1. **[`RuleAction::BreakThroughMute`]** (highest) — explicit escalation wins even if a broader
//!    [`RuleAction::Suppress`] rule or channel mute/DND would otherwise silence the message (for
//!    example: suppressing `#alerts` generally, while breaking through on `"SEV-1"`).
//! 2. **[`RuleAction::Suppress`]** — silences matching bot or noisy channel chatter, overriding
//!    default notification settings and [`RuleAction::PriorityHighlight`].
//! 3. **[`RuleAction::PriorityHighlight`]** — elevates a matching message in unmuted/allowed
//!    contexts to a priority alert (DM/mention sound, dock attention, highlight styling).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use slackless_types::time::civil_from_days;

/// Default cosine-similarity threshold when a rule specifies `semantic_topic` without an
/// explicit `semantic_threshold`.
pub const DEFAULT_SEMANTIC_THRESHOLD: f32 = 0.45;

/// Upper bound on compiled regex NFA/DFA memory to prevent pathological expressions from
/// consuming excessive memory.
const REGEX_SIZE_LIMIT_BYTES: usize = 256 * 1024;

/// Maximum number of compiled patterns cached in memory across live message evaluations.
const REGEX_CACHE_CAPACITY: usize = 128;

/// Action taken when a [`NotificationRule`] matches an incoming message.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    /// Notify even when the channel is muted, Do Not Disturb / Focus is active, or outside the
    /// notification schedule. Highest precedence when multiple rules match.
    BreakThroughMute,
    /// Highlight as a priority notification (elevated sound & visual cue) in allowed contexts.
    #[default]
    PriorityHighlight,
    /// Silence notifications for matching messages (e.g. noisy bots or channels).
    Suppress,
}

impl RuleAction {
    /// All supported actions in display order.
    pub const ALL: [Self; 3] = [
        Self::BreakThroughMute,
        Self::PriorityHighlight,
        Self::Suppress,
    ];

    /// Numeric conflict-resolution precedence (higher wins):
    /// `BreakThroughMute` (3) > `Suppress` (2) > `PriorityHighlight` (1).
    pub const fn precedence(self) -> u8 {
        match self {
            Self::BreakThroughMute => 3,
            Self::Suppress => 2,
            Self::PriorityHighlight => 1,
        }
    }

    /// Short human-readable label for Preferences UI chips and rule summaries.
    pub const fn label(self) -> &'static str {
        match self {
            Self::BreakThroughMute => "Break through mute",
            Self::PriorityHighlight => "Priority highlight",
            Self::Suppress => "Suppress",
        }
    }
}

/// One user-configured local smart notification rule.
///
/// All non-empty conditions on a rule are combined with logical **AND**. Within multi-value lists
/// (`from_users`, `in_channels`, `keywords`), entries are combined with logical **OR**. A rule
/// with zero active conditions never matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub action: RuleAction,
    /// User IDs (`U0123...`), `@handles`, or display names (matched case-insensitively).
    pub from_users: Vec<String>,
    /// Channel IDs (`C0123...`) or `#names` (matched case-insensitively).
    pub in_channels: Vec<String>,
    /// Case-insensitive substring keywords (any match satisfies this clause).
    pub keywords: Vec<String>,
    /// Optional case-insensitive Rust regex pattern, safely validated & bounded (never panics).
    pub regex_pattern: Option<String>,
    /// When `true`, only thread replies (`thread_ts != ts`) can match.
    pub only_threads: bool,
    /// When `true`, only messages mentioning the signed-in user can match.
    pub mentions_me: bool,
    /// Optional natural-language topic matched via local embeddings when available.
    pub semantic_topic: Option<String>,
    /// Optional cosine similarity threshold (`0.0..=1.0`, defaults to [`DEFAULT_SEMANTIC_THRESHOLD`]).
    pub semantic_threshold: Option<f32>,
}

impl Default for NotificationRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            action: RuleAction::PriorityHighlight,
            from_users: Vec::new(),
            in_channels: Vec::new(),
            keywords: Vec::new(),
            regex_pattern: None,
            only_threads: false,
            mentions_me: false,
            semantic_topic: None,
            semantic_threshold: None,
        }
    }
}

fn canonical_opt_f32_bits(value: Option<f32>) -> Option<u32> {
    value.map(|v| {
        if v == 0.0 {
            0.0_f32.to_bits()
        } else if v.is_nan() {
            f32::NAN.to_bits()
        } else {
            v.to_bits()
        }
    })
}

impl PartialEq for NotificationRule {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.name == other.name
            && self.enabled == other.enabled
            && self.action == other.action
            && self.from_users == other.from_users
            && self.in_channels == other.in_channels
            && self.keywords == other.keywords
            && self.regex_pattern == other.regex_pattern
            && self.only_threads == other.only_threads
            && self.mentions_me == other.mentions_me
            && self.semantic_topic == other.semantic_topic
            && canonical_opt_f32_bits(self.semantic_threshold)
                == canonical_opt_f32_bits(other.semantic_threshold)
    }
}

impl Eq for NotificationRule {}

impl NotificationRule {
    /// Whether this rule specifies at least one active matching condition.
    ///
    /// A completely empty rule never matches, preventing accidental wildcard triggers while
    /// editing a new draft rule.
    pub fn has_any_condition(&self) -> bool {
        has_non_empty(&self.from_users)
            || has_non_empty(&self.in_channels)
            || has_non_empty(&self.keywords)
            || self
                .regex_pattern
                .as_deref()
                .is_some_and(|p| !p.trim().is_empty())
            || self.only_threads
            || self.mentions_me
            || self
                .semantic_topic
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty())
    }

    /// Resolved semantic similarity threshold clamped into `[0.0, 1.0]`.
    pub fn effective_semantic_threshold(&self) -> f32 {
        match self.semantic_threshold {
            Some(v) if v.is_finite() => v.clamp(0.0, 1.0),
            _ => DEFAULT_SEMANTIC_THRESHOLD,
        }
    }

    /// Compact one-line summary of the rule's configured conditions for UI lists.
    pub fn condition_summary(&self) -> String {
        let mut parts = Vec::new();
        let users = clean_tokens(&self.from_users);
        if !users.is_empty() {
            parts.push(format!("from: {}", users.join(", ")));
        }
        let channels = clean_tokens(&self.in_channels);
        if !channels.is_empty() {
            parts.push(format!("in: {}", channels.join(", ")));
        }
        let keywords = clean_tokens(&self.keywords);
        if !keywords.is_empty() {
            parts.push(format!("keywords: {}", keywords.join(", ")));
        }
        if let Some(pat) = self
            .regex_pattern
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(format!("regex: /{pat}/"));
        }
        if self.only_threads {
            parts.push("threads only".to_owned());
        }
        if self.mentions_me {
            parts.push("mentions me".to_owned());
        }
        if let Some(topic) = self
            .semantic_topic
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(format!(
                "topic: \"{topic}\" (\u{2265}{:.2})",
                self.effective_semantic_threshold()
            ));
        }
        if parts.is_empty() {
            "No conditions configured".to_owned()
        } else {
            parts.join(" \u{00B7} ")
        }
    }
}

/// Incoming message metadata supplied to [`evaluate_rules`] and [`test_rule`].
#[derive(Debug, Clone, Default)]
pub struct RuleContext<'a> {
    pub sender_id: Option<&'a str>,
    pub sender_handle: Option<&'a str>,
    pub sender_display_name: Option<&'a str>,
    pub channel_id: &'a str,
    pub channel_name: Option<&'a str>,
    pub text: &'a str,
    pub is_thread_reply: bool,
    pub mentions_me: bool,
}

/// Optional semantic-similarity scorer callback: `(message_text, semantic_topic) -> Option<cosine_similarity>`.
pub type SemanticScorer<'a> = &'a dyn Fn(&str, &str) -> Option<f32>;

/// Outcome of evaluating a slice of [`NotificationRule`]s against one message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleEvaluation {
    /// Winning action after precedence resolution:
    /// `BreakThroughMute` > `Suppress` > `PriorityHighlight`.
    pub action: Option<RuleAction>,
    /// True when a `PriorityHighlight` rule matched and was not overridden by `Suppress`.
    pub priority_highlight: bool,
    /// IDs of all enabled rules that matched this message, in rule-list order.
    pub matched_rule_ids: Vec<String>,
    /// Names of all enabled rules that matched this message, in rule-list order.
    pub matched_rule_names: Vec<String>,
    /// Any invalid regex patterns encountered (skipped safely without panicking): `(rule_id, err)`.
    pub regex_errors: Vec<(String, String)>,
}

impl RuleEvaluation {
    /// Whether the verdict breaks through channel mute, DND, Focus mode, and quiet schedules.
    pub fn breaks_through_mute(&self) -> bool {
        self.action == Some(RuleAction::BreakThroughMute)
    }

    /// Whether the verdict silences the notification.
    pub fn is_suppressed(&self) -> bool {
        self.action == Some(RuleAction::Suppress)
    }

    /// Whether the verdict elevates the notification as a priority highlight.
    pub fn is_priority_highlight(&self) -> bool {
        self.priority_highlight && !self.is_suppressed()
    }
}

/// Detailed result from [`test_rule`] for live rule testing in Preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleTestResult {
    pub matched: bool,
    pub action: RuleAction,
    pub disabled: bool,
    pub regex_error: Option<String>,
    pub semantic_score: Option<f32>,
    pub summary: String,
    pub reasons: Vec<String>,
}

/// Validate and compile a rule's regex pattern safely (case-insensitive, bounded NFA/DFA size).
/// Never panics on untrusted or malformed user input.
pub fn validate_regex(pattern: &str) -> Result<Regex, String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err("Regex pattern is empty".to_owned());
    }
    RegexBuilder::new(trimmed)
        .case_insensitive(true)
        .size_limit(REGEX_SIZE_LIMIT_BYTES)
        .dfa_size_limit(REGEX_SIZE_LIMIT_BYTES)
        .build()
        .map_err(|e| e.to_string())
}

fn compile_regex_cached(pattern: &str) -> Result<Regex, String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Result<Regex, String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(hit) = guard.get(pattern)
    {
        return hit.clone();
    }
    let compiled = validate_regex(pattern);
    if let Ok(mut guard) = cache.lock() {
        if guard.len() >= REGEX_CACHE_CAPACITY {
            guard.clear();
        }
        guard.insert(pattern.to_owned(), compiled.clone());
    }
    compiled
}

/// Evaluate all enabled rules against `ctx` without semantic scoring (sub-millisecond hot path).
pub fn evaluate_rules(rules: &[NotificationRule], ctx: &RuleContext<'_>) -> RuleEvaluation {
    evaluate_rules_with_semantic(rules, ctx, None)
}

/// Evaluate all enabled rules against `ctx`, using an optional local semantic similarity hook
/// only for rules that specify a non-empty `semantic_topic`.
pub fn evaluate_rules_with_semantic(
    rules: &[NotificationRule],
    ctx: &RuleContext<'_>,
    semantic_scorer: Option<SemanticScorer<'_>>,
) -> RuleEvaluation {
    let mut eval = RuleEvaluation::default();
    let mut any_priority = false;

    for rule in rules {
        if !rule.enabled || !rule.has_any_condition() {
            continue;
        }
        let outcome = match_single_rule(rule, ctx, semantic_scorer);
        if let Some(err) = outcome.regex_error {
            eval.regex_errors.push((rule.id.clone(), err));
            continue;
        }
        if !outcome.matched {
            continue;
        }
        eval.matched_rule_ids.push(rule.id.clone());
        eval.matched_rule_names.push(rule.name.clone());
        if rule.action == RuleAction::PriorityHighlight {
            any_priority = true;
        }
        eval.action = match eval.action {
            Some(current) if current.precedence() >= rule.action.precedence() => Some(current),
            _ => Some(rule.action),
        };
    }

    eval.priority_highlight = any_priority && eval.action != Some(RuleAction::Suppress);
    eval
}

/// Test one [`NotificationRule`] against a sample message and return a human-readable explanation
/// suitable for live preview in the Preferences UI.
pub fn test_rule(rule: &NotificationRule, sample: &RuleContext<'_>) -> RuleTestResult {
    test_rule_with_semantic(rule, sample, None)
}

/// Like [`test_rule`], with an optional semantic similarity callback for topic testing.
pub fn test_rule_with_semantic(
    rule: &NotificationRule,
    sample: &RuleContext<'_>,
    semantic_scorer: Option<SemanticScorer<'_>>,
) -> RuleTestResult {
    if !rule.has_any_condition() {
        return RuleTestResult {
            matched: false,
            action: rule.action,
            disabled: !rule.enabled,
            regex_error: None,
            semantic_score: None,
            summary: "Add at least one condition to test this rule.".to_owned(),
            reasons: vec!["Rule has no active conditions".to_owned()],
        };
    }

    let outcome = match_single_rule(rule, sample, semantic_scorer);
    let summary = if let Some(ref err) = outcome.regex_error {
        format!("Invalid regex: {err}")
    } else if outcome.matched {
        let mut s = format!("Matches \u{2192} {}", rule.action.label());
        if !rule.enabled {
            s.push_str(" (rule is currently disabled)");
        }
        s
    } else {
        let missing: Vec<&str> = outcome.failed_clauses.iter().map(String::as_str).collect();
        format!("No match (did not satisfy: {})", missing.join(", "))
    };

    RuleTestResult {
        matched: outcome.matched,
        action: rule.action,
        disabled: !rule.enabled,
        regex_error: outcome.regex_error,
        semantic_score: outcome.semantic_score,
        summary,
        reasons: outcome.reasons,
    }
}

struct SingleMatchOutcome {
    matched: bool,
    regex_error: Option<String>,
    semantic_score: Option<f32>,
    reasons: Vec<String>,
    failed_clauses: Vec<String>,
}

fn match_single_rule(
    rule: &NotificationRule,
    ctx: &RuleContext<'_>,
    semantic_scorer: Option<SemanticScorer<'_>>,
) -> SingleMatchOutcome {
    let mut matched = true;
    let mut regex_error = None;
    let mut semantic_score = None;
    let mut reasons = Vec::new();
    let mut failed_clauses = Vec::new();

    // 1. Sender filter (`from_users`)
    let user_filters = clean_tokens(&rule.from_users);
    if !user_filters.is_empty() {
        let hit = user_filters.iter().any(|pat| {
            let needle = pat.trim_start_matches('@');
            !needle.is_empty()
                && (ctx
                    .sender_id
                    .is_some_and(|id| id.eq_ignore_ascii_case(needle))
                    || ctx
                        .sender_handle
                        .is_some_and(|h| h.trim_start_matches('@').eq_ignore_ascii_case(needle))
                    || ctx
                        .sender_display_name
                        .is_some_and(|d| d.eq_ignore_ascii_case(needle)))
        });
        if hit {
            reasons.push("sender matched".to_owned());
        } else {
            matched = false;
            failed_clauses.push("from_users".to_owned());
            reasons.push("sender did not match from_users".to_owned());
        }
    }

    // 2. Channel filter (`in_channels`)
    let channel_filters = clean_tokens(&rule.in_channels);
    if !channel_filters.is_empty() {
        let hit = channel_filters.iter().any(|pat| {
            let needle = pat.trim_start_matches('#');
            !needle.is_empty()
                && (ctx.channel_id.eq_ignore_ascii_case(needle)
                    || ctx
                        .channel_name
                        .is_some_and(|c| c.trim_start_matches('#').eq_ignore_ascii_case(needle)))
        });
        if hit {
            reasons.push("channel matched".to_owned());
        } else {
            matched = false;
            failed_clauses.push("in_channels".to_owned());
            reasons.push("channel did not match in_channels".to_owned());
        }
    }

    // 3. Thread-only flag
    if rule.only_threads {
        if ctx.is_thread_reply {
            reasons.push("is a thread reply".to_owned());
        } else {
            matched = false;
            failed_clauses.push("only_threads".to_owned());
            reasons.push("message is not a thread reply".to_owned());
        }
    }

    // 4. Mentions-me flag
    if rule.mentions_me {
        if ctx.mentions_me {
            reasons.push("mentions you".to_owned());
        } else {
            matched = false;
            failed_clauses.push("mentions_me".to_owned());
            reasons.push("message does not mention you".to_owned());
        }
    }

    // 5. Keyword filter (`keywords`)
    let keyword_filters = clean_tokens(&rule.keywords);
    if !keyword_filters.is_empty() {
        let lower_text = ctx.text.to_lowercase();
        if let Some(hit_kw) = keyword_filters
            .iter()
            .find(|kw| lower_text.contains(&kw.to_lowercase()))
        {
            reasons.push(format!("keyword \"{hit_kw}\" matched"));
        } else {
            matched = false;
            failed_clauses.push("keywords".to_owned());
            reasons.push("no configured keyword found".to_owned());
        }
    }

    // 6. Regex pattern (`regex_pattern`) — always validate even if earlier clauses failed so
    //    `test_rule` surfaces invalid regex immediately.
    if let Some(raw_pat) = rule
        .regex_pattern
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match compile_regex_cached(raw_pat) {
            Ok(re) => {
                if re.is_match(ctx.text) {
                    reasons.push(format!("regex /{raw_pat}/ matched"));
                } else {
                    matched = false;
                    failed_clauses.push("regex".to_owned());
                    reasons.push(format!("regex /{raw_pat}/ did not match"));
                }
            }
            Err(err) => {
                matched = false;
                failed_clauses.push("regex".to_owned());
                reasons.push(format!("invalid regex: {err}"));
                regex_error = Some(err);
            }
        }
    }

    // 7. Optional semantic topic (`semantic_topic`) — only invoked when topic is configured AND
    //    a semantic scorer is provided (keeps hot path zero-overhead when unused).
    if let Some(topic) = rule
        .semantic_topic
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let threshold = rule.effective_semantic_threshold();
        if let Some(scorer) = semantic_scorer {
            if let Some(score) = scorer(ctx.text, topic) {
                semantic_score = Some(score);
                if score.is_finite() && score >= threshold {
                    reasons.push(format!(
                        "semantic topic \"{topic}\" matched ({score:.2} \u{2265} {threshold:.2})"
                    ));
                } else {
                    matched = false;
                    failed_clauses.push("semantic_topic".to_owned());
                    reasons.push(format!(
                        "semantic similarity {score:.2} below threshold {threshold:.2}"
                    ));
                }
            } else {
                matched = false;
                failed_clauses.push("semantic_topic".to_owned());
                reasons.push("local embedding model unavailable for semantic topic".to_owned());
            }
        } else {
            matched = false;
            failed_clauses.push("semantic_topic".to_owned());
            reasons.push("semantic matcher inactive (download local embedding model)".to_owned());
        }
    }

    SingleMatchOutcome {
        matched,
        regex_error,
        semantic_score,
        reasons,
        failed_clauses,
    }
}

fn has_non_empty(items: &[String]) -> bool {
    items.iter().any(|s| !s.trim().is_empty())
}

fn clean_tokens(items: &[String]) -> Vec<&str> {
    items
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Split a comma- or newline-separated user input string into trimmed non-empty tokens.
pub fn parse_csv_tokens(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// Daily Catch-Up Reminder pure scheduling / due-check logic
// ---------------------------------------------------------------------------

/// Default daily Catch-Up reminder wall-clock time (local 24-hour `HH:MM`).
pub const DEFAULT_CATCHUP_REMINDER_TIME: &str = "09:00";

/// Result of evaluating whether today's Daily Catch-Up reminder is due and when it fires next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchUpReminderStatus {
    /// `true` iff the reminder is enabled, the configured time is valid, local time has reached
    /// or passed the scheduled minute today, and it has not already fired on `today`.
    pub due: bool,
    /// Current local calendar day formatted as `"YYYY-MM-DD"`.
    pub today: String,
    /// Next scheduled fire timestamp in UTC epoch seconds (`None` when disabled or invalid time).
    pub next_fire_utc_secs: Option<i64>,
    /// Seconds remaining until `next_fire_utc_secs` (`Some(0)` when due now).
    pub seconds_until_next: Option<u64>,
}

/// Parse a 24-hour wall-clock time string (`"09:00"`, `"9:30"`, `"17:45"`) into `(hour, minute)`.
pub fn parse_daily_time_hhmm(raw: &str) -> Option<(u8, u8)> {
    let trimmed = raw.trim();
    let (h_str, m_str) = trimmed.split_once(':')?;
    if h_str.is_empty()
        || h_str.len() > 2
        || m_str.len() != 2
        || !h_str.bytes().all(|b| b.is_ascii_digit())
        || !m_str.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let hour: u8 = h_str.parse().ok()?;
    let minute: u8 = m_str.parse().ok()?;
    if hour < 24 && minute < 60 {
        Some((hour, minute))
    } else {
        None
    }
}

/// Format the local calendar date (`"YYYY-MM-DD"`) for `utc_secs` at `tz_offset_secs`.
pub fn format_local_day(utc_secs: i64, tz_offset_secs: i64) -> String {
    let local_secs = utc_secs.saturating_add(tz_offset_secs);
    let local_day = local_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(local_day);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Pure, deterministic check for the Daily Catch-Up reminder.
///
/// Guarantees the reminder fires **at most once per local calendar day** once `last_fired_day`
/// is updated to `status.today` by the caller after dispatching the notification.
pub fn check_catchup_reminder(
    enabled: bool,
    time_hhmm: &str,
    last_fired_day: Option<&str>,
    now_utc_secs: i64,
    tz_offset_secs: i64,
) -> CatchUpReminderStatus {
    let local_secs = now_utc_secs.saturating_add(tz_offset_secs);
    let local_day = local_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(local_day);
    let today = format!("{y:04}-{m:02}-{d:02}");

    if !enabled {
        return CatchUpReminderStatus {
            due: false,
            today,
            next_fire_utc_secs: None,
            seconds_until_next: None,
        };
    }

    let Some((hour, minute)) = parse_daily_time_hhmm(time_hhmm) else {
        return CatchUpReminderStatus {
            due: false,
            today,
            next_fire_utc_secs: None,
            seconds_until_next: None,
        };
    };

    let target_sod = i64::from(hour) * 3_600 + i64::from(minute) * 60;
    let current_sod = local_secs.rem_euclid(86_400);
    let today_fire_utc = local_day * 86_400 + target_sod - tz_offset_secs;
    let tomorrow_fire_utc = (local_day + 1) * 86_400 + target_sod - tz_offset_secs;
    let already_fired_today = last_fired_day.is_some_and(|day| day.trim() == today);

    if !already_fired_today && current_sod >= target_sod {
        CatchUpReminderStatus {
            due: true,
            today,
            next_fire_utc_secs: Some(today_fire_utc),
            seconds_until_next: Some(0),
        }
    } else if !already_fired_today && current_sod < target_sod {
        let wait = (today_fire_utc - now_utc_secs).max(0) as u64;
        CatchUpReminderStatus {
            due: false,
            today,
            next_fire_utc_secs: Some(today_fire_utc),
            seconds_until_next: Some(wait),
        }
    } else {
        let wait = (tomorrow_fire_utc - now_utc_secs).max(0) as u64;
        CatchUpReminderStatus {
            due: false,
            today,
            next_fire_utc_secs: Some(tomorrow_fire_utc),
            seconds_until_next: Some(wait),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_types::time::days_from_civil;

    fn sample_ctx<'a>() -> RuleContext<'a> {
        RuleContext {
            sender_id: Some("U01ALICE"),
            sender_handle: Some("alice"),
            sender_display_name: Some("Alice Smith"),
            channel_id: "C0INCIDENTS",
            channel_name: Some("incidents"),
            text: "SEV-1 prod database replication lag > 30s",
            is_thread_reply: false,
            mentions_me: false,
        }
    }

    #[test]
    fn empty_or_disabled_rules_never_match() {
        let empty = NotificationRule {
            id: "r0".into(),
            name: "Blank".into(),
            enabled: true,
            action: RuleAction::BreakThroughMute,
            ..Default::default()
        };
        assert!(!empty.has_any_condition());
        let eval = evaluate_rules(std::slice::from_ref(&empty), &sample_ctx());
        assert_eq!(eval.action, None);
        assert!(eval.matched_rule_ids.is_empty());

        let preview = test_rule(&empty, &sample_ctx());
        assert!(!preview.matched);
        assert!(preview.summary.contains("at least one condition"));

        let disabled = NotificationRule {
            id: "r1".into(),
            name: "Disabled".into(),
            enabled: false,
            action: RuleAction::BreakThroughMute,
            keywords: vec!["sev-1".into()],
            ..Default::default()
        };
        let eval = evaluate_rules(std::slice::from_ref(&disabled), &sample_ctx());
        assert_eq!(eval.action, None);

        // `test_rule` still previews match semantics while flagging disabled status.
        let preview = test_rule(&disabled, &sample_ctx());
        assert!(preview.matched);
        assert!(preview.disabled);
        assert!(preview.summary.contains("disabled"));
    }

    #[test]
    fn matches_users_channels_keywords_regex_threads_and_mentions() {
        let rule = NotificationRule {
            id: "r-full".into(),
            name: "Prod alerts".into(),
            enabled: true,
            action: RuleAction::BreakThroughMute,
            from_users: vec!["@ALICE".into(), "U999".into()],
            in_channels: vec!["#Incidents".into()],
            keywords: vec!["replication".into(), "outage".into()],
            regex_pattern: Some(r"sev-[01]\s+prod".into()),
            only_threads: true,
            mentions_me: true,
            semantic_topic: None,
            semantic_threshold: None,
        };

        // Missing thread + mention -> does not match
        let mut ctx = sample_ctx();
        let eval = evaluate_rules(std::slice::from_ref(&rule), &ctx);
        assert_eq!(eval.action, None);
        let preview = test_rule(&rule, &ctx);
        assert!(!preview.matched);
        assert!(preview.summary.contains("only_threads"));
        assert!(preview.summary.contains("mentions_me"));

        // Enable both flags -> matches!
        ctx.is_thread_reply = true;
        ctx.mentions_me = true;
        let eval = evaluate_rules(std::slice::from_ref(&rule), &ctx);
        assert_eq!(eval.action, Some(RuleAction::BreakThroughMute));
        assert!(eval.breaks_through_mute());
        assert!(!eval.is_suppressed());
        assert_eq!(eval.matched_rule_ids, vec!["r-full"]);

        let preview = test_rule(&rule, &ctx);
        assert!(preview.matched);
        assert!(preview.regex_error.is_none());
        assert!(preview.summary.contains("Break through mute"));
    }

    #[test]
    fn user_and_channel_matching_accepts_ids_handles_and_display_names() {
        let by_id = NotificationRule {
            id: "by-id".into(),
            name: "By IDs".into(),
            enabled: true,
            action: RuleAction::PriorityHighlight,
            from_users: vec!["u01alice".into()],
            in_channels: vec!["c0incidents".into()],
            ..Default::default()
        };
        assert_eq!(
            evaluate_rules(&[by_id], &sample_ctx()).action,
            Some(RuleAction::PriorityHighlight)
        );

        let by_display = NotificationRule {
            id: "by-display".into(),
            name: "By display name".into(),
            enabled: true,
            action: RuleAction::PriorityHighlight,
            from_users: vec!["Alice Smith".into()],
            ..Default::default()
        };
        assert_eq!(
            evaluate_rules(&[by_display], &sample_ctx()).action,
            Some(RuleAction::PriorityHighlight)
        );
    }

    #[test]
    fn invalid_regex_never_panics_and_reports_clean_error() {
        let bad_rule = NotificationRule {
            id: "bad-re".into(),
            name: "Broken".into(),
            enabled: true,
            action: RuleAction::BreakThroughMute,
            regex_pattern: Some("[unclosed(".into()),
            ..Default::default()
        };

        assert!(validate_regex("[unclosed(").is_err());
        assert!(validate_regex("   ").is_err());
        assert!(validate_regex(r"deploy\s+#\d+").is_ok());

        let eval = evaluate_rules(std::slice::from_ref(&bad_rule), &sample_ctx());
        assert_eq!(eval.action, None);
        assert_eq!(eval.regex_errors.len(), 1);
        assert_eq!(eval.regex_errors[0].0, "bad-re");

        let preview = test_rule(&bad_rule, &sample_ctx());
        assert!(!preview.matched);
        assert!(preview.regex_error.is_some());
        assert!(preview.summary.starts_with("Invalid regex:"));
    }

    #[test]
    fn conflict_resolution_precedence_breakthrough_over_suppress_over_highlight() {
        let highlight = NotificationRule {
            id: "r-hl".into(),
            name: "Highlight prod".into(),
            enabled: true,
            action: RuleAction::PriorityHighlight,
            keywords: vec!["prod".into()],
            ..Default::default()
        };
        let suppress = NotificationRule {
            id: "r-sup".into(),
            name: "Mute incidents".into(),
            enabled: true,
            action: RuleAction::Suppress,
            in_channels: vec!["#incidents".into()],
            ..Default::default()
        };
        let breakthrough = NotificationRule {
            id: "r-bt".into(),
            name: "SEV-1 page".into(),
            enabled: true,
            action: RuleAction::BreakThroughMute,
            keywords: vec!["sev-1".into()],
            ..Default::default()
        };

        // Highlight + Suppress -> Suppress wins (and clears priority_highlight).
        let eval_hs = evaluate_rules(&[highlight.clone(), suppress.clone()], &sample_ctx());
        assert_eq!(eval_hs.action, Some(RuleAction::Suppress));
        assert!(eval_hs.is_suppressed());
        assert!(!eval_hs.is_priority_highlight());
        assert_eq!(eval_hs.matched_rule_ids, vec!["r-hl", "r-sup"]);

        // Suppress + BreakThroughMute -> BreakThroughMute wins regardless of order!
        let eval_sb = evaluate_rules(&[suppress.clone(), breakthrough.clone()], &sample_ctx());
        assert_eq!(eval_sb.action, Some(RuleAction::BreakThroughMute));
        assert!(eval_sb.breaks_through_mute());
        assert!(!eval_sb.is_suppressed());

        let eval_all = evaluate_rules(&[highlight, suppress, breakthrough], &sample_ctx());
        assert_eq!(eval_all.action, Some(RuleAction::BreakThroughMute));
        assert!(eval_all.breaks_through_mute());
        assert!(eval_all.is_priority_highlight());
        assert_eq!(eval_all.matched_rule_ids.len(), 3);
    }

    #[test]
    fn semantic_topic_optional_hook_gracefully_degrades_and_scores_when_provided() {
        let rule = NotificationRule {
            id: "r-sem".into(),
            name: "DB outage topic".into(),
            enabled: true,
            action: RuleAction::PriorityHighlight,
            semantic_topic: Some("database outage".into()),
            semantic_threshold: Some(0.60),
            ..Default::default()
        };

        // Without scorer: zero overhead, graceful no-match.
        let eval_none = evaluate_rules(std::slice::from_ref(&rule), &sample_ctx());
        assert_eq!(eval_none.action, None);

        // With scorer below threshold: no match.
        let low_scorer = |_text: &str, _topic: &str| Some(0.42_f32);
        let eval_low = evaluate_rules_with_semantic(
            std::slice::from_ref(&rule),
            &sample_ctx(),
            Some(&low_scorer),
        );
        assert_eq!(eval_low.action, None);
        let preview_low = test_rule_with_semantic(&rule, &sample_ctx(), Some(&low_scorer));
        assert!(!preview_low.matched);
        assert_eq!(preview_low.semantic_score, Some(0.42));

        // With scorer at/above threshold: matches!
        let high_scorer = |_text: &str, _topic: &str| Some(0.78_f32);
        let eval_high = evaluate_rules_with_semantic(
            std::slice::from_ref(&rule),
            &sample_ctx(),
            Some(&high_scorer),
        );
        assert_eq!(eval_high.action, Some(RuleAction::PriorityHighlight));
        let preview_high = test_rule_with_semantic(&rule, &sample_ctx(), Some(&high_scorer));
        assert!(preview_high.matched);
        assert_eq!(preview_high.semantic_score, Some(0.78));
    }

    #[test]
    fn serde_round_trip_and_defaults_are_backwards_compatible() {
        let minimal: NotificationRule =
            serde_json::from_str(r#"{"id":"1","name":"Alerts"}"#).expect("minimal rule json");
        assert!(minimal.enabled);
        assert_eq!(minimal.action, RuleAction::PriorityHighlight);
        assert_eq!(
            minimal.effective_semantic_threshold(),
            DEFAULT_SEMANTIC_THRESHOLD
        );

        let full = NotificationRule {
            id: "rule-42".into(),
            name: "Escalate SEV-1".into(),
            enabled: true,
            action: RuleAction::BreakThroughMute,
            from_users: vec!["@oncall".into()],
            in_channels: vec!["#incidents".into()],
            keywords: vec!["sev-1".into()],
            regex_pattern: Some("sev-[01]".into()),
            only_threads: true,
            mentions_me: false,
            semantic_topic: Some("production outage".into()),
            semantic_threshold: Some(0.55),
        };
        let encoded = serde_json::to_string(&full).expect("serialize rule");
        let decoded: NotificationRule = serde_json::from_str(&encoded).expect("deserialize rule");
        assert_eq!(decoded, full);
    }

    #[test]
    fn parse_daily_time_hhmm_validates_strictly() {
        assert_eq!(parse_daily_time_hhmm("09:00"), Some((9, 0)));
        assert_eq!(parse_daily_time_hhmm(" 8:05 "), Some((8, 5)));
        assert_eq!(parse_daily_time_hhmm("00:00"), Some((0, 0)));
        assert_eq!(parse_daily_time_hhmm("23:59"), Some((23, 59)));

        for bad in [
            "", "9:0", "24:00", "12:60", "-1:00", "09:00:00", "noon", "009:00",
        ] {
            assert_eq!(parse_daily_time_hhmm(bad), None, "expected None for {bad}");
        }
    }

    #[test]
    fn catchup_reminder_fires_at_most_once_per_local_calendar_day() {
        // 2026-11-26 in UTC+1 (3600s)
        let day_26_utc_midnight = days_from_civil(2026, 11, 26) * 86_400;
        let tz = 3_600; // UTC+1

        // 08:59 local = 07:59 UTC -> not due yet, 60s until fire
        let at_0859 = day_26_utc_midnight + 7 * 3_600 + 59 * 60;
        let before = check_catchup_reminder(true, "09:00", None, at_0859, tz);
        assert!(!before.due);
        assert_eq!(before.today, "2026-11-26");
        assert_eq!(before.seconds_until_next, Some(60));
        assert_eq!(before.next_fire_utc_secs, Some(at_0859 + 60));

        // 09:00 local = 08:00 UTC -> due!
        let at_0900 = day_26_utc_midnight + 8 * 3_600;
        let due = check_catchup_reminder(true, "09:00", None, at_0900, tz);
        assert!(due.due);
        assert_eq!(due.today, "2026-11-26");
        assert_eq!(due.seconds_until_next, Some(0));

        // Same day at 09:05 after marking fired on 2026-11-26 -> NOT due again; next is tomorrow 09:00.
        let at_0905 = at_0900 + 5 * 60;
        let after = check_catchup_reminder(true, "09:00", Some("2026-11-26"), at_0905, tz);
        assert!(!after.due);
        assert_eq!(after.today, "2026-11-26");
        assert_eq!(after.next_fire_utc_secs, Some(at_0900 + 86_400));
        assert_eq!(after.seconds_until_next, Some(86_400 - 300));

        // Next calendar day at 09:00 -> due again!
        let next_day_0900 = at_0900 + 86_400;
        let next_due = check_catchup_reminder(true, "09:00", Some("2026-11-26"), next_day_0900, tz);
        assert!(next_due.due);
        assert_eq!(next_due.today, "2026-11-27");

        // Disabled or invalid time -> never due
        let disabled = check_catchup_reminder(false, "09:00", None, at_0900, tz);
        assert!(!disabled.due);
        assert_eq!(disabled.next_fire_utc_secs, None);

        let bad_time = check_catchup_reminder(true, "25:99", None, at_0900, tz);
        assert!(!bad_time.due);
        assert_eq!(bad_time.next_fire_utc_secs, None);
    }
}
