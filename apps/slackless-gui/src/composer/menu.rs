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
    pub app_id: Option<String>,
    /// True when multiple apps (or a built-in + an app) register the same `/name` (`F-COLLAB-686`).
    pub is_duplicate: bool,
    /// True for a Slackless built-in command.
    pub is_builtin: bool,
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
    /// sends instead of completing. When multiple apps share that command name (`is_duplicate`),
    /// returns `false` so Enter explicitly picks the highlighted app entry (`F-COLLAB-686`).
    pub fn is_typed_command(&self, typed: &str) -> bool {
        matches!(
            self.entries.get(self.selected),
            Some(Entry::Slash(command))
                if !command.is_duplicate
                    && slash::typing(typed).is_some_and(|t| t.eq_ignore_ascii_case(&command.name))
        )
    }
}

/// Built-in commands merged with the workspace's `client.appCommands`, prefix matches ahead of
/// substring matches. Duplicate command names across distinct apps (or built-in + app) are all
/// preserved and flagged with `is_duplicate = true` so the user can disambiguate (`F-COLLAB-686`).
fn slash_commands(fixtures: &Fixtures, query: &str) -> Vec<Entry> {
    let query = query.to_lowercase();
    let mut seen: std::collections::HashSet<(String, String, String)> =
        std::collections::HashSet::new();
    let mut found: Vec<(u8, String, u8, String, SlashEntry)> = Vec::new();
    for cmd in &slash::BUILTINS {
        if let Some(score) = rank(cmd.name, &query) {
            seen.insert((cmd.name.to_owned(), String::new(), String::new()));
            found.push((
                score,
                cmd.name.to_owned(),
                0,
                String::new(),
                SlashEntry {
                    name: cmd.name.to_owned(),
                    usage: cmd.usage.to_owned(),
                    description: cmd.description.to_owned(),
                    app_name: None,
                    app_id: None,
                    is_duplicate: false,
                    is_builtin: true,
                },
            ));
        }
    }
    for cmd in &fixtures.app_commands {
        let dedup_key = (
            cmd.name.clone(),
            cmd.app_id.clone().unwrap_or_default(),
            cmd.app_name
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        );
        if !seen.insert(dedup_key) {
            continue;
        }
        let score = [Some(cmd.name.as_str()), cmd.app_name.as_deref()]
            .into_iter()
            .flatten()
            .filter_map(|field| rank(field, &query))
            .min();
        if let Some(score) = score {
            let app_sort = cmd.app_name.clone().unwrap_or_default().to_lowercase();
            found.push((
                score,
                cmd.name.clone(),
                1,
                app_sort,
                SlashEntry {
                    name: cmd.name.clone(),
                    usage: cmd.usage.clone(),
                    description: cmd.description.clone(),
                    app_name: cmd.app_name.clone(),
                    app_id: cmd.app_id.clone(),
                    is_duplicate: false,
                    is_builtin: false,
                },
            ));
        }
    }
    // Count how many distinct providers share each command name across built-ins + workspace apps.
    let mut name_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (_, name, _, _, _) in &found {
        *name_counts.entry(name.clone()).or_insert(0) += 1;
    }
    for (_, name, _, _, entry) in &mut found {
        entry.is_duplicate = name_counts.get(name).copied().unwrap_or(0) > 1;
    }
    found.sort_unstable_by(|a, b| (a.0, &a.1, a.2, &a.3).cmp(&(b.0, &b.1, b.2, &b.3)));
    found.truncate(LIMIT);
    found
        .into_iter()
        .map(|(_, _, _, _, entry)| Entry::Slash(entry))
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
            Entry::Slash(command) => {
                self.selected_slash_target = Some(super::SelectedSlashTarget {
                    name: command.name.clone(),
                    app_id: command.app_id,
                    app_name: command.app_name,
                    is_builtin: command.is_builtin,
                });
                self.editor.replace_all(&format!("/{} ", command.name));
            }
        }
        self.push(window, cx);
        self.focus_input(window, cx);
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::api::AppCommand;
    use slackless_core::types::{Conversation, User, Usergroup};

    fn editor_with(text: &str) -> Editor {
        let mut ed = Editor::default();
        ed.replace_all(text);
        ed
    }

    #[test]
    fn menu_at_caret_rejects_selection_and_opens_for_all_triggers() {
        let fixtures = crate::fixtures::load();

        // Non-empty selection suppresses completion menu.
        let mut selected = editor_with("@al");
        selected.select(0..3);
        assert!(Menu::at_caret(&selected, &fixtures).is_none());

        // Slash trigger
        let slash_menu = Menu::at_caret(&editor_with("/sh"), &fixtures).expect("slash menu");
        assert_eq!(slash_menu.range, 0..3);
        assert!(matches!(slash_menu.entries.first(), Some(Entry::Slash(s)) if s.name == "shrug"));

        // People trigger (`@`)
        let people_menu = Menu::at_caret(&editor_with("hi @"), &fixtures).expect("people menu");
        assert_eq!(people_menu.range, 3..4);
        assert!(!people_menu.entries.is_empty());

        // Channels trigger (`#`)
        let chan_menu = Menu::at_caret(&editor_with("in #gen"), &fixtures).expect("channel menu");
        assert_eq!(chan_menu.range, 3..7);
        assert!(matches!(chan_menu.entries.first(), Some(Entry::Channel(_))));

        // Emoji trigger (`:` + >= 2 chars)
        let emoji_menu = Menu::at_caret(&editor_with("ok :smi"), &fixtures).expect("emoji menu");
        assert_eq!(emoji_menu.range, 3..7);
        assert!(matches!(emoji_menu.entries.first(), Some(Entry::Emoji(_))));
    }

    #[test]
    fn menu_step_wraps_forward_and_backward_and_is_typed_command_checks_exact_match() {
        let fixtures = crate::fixtures::load();
        let mut menu = Menu::at_caret(&editor_with("/s"), &fixtures).expect("slash menu");
        let n = menu.entries.len();
        assert!(n >= 2);
        assert_eq!(menu.selected, 0);

        menu.step(false);
        assert_eq!(menu.selected, n - 1, "backward wrap from 0");
        menu.step(true);
        assert_eq!(menu.selected, 0, "forward wrap from n-1");
        menu.step(true);
        assert_eq!(menu.selected, 1);

        let shrug_menu = Menu::at_caret(&editor_with("/SHRUG"), &fixtures).unwrap();
        assert!(shrug_menu.is_typed_command("/shrug"));
        assert!(shrug_menu.is_typed_command("/SHRUG"));
        assert!(!shrug_menu.is_typed_command("/shr"));

        let chan_menu = Menu::at_caret(&editor_with("#gen"), &fixtures).unwrap();
        assert!(!chan_menu.is_typed_command("#gen"));
    }

    #[test]
    fn slash_commands_preserves_duplicates_marks_them_and_caps_at_limit() {
        let mut fixtures = crate::fixtures::load();
        fixtures.app_commands = vec![
            AppCommand {
                name: "shrug".into(),
                usage: "[custom]".into(),
                description: "App duplicate of builtin".into(),
                app_name: Some("DupApp".into()),
                app_id: Some("A0DUP".into()),
            },
            AppCommand {
                name: "deploy".into(),
                usage: "<env>".into(),
                description: "Deploy service".into(),
                app_name: Some("ReleaseBot".into()),
                app_id: Some("A0REL".into()),
            },
            AppCommand {
                name: "deploy".into(),
                usage: "<dup>".into(),
                description: "Duplicate app command".into(),
                app_name: Some("OtherBot".into()),
                app_id: Some("A0OTH".into()),
            },
        ];

        // Builtin `shrug` sorts ahead of app duplicate, and BOTH are preserved & flagged duplicate (`F-COLLAB-686`).
        let shrug = slash_commands(&fixtures, "shrug");
        assert_eq!(shrug.len(), 2);
        match (&shrug[0], &shrug[1]) {
            (Entry::Slash(first), Entry::Slash(second)) => {
                assert!(first.is_builtin);
                assert!(first.is_duplicate);
                assert_eq!(first.app_name, None);
                assert!(!second.is_builtin);
                assert!(second.is_duplicate);
                assert_eq!(second.app_name.as_deref(), Some("DupApp"));
            }
            _ => panic!("expected Slash entries"),
        }

        // Duplicate `/deploy` entries from distinct apps are both surfaced with `is_duplicate = true`.
        let deploys = slash_commands(&fixtures, "deploy");
        assert_eq!(deploys.len(), 2);
        for entry in &deploys {
            match entry {
                Entry::Slash(s) => {
                    assert_eq!(s.name, "deploy");
                    assert!(s.is_duplicate);
                }
                _ => panic!("expected Slash entry"),
            }
        }
        // Typing `/deploy` in full does NOT auto-submit while duplicates exist; Enter picks the highlighted app.
        let dup_menu = Menu::at_caret(&editor_with("/deploy"), &fixtures).expect("deploy menu");
        assert!(!dup_menu.is_typed_command("/deploy"));

        // App commands match by `app_name`.
        let by_app = slash_commands(&fixtures, "releasebot");
        assert_eq!(by_app.len(), 1);
        match &by_app[0] {
            Entry::Slash(entry) => {
                assert_eq!(entry.name, "deploy");
                assert_eq!(entry.app_id.as_deref(), Some("A0REL"));
            }
            _ => panic!("expected Slash entry"),
        }

        // Empty query returns at most LIMIT entries.
        for i in 0..12 {
            fixtures.app_commands.push(AppCommand {
                name: format!("cmd{i:02}"),
                usage: String::new(),
                description: String::new(),
                app_name: None,
                app_id: None,
            });
        }
        assert_eq!(slash_commands(&fixtures, "").len(), LIMIT);
    }

    #[test]
    fn people_filters_deleted_orders_users_groups_broadcasts_and_caps_limit() {
        let mut fixtures = crate::fixtures::load();
        fixtures.users = vec![
            User {
                id: UserId::from("U_DEL"),
                name: "herbie".into(),
                deleted: true,
                ..Default::default()
            },
            User {
                id: UserId::from("U_HER"),
                name: "herman".into(),
                ..Default::default()
            },
        ];
        fixtures.usergroups = vec![
            Usergroup {
                id: UsergroupId::from("S_DEL"),
                handle: "her-deleted".into(),
                name: "Deleted group".into(),
                date_delete: 1_700_000_000,
                ..Default::default()
            },
            Usergroup {
                id: UsergroupId::from("S_EMPTY"),
                handle: String::new(),
                name: "her-empty".into(),
                ..Default::default()
            },
            Usergroup {
                id: UsergroupId::from("S_OK"),
                handle: "heroes".into(),
                name: "Heroes".into(),
                ..Default::default()
            },
        ];

        // Prefix `"her"` matches user -> usergroup -> broadcast `@here`, excluding deleted/empty.
        let res = people(&fixtures, "her");
        assert_eq!(res.len(), 3);
        assert!(matches!(&res[0], Entry::User(u) if u.as_str() == "U_HER"));
        assert!(matches!(&res[1], Entry::Usergroup(g) if g.as_str() == "S_OK"));
        assert!(matches!(&res[2], Entry::Broadcast(BroadcastRange::Here)));

        // Non-prefix substring `"ere"` must NOT match broadcast `@here` (prefix-only `rank == 0`).
        let sub = people(&fixtures, "ere");
        assert!(sub.iter().all(|e| !matches!(e, Entry::Broadcast(_))));

        // Respects LIMIT = 8.
        for i in 0..12 {
            fixtures.users.push(User {
                id: UserId::from(format!("U{i:02}")),
                name: format!("alpha{i:02}"),
                ..Default::default()
            });
        }
        assert_eq!(people(&fixtures, "alpha").len(), LIMIT);
    }

    #[test]
    fn channels_excludes_dms_archived_and_dedups_directory() {
        let mut fixtures = crate::fixtures::load();
        let pub_ch = Conversation {
            id: ChannelId::from("C_ENG"),
            name: Some("engineering".into()),
            is_channel: true,
            ..Default::default()
        };
        fixtures.channels = vec![
            pub_ch.clone(),
            Conversation {
                id: ChannelId::from("C_ARCH"),
                name: Some("engineering-old".into()),
                is_channel: true,
                is_archived: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("D_IM"),
                name: Some("engineering-dm".into()),
                is_im: true,
                ..Default::default()
            },
            Conversation {
                id: ChannelId::from("G_MPIM"),
                name: Some("engineering-mpim".into()),
                is_mpim: true,
                ..Default::default()
            },
        ];
        fixtures.directory = vec![
            pub_ch, // Duplicate of joined channel
            Conversation {
                id: ChannelId::from("C_DIR"),
                name: Some("engineering-ops".into()),
                is_channel: true,
                ..Default::default()
            },
        ];

        let res = channels(&fixtures, "eng");
        let ids: Vec<&str> = res
            .iter()
            .filter_map(|e| match e {
                Entry::Channel(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec!["C_ENG", "C_DIR"]);
    }

    #[test]
    fn emoji_names_merges_custom_and_standard_with_dedup() {
        let mut fixtures = crate::fixtures::load();
        fixtures.custom_emoji.insert(
            "smile".into(),
            crate::fixtures::EmojiImage::Url("https://example.com/smile.png".into()),
        );
        fixtures.custom_emoji.insert(
            "smile_custom_xyz".into(),
            crate::fixtures::EmojiImage::Url("https://example.com/custom.png".into()),
        );

        let res = emoji_names(&fixtures, "smile");
        assert!(res.len() <= LIMIT);
        let names: Vec<&str> = res
            .iter()
            .filter_map(|e| match e {
                Entry::Emoji(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names.iter().filter(|&&n| n == "smile").count(),
            1,
            "duplicate custom+standard shortcode must appear once"
        );
        assert!(names.contains(&"smile_custom_xyz"));
    }
}
