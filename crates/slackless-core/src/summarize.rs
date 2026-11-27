//! Provider-agnostic OpenAI-compatible AI summarization engine (Local / BYO-key).
//!
//! Supports local runtimes (`Ollama`, `LmStudio`), `OpenAi`, and arbitrary `Custom`
//! OpenAI-compatible `/v1/chat/completions` endpoints using the workspace's existing `reqwest`
//! client with strict `http://` / `https://` URL validation, bounded UTF-8-safe prompt building,
//! and redacted `Debug` output so API keys are never logged.

use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::catchup::CatchUpBriefing;
use crate::domain::mrkdwn::mrkdwn_to_plain;
use crate::runtime::block_on;
use crate::store::{Store, StoredMessage};
use crate::types::time::format_utc;
use crate::types::{ChannelId, FileId, TeamId, TranscriptCue, Ts, UserId};

/// Default request timeout in seconds for chat completions.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
/// Minimum allowed timeout in seconds.
pub const MIN_TIMEOUT_SECS: u64 = 5;
/// Maximum allowed timeout in seconds.
pub const MAX_TIMEOUT_SECS: u64 = 300;
/// Default input character budget for prompt construction.
pub const DEFAULT_MAX_INPUT_CHARS: usize = 24_000;
/// Minimum allowed input character budget.
pub const MIN_MAX_INPUT_CHARS: usize = 512;
/// Maximum allowed input character budget.
pub const MAX_MAX_INPUT_CHARS: usize = 128_000;
/// Default response token cap requested from the model.
pub const DEFAULT_MAX_TOKENS: u32 = 1_024;
/// Default message window loaded for channel/thread summarization.
pub const DEFAULT_SUMMARY_MESSAGE_LIMIT: usize = 80;
/// Hard byte cap on HTTP response bodies buffered from AI endpoints (2 MiB).
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

const USER_AGENT: &str = concat!("slackless/", env!("CARGO_PKG_VERSION"));

/// Built-in provider presets for OpenAI-compatible chat-completions endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiProvider {
    #[default]
    Ollama,
    LmStudio,
    OpenAi,
    Custom,
}

impl AiProvider {
    pub const ALL: [Self; 4] = [Self::Ollama, Self::LmStudio, Self::OpenAi, Self::Custom];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama (Local)",
            Self::LmStudio => "LM Studio (Local)",
            Self::OpenAi => "OpenAI",
            Self::Custom => "Custom (OpenAI-compatible)",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LmStudio => "LM Studio",
            Self::OpenAi => "OpenAI",
            Self::Custom => "Custom",
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LmStudio => "lmstudio",
            Self::OpenAi => "openai",
            Self::Custom => "custom",
        }
    }

    pub fn from_key(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ollama" => Some(Self::Ollama),
            "lmstudio" | "lm-studio" | "lm_studio" => Some(Self::LmStudio),
            "openai" | "open-ai" | "open_ai" => Some(Self::OpenAi),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::Ollama => 0,
            Self::LmStudio => 1,
            Self::OpenAi => 2,
            Self::Custom => 3,
        }
    }

    pub const fn from_index(ix: usize) -> Self {
        match ix {
            0 => Self::Ollama,
            1 => Self::LmStudio,
            2 => Self::OpenAi,
            _ => Self::Custom,
        }
    }

    /// Preset OpenAI-compatible base URL (`""` for [`Self::Custom`]).
    pub const fn default_base_url(self) -> &'static str {
        match self {
            Self::Ollama => "http://localhost:11434/v1",
            Self::LmStudio => "http://localhost:1234/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Custom => "",
        }
    }

    /// Suggested default model name for this preset (`""` for [`Self::Custom`]).
    pub const fn default_model(self) -> &'static str {
        match self {
            Self::Ollama => "llama3.2",
            Self::LmStudio => "local-model",
            Self::OpenAi => "gpt-4o-mini",
            Self::Custom => "",
        }
    }

    /// Whether this preset typically requires a Bearer API key.
    pub const fn requires_api_key(self) -> bool {
        matches!(self, Self::OpenAi)
    }
}

/// Runtime configuration for calling an OpenAI-compatible summarization endpoint.
///
/// Implements a custom [`fmt::Debug`] that always redacts `api_key`.
#[derive(Clone, PartialEq, Eq)]
pub struct AiSummaryConfig {
    pub provider: AiProvider,
    /// Base URL (e.g. `http://localhost:11434/v1`). When empty, falls back to
    /// `provider.default_base_url()`.
    pub base_url: String,
    /// Model identifier sent in `chat/completions` (e.g. `llama3.2`, `gpt-4o-mini`).
    pub model: String,
    /// Optional Bearer API key. Never logged in `Debug`, tracing, or error messages.
    pub api_key: Option<String>,
    /// HTTP timeout in seconds (clamped to [`MIN_TIMEOUT_SECS`]..=[`MAX_TIMEOUT_SECS`]).
    pub timeout_secs: u64,
    /// Character cap on prompt input (clamped to [`MIN_MAX_INPUT_CHARS`]..=[`MAX_MAX_INPUT_CHARS`]).
    pub max_input_chars: usize,
    /// Optional `max_tokens` sent in the completion payload.
    pub max_tokens: Option<u32>,
}

impl Default for AiSummaryConfig {
    fn default() -> Self {
        Self {
            provider: AiProvider::Ollama,
            base_url: AiProvider::Ollama.default_base_url().to_owned(),
            model: AiProvider::Ollama.default_model().to_owned(),
            api_key: None,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            max_input_chars: DEFAULT_MAX_INPUT_CHARS,
            max_tokens: Some(DEFAULT_MAX_TOKENS),
        }
    }
}

impl fmt::Debug for AiSummaryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AiSummaryConfig")
            .field("provider", &self.provider)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field(
                "api_key",
                &self
                    .api_key
                    .as_deref()
                    .filter(|k| !k.trim().is_empty())
                    .map(|_| "[REDACTED]"),
            )
            .field("timeout_secs", &self.timeout_secs)
            .field("max_input_chars", &self.max_input_chars)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

impl AiSummaryConfig {
    /// Resolve the effective base URL string (configured `base_url` or the preset's default).
    pub fn effective_base_url(&self) -> &str {
        let trimmed = self.base_url.trim();
        if trimmed.is_empty() {
            self.provider.default_base_url()
        } else {
            trimmed
        }
    }

    /// Resolve the effective model name (configured `model` or the preset's default).
    pub fn effective_model(&self) -> &str {
        let trimmed = self.model.trim();
        if trimmed.is_empty() {
            self.provider.default_model()
        } else {
            trimmed
        }
    }

    /// Returns a normalized non-empty Bearer token if present.
    fn normalized_api_key(&self) -> Option<&str> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
    }

    /// Clamped timeout duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS))
    }

    /// Clamped input character budget.
    pub fn input_budget(&self) -> usize {
        self.max_input_chars
            .clamp(MIN_MAX_INPUT_CHARS, MAX_MAX_INPUT_CHARS)
    }

    /// Validate endpoint URL, model name, API key characters, transport security (no cleartext
    /// `http://` to non-loopback hosts when an API key is used or OpenAI is selected), and
    /// required API key presence.
    pub fn validate(&self) -> Result<(Url, String), SummarizeError> {
        let base = validate_base_url(self.effective_base_url())?;
        let model = self.effective_model().trim().to_owned();
        if model.is_empty() {
            return Err(SummarizeError::MissingModel);
        }
        let key = self.normalized_api_key();
        if let Some(k) = key
            && k.chars().any(|c| c.is_control())
        {
            return Err(SummarizeError::InvalidApiKey);
        }
        if self.provider.requires_api_key() && key.is_none() {
            return Err(SummarizeError::MissingApiKey {
                provider: self.provider.short_label(),
            });
        }
        if base.scheme() == "http"
            && !is_loopback_url(&base)
            && (key.is_some() || self.provider == AiProvider::OpenAi)
        {
            return Err(SummarizeError::InvalidUrl(
                "plain http:// is only allowed for localhost/loopback when using an API key or OpenAI; use https://".to_owned(),
            ));
        }
        Ok((base, model))
    }
}

/// Returns `true` iff `url`'s host is `localhost` or a loopback IP (`127.0.0.0/8`, `::1`).
pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Validate an OpenAI-compatible base URL: strictly `http://` or `https://`, non-empty host,
/// no embedded credentials, no query string or fragment.
pub fn validate_base_url(raw: &str) -> Result<Url, SummarizeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(SummarizeError::InvalidUrl(
            "endpoint URL cannot be empty".to_owned(),
        ));
    }
    let parsed = Url::parse(trimmed)
        .map_err(|err| SummarizeError::InvalidUrl(format!("invalid URL: {err}")))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(SummarizeError::InvalidUrl(format!(
                "unsupported URL scheme {other:?}; only http:// and https:// are allowed"
            )));
        }
    }
    if !parsed.has_host() {
        return Err(SummarizeError::InvalidUrl(
            "endpoint URL must include a host".to_owned(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(SummarizeError::InvalidUrl(
            "endpoint URL must not contain embedded credentials".to_owned(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(SummarizeError::InvalidUrl(
            "endpoint URL must not contain query parameters or fragments".to_owned(),
        ));
    }
    Ok(parsed)
}

/// Build the full `chat/completions` URL from a validated base URL.
pub fn chat_completions_url(base: &Url) -> Url {
    let mut url = base.clone();
    let trimmed_path = url.path().trim_end_matches('/').to_owned();
    if trimmed_path.ends_with("/chat/completions") {
        url.set_path(&trimmed_path);
    } else if trimmed_path.is_empty() {
        url.set_path("/v1/chat/completions");
    } else {
        url.set_path(&format!("{trimmed_path}/chat/completions"));
    }
    url
}

/// Build the `/models` probe URL from a validated base URL.
pub fn models_url(base: &Url) -> Url {
    let mut url = base.clone();
    let trimmed_path = url.path().trim_end_matches('/').to_owned();
    if let Some(prefix) = trimmed_path.strip_suffix("/chat/completions") {
        let prefix = prefix.trim_end_matches('/');
        if prefix.is_empty() {
            url.set_path("/models");
        } else {
            url.set_path(&format!("{prefix}/models"));
        }
    } else if trimmed_path.is_empty() {
        url.set_path("/v1/models");
    } else {
        url.set_path(&format!("{trimmed_path}/models"));
    }
    url
}

/// Errors produced by prompt construction, endpoint validation, or HTTP completion calls.
#[derive(Debug, Error)]
pub enum SummarizeError {
    #[error("invalid endpoint URL: {0}")]
    InvalidUrl(String),
    #[error("no model configured — set a model name (e.g. llama3.2 or gpt-4o-mini)")]
    MissingModel,
    #[error("{provider} requires an API key — configure one in Preferences or via --api-key-env")]
    MissingApiKey { provider: &'static str },
    #[error("API key contains invalid control characters")]
    InvalidApiKey,
    #[error("nothing to summarize: {0}")]
    EmptyInput(String),
    #[error("request timed out after {secs}s")]
    Timeout { secs: u64 },
    #[error("request failed: {0}")]
    Transport(String),
    #[error("endpoint returned HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },
    #[error("invalid response JSON: {0}")]
    InvalidResponse(String),
    #[error("model returned an empty summary")]
    EmptyResponse,
}

/// One normalized message supplied to a thread or channel summarization target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryMessage {
    pub author: String,
    pub timestamp: Option<String>,
    pub text: String,
    pub is_thread_root: bool,
}

/// Input target to summarize.
#[derive(Debug, Clone, PartialEq)]
pub enum SummaryTarget {
    /// Summarize a thread (root + replies): decisions, action items, open questions.
    Thread {
        channel_label: String,
        thread_ts: Option<String>,
        messages: Vec<SummaryMessage>,
    },
    /// Summarize recent channel activity: highlights, key discussions, follow-ups.
    Channel {
        channel_label: String,
        topic: Option<String>,
        messages: Vec<SummaryMessage>,
    },
    /// Synthesize a local Catch-Up briefing into a concise executive summary.
    CatchUp { briefing: CatchUpBriefing },
    /// Summarize a video/audio transcript (with timestamped highlights when cues exist).
    Transcript {
        title: String,
        file_id: Option<String>,
        text: String,
        cues: Vec<TranscriptCue>,
    },
}

impl SummaryTarget {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Thread { .. } => "thread",
            Self::Channel { .. } => "channel",
            Self::CatchUp { .. } => "catchup",
            Self::Transcript { .. } => "transcript",
        }
    }

    pub fn display_title(&self) -> String {
        match self {
            Self::Thread { channel_label, .. } => format!("Thread in {channel_label}"),
            Self::Channel { channel_label, .. } => channel_label.clone(),
            Self::CatchUp { briefing } => {
                format!("Catch-Up ({})", briefing.window.label())
            }
            Self::Transcript { title, .. } => format!("Transcript · {title}"),
        }
    }
}

/// Structured prompt ready to send to a chat-completions model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryPrompt {
    pub system: String,
    pub user: String,
    pub source_items: usize,
    pub truncated: bool,
    pub input_chars: usize,
}

/// Successful AI summary response plus execution metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryResult {
    pub summary: String,
    pub provider: AiProvider,
    pub endpoint: String,
    pub model: String,
    pub target_kind: String,
    pub title: String,
    pub source_items: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    pub elapsed_ms: u64,
}

/// Successful connection check result returned by [`test_connection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionTestResult {
    pub provider: AiProvider,
    pub endpoint: String,
    pub model: String,
    pub available_models_count: Option<usize>,
    pub model_listed: Option<bool>,
    pub latency_ms: u64,
    pub detail: String,
}

/// Format seconds into `[MM:SS]` (or `[HH:MM:SS]` when >= 1 hour) for transcript cues.
pub fn format_cue_timestamp(secs: f64) -> String {
    let total = if secs.is_finite() && secs >= 0.0 {
        secs.floor() as u64
    } else {
        0
    };
    let hours = total / 3_600;
    let mins = (total % 3_600) / 60;
    let rem = total % 60;
    if hours > 0 {
        format!("[{hours:02}:{mins:02}:{rem:02}]")
    } else {
        format!("[{mins:02}:{rem:02}]")
    }
}

/// Truncate `s` cleanly on a UTF-8 character boundary to at most `max_chars` characters.
fn truncate_chars(s: &str, max_chars: usize) -> (&str, bool) {
    if max_chars == 0 {
        return ("", !s.is_empty());
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => (&s[..byte_idx], true),
        None => (s, false),
    }
}

fn format_message_line(msg: &SummaryMessage) -> String {
    let clean = msg.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let prefix = if msg.is_thread_root { "[root] " } else { "" };
    match msg.timestamp.as_deref().filter(|t| !t.is_empty()) {
        Some(ts) => format!("{prefix}[{ts}] {}: {clean}", msg.author),
        None => format!("{prefix}{}: {clean}", msg.author),
    }
}

/// Build a bounded, target-specific `(system, user)` prompt from `target`.
pub fn build_prompt(
    target: &SummaryTarget,
    max_input_chars: usize,
) -> Result<SummaryPrompt, SummarizeError> {
    let budget = max_input_chars.clamp(MIN_MAX_INPUT_CHARS, MAX_MAX_INPUT_CHARS);
    match target {
        SummaryTarget::Thread {
            channel_label,
            thread_ts,
            messages,
        } => {
            let non_empty: Vec<&SummaryMessage> = messages
                .iter()
                .filter(|m| !m.text.trim().is_empty())
                .collect();
            if non_empty.is_empty() {
                return Err(SummarizeError::EmptyInput(
                    "thread has no text messages to summarize".to_owned(),
                ));
            }
            let system = "You summarize Slack threads clearly and accurately using only the provided messages. \
Do not invent facts, owners, or deadlines. Respond in concise plain text with clear headings: \
Overview, Decisions, Action Items, and Open Questions (omit a section if none apply)."
                .to_owned();

            let header = match thread_ts.as_deref() {
                Some(ts) => format!("Slack thread in {channel_label} (root ts {ts}):\n\n"),
                None => format!("Slack thread in {channel_label}:\n\n"),
            };

            // Preserve the root message plus the newest replies that fit within `budget`.
            let lines: Vec<String> = non_empty.iter().map(|m| format_message_line(m)).collect();
            let (body, truncated) = pack_lines_keep_head_and_tail(&lines, budget, true);
            let user = format!("{header}{body}");
            let input_chars = user.chars().count();
            Ok(SummaryPrompt {
                system,
                user,
                source_items: non_empty.len(),
                truncated,
                input_chars,
            })
        }
        SummaryTarget::Channel {
            channel_label,
            topic,
            messages,
        } => {
            let non_empty: Vec<&SummaryMessage> = messages
                .iter()
                .filter(|m| !m.text.trim().is_empty())
                .collect();
            if non_empty.is_empty() {
                return Err(SummarizeError::EmptyInput(format!(
                    "{channel_label} has no recent text messages to summarize"
                )));
            }
            let system = "You summarize recent Slack channel activity clearly and accurately using only the provided messages. \
Do not invent facts or owners. Respond in concise plain text with clear headings: \
Highlights, Key Decisions & Discussions, and Follow-ups / Action Items (omit empty sections)."
                .to_owned();

            let mut header = format!("Recent Slack messages in {channel_label}");
            if let Some(t) = topic.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                let (clipped_topic, _) = truncate_chars(t, 200);
                header.push_str(&format!(" (topic: {clipped_topic})"));
            }
            header.push_str(":\n\n");

            let lines: Vec<String> = non_empty.iter().map(|m| format_message_line(m)).collect();
            let (body, truncated) = pack_lines_keep_head_and_tail(&lines, budget, false);
            let user = format!("{header}{body}");
            let input_chars = user.chars().count();
            Ok(SummaryPrompt {
                system,
                user,
                source_items: non_empty.len(),
                truncated,
                input_chars,
            })
        }
        SummaryTarget::CatchUp { briefing } => {
            if briefing.is_empty() {
                return Err(SummarizeError::EmptyInput(
                    "Catch-Up briefing has no mentions, unread channels, active threads, or transcripts"
                        .to_owned(),
                ));
            }
            let system = "You synthesize a local Slack Catch-Up briefing into a fast executive summary. \
Use only the provided items and never invent details. Respond in concise plain text with clear headings: \
Needs Your Attention, Channel Highlights, Active Threads, and Suggested Next Actions (omit empty sections)."
                .to_owned();

            let mut raw = String::new();
            let ws_names: Vec<String> = briefing
                .workspaces
                .iter()
                .map(|w| {
                    w.name
                        .clone()
                        .unwrap_or_else(|| w.team_id.as_str().to_owned())
                })
                .collect();
            raw.push_str(&format!(
                "Catch-Up window: {} | Workspaces: {}\n\n",
                briefing.window.label(),
                if ws_names.is_empty() {
                    "local".to_owned()
                } else {
                    ws_names.join(", ")
                }
            ));

            let mut items = 0usize;
            if !briefing.mentions.is_empty() {
                raw.push_str("## Direct mentions & DMs\n");
                for m in &briefing.mentions {
                    items += 1;
                    raw.push_str(&format!(
                        "- [{}] {} in {}: {}\n",
                        m.kind.label(),
                        m.author_name,
                        m.channel_label,
                        m.excerpt.trim()
                    ));
                }
                raw.push('\n');
            }
            if !briefing.unread_channels.is_empty() {
                raw.push_str("## Unread channels\n");
                for ch in &briefing.unread_channels {
                    items += 1;
                    raw.push_str(&format!(
                        "- {} ({} msgs, {} mentions)\n",
                        ch.channel_label, ch.message_count, ch.mention_count
                    ));
                    for ex in &ch.excerpts {
                        raw.push_str(&format!("  * {}: {}\n", ex.author_name, ex.text.trim()));
                    }
                }
                raw.push('\n');
            }
            if !briefing.active_threads.is_empty() {
                raw.push_str("## Active threads\n");
                for th in &briefing.active_threads {
                    items += 1;
                    raw.push_str(&format!(
                        "- {} ({} new / {} total replies) root by {}: {} | latest by {}: {}\n",
                        th.channel_label,
                        th.new_reply_count,
                        th.total_reply_count,
                        th.root_author,
                        th.root_excerpt.trim(),
                        th.latest_reply_author,
                        th.latest_reply_excerpt.trim()
                    ));
                }
                raw.push('\n');
            }
            if !briefing.transcripts.is_empty() {
                raw.push_str("## New transcripts\n");
                for tr in &briefing.transcripts {
                    items += 1;
                    raw.push_str(&format!(
                        "- [{}] {}: {}\n",
                        tr.kind.label(),
                        tr.title,
                        tr.excerpt.trim()
                    ));
                }
                raw.push('\n');
            }

            let (clipped, truncated) = truncate_chars(&raw, budget);
            let user = if truncated {
                format!("{clipped}\n[... truncated to fit context budget ...]")
            } else {
                clipped.to_owned()
            };
            let input_chars = user.chars().count();
            Ok(SummaryPrompt {
                system,
                user,
                source_items: items,
                truncated,
                input_chars,
            })
        }
        SummaryTarget::Transcript {
            title,
            file_id,
            text,
            cues,
        } => {
            let valid_cues: Vec<&TranscriptCue> =
                cues.iter().filter(|c| !c.text.trim().is_empty()).collect();
            let trimmed_text = text.trim();
            if valid_cues.is_empty() && trimmed_text.is_empty() {
                return Err(SummarizeError::EmptyInput("transcript is empty".to_owned()));
            }
            let has_cues = !valid_cues.is_empty();
            let system = if has_cues {
                "You summarize audio/video transcripts clearly using only the provided transcript. \
Do not invent details. Respond in concise plain text with two headings: \
Key Takeaways (bullet list) and Timestamped Highlights (bullet list referencing the exact [MM:SS] timestamps from the transcript)."
                    .to_owned()
            } else {
                "You summarize audio/video transcripts clearly using only the provided transcript. \
Do not invent details. Respond in concise plain text with two headings: \
Key Takeaways (bullet list) and Summary (short paragraph)."
                    .to_owned()
            };

            let header = match file_id.as_deref() {
                Some(fid) => format!("Transcript for \"{title}\" ({fid}):\n\n"),
                None => format!("Transcript for \"{title}\":\n\n"),
            };

            let (body, source_items, truncated) = if has_cues {
                let mut raw = String::new();
                for cue in &valid_cues {
                    let clean = cue.text.split_whitespace().collect::<Vec<_>>().join(" ");
                    raw.push_str(&format!(
                        "{} {clean}\n",
                        format_cue_timestamp(cue.start_secs)
                    ));
                }
                let (clipped, truncated) = truncate_chars(&raw, budget);
                let formatted = if truncated {
                    format!("{clipped}\n[... transcript truncated to fit context budget ...]")
                } else {
                    clipped.to_owned()
                };
                (formatted, valid_cues.len(), truncated)
            } else {
                let (clipped, truncated) = truncate_chars(trimmed_text, budget);
                let formatted = if truncated {
                    format!("{clipped}\n[... transcript truncated to fit context budget ...]")
                } else {
                    clipped.to_owned()
                };
                (formatted, 1, truncated)
            };

            let user = format!("{header}{body}");
            let input_chars = user.chars().count();
            Ok(SummaryPrompt {
                system,
                user,
                source_items,
                truncated,
                input_chars,
            })
        }
    }
}

/// Pack message lines within `budget` characters. When `keep_first` is true (threads), the first
/// line (root message) is preserved and older middle replies are dropped before newer ones;
/// otherwise (channels) the newest trailing messages are preserved.
fn pack_lines_keep_head_and_tail(
    lines: &[String],
    budget: usize,
    keep_first: bool,
) -> (String, bool) {
    let full = lines.join("\n");
    if full.chars().count() <= budget {
        return (full, false);
    }

    if lines.len() == 1 {
        let (clipped, _) = truncate_chars(&lines[0], budget.saturating_sub(1));
        return (format!("{clipped}…"), true);
    }

    let marker = "[... earlier messages omitted to fit context budget ...]";
    let marker_chars = marker.chars().count() + 2;

    if keep_first && lines.len() >= 2 {
        let root_budget = (budget / 3).max(160);
        let (root_clip, _) = truncate_chars(&lines[0], root_budget);
        let mut remaining = budget.saturating_sub(root_clip.chars().count() + marker_chars);
        let mut tail: Vec<String> = Vec::new();
        for line in lines[1..].iter().rev() {
            let cost = line.chars().count() + 1;
            if cost <= remaining {
                tail.push(line.clone());
                remaining = remaining.saturating_sub(cost);
            } else if tail.is_empty() && remaining > 32 {
                let (clipped, _) = truncate_chars(line, remaining);
                tail.push(format!("{clipped}…"));
                break;
            } else {
                break;
            }
        }
        tail.reverse();
        let mut out = String::from(root_clip);
        out.push('\n');
        out.push_str(marker);
        for line in tail {
            out.push('\n');
            out.push_str(&line);
        }
        return (out, true);
    }

    let mut remaining = budget.saturating_sub(marker_chars);
    let mut tail: Vec<String> = Vec::new();
    for line in lines.iter().rev() {
        let cost = line.chars().count() + 1;
        if cost <= remaining {
            tail.push(line.clone());
            remaining = remaining.saturating_sub(cost);
        } else if tail.is_empty() && remaining > 32 {
            let (clipped, _) = truncate_chars(line, remaining);
            tail.push(format!("{clipped}…"));
            break;
        } else {
            break;
        }
    }
    tail.reverse();
    let mut out = String::from(marker);
    for line in tail {
        out.push('\n');
        out.push_str(&line);
    }
    (out, true)
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: [ChatMessage<'a>; 2],
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    #[serde(default)]
    message: Option<ChatResponseMessage>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponseMessage {
    #[serde(default)]
    content: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ModelsListResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    #[serde(default)]
    id: String,
}

fn build_http_client(timeout: Duration) -> Result<reqwest::Client, SummarizeError> {
    crate::api::install_default_crypto_provider();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(15)))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| SummarizeError::Transport(err.to_string()))
}

async fn read_bounded_bytes(
    mut resp: reqwest::Response,
    max_bytes: usize,
    timeout_secs: u64,
) -> Result<Vec<u8>, SummarizeError> {
    if let Some(len) = resp.content_length()
        && len > max_bytes as u64
    {
        return Err(SummarizeError::InvalidResponse(format!(
            "response body ({len} bytes) exceeded {max_bytes} byte safety cap"
        )));
    }
    let mut buf = Vec::with_capacity(
        resp.content_length()
            .and_then(|l| usize::try_from(l).ok())
            .map_or(4096, |l| l.min(max_bytes)),
    );
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|err| map_reqwest_error(err, timeout_secs))?
    {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(SummarizeError::InvalidResponse(format!(
                "response body exceeded {max_bytes} byte safety cap"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

fn scrub_secret(text: &str, api_key: Option<&str>) -> String {
    match api_key.filter(|k| !k.is_empty()) {
        Some(secret) => text.replace(secret, "[REDACTED]"),
        None => text.to_owned(),
    }
}

fn extract_error_detail(status: u16, body: &[u8], api_key: Option<&str>) -> String {
    if let Ok(val) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(msg) = val
            .get("error")
            .and_then(|e| e.get("message").or(Some(e)))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let scrubbed = scrub_secret(msg, api_key);
            return truncate_chars(&scrubbed, 300).0.to_owned();
        }
        if let Some(msg) = val
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let scrubbed = scrub_secret(msg, api_key);
            return truncate_chars(&scrubbed, 300).0.to_owned();
        }
    }
    let text = String::from_utf8_lossy(body);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        format!("HTTP {status}")
    } else {
        let scrubbed = scrub_secret(trimmed, api_key);
        truncate_chars(&scrubbed, 300).0.to_owned()
    }
}

fn extract_choice_text(choice: &ChatChoice) -> Option<String> {
    if let Some(msg) = &choice.message
        && let Some(content) = &msg.content
    {
        match content {
            serde_json::Value::String(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_owned());
                }
            }
            serde_json::Value::Array(parts) => {
                let joined: Vec<&str> = parts
                    .iter()
                    .filter_map(|p| {
                        p.get("text")
                            .and_then(|t| t.as_str())
                            .or_else(|| p.as_str())
                    })
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                if !joined.is_empty() {
                    return Some(joined.join("\n"));
                }
            }
            _ => {}
        }
    }
    choice
        .text
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn map_reqwest_error(err: reqwest::Error, timeout_secs: u64) -> SummarizeError {
    if err.is_timeout() {
        SummarizeError::Timeout { secs: timeout_secs }
    } else {
        // Strip any URL credentials just in case, though URLs are already validated.
        SummarizeError::Transport(err.without_url().to_string())
    }
}

/// Summarize `target` against the OpenAI-compatible chat-completions endpoint in `config`.
pub async fn summarize(
    config: &AiSummaryConfig,
    target: &SummaryTarget,
) -> Result<SummaryResult, SummarizeError> {
    let (base_url, model) = config.validate()?;
    let prompt = build_prompt(target, config.input_budget())?;
    let endpoint = chat_completions_url(&base_url);
    let timeout = config.timeout();
    let timeout_secs = timeout.as_secs();
    let client = build_http_client(timeout)?;

    let payload = ChatCompletionRequest {
        model: &model,
        messages: [
            ChatMessage {
                role: "system",
                content: &prompt.system,
            },
            ChatMessage {
                role: "user",
                content: &prompt.user,
            },
        ],
        temperature: 0.2,
        max_tokens: config.max_tokens,
        stream: false,
    };
    let body = serde_json::to_vec(&payload)
        .map_err(|err| SummarizeError::InvalidResponse(format!("encoding request: {err}")))?;

    let mut req = client
        .post(endpoint.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        .body(body);
    if let Some(key) = config.normalized_api_key() {
        req = req.bearer_auth(key);
    }

    let started = Instant::now();
    let resp = req
        .send()
        .await
        .map_err(|err| map_reqwest_error(err, timeout_secs))?;
    let status = resp.status();
    let bytes = read_bounded_bytes(resp, MAX_RESPONSE_BYTES, timeout_secs).await?;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    if !status.is_success() {
        return Err(SummarizeError::HttpStatus {
            status: status.as_u16(),
            message: extract_error_detail(status.as_u16(), &bytes, config.normalized_api_key()),
        });
    }

    let parsed: ChatCompletionResponse = serde_json::from_slice(&bytes)
        .map_err(|err| SummarizeError::InvalidResponse(err.to_string()))?;
    let summary = parsed
        .choices
        .iter()
        .find_map(extract_choice_text)
        .ok_or(SummarizeError::EmptyResponse)?;

    let response_model = parsed
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(&model)
        .to_owned();

    Ok(SummaryResult {
        summary,
        provider: config.provider,
        endpoint: base_url.to_string().trim_end_matches('/').to_owned(),
        model: response_model,
        target_kind: target.kind().to_owned(),
        title: target.display_title(),
        source_items: prompt.source_items,
        truncated: prompt.truncated,
        prompt_tokens: parsed.usage.as_ref().and_then(|u| u.prompt_tokens),
        completion_tokens: parsed.usage.as_ref().and_then(|u| u.completion_tokens),
        total_tokens: parsed.usage.as_ref().and_then(|u| u.total_tokens),
        elapsed_ms,
    })
}

/// Probe connectivity and authentication against the configured OpenAI-compatible endpoint.
///
/// Queries `GET <base>/models` first; if the server returns `404` or `405` (some minimal proxies
/// only expose `/chat/completions`), falls back to a minimal 1-token completion ping.
pub async fn test_connection(
    config: &AiSummaryConfig,
) -> Result<ConnectionTestResult, SummarizeError> {
    let (base_url, model) = config.validate()?;
    let timeout = config.timeout().min(Duration::from_secs(20));
    let timeout_secs = timeout.as_secs();
    let client = build_http_client(timeout)?;

    let probe_url = models_url(&base_url);
    let mut req = client
        .get(probe_url)
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(key) = config.normalized_api_key() {
        req = req.bearer_auth(key);
    }

    let started = Instant::now();
    let resp = req
        .send()
        .await
        .map_err(|err| map_reqwest_error(err, timeout_secs))?;
    let status = resp.status();
    let bytes = read_bounded_bytes(resp, MAX_RESPONSE_BYTES, timeout_secs).await?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let endpoint = base_url.to_string().trim_end_matches('/').to_owned();

    if status.is_success() {
        let parsed = serde_json::from_slice::<ModelsListResponse>(&bytes).ok();
        let (count, listed) = match parsed {
            Some(list) => {
                let found = list.data.iter().any(|entry| {
                    entry.id == model
                        || entry.id.strip_suffix(":latest") == Some(model.as_str())
                        || model.strip_suffix(":latest") == Some(entry.id.as_str())
                });
                (Some(list.data.len()), Some(found))
            }
            None => (None, None),
        };
        let detail = match (count, listed) {
            (Some(n), Some(true)) => {
                format!("Connected ({latency_ms} ms) · {n} model(s) available · \"{model}\" ready")
            }
            (Some(n), Some(false)) if n > 0 => format!(
                "Connected ({latency_ms} ms) · {n} model(s) listed (\"{model}\" not in /models list)"
            ),
            (Some(0), _) => format!("Connected ({latency_ms} ms) · endpoint reachable"),
            _ => format!("Connected ({latency_ms} ms) · endpoint reachable"),
        };
        return Ok(ConnectionTestResult {
            provider: config.provider,
            endpoint,
            model,
            available_models_count: count,
            model_listed: listed,
            latency_ms,
            detail,
        });
    }

    // Fallback: some OpenAI-compatible servers only implement POST /chat/completions.
    if status.as_u16() == 404 || status.as_u16() == 405 {
        let ping_url = chat_completions_url(&base_url);
        let payload = ChatCompletionRequest {
            model: &model,
            messages: [
                ChatMessage {
                    role: "system",
                    content: "Reply with OK.",
                },
                ChatMessage {
                    role: "user",
                    content: "ping",
                },
            ],
            temperature: 0.0,
            max_tokens: Some(4),
            stream: false,
        };
        let body = serde_json::to_vec(&payload)
            .map_err(|err| SummarizeError::InvalidResponse(format!("encoding request: {err}")))?;
        let mut ping_req = client
            .post(ping_url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body);
        if let Some(key) = config.normalized_api_key() {
            ping_req = ping_req.bearer_auth(key);
        }
        let ping_started = Instant::now();
        let ping_resp = ping_req
            .send()
            .await
            .map_err(|err| map_reqwest_error(err, timeout_secs))?;
        let ping_status = ping_resp.status();
        let ping_bytes = read_bounded_bytes(ping_resp, MAX_RESPONSE_BYTES, timeout_secs).await?;
        let ping_ms = ping_started.elapsed().as_millis() as u64;
        if !ping_status.is_success() {
            return Err(SummarizeError::HttpStatus {
                status: ping_status.as_u16(),
                message: extract_error_detail(
                    ping_status.as_u16(),
                    &ping_bytes,
                    config.normalized_api_key(),
                ),
            });
        }
        let parsed: ChatCompletionResponse = serde_json::from_slice(&ping_bytes)
            .map_err(|err| SummarizeError::InvalidResponse(err.to_string()))?;
        let _ = parsed
            .choices
            .iter()
            .find_map(extract_choice_text)
            .ok_or(SummarizeError::EmptyResponse)?;
        return Ok(ConnectionTestResult {
            provider: config.provider,
            endpoint,
            model: model.clone(),
            available_models_count: None,
            model_listed: Some(true),
            latency_ms: ping_ms,
            detail: format!("Connected ({ping_ms} ms) · \"{model}\" responded to completion probe"),
        });
    }

    Err(SummarizeError::HttpStatus {
        status: status.as_u16(),
        message: extract_error_detail(status.as_u16(), &bytes, config.normalized_api_key()),
    })
}

/// Blocking wrapper around [`summarize`] for callers outside a tokio runtime (e.g. GPUI
/// background executor tasks); safe to call from inside any runtime flavor too.
pub fn summarize_blocking(
    config: &AiSummaryConfig,
    target: &SummaryTarget,
) -> Result<SummaryResult, SummarizeError> {
    block_on(summarize(config, target)).map_err(|e| {
        SummarizeError::Transport(format!("failed to start async HTTP runtime: {e}"))
    })?
}

/// Blocking wrapper around [`test_connection`] for callers outside a tokio runtime (e.g. GPUI
/// background executor tasks); safe to call from inside any runtime flavor too.
pub fn test_connection_blocking(
    config: &AiSummaryConfig,
) -> Result<ConnectionTestResult, SummarizeError> {
    block_on(test_connection(config)).map_err(|e| {
        SummarizeError::Transport(format!("failed to start async HTTP runtime: {e}"))
    })?
}

// --------------------------------------------------------------------------------------------
// Local SQLite store target loaders (shared by CLI & GUI)
// --------------------------------------------------------------------------------------------

fn channel_label_and_topic(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    users: &HashMap<UserId, String>,
) -> (String, Option<String>) {
    if let Ok(Some(conv)) = store.conversation(workspace, channel) {
        let topic = conv
            .topic
            .as_ref()
            .map(|t| t.value.trim().to_owned())
            .filter(|s| !s.is_empty());
        if let Some(name) = conv.name.as_deref().filter(|s| !s.is_empty()) {
            return (format!("#{name}"), topic);
        }
        if let Some(uid) = conv.user.as_ref() {
            let peer = users
                .get(uid)
                .cloned()
                .unwrap_or_else(|| uid.as_str().to_owned());
            return (format!("@{peer}"), topic);
        }
    }
    (channel.as_str().to_owned(), None)
}

fn format_ts_utc(ts: &Ts) -> Option<String> {
    let secs = ts.as_str().split('.').next()?.parse::<i64>().ok()?;
    Some(format_utc(secs))
}

fn stored_to_summary_message(
    row: &StoredMessage,
    users: &HashMap<UserId, String>,
    is_thread_root: bool,
) -> Option<SummaryMessage> {
    if row.deleted {
        return None;
    }
    let plain = mrkdwn_to_plain(&row.message.text);
    let text = plain.trim().to_owned();
    if text.is_empty() {
        return None;
    }
    let author = row
        .message
        .user
        .as_ref()
        .and_then(|u| users.get(u).cloned())
        .or_else(|| {
            row.message
                .username
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            row.message
                .bot_profile
                .as_ref()
                .map(|bp| bp.name.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| row.message.user.as_ref().map(|u| u.as_str().to_owned()))
        .unwrap_or_else(|| "Unknown".to_owned());

    Some(SummaryMessage {
        author,
        timestamp: format_ts_utc(&row.message.ts),
        text,
        is_thread_root,
    })
}

/// Build a [`SummaryTarget::Thread`] from the local SQLite store.
pub fn load_thread_target(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    thread_ts: &Ts,
) -> Result<SummaryTarget, SummarizeError> {
    let rows = store
        .thread_replies(workspace, channel, thread_ts)
        .map_err(|e| SummarizeError::EmptyInput(format!("reading thread: {e}")))?;
    let users = store.user_labels(workspace).unwrap_or_default();
    let (channel_label, _) = channel_label_and_topic(store, workspace, channel, &users);
    let messages: Vec<SummaryMessage> = rows
        .iter()
        .filter_map(|r| stored_to_summary_message(r, &users, &r.message.ts == thread_ts))
        .collect();
    if messages.is_empty() {
        return Err(SummarizeError::EmptyInput(format!(
            "no local messages found for thread {} in {channel_label}",
            thread_ts.as_str()
        )));
    }
    Ok(SummaryTarget::Thread {
        channel_label,
        thread_ts: Some(thread_ts.as_str().to_owned()),
        messages,
    })
}

/// Build a [`SummaryTarget::Channel`] from the newest `limit` non-deleted messages in `store`.
pub fn load_channel_target(
    store: &Store,
    workspace: &TeamId,
    channel: &ChannelId,
    limit: usize,
) -> Result<SummaryTarget, SummarizeError> {
    let limit = limit.clamp(1, 500);
    let window = crate::export::ChannelWindow {
        limit,
        include_deleted: false,
        ..Default::default()
    };
    let rows = crate::export::read_channel_window(store, workspace, channel, &window)
        .map_err(|e| SummarizeError::EmptyInput(format!("reading channel: {e}")))?;
    let users = store.user_labels(workspace).unwrap_or_default();
    let (channel_label, topic) = channel_label_and_topic(store, workspace, channel, &users);
    let messages: Vec<SummaryMessage> = rows
        .iter()
        .filter_map(|r| stored_to_summary_message(r, &users, false))
        .collect();
    if messages.is_empty() {
        return Err(SummarizeError::EmptyInput(format!(
            "no local messages found in {channel_label}"
        )));
    }
    Ok(SummaryTarget::Channel {
        channel_label,
        topic,
        messages,
    })
}

/// Build a [`SummaryTarget::Transcript`] from a stored audio/video file transcript in `store`.
pub fn load_transcript_target(
    store: &Store,
    workspace: &TeamId,
    file_id: &FileId,
) -> Result<SummaryTarget, SummarizeError> {
    let stored = store
        .file_transcript(workspace, file_id)
        .map_err(|e| SummarizeError::EmptyInput(format!("reading file transcript: {e}")))?
        .ok_or_else(|| {
            SummarizeError::EmptyInput(format!(
                "no local transcript stored for file {}",
                file_id.as_str()
            ))
        })?;
    Ok(SummaryTarget::Transcript {
        title: file_id.as_str().to_owned(),
        file_id: Some(file_id.as_str().to_owned()),
        text: stored.text,
        cues: stored.cues,
    })
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;
    use crate::catchup::{
        CatchUpExcerpt, CatchUpMention, CatchUpUnreadChannel, CatchUpWindow,
        CatchUpWorkspaceSummary, MentionKind,
    };

    #[test]
    fn debug_redacts_api_key() {
        let cfg = AiSummaryConfig {
            provider: AiProvider::OpenAi,
            base_url: AiProvider::OpenAi.default_base_url().to_owned(),
            model: "gpt-4o-mini".to_owned(),
            api_key: Some("sk-super-secret-value-9999".to_owned()),
            ..AiSummaryConfig::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("sk-super-secret-value-9999"),
            "Debug output must never contain API key: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn validates_base_url_strictly() {
        assert!(validate_base_url("http://localhost:11434/v1").is_ok());
        assert!(validate_base_url("https://api.openai.com/v1").is_ok());
        assert!(validate_base_url("https://llm.internal:8443/openai/v1/").is_ok());

        assert!(validate_base_url("").is_err());
        assert!(validate_base_url("file:///etc/passwd").is_err());
        assert!(validate_base_url("ftp://example.com/v1").is_err());
        assert!(validate_base_url("javascript:alert(1)").is_err());
        assert!(validate_base_url("https://user:pass@api.openai.com/v1").is_err());
        assert!(validate_base_url("https://api.openai.com/v1?key=leak").is_err());
        assert!(validate_base_url("https://api.openai.com/v1#frag").is_err());
    }

    #[test]
    fn derives_chat_and_models_urls() {
        let ollama = validate_base_url("http://localhost:11434/v1").expect("valid");
        assert_eq!(
            chat_completions_url(&ollama).as_str(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            models_url(&ollama).as_str(),
            "http://localhost:11434/v1/models"
        );

        let direct =
            validate_base_url("https://proxy.example/custom/chat/completions").expect("ok");
        assert_eq!(
            chat_completions_url(&direct).as_str(),
            "https://proxy.example/custom/chat/completions"
        );
        assert_eq!(
            models_url(&direct).as_str(),
            "https://proxy.example/custom/models"
        );

        let root = validate_base_url("http://localhost:1234").expect("ok");
        assert_eq!(
            chat_completions_url(&root).as_str(),
            "http://localhost:1234/v1/chat/completions"
        );
        assert_eq!(
            models_url(&root).as_str(),
            "http://localhost:1234/v1/models"
        );
    }

    #[test]
    fn openai_preset_requires_api_key_while_ollama_does_not() {
        let ollama = AiSummaryConfig {
            provider: AiProvider::Ollama,
            base_url: String::new(),
            model: "llama3.2".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        let (url, model) = ollama.validate().expect("ollama valid without key");
        assert_eq!(url.as_str(), "http://localhost:11434/v1");
        assert_eq!(model, "llama3.2");

        let openai_no_key = AiSummaryConfig {
            provider: AiProvider::OpenAi,
            base_url: String::new(),
            model: "gpt-4o-mini".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            openai_no_key.validate(),
            Err(SummarizeError::MissingApiKey { .. })
        ));
    }

    #[test]
    fn thread_prompt_truncates_utf8_safely_and_preserves_root_and_latest() {
        let mut messages = vec![SummaryMessage {
            author: "Alice".to_owned(),
            timestamp: Some("2026-11-26 09:00:00Z".to_owned()),
            text: "Root proposal: ship v2 on Friday 🚀".to_owned(),
            is_thread_root: true,
        }];
        for i in 0..40 {
            messages.push(SummaryMessage {
                author: format!("User{i}"),
                timestamp: None,
                text: format!("Middle reply #{i} with unicode café — {}", "x".repeat(40)),
                is_thread_root: false,
            });
        }
        messages.push(SummaryMessage {
            author: "Bob".to_owned(),
            timestamp: None,
            text: "Final decision: approved for Friday!".to_owned(),
            is_thread_root: false,
        });

        let target = SummaryTarget::Thread {
            channel_label: "#releases".to_owned(),
            thread_ts: Some("1764147600.000100".to_owned()),
            messages,
        };
        let prompt = build_prompt(&target, 600).expect("prompt");
        assert!(prompt.truncated);
        assert!(prompt.user.contains("Root proposal: ship v2 on Friday 🚀"));
        assert!(prompt.user.contains("Final decision: approved for Friday!"));
        assert!(prompt.user.contains("earlier messages omitted"));
        assert!(prompt.system.contains("Decisions"));
        assert!(prompt.system.contains("Action Items"));
        assert!(prompt.system.contains("Open Questions"));
    }

    #[test]
    fn transcript_prompt_includes_cue_timestamps_when_present() {
        let target = SummaryTarget::Transcript {
            title: "standup.mp4".to_owned(),
            file_id: Some("F123".to_owned()),
            text: "Fallback text".to_owned(),
            cues: vec![
                TranscriptCue {
                    start_secs: 5.2,
                    end_secs: 10.0,
                    text: "Welcome to the sprint review.".to_owned(),
                },
                TranscriptCue {
                    start_secs: 75.0,
                    end_secs: 82.0,
                    text: "We cut latency by 40%.".to_owned(),
                },
            ],
        };
        let prompt = build_prompt(&target, 4_000).expect("prompt");
        assert!(!prompt.truncated);
        assert_eq!(prompt.source_items, 2);
        assert!(prompt.system.contains("Timestamped Highlights"));
        assert!(
            prompt
                .user
                .contains("[00:05] Welcome to the sprint review.")
        );
        assert!(prompt.user.contains("[01:15] We cut latency by 40%."));
    }

    #[test]
    fn catchup_prompt_rejects_empty_and_formats_sections() {
        let empty = CatchUpBriefing::empty(CatchUpWindow::Last24h, 1_750_000_000);
        assert!(matches!(
            build_prompt(&SummaryTarget::CatchUp { briefing: empty }, 4_000),
            Err(SummarizeError::EmptyInput(_))
        ));

        let mut briefing = CatchUpBriefing::empty(CatchUpWindow::Last24h, 1_750_000_000);
        briefing.workspaces.push(CatchUpWorkspaceSummary {
            team_id: TeamId::from("T1"),
            name: Some("Acme".to_owned()),
            cutoff_secs: 1_749_913_600,
        });
        briefing.mentions.push(CatchUpMention {
            workspace: TeamId::from("T1"),
            workspace_name: Some("Acme".to_owned()),
            channel: ChannelId::from("C1"),
            channel_label: "#ops".to_owned(),
            ts: Ts::from("1749990000.000100"),
            thread_ts: None,
            author_id: None,
            author_name: "Alice".to_owned(),
            excerpt: "Can you approve the deploy?".to_owned(),
            created_at: 1_749_990_000,
            kind: MentionKind::DirectMention,
        });
        briefing.unread_channels.push(CatchUpUnreadChannel {
            workspace: TeamId::from("T1"),
            workspace_name: Some("Acme".to_owned()),
            channel: ChannelId::from("C2"),
            channel_label: "#eng".to_owned(),
            is_dm: false,
            message_count: 5,
            mention_count: 0,
            latest_ts: Ts::from("1749991000.000100"),
            latest_at: 1_749_991_000,
            excerpts: vec![CatchUpExcerpt {
                ts: Ts::from("1749991000.000100"),
                author_name: "Bob".to_owned(),
                text: "Migrations finished cleanly.".to_owned(),
                created_at: 1_749_991_000,
            }],
        });

        let prompt = build_prompt(&SummaryTarget::CatchUp { briefing }, 4_000).expect("prompt");
        assert_eq!(prompt.source_items, 2);
        assert!(prompt.user.contains("Can you approve the deploy?"));
        assert!(prompt.user.contains("Migrations finished cleanly."));
    }

    #[tokio::test]
    async fn wiremock_ollama_sends_payload_without_auth_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("content-type", "application/json"))
            .respond_with(|req: &Request| {
                assert!(
                    req.headers.get("authorization").is_none(),
                    "Ollama without API key must not send Authorization header"
                );
                let body: serde_json::Value =
                    serde_json::from_slice(&req.body).expect("valid JSON request");
                assert_eq!(body["model"], "llama3.2");
                assert_eq!(body["stream"], false);
                assert_eq!(body["messages"].as_array().expect("messages").len(), 2);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "id": "chatcmpl-1",
                    "model": "llama3.2",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "Overview: Ship on Friday." }
                    }],
                    "usage": { "prompt_tokens": 42, "completion_tokens": 12, "total_tokens": 54 }
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let cfg = AiSummaryConfig {
            provider: AiProvider::Ollama,
            base_url: format!("{}/v1", server.uri()),
            model: "llama3.2".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        let target = SummaryTarget::Channel {
            channel_label: "#general".to_owned(),
            topic: Some("Team updates".to_owned()),
            messages: vec![SummaryMessage {
                author: "Alice".to_owned(),
                timestamp: None,
                text: "Let's ship on Friday!".to_owned(),
                is_thread_root: false,
            }],
        };

        let result = summarize(&cfg, &target).await.expect("summary");
        assert_eq!(result.summary, "Overview: Ship on Friday.");
        assert_eq!(result.model, "llama3.2");
        assert_eq!(result.total_tokens, Some(54));
        assert_eq!(result.target_kind, "channel");
    }

    #[tokio::test]
    async fn wiremock_openai_sends_bearer_token_and_parses_content_parts() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-test-key"))
            .and(header_exists("content-type"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "gpt-4o-mini-2025",
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": [
                            {"type": "text", "text": "Key Takeaways:"},
                            {"type": "text", "text": "- Shipped v2"}
                        ]
                    }
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let cfg = AiSummaryConfig {
            provider: AiProvider::OpenAi,
            base_url: format!("{}/v1", server.uri()),
            model: "gpt-4o-mini".to_owned(),
            api_key: Some("sk-test-key".to_owned()),
            ..AiSummaryConfig::default()
        };
        let target = SummaryTarget::Transcript {
            title: "demo.mov".to_owned(),
            file_id: None,
            text: "We shipped v2 today.".to_owned(),
            cues: Vec::new(),
        };

        let res = summarize(&cfg, &target).await.expect("summary");
        assert_eq!(res.summary, "Key Takeaways:\n- Shipped v2");
        assert_eq!(res.model, "gpt-4o-mini-2025");
    }

    #[tokio::test]
    async fn wiremock_handles_http_errors_and_malformed_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/err/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": { "message": "Invalid API key provided" }
            })))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/badjson/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
            .mount(&server)
            .await;

        let target = SummaryTarget::Channel {
            channel_label: "#ops".to_owned(),
            topic: None,
            messages: vec![SummaryMessage {
                author: "Bob".to_owned(),
                timestamp: None,
                text: "All green.".to_owned(),
                is_thread_root: false,
            }],
        };

        let cfg_err = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/err", server.uri()),
            model: "custom-model".to_owned(),
            api_key: Some("bad-key".to_owned()),
            ..AiSummaryConfig::default()
        };
        match summarize(&cfg_err, &target).await {
            Err(SummarizeError::HttpStatus { status, message }) => {
                assert_eq!(status, 401);
                assert!(message.contains("Invalid API key"));
            }
            other => panic!("expected HttpStatus(401), got {other:?}"),
        }

        let cfg_bad = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/badjson", server.uri()),
            model: "custom-model".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            summarize(&cfg_bad, &target).await,
            Err(SummarizeError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn wiremock_test_connection_probes_models_and_falls_back_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {"id": "llama3.2:latest"},
                    {"id": "mistral:latest"}
                ]
            })))
            .mount(&server)
            .await;

        let cfg = AiSummaryConfig {
            provider: AiProvider::Ollama,
            base_url: format!("{}/v1", server.uri()),
            model: "llama3.2".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        let res = test_connection(&cfg).await.expect("connection ok");
        assert_eq!(res.available_models_count, Some(2));
        assert_eq!(res.model_listed, Some(true));
        assert!(res.detail.contains("llama3.2"));

        // Custom server without /models falls back to /chat/completions.
        Mock::given(method("GET"))
            .and(path("/custom/models"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/custom/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"role": "assistant", "content": "OK"}}]
            })))
            .mount(&server)
            .await;

        let cfg_custom = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/custom", server.uri()),
            model: "my-local-llm".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        let res2 = test_connection(&cfg_custom).await.expect("fallback ok");
        assert_eq!(res2.model_listed, Some(true));
        assert!(res2.detail.contains("completion probe"));
    }

    #[test]
    fn rejects_plain_http_to_non_loopback_with_api_key_or_openai() {
        let remote_with_key = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: "http://remote.example.com/v1".to_owned(),
            model: "custom".to_owned(),
            api_key: Some("sk-secret".to_owned()),
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            remote_with_key.validate(),
            Err(SummarizeError::InvalidUrl(_))
        ));

        let openai_http = AiSummaryConfig {
            provider: AiProvider::OpenAi,
            base_url: "http://api.openai.com/v1".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            api_key: Some("sk-secret".to_owned()),
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            openai_http.validate(),
            Err(SummarizeError::InvalidUrl(_))
        ));

        // Loopback http:// (localhost / 127.0.0.1 / [::1]) stays allowed even with an API key.
        for loopback in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080/v1",
            "http://[::1]:1234/v1",
        ] {
            let ok_cfg = AiSummaryConfig {
                provider: AiProvider::Custom,
                base_url: loopback.to_owned(),
                model: "local".to_owned(),
                api_key: Some("local-key".to_owned()),
                ..AiSummaryConfig::default()
            };
            assert!(ok_cfg.validate().is_ok(), "{loopback} should be allowed");
        }

        // Control chars in API key are rejected cleanly.
        let bad_key = AiSummaryConfig {
            provider: AiProvider::OpenAi,
            base_url: "https://api.openai.com/v1".to_owned(),
            model: "gpt-4o-mini".to_owned(),
            api_key: Some("sk-bad\r\nX-Inject: 1".to_owned()),
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            bad_key.validate(),
            Err(SummarizeError::InvalidApiKey)
        ));
    }

    #[tokio::test]
    async fn wiremock_does_not_follow_redirects_caps_response_size_and_scrubs_echoed_key() {
        let server = MockServer::start().await;

        // 1. 307 redirect must NOT be followed.
        Mock::given(method("POST"))
            .and(path("/redir/chat/completions"))
            .respond_with(ResponseTemplate::new(307).insert_header(
                "location",
                format!("{}/target/chat/completions", server.uri()),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/target/chat/completions"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;

        // 2. Echoed API key in error body must be scrubbed to [REDACTED].
        let secret = "sk-echo-secret-123456";
        Mock::given(method("POST"))
            .and(path("/echo/chat/completions"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": { "message": format!("Rejected key {secret}") }
            })))
            .mount(&server)
            .await;

        // 3. Oversized response body must be rejected before buffering unboundedly.
        let oversized = vec![b'a'; MAX_RESPONSE_BYTES + 1024];
        Mock::given(method("POST"))
            .and(path("/huge/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(oversized))
            .mount(&server)
            .await;

        let target = SummaryTarget::Channel {
            channel_label: "#ops".to_owned(),
            topic: None,
            messages: vec![SummaryMessage {
                author: "Alice".to_owned(),
                timestamp: None,
                text: "Sensitive Slack content".to_owned(),
                is_thread_root: false,
            }],
        };

        let cfg_redir = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/redir", server.uri()),
            model: "m".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            summarize(&cfg_redir, &target).await,
            Err(SummarizeError::HttpStatus { status: 307, .. })
        ));

        let cfg_echo = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/echo", server.uri()),
            model: "m".to_owned(),
            api_key: Some(secret.to_owned()),
            ..AiSummaryConfig::default()
        };
        match summarize(&cfg_echo, &target).await {
            Err(SummarizeError::HttpStatus {
                status: 401,
                message,
            }) => {
                assert!(!message.contains(secret), "key leaked in error: {message}");
                assert!(message.contains("[REDACTED]"));
            }
            other => panic!("expected HttpStatus(401), got {other:?}"),
        }

        let cfg_huge = AiSummaryConfig {
            provider: AiProvider::Custom,
            base_url: format!("{}/huge", server.uri()),
            model: "m".to_owned(),
            api_key: None,
            ..AiSummaryConfig::default()
        };
        assert!(matches!(
            summarize(&cfg_huge, &target).await,
            Err(SummarizeError::InvalidResponse(_))
        ));
    }

    #[test]
    fn load_channel_target_paginates_past_deleted_tombstones() {
        let mut store = Store::open_in_memory().unwrap();
        let ws = TeamId::from("T1");
        let ch = ChannelId::from("C1");
        store
            .upsert_conversation(
                &ws,
                &crate::types::Conversation {
                    id: ch.clone(),
                    name: Some("general".into()),
                    is_channel: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut msgs = Vec::new();
        for i in 1..=6 {
            msgs.push(crate::types::Message {
                ts: Ts::from(format!("170000000{i}.000100")),
                user: Some(UserId::from("U1")),
                text: format!("msg {i}"),
                ..Default::default()
            });
        }
        store.upsert_messages(&ws, &ch, &msgs).unwrap();
        // Delete the 4 newest messages so a naive limit=3 read sees only tombstones.
        for i in 3..=6 {
            store
                .record_deletion(
                    &ws,
                    &ch,
                    &Ts::from(format!("170000000{i}.000100")),
                    Some(&Ts::from(format!("170000010{i}.000100"))),
                    None,
                )
                .unwrap();
        }
        let target = load_channel_target(&store, &ws, &ch, 3).unwrap();
        match target {
            SummaryTarget::Channel { messages, .. } => {
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[0].text, "msg 1");
                assert_eq!(messages[1].text, "msg 2");
            }
            other => panic!("expected Channel target, got {other:?}"),
        }
    }
}
