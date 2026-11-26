//! Cmd+K quick switcher (mounted in a gpui-kit dialog by the root): a filter field over every
//! joined channel and DM. Typing filters by name (prefix matches first), Up/Down move the
//! selection, Enter or a click opens the conversation.
//!
//! `QuickSwitcher::new(Arc<Fixtures>, &mut Window, &mut App) -> Entity<QuickSwitcher>`
//! `QuickSwitcher::open(&mut self, &mut Window, &mut Context<Self>)` (reset + focus)
//! emits `SwitcherEvent::Open(ChannelId)`

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window, div, px,
};
use slackless_core::api::EdgeSearchResults;
use slackless_core::types::{ChannelId, Conversation, ConversationKind, User, UserId};

use crate::avatars::{conversation_avatar, user_chip};
use crate::fixtures::Fixtures;

/// Most rows listed at once.
const MAX_ROWS: usize = 12;

pub enum SwitcherEvent {
    Open(ChannelId),
    /// An unjoined channel returned by remote Edge search; open/preview it in the workspace.
    OpenRemoteChannel(Box<Conversation>),
    /// A colleague with no open DM yet; the root opens or creates the DM.
    OpenUser(UserId),
    /// Ask the live backend for remote `edgeapi` people & channel matches.
    SearchRemote(String),
}

#[derive(Clone)]
enum Target {
    Channel(ChannelId),
    RemoteChannel(Box<Conversation>),
    User(UserId),
    RemoteUser(Box<User>),
}

/// What a [`Candidate`] opens, by id or position, so listing every candidate copies no
/// conversation or person.
enum Pick {
    Conversation(ChannelId),
    /// An unjoined channel of the snapshot's directory.
    Directory(usize),
    Person(UserId),
    RemoteChannel(usize),
    RemoteUser(usize),
}

/// A conversation or person the filter can match, lowercased once.
struct Candidate {
    pick: Pick,
    label: String,
    /// `label` lowercased: what rows of one match quality and frecency sort by.
    sort_key: String,
    /// Where the matched name starts in `sort_key` (past a channel's "# ").
    name_at: usize,
    /// A person's handle, lowercased: they match by it too.
    handle: Option<String>,
    /// Conversations, people, unjoined channels, remote people: the order of rows that tie.
    group: u8,
    /// A remote Edge result, listed even when only Slack matched it.
    remote: bool,
}

impl Candidate {
    fn new(pick: Pick, label: String, handle: Option<&str>, group: u8, remote: bool) -> Self {
        let sort_key = label.to_lowercase();
        let name_at = sort_key.len() - sort_key.trim_start_matches("# ").len();
        Self {
            pick,
            label,
            sort_key,
            name_at,
            handle: handle.map(str::to_lowercase),
            group,
            remote,
        }
    }
}

pub struct QuickSwitcher {
    fixtures: Arc<Fixtures>,
    input: Entity<TextareaState>,
    query: String,
    selected: usize,
    /// What the filter matches in `fixtures`, built on the first match after a snapshot.
    candidates: Option<Rc<[Candidate]>>,
    /// The rows for the current query, ranked once per change of query, snapshot, frecency or
    /// remote results.
    rows: Option<Rc<[(Target, String)]>>,
    /// Recently visited conversation ids (newest first) for frecency ordering.
    recent: Vec<ChannelId>,
    /// Visit count per conversation id for frecency tie-breaking.
    visits: HashMap<ChannelId, u32>,
    /// Remote Edge search matches for `remote_query`.
    remote_query: String,
    remote_channels: Vec<Conversation>,
    remote_users: Vec<User>,
    remote_debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SwitcherEvent> for QuickSwitcher {}

impl QuickSwitcher {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<QuickSwitcher> {
        cx.new(|cx| {
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Where would you like to go?")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &input,
                window,
                |this: &mut QuickSwitcher, input, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        this.set_query(input.read(cx).value().trim().to_lowercase());
                        this.schedule_remote_search(cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.confirm(cx),
                    _ => {}
                },
            )];
            QuickSwitcher {
                fixtures,
                input,
                query: String::new(),
                selected: 0,
                candidates: None,
                rows: None,
                recent: Vec::new(),
                visits: HashMap::new(),
                remote_query: String::new(),
                remote_channels: Vec::new(),
                remote_users: Vec::new(),
                remote_debounce: None,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Live refresh: match the query against the conversations in `fixtures`.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.candidates = None;
        self.rows = None;
        cx.notify();
    }

    /// Supply the workspace's recent conversation order and visit counts for frecency ranking.
    pub fn set_frecency(
        &mut self,
        recent: Vec<ChannelId>,
        visits: HashMap<ChannelId, u32>,
        cx: &mut Context<Self>,
    ) {
        self.recent = recent;
        self.visits = visits;
        self.rows = None;
        cx.notify();
    }

    /// Receive remote Edge API results for `query`.
    pub fn set_remote_results(
        &mut self,
        query: &str,
        results: EdgeSearchResults,
        cx: &mut Context<Self>,
    ) {
        if query.trim().to_lowercase() != self.query {
            return;
        }
        self.remote_query = self.query.clone();
        self.remote_channels = results.channels;
        self.remote_users = results.users;
        self.rows = None;
        cx.notify();
    }

    /// Filter by `query` (lowercased), from the first row.
    fn set_query(&mut self, query: String) {
        self.query = query;
        self.selected = 0;
        self.rows = None;
    }

    /// Clear the filter and focus the field; call when the dialog opens.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_query(String::new());
        self.remote_query.clear();
        self.remote_channels.clear();
        self.remote_users.clear();
        self.remote_debounce = None;
        self.input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn schedule_remote_search(&mut self, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.remote_debounce = None;
            return;
        }
        let query = self.query.clone();
        if query.chars().count() < 2 {
            self.remote_debounce = None;
            self.remote_query.clear();
            self.remote_channels.clear();
            self.remote_users.clear();
            return;
        }
        self.remote_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(180))
                .await;
            let _ = this.update(cx, |switcher, cx| {
                if switcher.query == query {
                    cx.emit(SwitcherEvent::SearchRemote(query));
                }
            });
        }));
    }

    /// Frecency bonus for `channel`: combines recency position in `self.recent` with visit count.
    fn frecency_score(&self, channel: &ChannelId) -> u32 {
        let recency = self
            .recent
            .iter()
            .position(|c| c == channel)
            .map(|pos| 100u32.saturating_sub((pos as u32) * 5))
            .unwrap_or(0);
        let freq = self.visits.get(channel).copied().unwrap_or(0).min(50) * 4;
        recency + freq
    }

    /// The rows for the current query (see [`Self::rank`]), ranked once per change of query,
    /// snapshot, frecency or remote results however often the switcher draws.
    fn matches(&mut self) -> Rc<[(Target, String)]> {
        if let Some(rows) = &self.rows {
            return Rc::clone(rows);
        }
        let rows: Rc<[(Target, String)]> = self.rank().into();
        self.rows = Some(Rc::clone(&rows));
        rows
    }

    /// Conversations and colleagues matching the filter, ranked by match quality (exact > prefix >
    /// word prefix > substring) and frecency, merging live remote Edge results for unjoined
    /// channels and colleagues not yet in local fixtures. Only the [`MAX_ROWS`] shown are sorted.
    fn rank(&mut self) -> Vec<(Target, String)> {
        let fixtures = Arc::clone(&self.fixtures);
        let local = Rc::clone(
            self.candidates
                .get_or_insert_with(|| candidates(&fixtures).into()),
        );
        let q = self.query.as_str();
        let remote = if !q.is_empty() && self.remote_query == q {
            self.remote_candidates()
        } else {
            Vec::new()
        };
        // (match tier, reverse frecency, group, sort key, position): unique, so ties keep the
        // order candidates were listed in.
        type Rank<'a> = (u8, u32, u8, &'a str, usize);
        let mut scored: Vec<(Rank<'_>, &Candidate)> = local
            .iter()
            .chain(&remote)
            .enumerate()
            .filter(|(_, c)| !q.is_empty() || c.group != 2)
            .filter_map(|(ix, c)| {
                let tier = match_tier(q, &c.sort_key[c.name_at..])
                    .or_else(|| c.handle.as_deref().and_then(|handle| match_tier(q, handle)))
                    .or_else(|| c.remote.then_some(3))?;
                let frecency = match &c.pick {
                    Pick::Conversation(id) => u32::MAX - self.frecency_score(id),
                    _ => u32::MAX,
                };
                Some(((tier, frecency, c.group, c.sort_key.as_str(), ix), c))
            })
            .collect();
        if scored.len() > MAX_ROWS {
            scored.select_nth_unstable_by(MAX_ROWS - 1, |a, b| a.0.cmp(&b.0));
            scored.truncate(MAX_ROWS);
        }
        scored.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        scored
            .into_iter()
            .filter_map(|(_, c)| Some((self.target(&c.pick)?, c.label.clone())))
            .collect()
    }

    /// The remote Edge results for the current query that the snapshot does not hold.
    fn remote_candidates(&self) -> Vec<Candidate> {
        let mut seen = HashSet::new();
        let channels = (self.remote_channels.iter().enumerate())
            .filter(|(_, c)| {
                !c.is_archived && self.fixtures.conversation(&c.id).is_none() && seen.insert(&c.id)
            })
            .map(|(ix, c)| {
                let label = c
                    .name
                    .as_deref()
                    .map(|n| format!("# {n}"))
                    .unwrap_or_else(|| c.id.to_string());
                Candidate::new(Pick::RemoteChannel(ix), label, None, 2, true)
            });
        let with_dm = |u: &User| {
            self.fixtures
                .dms
                .iter()
                .any(|dm| dm.user.as_ref() == Some(&u.id))
        };
        let people = (self.remote_users.iter().enumerate())
            .filter(|(_, u)| !u.deleted && !with_dm(u) && self.fixtures.user(&u.id).is_none())
            .map(|(ix, u)| {
                let name = self.fixtures.name_of(u).to_owned();
                Candidate::new(Pick::RemoteUser(ix), name, Some(u.name.as_str()), 3, true)
            });
        channels.chain(people).collect()
    }

    /// What `pick` opens.
    fn target(&self, pick: &Pick) -> Option<Target> {
        Some(match pick {
            Pick::Conversation(id) => Target::Channel(id.clone()),
            Pick::Directory(ix) => {
                Target::RemoteChannel(Box::new(self.fixtures.directory.get(*ix)?.clone()))
            }
            Pick::Person(id) => Target::User(id.clone()),
            Pick::RemoteChannel(ix) => {
                Target::RemoteChannel(Box::new(self.remote_channels.get(*ix)?.clone()))
            }
            Pick::RemoteUser(ix) => {
                Target::RemoteUser(Box::new(self.remote_users.get(*ix)?.clone()))
            }
        })
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some((target, _)) = self.matches().get(self.selected).cloned() else {
            return;
        };
        match target {
            Target::Channel(channel) => cx.emit(SwitcherEvent::Open(channel)),
            Target::RemoteChannel(channel) => cx.emit(SwitcherEvent::OpenRemoteChannel(channel)),
            Target::User(user) => cx.emit(SwitcherEvent::OpenUser(user)),
            Target::RemoteUser(user) => cx.emit(SwitcherEvent::OpenUser(user.id)),
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches().len();
        match event.keystroke.key.as_str() {
            "down" if count > 0 => self.selected = (self.selected + 1) % count,
            "up" if count > 0 => self.selected = (self.selected + count - 1) % count,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

/// Everything a query can match in `fixtures`: its conversations, the people without a DM
/// and the unjoined channels it lists.
fn candidates(fixtures: &Fixtures) -> Vec<Candidate> {
    let with_dm: HashSet<&UserId> = (fixtures.dms.iter())
        .filter_map(|dm| dm.user.as_ref())
        .collect();
    let conversations = fixtures.dms.iter().chain(&fixtures.channels);
    let known: HashSet<&ChannelId> = conversations.clone().map(|c| &c.id).collect();
    let mut all: Vec<Candidate> = conversations
        .map(|c| {
            let label = fixtures.conversation_label(c);
            Candidate::new(Pick::Conversation(c.id.clone()), label, None, 0, false)
        })
        .collect();
    all.extend(
        (fixtures.directory.iter().enumerate())
            .filter(|(_, c)| !c.is_archived && !known.contains(&c.id))
            .map(|(ix, c)| {
                let label = fixtures.conversation_label(c);
                Candidate::new(Pick::Directory(ix), label, None, 2, false)
            }),
    );
    all.extend(
        (fixtures.users.iter())
            .filter(|u| !u.deleted && !with_dm.contains(&u.id))
            .map(|u| {
                let name = if u.id == fixtures.self_user {
                    format!("{} (you)", fixtures.name_of(u))
                } else {
                    fixtures.name_of(u).to_owned()
                };
                Candidate::new(
                    Pick::Person(u.id.clone()),
                    name,
                    Some(u.name.as_str()),
                    1,
                    false,
                )
            }),
    );
    all
}

/// How well lowercased `key` matches `query`: exact, prefix, word prefix, substring; an empty
/// query matches everything.
fn match_tier(query: &str, key: &str) -> Option<u8> {
    if query.is_empty() || key == query {
        Some(0)
    } else if key.starts_with(query) {
        Some(1)
    } else if key
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.starts_with(query))
    {
        Some(2)
    } else if key.contains(query) {
        Some(3)
    } else {
        None
    }
}

impl Render for QuickSwitcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover, active, active_fg, popover) = (
            theme.muted_foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.popover,
        );
        let matches = self.matches();
        let selected = self.selected.min(matches.len().saturating_sub(1));

        let rows: Vec<_> = matches
            .iter()
            .enumerate()
            .map(|(ix, (target, label))| {
                let on = ix == selected;
                let (leading, kind) = match target {
                    Target::Channel(channel) => {
                        let conversation = self.fixtures.conversation(channel);
                        let icon = match conversation.map(|c| c.kind()) {
                            Some(ConversationKind::PrivateChannel) => div()
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Lock)
                                        .text_color(if on { active_fg } else { muted })
                                        .size(px(14.)),
                                ),
                            Some(ConversationKind::PublicChannel) => div()
                                .size(px(20.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Hash)
                                        .text_color(if on { active_fg } else { muted })
                                        .size(px(14.)),
                                ),
                            _ => conversation
                                .map(|c| {
                                    conversation_avatar(
                                        &self.fixtures,
                                        c,
                                        20.,
                                        if on { active } else { popover },
                                        cx,
                                    )
                                })
                                .unwrap_or_else(div),
                        };
                        let kind = match conversation.map(|c| c.kind()) {
                            Some(ConversationKind::Im) => "Direct message",
                            Some(ConversationKind::Mpim) => "Group DM",
                            _ => "Channel",
                        };
                        (icon, kind)
                    }
                    Target::RemoteChannel(conv) => {
                        let icon = if conv.is_private {
                            IconName::Lock
                        } else {
                            IconName::Hash
                        };
                        let glyph = div()
                            .size(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(icon)
                                    .text_color(if on { active_fg } else { muted })
                                    .size(px(14.)),
                            );
                        let kind = if conv.is_member {
                            "Channel"
                        } else {
                            "Channel · Not joined"
                        };
                        (glyph, kind)
                    }
                    Target::User(user) => {
                        let kind = if *user == self.fixtures.self_user {
                            "Direct message"
                        } else {
                            "Person"
                        };
                        (user_chip(&self.fixtures, user, 20.), kind)
                    }
                    Target::RemoteUser(user) => {
                        (user_chip(&self.fixtures, &user.id, 20.), "Person")
                    }
                };
                let target_clone = target.clone();
                h_flex()
                    .id(("switcher-row", ix))
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |row| row.bg(active).text_color(active_fg))
                    .when(!on, |row| row.hover(|el| el.bg(hover)))
                    .child(leading)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(label.trim_start_matches("# ").to_owned()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if on { active_fg } else { muted })
                            .child(kind),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| match &target_clone {
                        Target::Channel(channel) => cx.emit(SwitcherEvent::Open(channel.clone())),
                        Target::RemoteChannel(channel) => {
                            cx.emit(SwitcherEvent::OpenRemoteChannel(channel.clone()))
                        }
                        Target::User(user) => cx.emit(SwitcherEvent::OpenUser(user.clone())),
                        Target::RemoteUser(user) => {
                            cx.emit(SwitcherEvent::OpenUser(user.id.clone()))
                        }
                    }))
            })
            .collect();

        let empty = rows.is_empty().then(|| {
            div()
                .px_3()
                .py_4()
                .text_sm()
                .text_color(muted)
                .child("No conversations match.")
        });

        v_flex()
            .id("quick-switcher")
            .w_full()
            .gap_2()
            .capture_key_down(cx.listener(Self::on_key))
            .child(crate::preferences::search_input_box(&self.input, cx))
            .child(v_flex().gap_0p5().children(rows).children(empty))
            .child(div().pt_1().text_xs().text_color(muted).child(
                "\u{2191}\u{2193} to navigate \u{b7} \u{21B5} to open \u{b7} esc to dismiss",
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui;

    #[gpui::test]
    fn quick_switcher_ranks_by_match_tier_frecency_and_merges_remote(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
        });
        let mut fixtures = crate::fixtures::load();
        fixtures.channels = vec![
            Conversation {
                id: ChannelId::from("C_SUB"),
                name: Some("my-eng-team".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("C_PRE"),
                name: Some("engineering".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("C_EXACT"),
                name: Some("eng".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("C_MID"),
                name: Some("strength".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            },
        ];
        fixtures.dms.clear();
        fixtures.directory.clear();
        fixtures.users.clear();

        let fixtures = Arc::new(fixtures);
        let mounted = std::rc::Rc::new(std::cell::RefCell::new(None));
        let slot = mounted.clone();
        let (_root, cx) = cx.add_window_view(move |window, cx| {
            let entity = QuickSwitcher::new(fixtures.clone(), window, cx);
            *slot.borrow_mut() = Some(entity.clone());
            gpui_kit::component::Root::new(entity, window, cx)
        });
        let switcher = mounted.borrow_mut().take().unwrap();

        let channel_ids = |s: &mut QuickSwitcher| -> Vec<String> {
            (s.matches().iter())
                .filter_map(|(t, _)| match t {
                    Target::Channel(id) => Some(id.to_string()),
                    _ => None,
                })
                .collect()
        };

        // An empty query orders by frecency (recency + visit count) once it arrives.
        switcher.update(cx, |s, cx| {
            assert_eq!(
                channel_ids(s)[0],
                "C_EXACT",
                "alphabetical without frecency"
            );
            s.set_frecency(
                vec![ChannelId::from("C_MID"), ChannelId::from("C_SUB")],
                HashMap::from([(ChannelId::from("C_SUB"), 20)]),
                cx,
            );
            let ids = channel_ids(s);
            assert_eq!(ids[0], "C_SUB"); // 95 + 80 = 175 > 100
            assert_eq!(ids[1], "C_MID");
        });

        // Query "eng" ranks exact > prefix > word-prefix > substring.
        switcher.update(cx, |s, _| {
            s.set_query("eng".into());
            assert_eq!(channel_ids(s), ["C_EXACT", "C_PRE", "C_SUB", "C_MID"]);
        });

        // Merging remote results deduplicates known channels and ignores stale queries.
        switcher.update(cx, |s, cx| {
            s.set_remote_results(
                "stale",
                EdgeSearchResults {
                    channels: vec![Conversation {
                        id: ChannelId::from("C_STALE"),
                        name: Some("eng-stale".into()),
                        ..Default::default()
                    }],
                    users: Vec::new(),
                },
                cx,
            );
            s.set_remote_results(
                "eng",
                EdgeSearchResults {
                    channels: vec![
                        Conversation {
                            id: ChannelId::from("C_EXACT"),
                            name: Some("eng".into()),
                            ..Default::default()
                        },
                        Conversation {
                            id: ChannelId::from("C_REMOTE"),
                            name: Some("eng-remote".into()),
                            ..Default::default()
                        },
                    ],
                    users: Vec::new(),
                },
                cx,
            );
            let labels: Vec<String> = s.matches().iter().map(|(_, l)| l.clone()).collect();
            assert!(labels.iter().any(|l| l == "# eng-remote"));
            assert!(!labels.iter().any(|l| l == "# eng-stale"));
            assert_eq!(labels.iter().filter(|l| *l == "# eng").count(), 1);
        });

        // A new snapshot's conversations match at once.
        switcher.update(cx, |s, cx| {
            let mut next = s.fixtures.with_counts(HashMap::new());
            next.channels.push(Conversation {
                id: ChannelId::from("C_NEW"),
                name: Some("eng-new".into()),
                is_channel: true,
                is_member: true,
                ..Default::default()
            });
            s.set_fixtures(Arc::new(next), cx);
            assert!(channel_ids(s).iter().any(|id| id == "C_NEW"));
        });
    }
}
