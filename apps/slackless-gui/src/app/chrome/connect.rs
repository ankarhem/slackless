//! Slack Connect context bar, members popover, channel people lookup, and Join Channel card.

use super::super::*;

impl App {
    /// Slack's context bar for an externally shared channel, drawn inside the composer's
    /// frame: the first organizations' icons stacked, then "<N> external people are from
    /// <Org>" (two organizations by name, more as a count), the count linking to the channel's
    /// details. Built when the members or the snapshot change, never per frame.
    pub(in crate::app) fn connect_bar(&self, cx: &Context<Self>) -> Option<ContextBar> {
        let (teams, count, rest) = self.connect_line()?;
        let (app, channel) = (cx.entity().downgrade(), self.channel.clone());
        Some(Rc::new(move |_, cx| {
            let (bg, fg, link) = (
                crate::theme::connect_bar(cx),
                cx.theme().foreground,
                cx.theme().link,
            );
            let (app, channel) = (app.clone(), channel.clone());
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                // Slack's bar shows about 38pt of amber; the composer box covers the rest.
                .pt(px(10.))
                .pb(px(10. + composer::BAR_TUCK))
                .rounded_tl(px(8.))
                .rounded_tr(px(8.))
                .bg(bg)
                .text_size(px(13.))
                .text_color(fg)
                .child(org_stack(&teams, bg))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .id("connect-count")
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(link)
                                .cursor_pointer()
                                .hover(|count| count.underline())
                                .child(count.clone())
                                .on_click(move |_, window, cx| {
                                    let _ = app.update(cx, |app, cx| {
                                        app.open_channel_details(
                                            &channel,
                                            channel_details::DetailsTab::About,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        )
                        .child(div().min_w_0().truncate().child(rest.clone())),
                )
                .into_any_element()
        }))
    }

    /// Slack's Slack Connect line for the open conversation: up to three of the other
    /// organizations, then "<N> external people" and "are from <Org>" (two organizations by
    /// name, more as a count). `None` for a channel only this workspace is in.
    pub(in crate::app) fn connect_line(&self) -> Option<(Vec<Team>, String, String)> {
        let conversation = self.fixtures.conversation(&self.channel)?;
        if !conversation.is_ext_shared {
            return None;
        }
        let mut teams: Vec<Team> = self
            .fixtures
            .shared_teams(conversation)
            .into_iter()
            .cloned()
            .collect();
        if teams.is_empty() {
            teams = self
                .connect_orgs
                .iter()
                .map(|org| org.team.clone())
                .collect();
        }
        if teams.is_empty() {
            return None;
        }
        let people: u32 = self.connect_orgs.iter().map(|org| org.members).sum();
        let (count, rest) = if people == 0 {
            (
                format!("{} external organizations", teams.len()),
                "can view this channel".to_owned(),
            )
        } else {
            let from = match teams.as_slice() {
                [team] => team.name.to_string(),
                [a, b] => format!("{} and {}", a.name, b.name),
                _ => format!("{} organizations", teams.len()),
            };
            (
                format!(
                    "{people} external {}",
                    if people == 1 { "person" } else { "people" }
                ),
                format!("{} from {from}", if people == 1 { "is" } else { "are" }),
            )
        };
        let owner_suffix = channel_details::host_team(conversation)
            .and_then(|host_id| {
                self.fixtures
                    .teams
                    .iter()
                    .chain(&self.fixtures.workspaces)
                    .find(|t| t.id == host_id)
                    .map(|t| t.name.clone())
            })
            .map(|name| format!(" \u{b7} Owned by {name}"))
            .unwrap_or_default();
        teams.truncate(3);
        Some((teams, count, format!("{rest}{owner_suffix}")))
    }

    /// Recompute [`Self::connect_orgs`] for the open conversation: only a Slack Connect
    /// channel has other organizations to count.
    pub(in crate::app) fn sync_connect_orgs(&mut self, cx: &mut Context<Self>) {
        self.connect_orgs = match self.fixtures.conversation(&self.channel) {
            Some(conversation) if conversation.is_ext_shared => channel_details::external_orgs(
                &channel_details::own_team(&self.fixtures),
                self.member_ids(&self.channel, conversation),
                self.fixtures.people(),
                &self.fixtures.teams,
            ),
            _ => Vec::new(),
        };
        let bar = self.connect_bar(cx);
        let ext_file_notice = self.external_file_sharing_notice();
        let broadcast_hint = self.open_channel_broadcast_hint();
        self.composer.update(cx, |composer, cx| {
            composer.set_context_bar(bar, cx);
            composer.set_external_file_notice(ext_file_notice.clone(), cx);
            composer.set_broadcast_hint(broadcast_hint, cx);
        });
        self.sync_thread_bar(cx);
    }

    /// Slack Connect external file-sharing notice shown when files are staged in an
    /// externally shared channel (`F-COLLAB-879`).
    pub(in crate::app) fn external_file_sharing_notice(&self) -> Option<SharedString> {
        let conversation = self.fixtures.conversation(&self.channel)?;
        if !conversation.is_ext_shared {
            return None;
        }
        let mut names: Vec<String> = self
            .fixtures
            .shared_teams(conversation)
            .into_iter()
            .map(|t| t.name.clone())
            .collect();
        if names.is_empty() {
            names = self
                .connect_orgs
                .iter()
                .map(|org| org.team.name.clone())
                .collect();
        }
        let target = match names.as_slice() {
            [] => "external organizations".to_owned(),
            [one] => one.clone(),
            [a, b] => format!("{a} and {b}"),
            many => format!("{} external organizations", many.len()),
        };
        Some(
            format!(
                "Slack Connect \u{b7} Files added here will be shared externally with {target}"
            )
            .into(),
        )
    }

    /// Autocomplete badge for `@channel`/`@here`/`@everyone` in the open conversation
    /// (`F-COLLAB-1248`, `F-COLLAB-1249`).
    pub(in crate::app) fn open_channel_broadcast_hint(&self) -> Option<SharedString> {
        let conv = self.fixtures.conversation(&self.channel)?;
        let members = self
            .ws
            .member_counts
            .get(&self.channel)
            .map(|&n| n as usize)
            .or_else(|| conv.num_members.map(|n| n as usize))
            .unwrap_or(conv.members.len());
        crate::app::broadcast_autocomplete_hint(conv, members).map(SharedString::from)
    }

    /// Slack shows the channel's Slack Connect bar over the thread's reply box too.
    pub(in crate::app) fn sync_thread_bar(&mut self, cx: &mut Context<Self>) {
        let here = self
            .thread
            .read(cx)
            .open_thread()
            .is_some_and(|(channel, _)| channel == self.channel);
        let bar = here.then(|| self.connect_bar(cx)).flatten();
        self.thread
            .update(cx, |panel, cx| panel.set_context_bar(bar, cx));
    }

    /// The freshest member ids of `channel`: the core's announcement while it is the open
    /// channel, else the per-workspace cache from an earlier visit, else the conversation row.
    pub(in crate::app) fn member_ids<'a>(
        &'a self,
        channel: &ChannelId,
        conversation: &'a Conversation,
    ) -> &'a [UserId] {
        match &self.open_members {
            Some((open, members)) if open == channel => members,
            _ => self
                .ws
                .members_cache
                .get(channel)
                .map(Vec::as_slice)
                .unwrap_or(&conversation.members),
        }
    }

    /// Slack's dark tooltip hanging under the header members chip (`.c-tooltip__tip`: the
    /// inverse surface, 200px wide, 13px text on a 1.25 line, 8x12 padding, radius 8, the
    /// arrow filling the 8px gap up to the chip).
    pub(in crate::app) fn members_popover(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (surface, ink) = (theme.foreground, theme.background);
        div()
            .id("hdr-members-popover")
            .absolute()
            .top(relative(1.))
            .left_1_2()
            .ml(px(-100.))
            .pt(px(8.))
            .block_mouse_except_scroll()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.members_hover.1 = *hovered;
                cx.notify();
            }))
            .child(
                v_flex()
                    .items_center()
                    .child(crate::sidebar::tooltip_arrow(surface, true))
                    .child(
                        v_flex()
                            .w(px(200.))
                            .px_3()
                            .py_2()
                            .rounded(px(8.))
                            .bg(surface)
                            .text_size(px(13.))
                            .line_height(relative(1.25))
                            .text_center()
                            .text_color(ink)
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child("View all members of this channel"),
                            )
                            .children(
                                includes_line(&self.people_names())
                                    .map(|line| div().text_color(ink.opacity(0.65)).child(line)),
                            ),
                    ),
            )
    }

    /// The open channel's people ([`Self::channel_people`]): fixture mode reads the snapshot's
    /// loaded history in hand; live mode reads the same local-store history the transcript
    /// opened with, off the UI thread. Cached until the channel or its history changes -
    /// never per frame.
    pub(in crate::app) fn load_channel_people(&mut self, cx: &mut Context<Self>) {
        if self.channel_people.is_some() {
            return;
        }
        let channel = self.channel.clone();
        if self.bridge.store().is_some() {
            let Some(read) = self.bridge.history(channel.clone(), None) else {
                return;
            };
            cx.spawn(async move |this, cx| {
                let history = cx.background_executor().spawn(read).await;
                this.update(cx, |app, cx| {
                    if app.channel == channel {
                        app.channel_people = Some(app.people(&channel, &history));
                        if app.members_hover.0 || app.members_hover.1 {
                            cx.notify();
                        }
                    }
                })
            })
            .detach();
        } else {
            let history = self
                .fixtures
                .messages
                .get(&channel)
                .cloned()
                .unwrap_or_default();
            self.channel_people = Some(self.people(&channel, &history));
            cx.notify();
        }
    }

    /// `channel`'s people from its loaded history: the most recent distinct human authors,
    /// newest first, topped up from the member list.
    pub(in crate::app) fn people(&self, channel: &ChannelId, history: &[Message]) -> Vec<UserId> {
        let conversation = self.fixtures.conversation(channel);
        channel_people(
            &self.fixtures,
            self.member_ids(channel, conversation.unwrap_or(&Conversation::default())),
            history,
        )
    }

    /// The names of [`Self::channel_people`], as the members popover lists them.
    pub(in crate::app) fn people_names(&self) -> Vec<String> {
        self.channel_people
            .iter()
            .flatten()
            .map(|user| self.fixtures.user_name(user).to_owned())
            .collect()
    }

    /// Load the open channel's people when it shows the Join card, which names them.
    pub(in crate::app) fn load_join_people(&mut self, cx: &mut Context<Self>) {
        if self
            .fixtures
            .conversation(&self.channel)
            .is_some_and(joinable)
        {
            self.load_channel_people(cx);
        }
    }

    /// Slack's card in place of the composer in a public channel the user is not in: the
    /// faces of up to three of its people, "Join #name", who is here (two names and how many
    /// more people, then its apps), a shared channel's Slack Connect line, and the green Join
    /// Channel button.
    pub(in crate::app) fn join_card(
        &self,
        conversation: &Conversation,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (surface, _) = crate::theme::composer_surfaces(cx);
        let (foreground, muted) = (theme.foreground, theme.muted_foreground);
        let people = self.channel_people.as_deref().unwrap_or(&[]);
        // Each face is ringed in the card's color, overlapping the one before by 8px.
        let faces = h_flex().children(people.iter().take(3).enumerate().map(|(ix, user)| {
            let face = crate::avatars::with_external_badge(
                &self.fixtures,
                Some(user),
                crate::avatars::user_chip(&self.fixtures, user, 36.),
                36.,
                surface,
            );
            div()
                .when(ix > 0, |face| face.ml(px(-12.)))
                .p(px(2.))
                .rounded(px(10.))
                .bg(surface)
                .child(face)
        }));
        let names: Vec<&str> = people
            .iter()
            .take(2)
            .map(|user| self.fixtures.user_name(user))
            .collect();
        let humans = self
            .ws
            .member_counts
            .get(&conversation.id)
            .copied()
            .or(conversation.num_members)
            .map_or(names.len(), |count| count as usize);
        let apps: Vec<&str> = self
            .member_ids(&conversation.id, conversation)
            .iter()
            .filter_map(|member| self.fixtures.user(member))
            // Slack names the channel's apps, not its workflows' bots, alphabetically.
            .filter(|user| {
                let workflow = user.extra.get("is_workflow_bot").and_then(|v| v.as_bool());
                user.is_bot && !user.deleted && workflow != Some(true)
            })
            .map(|user| self.fixtures.name_of(user))
            .collect();
        let mut apps = apps;
        apps.sort_by_key(|name| name.to_lowercase());
        let who = [
            who_is_here(&names, humans.saturating_sub(names.len())),
            apps_also_here(&apps),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let external = self.connect_line().map(|(teams, count, rest)| {
            h_flex()
                .items_center()
                .gap_2()
                .child(org_stack(&teams, surface))
                .child(
                    h_flex()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .text_color(foreground)
                                .child(count),
                        )
                        .child(div().min_w_0().truncate().child(rest)),
                )
        });
        let name = conversation.name.clone().unwrap_or_default();
        div().w_full().px_4().pb_3().child(
            h_flex()
                .w_full()
                .items_start()
                .justify_between()
                .gap_4()
                .p_4()
                .rounded_lg()
                .border_1()
                .border_color(foreground.opacity(0.15))
                .bg(surface)
                .child(
                    v_flex()
                        .min_w_0()
                        .gap_1()
                        .text_sm()
                        .text_color(muted)
                        // The faces' 2px rings sit in the card's padding, as Slack's do.
                        .child(faces.ml(px(-2.)).mt(px(-2.)))
                        .child(
                            div()
                                .text_size(px(18.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(foreground)
                                .child(format!("Join # {name}")),
                        )
                        .when(!who.is_empty(), |column| column.child(who))
                        .children(external),
                )
                .child(
                    Button::new("join-channel")
                        .primary()
                        .with_size(gpui_kit::component::Size::Large)
                        .label("Join Channel")
                        .on_click(cx.listener(|app, _, window, cx| app.join_channel(window, cx))),
                ),
        )
    }

    /// The Join card's button: join the open channel on Slack; the card gives way to the
    /// composer once the core stores it as joined. Fixture mode logs it.
    pub(in crate::app) fn join_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            notify(
                "Join Channel",
                "Logged. Joining runs against a live workspace.",
                window,
                cx,
            );
            return;
        }
        self.bridge
            .send(ChannelCommand::JoinConversation(self.channel.clone()));
    }
}

/// The members popover's "Includes …" line for one to three names, Slack's list
/// punctuation ("Includes A", "Includes A and B", "Includes A, B, and C").
pub(in crate::app) fn includes_line(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("Includes {a}")),
        [a, b] => Some(format!("Includes {a} and {b}")),
        [a, b, c, ..] => Some(format!("Includes {a}, {b}, and {c}")),
    }
}

/// Slack's stacked organization icons (Slack Connect bar and Join card), left to right, each
/// ringed in `ring`, the surface behind them.
pub(in crate::app) fn org_stack(teams: &[Team], ring: Hsla) -> Div {
    h_flex()
        .flex_none()
        .children(teams.iter().enumerate().map(|(ix, team)| {
            div()
                .when(ix > 0, |chip| chip.ml(px(-7.)))
                .p(px(1.))
                .rounded(px(5.))
                .bg(ring)
                .child(channel_details::org_chip(team, 16.))
        }))
}

/// Whether `conversation` is a public channel the user is not in, where Slack shows the
/// Join card instead of the composer.
pub(in crate::app) fn joinable(conversation: &Conversation) -> bool {
    conversation.kind() == ConversationKind::PublicChannel
        && !conversation.is_member
        && !conversation.is_archived
}

/// The Join card's "who is here": up to two names, then how many more people.
pub(in crate::app) fn who_is_here(names: &[&str], more: usize) -> Option<String> {
    let more_people = |n: usize| match n {
        1 => "1 more person is".to_owned(),
        n => format!("{n} more people are"),
    };
    Some(match (names, more) {
        ([], 0) => return None,
        ([], 1) => "1 person is here.".to_owned(),
        ([], n) => format!("{n} people are here."),
        ([a], 0) => format!("{a} is here."),
        ([a], n) => format!("{a} and {} here.", more_people(n)),
        ([a, b, ..], 0) => format!("{a} and {b} are here."),
        ([a, b, ..], n) => format!("{a}, {b}, and {} here.", more_people(n)),
    })
}

/// The Join card's apps: "mastermind and norcebot are also here."
pub(in crate::app) fn apps_also_here(apps: &[&str]) -> Option<String> {
    match apps {
        [] => None,
        [a] => Some(format!("{a} is also here.")),
        [a, b] => Some(format!("{a} and {b} are also here.")),
        [a, b, rest @ ..] => Some(format!(
            "{a}, {b}, and {} more apps are also here.",
            rest.len()
        )),
    }
}

/// A channel's people as Slack names them: the most recent distinct human authors of
/// `history` (newest first, never the signed-in user, never bots), topped up from `members`.
/// Free-standing for the test.
pub(in crate::app) fn channel_people(
    fixtures: &Fixtures,
    members: &[UserId],
    history: &[Message],
) -> Vec<UserId> {
    const LIMIT: usize = 3;
    let mut names: Vec<UserId> = Vec::new();
    let mut seen = HashSet::new();
    let self_user = &fixtures.self_user;
    for message in history.iter().rev() {
        // Thread replies and hidden metadata events are not channel authors; bots are not
        // people Slack would name.
        if message.hidden || message.bot_id.is_some() || message.bot_profile.is_some() {
            continue;
        }
        if let Some(user) = message.user.as_ref() {
            push_person(fixtures, self_user, &mut names, &mut seen, user);
            if names.len() == LIMIT {
                return names;
            }
        }
    }
    for member in members {
        push_person(fixtures, self_user, &mut names, &mut seen, member);
        if names.len() == LIMIT {
            break;
        }
    }
    names
}

/// Count `user` toward the channel's people once: only resolved people who are not the
/// signed-in user.
pub(in crate::app) fn push_person(
    fixtures: &Fixtures,
    self_user: &UserId,
    names: &mut Vec<UserId>,
    seen: &mut HashSet<UserId>,
    user: &UserId,
) {
    if user == self_user || !seen.insert(user.clone()) {
        return;
    }
    let Some(record) = fixtures.user(user) else {
        return;
    };
    if record.is_bot || record.deleted {
        return;
    }
    names.push(user.clone());
}
