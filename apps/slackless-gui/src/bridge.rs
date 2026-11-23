//! Bridge between the GUI (smol, GPUI's executor) and slackless-core (tokio on its own thread),
//! following the verified pattern in `docs/research/gpui-kit-guide.md` section 13.
//!
//! Live mode ([`CoreBridge::start`]) opens the signed-in session's local store and hands both to
//! [`slackless_core::Client::start`], which owns the dedicated OS thread running the
//! tokio runtime. The core talks to this process over runtime-agnostic
//! `futures::channel::mpsc` channels: [`Command`]s in, [`Event`]s out. Incoming events are
//! drained into gpui by a `cx.spawn` task and re-emitted from an internal [`EventHub`] entity,
//! so views subscribe with gpui's native `subscribe` and always run on the UI thread.
//!
//! Fixture mode ([`CoreBridge::offline`]) has no core: [`CoreBridge::send`] drops commands and
//! the views apply their changes locally, so the app renders without Slack credentials or a
//! network connection.
//!
//! One deviation from the guide's drain snippet: it awaits the event stream with
//! `futures::StreamExt::next`, but the `futures` crate is not a GUI dependency (only
//! `slackless-core` may name its types). The drain therefore polls the receiver's inherent
//! `try_recv` between `background_executor().timer` ticks; when the core's command receiver is
//! gone (`sender.is_closed()`), the channel is drained and the loop ends.

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::gpui::{self, AppContext as _, Context, Entity, EventEmitter, Subscription, Window};
use slackless_core::api::ConversationCounts;
use slackless_core::auth::Session;
use slackless_core::store::Store;
use slackless_core::types::{ChannelId, Message, TeamId, Ts};
use slackless_core::{Client, Command, Event, SharedStore};

use crate::fixtures::{self, Fixtures};
use crate::media::CoreHttp;

/// `serde_json` through gpui's re-export (see `crate::blockkit`).
use crate::blockkit::serde_json;
use crate::preferences::{AppearanceMode, Settings};
use crate::theme;

/// One persisted composer draft in the workspace's local store.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SavedDraft {
    pub channel: ChannelId,
    pub label: String,
    pub text: String,
    pub at: u64,
}

/// Persisted per-section sidebar filter and sort preferences for one workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SectionPrefs {
    #[serde(default)]
    pub filters: HashMap<String, crate::sidebar_menu::SectionFilter>,
    #[serde(default)]
    pub sorts: HashMap<String, crate::sidebar_menu::SectionSort>,
}

/// Idle wait between event polls while the core is running. The guide's pattern would await
/// the stream directly; without a `futures` dependency a short bounded poll keeps events at
/// sub-frame latency without spinning the executor.
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Newest messages of a conversation the message list shows in live mode.
const HISTORY_LIMIT: usize = 500;

/// The store's local-state key for the last open conversation.
const LAST_CONVERSATION: &str = "last_conversation";

/// The store's local-state key for per-section sidebar filter and sort choices.
const SECTION_PREFS: &str = "section_prefs";

/// The store's local-state key for the window's frame (AppKit restorable state, hex-encoded).
const WINDOW_STATE: &str = "window_state";

/// The store's local-state key for the Color Mode chosen in Preferences.
const COLOR_MODE: &str = "color_mode";

/// The store's local-state key for the search pill's recent searches.
const SEARCH_HISTORY: &str = "search_history";

/// The store's local-state key for the search panel's history toggle.
const SEARCH_HISTORY_SHOWN: &str = "search_history_shown";

/// The store's local-state key for the search panel's filter toggle.
const SEARCH_FILTERS_SHOWN: &str = "search_filters_shown";

/// The store's local-state key for the recently visited conversations (the search menu's
/// "Recent places" and the history clock's menu).
const RECENT_PLACES: &str = "recent_places";

/// The store's local-state key for the Font chosen in Preferences (its family name).
const FONT: &str = "font";

/// The store's local-state key for the zoom chosen in Preferences (its level).
const ZOOM: &str = "zoom";

/// The store's local-state key for all Preferences settings (JSON).
const SETTINGS: &str = "preferences_settings";

/// The store's local-state key for per-conversation composer drafts (JSON).
const DRAFTS: &str = "drafts";

/// The store's local-state key for recently/frequently used emoji shortcodes (JSON).
const FREQUENT_EMOJI: &str = "frequent_emoji";

/// The store's local-state key for the chosen emoji skin tone (1..=6).
const EMOJI_TONE: &str = "emoji_tone";

/// The store's local-state key for the user's preferred workspace rail order (JSON array of TeamId strings).
const WORKSPACE_ORDER: &str = "workspace_order";

/// Receives core events and re-emits them to subscribed views on the UI thread. The bridge
/// emits through this entity so subscribers can use gpui's native `subscribe` machinery.
struct EventHub;

impl EventEmitter<Event> for EventHub {}

pub struct CoreBridge {
    /// Live mode: forwards commands into the core's command loop. Fixture mode: `None`.
    /// The `futures::channel::mpsc::UnboundedSender` it captures is not nameable here, so it
    /// is wrapped in a closure instead of a field.
    command_sink: Option<Arc<dyn Fn(Command) + Send + Sync>>,
    /// Live mode: the store the core keeps current; views re-read it to render. Fixture
    /// mode: `None`.
    store: Option<SharedStore>,
    /// Live mode: the signed-in session the core runs with.
    session: Option<Session>,
    /// Live mode: authenticated HTTP fetcher for remote images and file downloads.
    fetcher: Option<slackless_core::Fetcher>,
    /// Created by [`CoreBridge::start`], or lazily by [`CoreBridge::subscribe`] in fixture
    /// mode (where nothing will ever emit on it).
    hub: Option<Entity<EventHub>>,
}

impl CoreBridge {
    /// A bridge with no core behind it (fixture mode).
    pub fn offline() -> Self {
        Self {
            command_sink: None,
            store: None,
            session: None,
            fetcher: None,
            hub: None,
        }
    }

    /// Starts the core live for `session`: opens its durable store and starts slackless-core on
    /// its dedicated tokio thread (guide section 13). Events flow into gpui through the drain
    /// task; views receive them via [`CoreBridge::subscribe_in`]. `None` when the store or the
    /// core cannot be started (the reason is logged).
    pub fn start(session: Session, cx: &mut gpui::App) -> Option<Self> {
        let store = open_store(&session)?;
        let handle = match Client::start(session.clone(), store) {
            Ok(handle) => handle,
            Err(e) => {
                eprintln!(
                    "slackless: could not start the core ({e}); running without a core connection"
                );
                return None;
            }
        };
        let hub = cx.new(|_| EventHub);
        let fetcher = handle.fetcher();
        let mut bridge = Self {
            command_sink: None,
            store: None,
            session: Some(session),
            fetcher: Some(fetcher),
            hub: Some(hub.clone()),
        };

        let (commands, mut events, store) = handle.into_parts();
        bridge.store = Some(store);
        let sender = commands.clone();
        bridge.command_sink = Some(Arc::new(move |command| {
            if let Err(e) = commands.unbounded_send(command) {
                eprintln!("slackless: core has stopped; command dropped ({e})");
            }
        }));

        // Detached: dropping a gpui task cancels it, and the drain must outlive this call.
        cx.spawn(async move |cx| {
            loop {
                match events.try_recv() {
                    Ok(event) => cx.update_entity(&hub, |_, cx| cx.emit(event)),
                    Err(_) => {
                        // The core's command loop ended (it drains its command channel last),
                        // so the event channel is closed too once buffered events are gone.
                        if sender.is_closed() {
                            break;
                        }
                        cx.background_executor().timer(EVENT_POLL_INTERVAL).await;
                    }
                }
            }
        })
        .detach();

        Some(bridge)
    }

    /// Route GPUI remote image fetches (`img(url)`) and file downloads through this
    /// workspace's authenticated HTTP fetcher. Call at launch for workspace 0 and whenever
    /// switching active live workspaces.
    pub fn activate_media(&self, cx: &mut gpui::App) {
        if let Some(fetcher) = &self.fetcher {
            cx.set_http_client(Arc::new(CoreHttp(fetcher.clone())));
            crate::media::enable_downloads(fetcher.clone(), cx);
        }
    }

    /// Live mode: the signed-in workspace as the store has it now, with Slack's latest read
    /// state (`counts`). `None` offline.
    pub fn snapshot(&self, counts: &HashMap<ChannelId, ConversationCounts>) -> Option<Fixtures> {
        let session = self.session.as_ref()?;
        let store = self.store.as_ref()?.lock().ok()?;
        Some(fixtures::live(session, &store, counts))
    }

    /// Live mode: a read of `channel`'s stored history (oldest first, without deleted
    /// messages) to run off the UI thread: the newest `HISTORY_LIMIT` messages, or the
    /// `HISTORY_LIMIT` before `before` when paging older on scroll-back. `None` offline.
    pub fn history(
        &self,
        channel: ChannelId,
        before: Option<Ts>,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.channel_history(&team, &channel, before.as_ref(), HISTORY_LIMIT) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|row| !row.deleted)
                    .map(|row| row.message)
                    .collect(),
                Err(e) => {
                    eprintln!("slackless: could not read {channel} from the local store ({e})");
                    Vec::new()
                }
            }
        })
    }

    /// Live mode: a read of `channel`'s newest undeleted top-level message (the DM list's
    /// preview) to run off the UI thread. `None` offline.
    pub fn latest(
        &self,
        channel: ChannelId,
    ) -> Option<impl Future<Output = Option<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let store = store.lock().ok()?;
            match store.latest_messages(&team, std::slice::from_ref(&channel)) {
                Ok(mut latest) => latest.remove(&channel).map(|stored| stored.message),
                Err(e) => {
                    eprintln!("slackless: could not read {channel}'s latest message ({e})");
                    None
                }
            }
        })
    }

    /// Live mode: a read of up to `limit` newest undeleted messages authored by `user` across
    /// all stored conversations (for Drafts & sent -> Sent). `None` offline.
    pub fn recent_sent(
        &self,
        user: slackless_core::types::UserId,
        limit: usize,
    ) -> Option<impl Future<Output = Vec<slackless_core::store::StoredMessage>> + Send + 'static>
    {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            store
                .recent_messages_by_user(&team, &user, limit)
                .unwrap_or_default()
        })
    }

    /// Live mode: a read of one thread's stored history (root + replies, oldest first,
    /// without deleted messages) to run off the UI thread. `None` offline.
    pub fn thread(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
    ) -> Option<impl Future<Output = Vec<Message>> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(store) = store.lock() else {
                return Vec::new();
            };
            match store.thread_replies(&team, &channel, &thread_ts) {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|row| !row.deleted)
                    .map(|row| row.message)
                    .collect(),
                Err(e) => {
                    eprintln!(
                        "slackless: could not read the thread {thread_ts} of {channel} from the local store ({e})"
                    );
                    Vec::new()
                }
            }
        })
    }

    /// Forward a command to the core. Offline, commands are dropped by design.
    pub fn send(&self, command: Command) {
        if let Some(sink) = &self.command_sink {
            sink(command);
        }
    }

    /// Clone a lightweight command forwarder for popped-out secondary GPUI windows.
    pub fn forwarder(&self) -> Arc<dyn Fn(Command) + Send + Sync> {
        self.command_sink
            .clone()
            .unwrap_or_else(|| Arc::new(|_| {}))
    }

    /// The store the core keeps current (live mode); views re-read it to render. `None` in
    /// fixture mode.
    pub fn store(&self) -> Option<&SharedStore> {
        self.store.as_ref()
    }

    pub fn team_id(&self) -> Option<&slackless_core::types::TeamId> {
        self.session.as_ref().map(|s| &s.team_id)
    }

    /// The conversation last open in this workspace, restored at launch like Slack. `None`
    /// offline or before the first one.
    pub fn last_conversation(&self) -> Option<ChannelId> {
        let id = self.local(LAST_CONVERSATION)?;
        Some(ChannelId::from(id.as_str()))
    }

    /// Live mode: a write remembering `channel` as the open conversation, to run off the UI
    /// thread. `None` offline.
    pub fn remember_conversation(
        &self,
        channel: ChannelId,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            LAST_CONVERSATION,
            channel.as_str().to_owned(),
            "open conversation",
        )
    }

    /// The window frame saved by [`Self::remember_window_state`], so the window reopens where
    /// and how it was left, like Slack's. `None` offline or before the first save.
    pub fn window_state(&self) -> Option<Vec<u8>> {
        let hex = self.local(WINDOW_STATE)?;
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
            .collect()
    }

    /// Live mode: a write remembering the window's native state (`Window::native_window_state`),
    /// to run off the UI thread. `None` offline.
    pub fn remember_window_state(
        &self,
        state: Vec<u8>,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let hex = state.iter().map(|byte| format!("{byte:02x}")).collect();
        self.write_local(WINDOW_STATE, hex, "window frame")
    }

    /// The Color Mode chosen in Preferences, which Slack keeps on the client rather than in the
    /// workspace prefs; `System` until one is chosen, following the computer as the user's
    /// Slack does. `None` offline: the sample workspace keeps its dark reference look.
    pub fn color_mode(&self) -> Option<AppearanceMode> {
        self.store.as_ref()?;
        let saved = self.local(COLOR_MODE);
        Some(
            saved
                .as_deref()
                .and_then(AppearanceMode::from_key)
                .unwrap_or(AppearanceMode::System),
        )
    }

    /// Live mode: a write remembering `mode` as the Color Mode, to run off the UI thread.
    /// `None` offline.
    pub fn remember_color_mode(
        &self,
        mode: AppearanceMode,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(COLOR_MODE, mode.key().to_owned(), "color mode")
    }

    /// The search pill's recent searches, newest first, as last recorded; empty until the
    /// first one. `None` offline.
    pub fn search_history(&self) -> Option<Vec<String>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(SEARCH_HISTORY)?).ok()
    }

    /// Live mode: a write remembering the search history, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_history(
        &self,
        queries: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SEARCH_HISTORY,
            serde_json::to_string(queries).ok()?,
            "search history",
        )
    }

    /// The search panel's history toggle, as last chosen; `None` offline or before the
    /// first choice (the panel then starts with the toggle off, like Slack's).
    pub fn search_history_shown(&self) -> Option<bool> {
        self.store.as_ref()?;
        self.local(SEARCH_HISTORY_SHOWN)?.parse().ok()
    }

    /// Live mode: a write remembering the history toggle, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_history_shown(
        &self,
        on: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(SEARCH_HISTORY_SHOWN, on.to_string(), "history toggle")
    }

    /// The search panel's filter toggle, as last chosen; `None` offline or before the first
    /// choice.
    pub fn search_filters_shown(&self) -> Option<bool> {
        self.store.as_ref()?;
        self.local(SEARCH_FILTERS_SHOWN)?.parse().ok()
    }

    /// Live mode: a write remembering the filter toggle, to run off the UI thread. `None`
    /// offline.
    pub fn remember_search_filters_shown(
        &self,
        on: bool,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(SEARCH_FILTERS_SHOWN, on.to_string(), "filter toggle")
    }

    /// The recently visited conversations, newest first, as last recorded. `None` offline.
    pub fn recent_places(&self) -> Option<Vec<ChannelId>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(RECENT_PLACES)?).ok()
    }

    /// Live mode: a write remembering the recently visited conversations, to run off the UI
    /// thread. `None` offline.
    pub fn remember_recent_places(
        &self,
        ids: &[ChannelId],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            RECENT_PLACES,
            serde_json::to_string(ids).ok()?,
            "recent places",
        )
    }

    /// The Font chosen in Preferences, an index into [`theme::FONT_LABELS`]; `None` offline
    /// or before one is chosen (Lato, Slack's default, applies).
    pub fn font(&self) -> Option<usize> {
        let family = self.local(FONT)?;
        theme::FONT_FAMILIES
            .iter()
            .position(|known| *known == family)
    }

    /// Live mode: a write remembering [`theme::FONT_LABELS`]`[font]` as the Font, to run off
    /// the UI thread. `None` offline.
    pub fn remember_font(&self, font: usize) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(FONT, theme::FONT_FAMILIES[font].to_owned(), "font")
    }

    /// The zoom chosen in Preferences, a level of [`crate::zoom::FACTORS`]; `None` offline or
    /// before one is chosen (100% applies).
    pub fn zoom(&self) -> Option<usize> {
        let level: usize = self.local(ZOOM)?.parse().ok()?;
        (level < crate::zoom::FACTORS.len()).then_some(level)
    }

    /// Live mode: a write remembering zoom level `level`, to run off the UI thread. `None`
    /// offline.
    pub fn remember_zoom(&self, level: usize) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(ZOOM, level.to_string(), "zoom")
    }

    /// The Preferences settings saved locally; `None` offline or before the first save.
    pub fn settings(&self) -> Option<Settings> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(SETTINGS)?).ok()
    }

    /// Live mode: persist Preferences `settings` locally, off the UI thread. `None` offline.
    pub fn remember_settings(
        &self,
        settings: &Settings,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SETTINGS,
            serde_json::to_string(settings).ok()?,
            "preferences",
        )
    }

    /// Saved per-conversation composer drafts for this workspace; `None` offline or unset.
    pub fn drafts(&self) -> Option<Vec<SavedDraft>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(DRAFTS)?).ok()
    }

    /// Live mode: persist per-conversation composer drafts off the UI thread. `None` offline.
    pub fn remember_drafts(
        &self,
        drafts: &[SavedDraft],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(DRAFTS, serde_json::to_string(drafts).ok()?, "drafts")
    }

    /// Saved frequently used emoji shortcodes; `None` offline or before the first pick.
    pub fn frequent_emoji(&self) -> Option<Vec<String>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(FREQUENT_EMOJI)?).ok()
    }

    /// Live mode: persist frequently used emoji shortcodes off the UI thread. `None` offline.
    pub fn remember_frequent_emoji(
        &self,
        list: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            FREQUENT_EMOJI,
            serde_json::to_string(list).ok()?,
            "frequent emoji",
        )
    }

    /// Saved emoji skin tone (1..=6); `None` offline or before the user picks one.
    pub fn emoji_tone(&self) -> Option<u8> {
        self.store.as_ref()?;
        let tone: u8 = self.local(EMOJI_TONE)?.parse().ok()?;
        (1..=6).contains(&tone).then_some(tone)
    }

    /// Live mode: persist emoji skin tone (1..=6) off the UI thread. `None` offline.
    pub fn remember_emoji_tone(
        &self,
        tone: u8,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(EMOJI_TONE, tone.to_string(), "emoji skin tone")
    }

    /// Saved workspace switcher order (`TeamId` strings); `None` offline or unset.
    pub fn workspace_order(&self) -> Option<Vec<String>> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(WORKSPACE_ORDER)?).ok()
    }

    /// Live mode: persist workspace switcher order off the UI thread. `None` offline.
    pub fn remember_workspace_order(
        &self,
        team_ids: &[String],
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            WORKSPACE_ORDER,
            serde_json::to_string(team_ids).ok()?,
            "workspace order",
        )
    }

    /// Saved per-section sidebar filter and sort choices; `None` offline or unset.
    pub fn section_prefs(&self) -> Option<SectionPrefs> {
        self.store.as_ref()?;
        serde_json::from_str(&self.local(SECTION_PREFS)?).ok()
    }

    /// Live mode: persist per-section sidebar filter & sort choices off the UI thread.
    pub fn remember_section_prefs(
        &self,
        prefs: &SectionPrefs,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        self.write_local(
            SECTION_PREFS,
            serde_json::to_string(prefs).ok()?,
            "sidebar section preferences",
        )
    }

    /// This workspace's stored local-state `key`; `None` offline or when unset.
    fn local(&self, key: &str) -> Option<String> {
        let team = &self.session.as_ref()?.team_id;
        let store = self.store.as_ref()?.lock().ok()?;
        store.local(team, key).ok().flatten()
    }

    /// A write storing `value` under this workspace's local-state `key`, to run off the UI
    /// thread; a failure is logged naming `what` was not remembered. `None` offline.
    fn write_local(
        &self,
        key: &'static str,
        value: String,
        what: &'static str,
    ) -> Option<impl Future<Output = ()> + Send + 'static> {
        let team = self.session.as_ref()?.team_id.clone();
        let store = self.store.clone()?;
        Some(async move {
            let Ok(mut store) = store.lock() else {
                return;
            };
            if let Err(e) = store.set_local(&team, key, &value) {
                eprintln!("slackless: could not remember the {what} ({e})");
            }
        })
    }

    /// Subscribe to the core's [`Event`]s. The handler runs on the UI thread whenever the
    /// drain task forwards an event; keep the returned [`Subscription`] alive or the
    /// subscription dies (guide section 2).
    pub fn subscribe_in<T: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<T>,
        mut on_event: impl FnMut(&mut T, &Event, &mut Window, &mut Context<T>) + 'static,
    ) -> Subscription {
        let hub = self.hub.get_or_insert_with(|| cx.new(|_| EventHub));
        cx.subscribe_in(hub, window, move |this, _, event: &Event, window, cx| {
            on_event(this, event, window, cx);
        })
    }
}

/// Opens the durable store for a session. Falls back to an in-memory store when no home
/// directory is available; returns `None` only if no store can be opened at all.
fn open_store(session: &Session) -> Option<Store> {
    match default_store_path(&session.team_id) {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match Store::open(&path) {
                Ok(store) => Some(store),
                Err(e) => {
                    eprintln!(
                        "slackless: could not open the local store at {} ({e}); running without a core connection",
                        path.display()
                    );
                    None
                }
            }
        }
        None => match Store::open_in_memory() {
            Ok(store) => Some(store),
            Err(e) => {
                eprintln!(
                    "slackless: could not open an in-memory store ({e}); running without a core connection"
                );
                None
            }
        },
    }
}

/// Directory holding per-workspace SQLite stores.
pub fn store_dir() -> Option<PathBuf> {
    let mut path = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        path.push("Library/Application Support/slackless");
    } else {
        path.push(".local/share/slackless");
    }
    Some(path)
}

/// The durable store lives under the platform's application-data directory, one database per
/// workspace (the store location stays a front-end decision, per slackless-core's docs).
fn default_store_path(team_id: &TeamId) -> Option<PathBuf> {
    let mut path = store_dir()?;
    path.push(format!("{team_id}.sqlite"));
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidebar_menu::{SectionFilter, SectionSort};

    #[test]
    fn saved_draft_and_section_prefs_json_round_trip() {
        let draft = SavedDraft {
            channel: ChannelId::from("C123"),
            label: "# general".into(),
            text: "Hello *world*".into(),
            at: 1_760_000_000,
        };
        let encoded = serde_json::to_string(&draft).unwrap();
        let decoded: SavedDraft = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, draft);

        let mut prefs = SectionPrefs::default();
        prefs
            .filters
            .insert("channels".into(), SectionFilter::Unreads);
        prefs.sorts.insert("channels".into(), SectionSort::Priority);
        let encoded = serde_json::to_string(&prefs).unwrap();
        let decoded: SectionPrefs = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, prefs);

        let empty: SectionPrefs = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SectionPrefs::default());
    }

    #[test]
    fn offline_bridge_is_a_safe_noop() {
        let bridge = CoreBridge::offline();
        assert!(bridge.store().is_none());
        assert!(bridge.snapshot(&HashMap::new()).is_none());
        assert!(bridge.history(ChannelId::from("C1"), None).is_none());
        assert!(bridge.settings().is_none());
        assert!(bridge.drafts().is_none());
        assert!(bridge.section_prefs().is_none());
        bridge.send(Command::MarkAllRead);
    }
}
