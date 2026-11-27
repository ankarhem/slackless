//! Slack's Preferences window: a left section nav and the selected section's controls on the
//! right, covering Notifications, Navigation, Home, Appearance & Themes, Messages & media,
//! Language & region, Audio & video, Accessibility, Mark as read, and Advanced.
//!
//! Every control writes the in-memory [`Settings`] and emits [`PreferencesEvent::Changed`].
//! Persisted schema, GPUI global caches, media probes, standalone window hosting, and per-section
//! renderers live in focused submodules and are re-exported here.

mod download;
mod prefs_globals;
mod probes;
mod sections;
mod settings;
#[cfg(test)]
mod tests;
mod window;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px,
};

use crate::theme::{self, Palette};

pub use crate::ui::forms::{
    ControlMetrics, control_metrics, form_button, search_input_box, text_input_box, text_input_mono,
};
pub(crate) use crate::ui::forms::{form_button_for, text_input_box_for};
pub use prefs_globals::{
    active_quick_reactions, display_prefs, matches_highlight_keyword, set_display_prefs,
};
#[cfg(test)]
pub(crate) use settings::AiSummarySettings;
pub use settings::{
    AppearanceMode, HOURS, MEDIA_CACHE_AGE_OPTIONS, MEDIA_CACHE_SIZE_OPTIONS, MESSAGE_VERBOSITY,
    MOBILE_HANDOFF, NOTIFY_LEVELS, SCHEDULE_DAYS, SOUND_FILES, Section, Settings, StorageInfo,
    TRANSCRIPTION_PROVIDERS, format_accessible_message, format_cross_workspace_summary,
};
pub(crate) use slackless_core::types::time::format_bytes;
pub use window::open_preferences_window;

pub enum PreferencesEvent {
    /// Some setting changed; read [`Preferences::settings`].
    Changed,
    /// User asked to compact/VACUUM the active workspace's local SQLite store.
    CompactStorage,
    /// User asked to clear the re-fetchable media, audio and avatar/emoji caches on disk.
    ClearMediaCache,
    /// User asked to export captured HTTP net-log JSON (`F-SHELL-158`).
    ExportNetLog,
    /// User clicked "Test speaker" in Preferences -> Audio & video (`F-COLLAB-040`).
    TestSpeaker(Option<String>),
    /// User clicked "Check for updates" in Preferences -> Advanced (`F-SHELL-163`).
    CheckForUpdates,
    /// User clicked "Download & restart" in Preferences -> Advanced (`F-SHELL-163`).
    InstallUpdate,
    /// Open the Archive & Storage modal dialog showing per-channel backfill progress.
    OpenStorageDialog,
    /// Start background archive backfill for the entire active workspace.
    StartWorkspaceBackfill,
    /// Pause active background archive backfill.
    PauseBackfill,
    /// Resume paused background archive backfill.
    ResumeBackfill,
}

/// Accessor for a checkbox's flag.
pub(super) type Flag = fn(&mut Settings) -> &mut bool;
/// Accessor for a radio group's / select's option index.
pub(super) type Choice = fn(&mut Settings) -> &mut usize;

/// Cached off-thread snapshot of local semantic embedding model + `semantic_index.sqlite` stats,
/// refreshed asynchronously so GPUI render frames never block on SQLite or disk I/O.
#[derive(Clone)]
pub struct SemanticStorageSnapshot {
    pub setup: slackless_core::embeddings::EmbeddingModelStatus,
    pub stats: slackless_core::store::SemanticIndexStats,
    /// Whether the ONNX embedding session is currently resident in memory.
    pub embedder_loaded: bool,
}

impl SemanticStorageSnapshot {
    /// Collect current model & index status (blocking — call on a background executor).
    pub fn collect() -> Self {
        let model = slackless_core::embeddings::EmbeddingModel::default();
        Self {
            setup: slackless_core::embeddings::detect_embedding_model_setup(model),
            stats: slackless_core::semantic_index::default_semantic_index_stats(),
            embedder_loaded: slackless_core::embeddings::embedder_loaded(),
        }
    }
}

/// Compute cosine similarity between `text` and `topic` using the locally installed ONNX
/// embedding model. **Blocking** — call only from a background executor, never on the UI thread.
pub(crate) fn compute_local_semantic_similarity(text: &str, topic: &str) -> Option<f32> {
    #[cfg(test)]
    {
        let _ = (text, topic);
        None
    }
    #[cfg(not(test))]
    {
        use slackless_core::embeddings::{
            Embedder as _, EmbeddingModel, detect_embedding_model_setup,
        };
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};

        let text = text.trim();
        let topic = topic.trim();
        if text.is_empty() || topic.is_empty() {
            return None;
        }
        let model = EmbeddingModel::default();
        if !detect_embedding_model_setup(model).model_ready {
            return None;
        }

        static TOPIC_CACHE: OnceLock<Mutex<HashMap<String, Vec<f32>>>> = OnceLock::new();

        // Reuse the shared ONNX embedder slot so Smart Rules never loads a duplicate ~90 MiB
        // session and inherits the 5-minute idle auto-unload TTL and model-deletion clearing.
        let embedder = crate::search::load_or_get_onnx_embedder().ok()?;

        let topic_vec = {
            let cache = TOPIC_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
            let cached = cache.lock().ok().and_then(|g| g.get(topic).cloned());
            if let Some(vec) = cached {
                vec
            } else {
                let vec = embedder.embed_query(topic).ok()?;
                if let Ok(mut guard) = cache.lock() {
                    if guard.len() >= 64 {
                        guard.clear();
                    }
                    guard.insert(topic.to_owned(), vec.clone());
                }
                vec
            }
        };

        let msg_vec = embedder.embed_document(text).ok()?;
        Some(slackless_core::store::cosine_similarity(
            &msg_vec, &topic_vec,
        ))
    }
}

/// Similarity threshold presets for smart-rule semantic topic matching.
pub(super) const RULE_SEMANTIC_THRESHOLDS: [(f32, &str); 4] = [
    (0.35, "0.35 (Broad)"),
    (0.45, "0.45 (Balanced)"),
    (0.55, "0.55 (Focused)"),
    (0.65, "0.65 (Strict)"),
];

/// Inline Add/Edit draft state and live test-preview inputs for Smart Notification Rules.
pub(super) struct RuleEditorState {
    pub(super) open: bool,
    pub(super) editing_id: Option<String>,
    pub(super) action: slackless_core::rules::RuleAction,
    pub(super) only_threads: bool,
    pub(super) mentions_me: bool,
    pub(super) semantic_threshold_idx: usize,
    pub(super) name: Entity<TextareaState>,
    pub(super) from_users: Entity<TextareaState>,
    pub(super) in_channels: Entity<TextareaState>,
    pub(super) keywords: Entity<TextareaState>,
    pub(super) regex_pattern: Entity<TextareaState>,
    pub(super) semantic_topic: Entity<TextareaState>,
    pub(super) test_sender: Entity<TextareaState>,
    pub(super) test_channel: Entity<TextareaState>,
    pub(super) test_message: Entity<TextareaState>,
    pub(super) test_is_thread: bool,
    pub(super) test_mentions_me: bool,
    /// Last off-thread semantic similarity preview result: `(sample_text, topic, score)`.
    pub(super) semantic_test_cache: Option<(String, String, Option<f32>)>,
    /// True while a background embedding preview computation is in flight.
    pub(super) semantic_test_loading: bool,
    /// Monotonic generation token dropping stale background semantic previews.
    pub(super) semantic_test_gen: u64,
    pub(super) next_rule_seq: u64,
}

impl RuleEditorState {
    /// Build a [`slackless_core::rules::NotificationRule`] snapshot from the current editor inputs.
    pub(super) fn draft_rule(
        &self,
        id: String,
        enabled: bool,
        cx: &App,
    ) -> slackless_core::rules::NotificationRule {
        let name = self.name.read(cx).value().trim().to_owned();
        let regex_raw = self.regex_pattern.read(cx).value().trim().to_owned();
        let topic_raw = self.semantic_topic.read(cx).value().trim().to_owned();
        let threshold = RULE_SEMANTIC_THRESHOLDS
            .get(self.semantic_threshold_idx)
            .map(|(v, _)| *v)
            .unwrap_or(slackless_core::rules::DEFAULT_SEMANTIC_THRESHOLD);
        slackless_core::rules::NotificationRule {
            id,
            name: if name.is_empty() {
                "Untitled rule".to_owned()
            } else {
                name
            },
            enabled,
            action: self.action,
            from_users: slackless_core::rules::parse_csv_tokens(&self.from_users.read(cx).value()),
            in_channels: slackless_core::rules::parse_csv_tokens(
                &self.in_channels.read(cx).value(),
            ),
            keywords: slackless_core::rules::parse_csv_tokens(&self.keywords.read(cx).value()),
            regex_pattern: (!regex_raw.is_empty()).then_some(regex_raw),
            only_threads: self.only_threads,
            mentions_me: self.mentions_me,
            semantic_topic: (!topic_raw.is_empty()).then_some(topic_raw),
            semantic_threshold: (!self.semantic_topic.read(cx).value().trim().is_empty())
                .then_some(threshold),
        }
    }

    pub(super) fn load_rule(
        &mut self,
        rule: Option<&slackless_core::rules::NotificationRule>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.open = true;
        if let Some(r) = rule {
            self.editing_id = Some(r.id.clone());
            self.action = r.action;
            self.only_threads = r.only_threads;
            self.mentions_me = r.mentions_me;
            let target_th = r.effective_semantic_threshold();
            self.semantic_threshold_idx = RULE_SEMANTIC_THRESHOLDS
                .iter()
                .enumerate()
                .min_by(|(_, (a, _)), (_, (b, _))| {
                    (a - target_th).abs().total_cmp(&(b - target_th).abs())
                })
                .map(|(ix, _)| ix)
                .unwrap_or(1);
            let name = r.name.clone();
            let users = r.from_users.join(", ");
            let channels = r.in_channels.join(", ");
            let keywords = r.keywords.join(", ");
            let regex = r.regex_pattern.clone().unwrap_or_default();
            let topic = r.semantic_topic.clone().unwrap_or_default();
            self.name.update(cx, |s, cx| s.set_value(name, window, cx));
            self.from_users
                .update(cx, |s, cx| s.set_value(users, window, cx));
            self.in_channels
                .update(cx, |s, cx| s.set_value(channels, window, cx));
            self.keywords
                .update(cx, |s, cx| s.set_value(keywords, window, cx));
            self.regex_pattern
                .update(cx, |s, cx| s.set_value(regex, window, cx));
            self.semantic_topic
                .update(cx, |s, cx| s.set_value(topic, window, cx));
        } else {
            self.editing_id = None;
            self.action = slackless_core::rules::RuleAction::BreakThroughMute;
            self.only_threads = false;
            self.mentions_me = false;
            self.semantic_threshold_idx = 1;
            self.name
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
            self.from_users
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
            self.in_channels
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
            self.keywords
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
            self.regex_pattern
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
            self.semantic_topic
                .update(cx, |s, cx| s.set_value(String::new(), window, cx));
        }
    }
}

pub struct Preferences {
    pub(super) section: Section,
    pub(super) settings: Settings,
    /// The select whose option list is expanded, by id.
    pub(super) open_select: Option<&'static str>,
    pub(super) keywords: Entity<TextareaState>,
    pub(super) vip_input: Entity<TextareaState>,
    pub(super) catchup_time_input: Entity<TextareaState>,
    pub(super) rule_editor: RuleEditorState,
    pub(super) excluded_channels: Entity<TextareaState>,
    pub(super) protocol_allowlist_input: Entity<TextareaState>,
    pub(super) proxy_input: Entity<TextareaState>,
    pub(super) quick_reaction_inputs: [Entity<TextareaState>; 3],
    pub(super) custom_sidebar_hex: Entity<TextareaState>,
    pub(super) custom_accent_hex: Entity<TextareaState>,
    pub(super) slack_theme_import: Entity<TextareaState>,
    pub(super) storage_stats: Option<StorageInfo>,
    pub(super) backfill_progress: Option<slackless_core::WorkspaceBackfillProgress>,
    pub(super) cli_status_note: Option<String>,
    pub(super) update_state: crate::updater::UpdateState,
    pub(super) available_update: Option<crate::updater::ReleaseInfo>,
    pub(super) audio_devices: slackless_core::huddle::AudioDevices,
    pub(super) cameras: Vec<slackless_core::huddle::CameraDeviceInfo>,
    pub(super) audio_loading: bool,
    pub(super) audio_probe_gen: u64,
    pub(super) mic_probe: Option<slackless_core::huddle::MicLevelProbe>,
    pub(super) mic_level: f32,
    pub(super) camera_preview_on: bool,
    pub(super) camera_probe_gen: u64,
    pub(super) camera_probe: Option<slackless_core::huddle::CameraPreviewProbe>,
    pub(super) camera_frame: Option<std::sync::Arc<gpui_kit::gpui::RenderImage>>,
    pub(super) stt_downloading: Option<slackless_core::huddle::TranscriptionProvider>,
    pub(super) stt_download_status: Option<String>,
    pub(super) semantic_downloading: bool,
    pub(super) semantic_indexing: bool,
    pub(super) semantic_status_note: Option<String>,
    pub(super) semantic_snapshot: Option<SemanticStorageSnapshot>,
    pub(super) ai_base_url_input: Entity<TextareaState>,
    pub(super) ai_model_input: Entity<TextareaState>,
    pub(super) ai_api_key_input: Entity<TextareaState>,
    /// Whether an AI API key is currently stored in the OS Keychain (`None` until probed off-thread).
    pub(super) ai_api_key_stored: Option<bool>,
    pub(super) ai_key_status_note: Option<String>,
    /// Monotonic generation guarding async Keychain probe/save/clear tasks against out-of-order writes.
    pub(super) ai_key_gen: u64,
    pub(super) ai_test_running: bool,
    pub(super) ai_test_status: Option<(bool, String)>,
    /// Result of the most recent "Verify integrity" run, keyed by workspace `TeamId`.
    pub(super) integrity_reports: std::collections::HashMap<
        slackless_core::types::TeamId,
        slackless_core::store::IntegrityReport,
    >,
    /// Workspaces with an in-flight "Verify integrity" run.
    pub(super) integrity_running: std::collections::HashSet<slackless_core::types::TeamId>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PreferencesEvent> for Preferences {}

impl Preferences {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Preferences> {
        cx.new(|cx| {
            let settings = Settings::default();
            let keywords = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("Add keywords, separated by commas")
                    .auto_grow(1, 3);
                state.set_value(settings.keywords.clone(), window, cx);
                state
            });
            let vip_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("Add VIP names, @handles, or user IDs (comma-separated)")
                    .auto_grow(1, 2);
                state.set_value(settings.vip_users.clone(), window, cx);
                state
            });
            let catchup_time_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("09:00")
                    .auto_grow(1, 1);
                state.set_value(settings.catchup_reminder_time.clone(), window, cx);
                state
            });
            let rule_name_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("e.g. Escalate SEV-1 in #incidents")
                    .auto_grow(1, 1)
            });
            let rule_users_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("@alice, U0123ABC (comma-separated)")
                    .auto_grow(1, 1)
            });
            let rule_channels_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("#incidents, C0123ABC (comma-separated)")
                    .auto_grow(1, 1)
            });
            let rule_keywords_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("sev-1, outage, rollback (comma-separated)")
                    .auto_grow(1, 1)
            });
            let rule_regex_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(r"Optional regex, e.g. prod.*(down|500)")
                    .auto_grow(1, 1)
            });
            let rule_topic_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Optional topic, e.g. database outage")
                    .auto_grow(1, 1)
            });
            let rule_test_sender = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("@alice")
                    .auto_grow(1, 1);
                state.set_value("@alice".to_owned(), window, cx);
                state
            });
            let rule_test_channel = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("#incidents")
                    .auto_grow(1, 1);
                state.set_value("#incidents".to_owned(), window, cx);
                state
            });
            let rule_test_message = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("Sample message text to test against this rule...")
                    .auto_grow(1, 2);
                state.set_value(
                    "SEV-1 prod database replication lag > 30s".to_owned(),
                    window,
                    cx,
                );
                state
            });
            let rule_editor = RuleEditorState {
                open: false,
                editing_id: None,
                action: slackless_core::rules::RuleAction::BreakThroughMute,
                only_threads: false,
                mentions_me: false,
                semantic_threshold_idx: 1,
                name: rule_name_input,
                from_users: rule_users_input,
                in_channels: rule_channels_input,
                keywords: rule_keywords_input,
                regex_pattern: rule_regex_input,
                semantic_topic: rule_topic_input,
                test_sender: rule_test_sender,
                test_channel: rule_test_channel,
                test_message: rule_test_message,
                test_is_thread: false,
                test_mentions_me: false,
                semantic_test_cache: None,
                semantic_test_loading: false,
                semantic_test_gen: 0,
                next_rule_seq: 1,
            };
            let excluded_channels = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("e.g. random, alerts, noisy-bot")
                    .auto_grow(1, 2);
                state.set_value(settings.excluded_channels.clone(), window, cx);
                state
            });
            let protocol_allowlist_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("https, http, mailto, slack")
                    .auto_grow(1, 1);
                state.set_value(settings.protocol_allowlist.clone(), window, cx);
                state
            });
            let proxy_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("http://127.0.0.1:8080 or socks5://127.0.0.1:1080")
                    .auto_grow(1, 1);
                state.set_value(settings.proxy_url.clone(), window, cx);
                state
            });
            let quick_reaction_inputs: [Entity<TextareaState>; 3] = std::array::from_fn(|ix| {
                let initial = settings.quick_reaction_emojis[ix].clone();
                let placeholder = crate::actions::DEFAULT_QUICK_REACTIONS[ix];
                cx.new(|cx| {
                    let mut state = TextareaState::new(window, cx)
                        .placeholder(placeholder)
                        .auto_grow(1, 1);
                    state.set_value(initial, window, cx);
                    state
                })
            });
            let custom_sidebar_hex = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("#3F0E40")
                    .auto_grow(1, 1);
                state.set_value("#3F0E40".to_owned(), window, cx);
                state
            });
            let custom_accent_hex = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("#1164A3")
                    .auto_grow(1, 1);
                state.set_value("#1164A3".to_owned(), window, cx);
                state
            });
            let slack_theme_import = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("#3F0E40,#350D36,#1164A3,#FFFFFF,...")
                    .auto_grow(1, 2)
            });
            let ai_base_url_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("http://localhost:11434/v1")
                    .auto_grow(1, 1);
                state.set_value(settings.ai_summary.base_url.clone(), window, cx);
                state
            });
            let ai_model_input = cx.new(|cx| {
                let mut state = TextareaState::new(window, cx)
                    .placeholder("llama3.2")
                    .auto_grow(1, 1);
                state.set_value(settings.ai_summary.model.clone(), window, cx);
                state
            });
            let ai_api_key_input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(
                        "Paste API key to store in Keychain (optional for Ollama/LM Studio)",
                    )
                    .auto_grow(1, 1)
            });
            let mut subscriptions = vec![
                cx.subscribe_in(
                    &keywords,
                    window,
                    |this: &mut Preferences, keywords, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.keywords = keywords.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &vip_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.vip_users = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &catchup_time_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.catchup_reminder_time =
                                field.read(cx).value().trim().to_owned();
                            cx.emit(PreferencesEvent::Changed);
                            cx.notify();
                        }
                    },
                ),
                cx.subscribe_in(
                    &excluded_channels,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.excluded_channels = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &protocol_allowlist_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.protocol_allowlist = field.read(cx).value().to_string();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &proxy_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.proxy_url = field.read(cx).value().trim().to_owned();
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &ai_base_url_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.ai_summary.base_url =
                                field.read(cx).value().trim().to_owned();
                            this.ai_test_status = None;
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
                cx.subscribe_in(
                    &ai_model_input,
                    window,
                    |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.settings.ai_summary.model =
                                field.read(cx).value().trim().to_owned();
                            this.ai_test_status = None;
                            cx.emit(PreferencesEvent::Changed);
                        }
                    },
                ),
            ];
            for (ix, input) in quick_reaction_inputs.iter().enumerate() {
                subscriptions.push(cx.subscribe_in(
                    input,
                    window,
                    move |this: &mut Preferences, field, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            let val = field.read(cx).value().trim().trim_matches(':').to_owned();
                            if this.settings.quick_reaction_emojis[ix] != val {
                                this.settings.quick_reaction_emojis[ix] = val;
                                cx.emit(PreferencesEvent::Changed);
                                cx.notify();
                            }
                        }
                    },
                ));
            }
            for editor_field in [
                &rule_editor.name,
                &rule_editor.from_users,
                &rule_editor.in_channels,
                &rule_editor.keywords,
                &rule_editor.regex_pattern,
                &rule_editor.test_sender,
                &rule_editor.test_channel,
            ] {
                subscriptions.push(cx.subscribe_in(
                    editor_field,
                    window,
                    |_this: &mut Preferences, _, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            cx.notify();
                        }
                    },
                ));
            }
            for semantic_field in [&rule_editor.semantic_topic, &rule_editor.test_message] {
                subscriptions.push(cx.subscribe_in(
                    semantic_field,
                    window,
                    |this: &mut Preferences, _, event: &InputEvent, _, cx| {
                        if let InputEvent::Change = event {
                            this.refresh_rule_editor_semantic_preview(cx);
                            cx.notify();
                        }
                    },
                ));
            }
            Preferences {
                section: Section::Notifications,
                settings,
                open_select: None,
                keywords,
                vip_input,
                catchup_time_input,
                rule_editor,
                excluded_channels,
                protocol_allowlist_input,
                proxy_input,
                quick_reaction_inputs,
                custom_sidebar_hex,
                custom_accent_hex,
                slack_theme_import,
                storage_stats: None,
                backfill_progress: None,
                cli_status_note: None,
                update_state: crate::updater::UpdateState::Idle,
                available_update: None,
                audio_devices: slackless_core::huddle::AudioDevices::default(),
                cameras: Vec::new(),
                audio_loading: false,
                audio_probe_gen: 0,
                mic_probe: None,
                mic_level: 0.0,
                camera_preview_on: false,
                camera_probe_gen: 0,
                camera_probe: None,
                camera_frame: None,
                stt_downloading: None,
                stt_download_status: None,
                semantic_downloading: false,
                semantic_indexing: false,
                semantic_status_note: None,
                semantic_snapshot: None,
                ai_base_url_input,
                ai_model_input,
                ai_api_key_input,
                ai_api_key_stored: None,
                ai_key_status_note: None,
                ai_key_gen: 0,
                ai_test_running: false,
                ai_test_status: None,
                integrity_reports: std::collections::HashMap::new(),
                integrity_running: std::collections::HashSet::new(),
                _subscriptions: subscriptions,
            }
        })
    }

    /// Update the local SQLite & media cache metrics shown in Preferences -> Advanced.
    pub fn set_storage_stats(&mut self, stats: Option<StorageInfo>, cx: &mut Context<Self>) {
        self.storage_stats = stats;
        cx.notify();
    }

    /// Snapshot of the latest local SQLite & media cache metrics (if loaded).
    pub fn storage_stats_snapshot(&self) -> Option<StorageInfo> {
        self.storage_stats.clone()
    }

    /// Update the cached off-thread semantic model & vector index snapshot.
    pub fn set_semantic_snapshot(
        &mut self,
        snapshot: SemanticStorageSnapshot,
        cx: &mut Context<Self>,
    ) {
        let ready = snapshot.setup.model_ready;
        self.semantic_snapshot = Some(snapshot);
        if ready && self.rule_editor.open {
            self.rule_editor.semantic_test_cache = None;
            self.refresh_rule_editor_semantic_preview(cx);
        }
        cx.notify();
    }

    /// Return the latest cached semantic model & vector index snapshot (zero UI-thread I/O).
    pub fn semantic_snapshot(&self) -> Option<SemanticStorageSnapshot> {
        self.semantic_snapshot.clone()
    }

    /// Refresh `semantic_snapshot` off the UI thread.
    pub fn refresh_semantic_snapshot(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let snap = cx
                .background_executor()
                .spawn(async move { SemanticStorageSnapshot::collect() })
                .await;
            let _ = this.update(cx, |prefs, cx| {
                prefs.set_semantic_snapshot(snap, cx);
            });
        })
        .detach();
    }

    /// Latest SQLite integrity check result and whether a check is currently running for `team`.
    pub fn integrity_status(
        &self,
        team: Option<&slackless_core::types::TeamId>,
    ) -> (Option<slackless_core::store::IntegrityReport>, bool) {
        let Some(team) = team else {
            return (None, false);
        };
        (
            self.integrity_reports.get(team).cloned(),
            self.integrity_running.contains(team),
        )
    }

    /// Mark an integrity check for `team` as started (`running == true`) or record its completion.
    pub fn set_integrity_status(
        &mut self,
        team: slackless_core::types::TeamId,
        report: Option<slackless_core::store::IntegrityReport>,
        running: bool,
        cx: &mut Context<Self>,
    ) {
        if running {
            self.integrity_running.insert(team.clone());
            if let Some(report) = report {
                self.integrity_reports.insert(team, report);
            }
        } else {
            self.integrity_running.remove(&team);
            match report {
                Some(report) => {
                    self.integrity_reports.insert(team, report);
                }
                None => {
                    self.integrity_reports.remove(&team);
                }
            }
        }
        cx.notify();
    }

    /// Update the automatic media cache retention policy (persisted via `Changed`).
    pub fn set_media_cache_policy(&mut self, max_mb: u32, max_days: u32, cx: &mut Context<Self>) {
        if (
            self.settings.media_cache_max_mb,
            self.settings.media_cache_max_days,
        ) != (max_mb, max_days)
        {
            self.settings.media_cache_max_mb = max_mb;
            self.settings.media_cache_max_days = max_days;
            self.changed(cx);
        }
    }

    /// Update the live workspace backfill progress shown in Preferences -> Advanced.
    pub fn set_backfill_progress(
        &mut self,
        progress: Option<slackless_core::WorkspaceBackfillProgress>,
        cx: &mut Context<Self>,
    ) {
        self.backfill_progress = progress;
        cx.notify();
    }

    /// Mirror the live auto-updater state into Preferences -> Advanced -> Software updates.
    pub fn set_update_state(
        &mut self,
        state: crate::updater::UpdateState,
        available: Option<crate::updater::ReleaseInfo>,
        cx: &mut Context<Self>,
    ) {
        self.update_state = state;
        self.available_update = available;
        cx.notify();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Restore persisted settings at launch without emitting `Changed`.
    pub fn load_settings(
        &mut self,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keywords = settings.keywords.clone();
        let vip_users = settings.vip_users.clone();
        let catchup_time = settings.catchup_reminder_time.clone();
        let excluded = settings.excluded_channels.clone();
        let protocols = settings.protocol_allowlist.clone();
        let proxy = settings.proxy_url.clone();
        let ai_base_url = settings.ai_summary.base_url.clone();
        let ai_model = settings.ai_summary.model.clone();
        let quick_emojis = settings.quick_reaction_emojis.clone();
        slackless_core::api::set_proxy_url(Some(proxy.clone()));
        slackless_core::api::set_net_log_enabled(
            settings.net_log_enabled || settings.debug_logging,
        );
        let rgb_pair = settings.palette.rgb_pair();
        self.settings = settings;
        self.keywords
            .update(cx, |state, cx| state.set_value(keywords, window, cx));
        self.vip_input
            .update(cx, |state, cx| state.set_value(vip_users, window, cx));
        self.catchup_time_input
            .update(cx, |state, cx| state.set_value(catchup_time, window, cx));
        self.excluded_channels
            .update(cx, |state, cx| state.set_value(excluded, window, cx));
        self.protocol_allowlist_input
            .update(cx, |state, cx| state.set_value(protocols, window, cx));
        self.proxy_input
            .update(cx, |state, cx| state.set_value(proxy, window, cx));
        self.ai_base_url_input
            .update(cx, |state, cx| state.set_value(ai_base_url, window, cx));
        self.ai_model_input
            .update(cx, |state, cx| state.set_value(ai_model, window, cx));
        for (ix, val) in quick_emojis.into_iter().enumerate() {
            self.quick_reaction_inputs[ix].update(cx, |state, cx| state.set_value(val, window, cx));
        }
        if let Some((sb, ac)) = rgb_pair {
            self.custom_sidebar_hex.update(cx, |state, cx| {
                state.set_value(theme::format_hex_rgb(sb), window, cx)
            });
            self.custom_accent_hex.update(cx, |state, cx| {
                state.set_value(theme::format_hex_rgb(ac), window, cx)
            });
        }
        cx.notify();
    }

    /// Toggle Stealth Mode directly (e.g. from the top-bar Stealth indicator pill).
    pub fn set_stealth_mode(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.settings.stealth_mode != enabled {
            self.settings.stealth_mode = enabled;
            self.changed(cx);
        }
    }

    /// Record the local calendar day (`"YYYY-MM-DD"`) on which the Daily Catch-Up reminder fired
    /// so it fires at most once per day (even across restarts).
    pub fn mark_catchup_reminder_fired(&mut self, day: String, cx: &mut Context<Self>) {
        if self.settings.catchup_reminder_last_fired_day.as_deref() != Some(day.as_str()) {
            self.settings.catchup_reminder_last_fired_day = Some(day);
            self.changed(cx);
        }
    }

    /// Compute the live semantic-topic preview score off the UI thread when the rule editor is
    /// open and has a non-empty `semantic_topic`.
    pub(super) fn refresh_rule_editor_semantic_preview(&mut self, cx: &mut Context<Self>) {
        if !self.rule_editor.open {
            self.rule_editor.semantic_test_loading = false;
            return;
        }
        let topic = self
            .rule_editor
            .semantic_topic
            .read(cx)
            .value()
            .trim()
            .to_owned();
        let text = self
            .rule_editor
            .test_message
            .read(cx)
            .value()
            .trim()
            .to_owned();
        if topic.is_empty() || text.is_empty() {
            self.rule_editor.semantic_test_gen = self.rule_editor.semantic_test_gen.wrapping_add(1);
            self.rule_editor.semantic_test_loading = false;
            self.rule_editor.semantic_test_cache = None;
            return;
        }
        if self
            .rule_editor
            .semantic_test_cache
            .as_ref()
            .is_some_and(|(t, top, _)| t == &text && top == &topic)
        {
            self.rule_editor.semantic_test_loading = false;
            return;
        }
        let model_ready = self
            .semantic_snapshot
            .as_ref()
            .is_some_and(|s| s.setup.model_ready);
        if !model_ready {
            self.rule_editor.semantic_test_loading = false;
            self.rule_editor.semantic_test_cache = Some((text, topic, None));
            return;
        }
        self.rule_editor.semantic_test_gen = self.rule_editor.semantic_test_gen.wrapping_add(1);
        let generation = self.rule_editor.semantic_test_gen;
        self.rule_editor.semantic_test_loading = true;
        cx.spawn(async move |this, cx| {
            let text_bg = text.clone();
            let topic_bg = topic.clone();
            let score = cx
                .background_executor()
                .spawn(async move { compute_local_semantic_similarity(&text_bg, &topic_bg) })
                .await;
            let _ = this.update(cx, |prefs, cx| {
                if prefs.rule_editor.semantic_test_gen != generation {
                    return;
                }
                prefs.rule_editor.semantic_test_loading = false;
                prefs.rule_editor.semantic_test_cache = Some((text, topic, score));
                cx.notify();
            });
        })
        .detach();
    }

    /// Save the active Smart Rule editor draft into `settings.smart_notification_rules`
    /// (updating an existing rule when editing, or appending a new one).
    pub fn save_rule_draft(&mut self, cx: &mut Context<Self>) -> bool {
        let existing_enabled = self
            .rule_editor
            .editing_id
            .as_deref()
            .and_then(|id| {
                self.settings
                    .smart_notification_rules
                    .iter()
                    .find(|r| r.id == id)
            })
            .map(|r| r.enabled)
            .unwrap_or(true);
        let id = self.rule_editor.editing_id.clone().unwrap_or_else(|| {
            let seq = self.rule_editor.next_rule_seq;
            self.rule_editor.next_rule_seq = seq.saturating_add(1);
            format!("rule-{}-{seq}", slackless_core::types::time::now_epoch_ms())
        });
        let draft = self
            .rule_editor
            .draft_rule(id.clone(), existing_enabled, cx);
        if !draft.has_any_condition() {
            return false;
        }
        let has_semantic_topic = draft
            .semantic_topic
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty());
        let model_ready = self
            .semantic_snapshot
            .as_ref()
            .is_some_and(|s| s.setup.model_ready);
        if has_semantic_topic && !model_ready {
            return false;
        }
        if let Some(pat) = draft.regex_pattern.as_deref()
            && slackless_core::rules::validate_regex(pat).is_err()
        {
            return false;
        }
        if let Some(slot) = self
            .settings
            .smart_notification_rules
            .iter_mut()
            .find(|r| r.id == id)
        {
            *slot = draft;
        } else {
            self.settings.smart_notification_rules.push(draft);
        }
        self.rule_editor.open = false;
        self.rule_editor.editing_id = None;
        self.rule_editor.semantic_test_loading = false;
        self.changed(cx);
        true
    }

    /// Toggle a smart rule's `enabled` flag by ID.
    pub fn toggle_rule_enabled(&mut self, rule_id: &str, cx: &mut Context<Self>) {
        if let Some(rule) = self
            .settings
            .smart_notification_rules
            .iter_mut()
            .find(|r| r.id == rule_id)
        {
            rule.enabled = !rule.enabled;
            self.changed(cx);
        }
    }

    /// Remove a smart rule by ID.
    pub fn delete_rule(&mut self, rule_id: &str, cx: &mut Context<Self>) {
        let before = self.settings.smart_notification_rules.len();
        self.settings
            .smart_notification_rules
            .retain(|r| r.id != rule_id);
        if self.rule_editor.editing_id.as_deref() == Some(rule_id) {
            self.rule_editor.open = false;
            self.rule_editor.editing_id = None;
        }
        if self.settings.smart_notification_rules.len() != before {
            self.changed(cx);
        }
    }

    /// Select a huddle transcription provider index (`0`=Slack, `1`=Parakeet v3, `2`=Whisper Turbo).
    pub fn set_transcription_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.settings.huddle_transcription_provider != index {
            self.settings.huddle_transcription_provider = index;
            self.changed(cx);
        }
    }

    /// Jump to `section` (the DND menu opens Notifications).
    pub fn show(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        self.open_select = None;
        if section == Section::AudioVideo {
            self.refresh_audio_devices_and_probe(cx);
        } else {
            self.stop_mic_probe(cx);
        }
        if matches!(section, Section::Advanced | Section::Notifications) {
            self.refresh_semantic_snapshot(cx);
            self.refresh_ai_api_key_status(cx);
        }
        cx.notify();
    }

    /// Check off-thread whether an AI summary API key is stored in the OS Keychain.
    pub fn refresh_ai_api_key_status(&mut self, cx: &mut Context<Self>) {
        self.ai_key_gen = self.ai_key_gen.wrapping_add(1);
        let key_gen = self.ai_key_gen;
        #[cfg(test)]
        {
            let _ = (cx, key_gen);
        }
        #[cfg(not(test))]
        {
            cx.spawn(async move |this, cx| {
                let has_key = cx
                    .background_executor()
                    .spawn(async move {
                        slackless_core::auth::SecretStore::new()
                            .and_then(|s| s.has(slackless_core::auth::AI_SUMMARY_API_KEY_ACCOUNT))
                            .unwrap_or(false)
                    })
                    .await;
                let _ = this.update(cx, |prefs, cx| {
                    if prefs.ai_key_gen != key_gen {
                        return;
                    }
                    prefs.ai_api_key_stored = Some(has_key);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// Show zoom level `level` (Cmd+= / Cmd+- / Cmd+0, or the remembered one) and apply it.
    pub fn set_zoom(&mut self, level: usize, cx: &mut Context<Self>) {
        if self.settings.zoom != level {
            self.settings.zoom = level;
            cx.emit(PreferencesEvent::Changed);
            cx.notify();
        }
    }

    /// Start on the remembered Color Mode and Font and the color theme Slack stores for the
    /// workspace, so the pane shows the look already in effect and a later change applies
    /// over it.
    pub fn set_appearance(
        &mut self,
        mode: AppearanceMode,
        palette: Palette,
        font: usize,
        cx: &mut Context<Self>,
    ) {
        let settings = &mut self.settings;
        if (settings.mode, settings.palette, settings.font) != (mode, palette, font) {
            (settings.mode, settings.palette, settings.font) = (mode, palette, font);
            cx.notify();
        }
    }

    pub(super) fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(PreferencesEvent::Changed);
        cx.notify();
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = self.render_body(cx);
        let nav = self.nav(cx);
        let title = self.section.label();
        h_flex()
            .h_full()
            .min_h(px(560.))
            .w_full()
            .items_start()
            .child(nav)
            .child(
                v_flex()
                    .id("prefs-body")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .pb_4()
                            .child(title),
                    )
                    .child(body),
            )
    }
}
