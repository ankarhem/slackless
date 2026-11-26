//! Slack's Preferences window: a left section nav and the selected section's controls on the
//! right, covering Notifications, Navigation, Home, Appearance & Themes, Messages & media,
//! Language & region, Audio & video, Accessibility, Mark as read, and Advanced.
//!
//! Every control writes the in-memory [`Settings`] and emits [`PreferencesEvent::Changed`].
//! Persisted schema, GPUI global caches, media probes, standalone window hosting, and per-section
//! renderers live in focused submodules and are re-exported here.

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
pub use settings::{
    AppearanceMode, HOURS, MESSAGE_VERBOSITY, MOBILE_HANDOFF, NOTIFY_LEVELS, SCHEDULE_DAYS,
    SOUND_FILES, Section, Settings, StorageInfo, TRANSCRIPTION_PROVIDERS,
    format_accessible_message, format_cross_workspace_summary,
};
pub(crate) use slackless_core::types::time::format_bytes;
pub use window::open_preferences_window;

pub enum PreferencesEvent {
    /// Some setting changed; read [`Preferences::settings`].
    Changed,
    /// User asked to reset the local SQLite & media cache in Preferences -> Advanced.
    ResetCache,
    /// User asked to compact/VACUUM the active workspace's local SQLite store.
    CompactStorage,
    /// User asked to prune cached media/temp files on disk.
    PruneMediaCache,
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
}

impl SemanticStorageSnapshot {
    /// Collect current model & index status (blocking — call on a background executor).
    pub fn collect() -> Self {
        let model = slackless_core::embeddings::EmbeddingModel::default();
        Self {
            setup: slackless_core::embeddings::detect_embedding_model_setup(model),
            stats: slackless_core::semantic_index::default_semantic_index_stats(),
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
            Preferences {
                section: Section::Notifications,
                settings,
                open_select: None,
                keywords,
                vip_input,
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
        self.semantic_snapshot = Some(snapshot);
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
        let excluded = settings.excluded_channels.clone();
        let protocols = settings.protocol_allowlist.clone();
        let proxy = settings.proxy_url.clone();
        let quick_emojis = settings.quick_reaction_emojis.clone();
        slackless_core::api::set_proxy_url(Some(proxy.clone()));
        slackless_core::api::set_net_log_enabled(settings.net_log_enabled);
        let rgb_pair = settings.palette.rgb_pair();
        self.settings = settings;
        self.keywords
            .update(cx, |state, cx| state.set_value(keywords, window, cx));
        self.vip_input
            .update(cx, |state, cx| state.set_value(vip_users, window, cx));
        self.excluded_channels
            .update(cx, |state, cx| state.set_value(excluded, window, cx));
        self.protocol_allowlist_input
            .update(cx, |state, cx| state.set_value(protocols, window, cx));
        self.proxy_input
            .update(cx, |state, cx| state.set_value(proxy, window, cx));
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
        if section == Section::Advanced {
            self.refresh_semantic_snapshot(cx);
        }
        cx.notify();
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
