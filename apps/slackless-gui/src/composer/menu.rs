//! The composer's completion menu: people (and `@here` / `@channel` / `@everyone`) after `@`,
//! channels after `#`, emoji after `:` and two characters, and the built-in slash commands
//! for a draft that starts with `/`. It floats above the frame; Up/Down move, Tab or Enter
//! pick, Esc dismisses.

use std::ops::Range;

use gpui_kit::gpui::{Context, Window};
use slackless_core::types::rich_text::BroadcastRange;
use slackless_core::types::{ChannelId, UserId, UsergroupId};

use super::Composer;
use crate::editor::{Atom, Directory as _, Editor, Lookup, rank, trigger};
use crate::fixtures::Fixtures;
use crate::slash;

/// Entries shown at most, as in Slack's menus.
const LIMIT: usize = 8;

/// One slash command shown in the `/` completion menu (built-in or workspace app command).
pub(super) struct SlashEntry {
    pub name: String,
    pub usage: String,
    pub description: String,
    pub app_name: Option<String>,
}

pub(super) enum Entry {
    User(UserId),
    Usergroup(UsergroupId),
    Broadcast(BroadcastRange),
    Channel(ChannelId),
    /// A shortcode without colons.
    Emoji(String),
    Slash(SlashEntry),
}

pub(super) struct Menu {
    /// The text a pick replaces: the trigger and its query.
    pub range: Range<usize>,
    pub entries: Vec<Entry>,
    pub selected: usize,
}

impl Menu {
    /// The menu for what is being typed at the caret, when anything matches it.
    pub fn at_caret(editor: &Editor, fixtures: &Fixtures) -> Option<Menu> {
        let selection = editor.selection();
        if !selection.is_empty() {
            return None;
        }
        let doc = editor.doc();
        let (range, entries) = match slash::typing(doc.text()) {
            Some(name) => (0..doc.text().len(), slash_commands(fixtures, name)),
            None => {
                let found = trigger(doc, selection.start)?;
                let query = found.query.to_lowercase();
                let entries = match found.lookup {
                    Lookup::People => people(fixtures, &query),
                    Lookup::Channels => channels(fixtures, &query),
                    Lookup::Emoji => emoji_names(fixtures, &query),
                };
                (found.range, entries)
            }
        };
        (!entries.is_empty()).then_some(Menu {
            range,
            entries,
            selected: 0,
        })
    }

    /// Move the highlight one row, wrapping around.
    pub fn step(&mut self, forward: bool) {
        let count = self.entries.len();
        let step = if forward { 1 } else { count - 1 };
        self.selected = (self.selected + step) % count;
    }

    /// Whether the highlighted row is the slash command already typed in full, which Enter
    /// sends instead of completing.
    pub fn is_typed_command(&self, typed: &str) -> bool {
        matches!(
            self.entries.get(self.selected),
            Some(Entry::Slash(command)) if slash::typing(typed).is_some_and(|t| t.eq_ignore_ascii_case(&command.name))
        )
    }
}

/// Built-in commands merged with the workspace's `client.appCommands`, prefix matches ahead of
/// substring matches and deduplicated by name.
fn slash_commands(fixtures: &Fixtures, query: &str) -> Vec<Entry> {
    let query = query.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut found: Vec<(u8, String, SlashEntry)> = Vec::new();
    for cmd in &slash::BUILTINS {
        if let Some(score) = rank(cmd.name, &query) {
            seen.insert(cmd.name.to_owned());
            found.push((
                score,
                cmd.name.to_owned(),
                SlashEntry {
                    name: cmd.name.to_owned(),
                    usage: cmd.usage.to_owned(),
                    description: cmd.description.to_owned(),
                    app_name: None,
                },
            ));
        }
    }
    for cmd in &fixtures.app_commands {
        if seen.contains(&cmd.name) {
            continue;
        }
        let score = [Some(cmd.name.as_str()), cmd.app_name.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|field| rank(field, &query))
            .min();
        if let Some(score) = score {
            seen.insert(cmd.name.clone());
            found.push((
                score,
                cmd.name.clone(),
                SlashEntry {
                    name: cmd.name.clone(),
                    usage: cmd.usage.clone(),
                    description: cmd.description.clone(),
                    app_name: cmd.app_name.clone(),
                },
            ));
        }
    }
    found.sort_unstable_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    found.truncate(LIMIT);
    found
        .into_iter()
        .map(|(_, _, entry)| Entry::Slash(entry))
        .collect()
}

/// The best of `found`: each candidate is its [`rank`], the name ties are ordered by, and
/// what it stands for.
fn best<T>(mut found: Vec<(u8, &str, T)>) -> Vec<T> {
    found.sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    found.truncate(LIMIT);
    found.into_iter().map(|(_, _, entry)| entry).collect()
}

fn people(fixtures: &Fixtures, query: &str) -> Vec<Entry> {
    let users = fixtures.users.iter().filter(|user| !user.deleted);
    let found = users
        .filter_map(|user| {
            let names = [
                Some(user.display_name()),
                user.real_name.as_deref(),
                Some(user.name.as_str()),
            ];
            let score = names
                .into_iter()
                .flatten()
                .filter_map(|name| rank(name, query))
                .min()?;
            Some((score, user.display_name(), &user.id))
        })
        .collect();
    let groups = fixtures
        .usergroups
        .iter()
        .filter(|group| !group.is_deleted() && !group.handle.is_empty())
        .filter_map(|group| {
            let score = [group.handle.as_str(), group.name.as_str()]
                .into_iter()
                .filter_map(|name| rank(name, query))
                .min()?;
            Some((score, group.handle.as_str(), &group.id))
        })
        .collect();
    let broadcasts = [
        BroadcastRange::Here,
        BroadcastRange::Channel,
        BroadcastRange::Everyone,
    ];
    best(found)
        .into_iter()
        .map(|id| Entry::User(id.clone()))
        .chain(
            best(groups)
                .into_iter()
                .map(|id| Entry::Usergroup(id.clone())),
        )
        .chain(
            broadcasts
                .into_iter()
                .filter(|range| rank(range.as_str(), query) == Some(0))
                .map(Entry::Broadcast),
        )
        .take(LIMIT)
        .collect()
}

fn channels(fixtures: &Fixtures, query: &str) -> Vec<Entry> {
    let mut found: Vec<_> = fixtures
        .channels
        .iter()
        .chain(&fixtures.directory)
        .filter(|channel| !channel.is_im && !channel.is_mpim && !channel.is_archived)
        .filter_map(|channel| {
            let name = channel.name.as_deref()?;
            Some((rank(name, query)?, name, &channel.id))
        })
        .collect();
    // A joined channel is in the directory too.
    found.sort_unstable_by_key(|(_, _, id)| *id);
    found.dedup_by_key(|(_, _, id)| *id);
    best(found)
        .into_iter()
        .map(|id| Entry::Channel(id.clone()))
        .collect()
}

fn emoji_names(fixtures: &Fixtures, query: &str) -> Vec<Entry> {
    let custom = fixtures.custom_emoji.keys().map(String::as_str);
    let standard = emojis::iter().flat_map(|emoji| emoji.shortcodes());
    let mut found: Vec<_> = custom
        .chain(standard)
        .filter_map(|name| Some((rank(name, query)?, name, name)))
        .collect();
    found.sort_unstable();
    found.dedup();
    best(found)
        .into_iter()
        .map(|name| Entry::Emoji(name.to_owned()))
        .collect()
}

impl Composer {
    /// Insert the menu's entry `ix` in place of what was typed.
    pub(super) fn pick(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let Some(entry) = menu.entries.into_iter().nth(ix) else {
            return;
        };
        let fixtures = &*self.fixtures;
        match entry {
            Entry::User(id) => {
                let text = format!("@{}", fixtures.mention(&id));
                self.editor.insert_atom(menu.range, Atom::User(id), &text);
            }
            Entry::Usergroup(id) => {
                let text = format!("@{}", fixtures.usergroup_handle(&id).unwrap_or(id.as_str()));
                self.editor
                    .insert_atom(menu.range, Atom::Usergroup(id), &text);
            }
            Entry::Broadcast(range) => {
                let text = format!("@{range}");
                self.editor
                    .insert_atom(menu.range, Atom::Broadcast(range), &text);
            }
            Entry::Channel(id) => {
                let text = format!("#{}", fixtures.channel_name(&id));
                self.editor
                    .insert_atom(menu.range, Atom::Channel(id), &text);
            }
            Entry::Emoji(name) => {
                let text = fixtures
                    .emoji_text(&name)
                    .unwrap_or_else(|| format!(":{name}:"));
                self.editor
                    .insert_atom(menu.range, Atom::Emoji(name.into()), &text);
            }
            Entry::Slash(command) => self.editor.replace_all(&format!("/{} ", command.name)),
        }
        self.push(window, cx);
        self.focus_input(window, cx);
        self.changed(cx);
    }
}
