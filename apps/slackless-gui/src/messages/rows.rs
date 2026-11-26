//! Message row rendering: transcript rows, cards, reply bars, headers, date/unread dividers,
//! and compact membership lines.

use super::render::is_deleted_ghost;
use super::*;

/// One transcript row: optional date/new-messages divider above the (possibly grouped)
/// message card with its reactions, "N replies" link, hover action bar and context menu.
pub(super) fn transcript_row(
    store: &MessageData,
    ix: usize,
    this: &Entity<MessageList>,
    now: &CivilTime,
    cx: &App,
) -> Div {
    let messages = &store.messages;
    let fixtures = &store.fixtures;
    let message = &messages[ix];
    let previous = ix.checked_sub(1).map(|previous| &messages[previous]);
    // Like Slack, a run of joins (or of leaves) reads as its first line, "<name> and N others
    // left #channel."; the rest of the run keeps an empty row (and so its scroll position).
    if previous.is_some_and(|prev| membership_run(prev, message)) {
        return div();
    }

    let date_break = previous.is_none_or(|prev| day_index(&prev.ts) != day_index(&message.ts));
    // Like Slack, your own messages never count as new: `first_unread_ix` is precomputed once
    // when messages or `last_read` change so row rendering is O(1).
    let new_break = !date_break && store.first_unread_ix == Some(ix);
    let grouped =
        !date_break && !new_break && previous.is_some_and(|prev| same_group(prev, message));
    let editing = store.editing.as_ref() == Some(&message.ts);
    let saved = store.saved.contains(&message.ts);

    let author_key = author_key(message);
    // Every link in this row's text routes out as one event; the root resolves it.
    let links: LinkHandler = {
        let this = this.clone();
        Rc::new(move |target, _window, cx| {
            this.update(cx, |_, cx| {
                cx.emit(MessageListEvent::OpenLink(target.clone()))
            });
        })
    };

    let mut row = v_flex().min_w_0();
    if date_break {
        let label = message
            .ts
            .secs()
            .map(|secs| date_label(&CivilTime::from_unix(secs), now))
            .unwrap_or_else(|| message.ts.as_str().to_owned());
        row = row.child(date_divider(
            label,
            format!("day-{}", message.ts.as_str()),
            this.downgrade(),
            cx,
        ));
    } else if new_break {
        row = row.child(new_messages_divider(cx));
    }
    if is_membership(message) {
        let author = author_name(message, fixtures);
        let others = messages[ix + 1..]
            .iter()
            .take_while(|next| membership_run(message, next))
            .count();
        return row.child(membership_line(message, &author, others, store, &links, cx));
    }

    let key = format!("msg-{}", message.ts.as_str());
    let handler: ActionHandler = {
        let this = this.clone();
        let ts = message.ts.clone();
        Rc::new(move |action, window, cx| {
            this.update(cx, |list, cx| list.run_action(&ts, action, window, cx));
        })
    };
    let image_open: ImageClick = {
        let this = this.clone();
        let ts = message.ts.clone();
        Rc::new(move |origin, _window, cx| {
            this.update(cx, |list, cx| list.open_image(&ts, &origin, cx));
        })
    };

    let prefs = crate::preferences::display_prefs(cx);
    let avatar_size = message_avatar_size(prefs.compact);
    let row_gap_x = if prefs.compact { px(8.) } else { px(12.) };
    let author = author_name_with(message, fixtures, prefs.display_names_only);
    let stamp = format_clock_with(&message.ts, prefs.clock_24h);
    let deleted_ghost = is_deleted_ghost(message);
    let deleted_expanded = !deleted_ghost
        || (prefs.show_deleted_messages ^ store.toggled_deleted.contains(&message.ts));

    // Minimal clickable placeholder row when a deleted message is collapsed (either because
    // "Expand deleted messages by default" is off, or because the user clicked Hide on it).
    if deleted_ghost && !deleted_expanded && !editing {
        let theme = cx.theme();
        let (muted, fg, hover_bg) = (
            theme.muted_foreground,
            theme.foreground,
            theme.secondary.opacity(0.38),
        );
        let toggle_list = this.clone();
        let toggle_ts = message.ts.clone();
        let collapsed = h_flex()
            .id(SharedString::from(format!(
                "deleted-collapsed-{}",
                message.ts.as_str()
            )))
            .group("deleted-row")
            .min_w_0()
            .items_center()
            .gap(row_gap_x)
            .ml(px(-GUTTER))
            .mr(px(-GUTTER))
            .px(px(GUTTER))
            .py(px(if prefs.compact { 2. } else { 4. }))
            .cursor_pointer()
            .hover(move |el| el.bg(hover_bg))
            .child(
                div()
                    .w(px(avatar_size))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::Trash)
                            .size(px(12.))
                            .text_color(muted.opacity(0.55)),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .italic()
                            .text_color(muted.opacity(0.8))
                            .child(format!("Deleted message \u{b7} {author} \u{b7} {stamp}")),
                    )
                    .child(
                        div()
                            .text_color(muted)
                            .group_hover("deleted-row", move |el| el.text_color(fg).underline())
                            .child("Show"),
                    ),
            )
            .on_click(move |_, _window, cx| {
                toggle_list.update(cx, |list, cx| list.toggle_deleted(&toggle_ts, cx));
            });
        return row.child(collapsed);
    }

    let mut body = v_flex()
        .flex_1()
        .min_w_0()
        .gap(px(if prefs.compact { 0. } else { 2. }))
        .text_sm();
    if !grouped && !editing {
        let mut header = message_header(
            message,
            &author,
            stamp.clone(),
            fixtures,
            Some(&handler),
            cx,
        );
        if deleted_ghost {
            let (muted, fg) = (cx.theme().muted_foreground, cx.theme().foreground);
            let toggle_list = this.clone();
            let toggle_ts = message.ts.clone();
            header = header
                .child(
                    div()
                        .text_xs()
                        .italic()
                        .text_color(muted)
                        .child("\u{b7} deleted"),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "deleted-hide-{}",
                            message.ts.as_str()
                        )))
                        .text_xs()
                        .text_color(muted)
                        .cursor_pointer()
                        .hover(move |el| el.text_color(fg).underline())
                        .child("Hide")
                        .on_click(move |_, _window, cx| {
                            toggle_list.update(cx, |list, cx| list.toggle_deleted(&toggle_ts, cx));
                        }),
                );
        }
        body = body.child(header);
    }
    if editing {
        body = body.child(store.edit_composer.clone());
    } else {
        let on_suggest: crate::blockkit::SuggestHandler = {
            let this = this.clone();
            let channel = store.channel.clone();
            let message_ts = message.ts.clone();
            let bot = message
                .bot_id
                .clone()
                .unwrap_or_else(|| slackless_core::types::BotId::from("B01"));
            let app_id = message
                .bot_profile
                .as_ref()
                .and_then(|p| p.app_id.clone())
                .unwrap_or_default();
            let team = message.bot_profile.as_ref().and_then(|p| p.team_id.clone());
            let is_ephemeral = message.is_ephemeral;
            Rc::new(move |request, cx| {
                this.update(cx, |_, cx| {
                    cx.emit(MessageListEvent::BlockSuggestions {
                        channel: channel.clone(),
                        message_ts: message_ts.clone(),
                        bot: bot.clone(),
                        app_id: app_id.clone(),
                        team: team.clone(),
                        is_ephemeral,
                        request,
                    });
                });
            })
        };
        let on_wf: crate::blockkit::WorkflowDetailsHandler = {
            let this = this.clone();
            Rc::new(move |req, _window, cx| {
                this.update(cx, |_, cx| cx.emit(MessageListEvent::WorkflowDetails(req)));
            })
        };
        let on_call: crate::blockkit::CallRefreshHandler = {
            let this = this.clone();
            Rc::new(move |call_id, cx| {
                this.update(cx, |_, cx| {
                    cx.emit(MessageListEvent::RefreshCallInfo { call_id });
                });
            })
        };
        let kcx = KitCx::new(fixtures, Some(&store.kit), &message.ts)
            .with_links(links.clone())
            .opening_images(image_open)
            .dispatching_actions(handler.clone())
            .with_suggestions(on_suggest)
            .with_workflow_details(on_wf)
            .with_call_refresh(on_call);
        // The root as loaded here, so an edit to it shows as in Slack; else the copy the
        // broadcast carries.
        let broadcast_root = message
            .root
            .as_deref()
            .filter(|_| message.subtype == Some(MessageSubtype::ThreadBroadcast))
            .map(|root| {
                messages
                    .binary_search_by(|m| m.ts.cmp(&root.ts))
                    .ok()
                    .and_then(|idx| messages.get(idx))
                    .unwrap_or(root)
            });
        if let Some(root) = broadcast_root.filter(|_| !grouped) {
            // Slack's broadcast reference opens the thread.
            let (open, channel, thread_ts) =
                (links.clone(), store.channel.clone(), root.ts.clone());
            body = body.child(
                h_flex()
                    .id(SharedString::from(format!(
                        "broadcast-{}",
                        message.ts.as_str()
                    )))
                    .min_w_0()
                    .gap_1()
                    .cursor_pointer()
                    .hover(|el| el.underline())
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_none().child("replied to a thread:"))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().link)
                            .child(fixtures.preview(root)),
                    )
                    .on_click(move |_, window, cx| {
                        open(
                            LinkTarget::Thread {
                                channel: channel.clone(),
                                thread_ts: thread_ts.clone(),
                            },
                            window,
                            cx,
                        )
                    }),
            );
        }
        body = body.child(
            div()
                .min_w_0()
                .when(deleted_ghost, |el| el.opacity(0.65))
                .child(message_body(message, &kcx, cx)),
        );
        if prefs.inline_media {
            if let Some(files) = crate::files::attachments_in_channel(
                fixtures,
                Some(&store.channel),
                message,
                kcx.image_open.as_ref(),
                cx,
            ) {
                body = body.child(files);
            }
        } else if !message.files.is_empty() {
            let muted = cx.theme().muted_foreground;
            body = body.child(div().text_xs().text_color(muted).child(format!(
                "{} file{} hidden (inline media off)",
                message.files.len(),
                if message.files.len() == 1 { "" } else { "s" }
            )));
        }
        body = body.children(actions::reaction_chips(
            &key,
            &message.reactions,
            fixtures,
            Some(&handler),
            cx,
        ));
        if let Some(bar) = reply_bar(
            fixtures,
            message,
            SharedString::from(format!("thread-{}", message.ts.as_str())),
            "msg".into(),
            "reply-bar".into(),
            {
                let reply = handler.clone();
                move |window, cx| reply(MessageAction::ReplyInThread, window, cx)
            },
            cx,
        ) {
            body = body.child(bar);
        }
    }

    let grouped_hover_stamp = (grouped && !editing).then(|| stamp.clone());
    let static_gutter = if grouped && !editing {
        None
    } else {
        let av = avatar_gutter(
            AvatarGutterSpec {
                fixtures,
                message,
                author_key: &author_key,
                author: &author,
                key: &key,
                size: avatar_size,
                handler: Some(&handler),
            },
            cx,
        );
        Some(if deleted_ghost {
            div().flex_none().opacity(0.65).child(av).into_any_element()
        } else {
            av
        })
    };

    let set = ActionSet {
        own: message.user.as_ref() == Some(&fixtures.self_user),
        reply: true,
        saved,
        pinned: message.pinned_to.contains(&store.channel),
        in_thread: false,
    };
    let keyword_hit = !set.own && crate::preferences::matches_highlight_keyword(&message.text, cx);
    let theme = cx.theme();
    let hover_bg = theme.secondary.opacity(0.5);
    let saved_bg = theme.warning.opacity(0.08);
    let keyword_bg = theme.warning.opacity(0.11);
    let keyword_bar = theme.warning;
    let thread_open = !saved && store.open_thread.as_ref() == Some(&message.ts);
    let highlighted = store.highlighted.as_ref() == Some(&message.ts);
    let active_row = store.active.as_ref() == Some(&message.ts);
    let is_ephemeral = message.is_ephemeral;
    let thread_bg = crate::theme::thread_root(cx);
    let editing_bg = crate::theme::editing_row(cx);
    let quick = crate::preferences::active_quick_reactions(&fixtures.quick_reactions, cx);
    let menu_handler = handler.clone();
    let row_key = key.clone();
    let card = actions::HoverOverlay::new(SharedString::from(key.clone()), move |hover, _, cx| {
        let revealed = hover.revealed(cx);
        let gutter = match static_gutter {
            Some(gutter) => gutter,
            None => div()
                .w(px(avatar_size))
                .flex_none()
                .flex()
                .items_start()
                .justify_center()
                .pt_0p5()
                .opacity(if revealed { 1. } else { 0. })
                .group_hover("msg", |slot| slot.opacity(1.))
                .child(
                    div()
                        .whitespace_nowrap()
                        .when(prefs.compact, |el| el.text_size(px(10.)))
                        .when(!prefs.compact, |el| el.text_xs())
                        .text_color(cx.theme().muted_foreground)
                        .child(grouped_hover_stamp.unwrap_or_default()),
                )
                .into_any_element(),
        };
        div()
            .id(SharedString::from(row_key.clone()))
            .group("msg")
            .relative()
            .min_w_0()
            .ml(px(-GUTTER))
            .mr(px(-GUTTER))
            .px(px(GUTTER))
            .when(editing, |card| card.py_2().bg(editing_bg))
            .when(!editing, |card| {
                card.py(px(if prefs.compact { 1. } else { 2. }))
                    .when(saved, |card| {
                        card.bg(saved_bg).child(actions::saved_banner(cx))
                    })
                    .when(!saved && !deleted_ghost && keyword_hit, |card| {
                        card.bg(keyword_bg).border_l_2().border_color(keyword_bar)
                    })
                    .when(is_ephemeral, |card| {
                        card.child(actions::ephemeral_banner(&row_key, Some(&handler), cx))
                    })
                    .when(highlighted, |card| card.bg(editing_bg))
                    .when(!highlighted && thread_open, |card| card.bg(thread_bg))
                    .when(!highlighted && !thread_open && active_row, |card| {
                        card.bg(hover_bg)
                    })
                    .when(
                        !saved && !keyword_hit && !highlighted && !thread_open && revealed,
                        |card| card.bg(hover_bg),
                    )
            })
            .child(
                h_flex()
                    .items_start()
                    .gap(row_gap_x)
                    .min_w_0()
                    .child(gutter)
                    .child(body),
            )
            .when(!editing && revealed, |card| {
                card.child(actions::hover_toolbar(
                    &row_key, &quick, set, &handler, &hover, cx,
                ))
            })
            .on_hover(hover.host_on_hover())
            .context_menu(move |menu, window, cx| {
                actions::actions_menu(
                    menu,
                    set,
                    &menu_handler,
                    actions::MenuOrigin::Context,
                    window,
                    cx,
                )
            })
            .into_any_element()
    })
    .on_revealed({
        let (this, ts) = (this.clone(), message.ts.clone());
        move |revealed, _, cx| {
            this.update(cx, |list, cx| {
                if revealed {
                    list.set_focused(&ts, cx);
                } else {
                    list.clear_focused(&ts, cx);
                }
            });
        }
    });
    row.child(card)
}

/// Slack's thread summary bar (`.c-message__reply_bar`): the repliers' 24px faces and bold
/// link reply count, 4px under the message. Hovering the enclosing message row (`row_group`)
/// raises it into a card (`surface`, 13% foreground ring) with `"Last reply …"` and a right
/// chevron; hovering the bar itself (`reply_group`) swaps that line for `"View thread"`.
/// Shared between [`MessageList`] and [`crate::unreads::UnreadsView`] so thread replies render
/// identically everywhere.
pub(crate) fn reply_bar(
    fixtures: &Fixtures,
    message: &Message,
    id: impl Into<gpui_kit::gpui::ElementId>,
    row_group: SharedString,
    reply_group: SharedString,
    on_open: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Option<gpui_kit::gpui::Stateful<Div>> {
    let reply_count = message.reply_count.filter(|&count| count > 0)?;
    let faces = message
        .reply_users
        .iter()
        .take(4)
        .map(|user| crate::avatars::user_chip(fixtures, user, 24.));
    let label = if reply_count == 1 {
        "1 reply".to_owned()
    } else {
        format!("{reply_count} replies")
    };
    let last_reply = message
        .latest_reply
        .as_ref()
        .and_then(|ts| ts.secs())
        .map(|secs| format!("Last reply {}", crate::image_viewer::relative_stamp(secs)));
    let theme = cx.theme();
    let (surface, edge, muted, link) = (
        theme.background,
        theme.foreground.opacity(0.13),
        theme.muted_foreground,
        theme.link,
    );
    let hover_meta = match last_reply {
        Some(last) => div()
            .relative()
            .min_w_0()
            .ml_1()
            .text_color(muted)
            .whitespace_nowrap()
            .opacity(0.)
            .group_hover(row_group.clone(), |line| line.opacity(1.))
            .child(
                div()
                    .group_hover(reply_group.clone(), |line| line.opacity(0.))
                    .child(last),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .opacity(0.)
                    .group_hover(reply_group.clone(), |line| line.opacity(1.))
                    .child("View thread"),
            ),
        None => div()
            .min_w_0()
            .ml_1()
            .text_color(muted)
            .whitespace_nowrap()
            .opacity(0.)
            .group_hover(row_group.clone(), |line| line.opacity(1.))
            .child("View thread"),
    };
    Some(
        h_flex()
            .id(id.into())
            .group(reply_group)
            .max_w(px(600.))
            .h(px(34.))
            // 4px under the message; under reaction pills Slack pulls the bar up into
            // the space their row keeps below them, leaving the same 4px.
            .mt(if message.reactions.is_empty() {
                px(4.)
            } else {
                px(4.) - actions::REACTIONS_BOTTOM
            })
            .ml(px(-5.))
            .p_1()
            .gap_1()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(edge.opacity(0.))
            .text_size(px(13.))
            .cursor_pointer()
            .group_hover(row_group.clone(), move |bar| {
                bar.bg(surface).border_color(edge)
            })
            .children(faces)
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(link)
                    .child(label),
            )
            .child(hover_meta)
            .child(
                div()
                    .ml_auto()
                    .opacity(0.)
                    .group_hover(row_group, |arrow| arrow.opacity(1.))
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(16.))
                            .text_color(muted),
                    ),
            )
            .on_click(move |_, window, cx| on_open(window, cx)),
    )
}

/// Two consecutive messages group into one card when they share an author, land on the same
/// day within [`GROUP_WINDOW_SECS`], and carry the same thread-ness.
pub(crate) fn same_group(prev: &Message, message: &Message) -> bool {
    if is_deleted_ghost(prev) || is_deleted_ghost(message) {
        return false;
    }
    let same_author = match (&prev.user, &message.user) {
        (Some(a), Some(b)) => a == b,
        (None, None) => prev.username == message.username,
        _ => false,
    };
    let close = match (prev.ts.secs(), message.ts.secs()) {
        (Some(prev_secs), Some(secs)) => secs.saturating_sub(prev_secs) <= GROUP_WINDOW_SECS,
        _ => false,
    };
    same_author && close && prev.is_thread_reply() == message.is_thread_reply()
}

fn author_key(message: &Message) -> String {
    message
        .user
        .as_ref()
        .map(|user| user.as_str().to_owned())
        .or_else(|| message.username.clone())
        .or_else(|| message.bot_id.as_ref().map(|bot| bot.as_str().to_owned()))
        .unwrap_or_else(|| "unknown".to_owned())
}

pub(super) fn author_name(message: &Message, fixtures: &Fixtures) -> String {
    author_name_with(message, fixtures, false)
}

fn author_name_with(message: &Message, fixtures: &Fixtures, display_only: bool) -> String {
    message
        .user
        .as_ref()
        .map(|user| {
            if display_only {
                fixtures
                    .user(user)
                    .and_then(|u| {
                        let d = u.profile.display_name.as_deref().unwrap_or("").trim();
                        (!d.is_empty()).then(|| d.to_owned())
                    })
                    .unwrap_or_else(|| fixtures.user_name(user).to_owned())
            } else {
                fixtures.user_name(user).to_owned()
            }
        })
        .or_else(|| message.username.clone())
        // An app message without a username (Giphy's preview) goes by its bot's name.
        .or_else(|| {
            let name = &message.bot_profile.as_ref()?.name;
            (!name.is_empty()).then(|| name.clone())
        })
        .or_else(|| {
            let profile = message
                .bot_id
                .as_ref()
                .and_then(|id| fixtures.bot_profile(id))?;
            (!profile.name.is_empty()).then_some(profile.name)
        })
        .unwrap_or_else(|| author_key(message))
}

/// One full message card: avatar, author header, rich-text / Block Kit body and static
/// reaction chips. Shared with the thread panel and search so every column renders messages
/// identically. Cards without a link handler stay plain (not selectable, links open as
/// URLs).
pub fn message_row(
    fixtures: &Fixtures,
    message: &Message,
    handler: Option<&actions::ActionHandler>,
    cx: &App,
) -> Div {
    message_row_linked(fixtures, message, handler, None, None, cx)
}

/// [`message_row`] with its text selectable and links routed through `links` (the thread
/// panel: its events reach the root without a transcript). With `grouped` (the row's hover
/// group) the card continues the previous message's: no avatar or name, its time showing in
/// the gutter on hover, as in Slack.
pub fn message_row_linked(
    fixtures: &Fixtures,
    message: &Message,
    handler: Option<&actions::ActionHandler>,
    links: Option<&LinkHandler>,
    grouped: Option<SharedString>,
    cx: &App,
) -> Div {
    let prefs = crate::preferences::display_prefs(cx);
    let author_key = author_key(message);
    let author = author_name_with(message, fixtures, prefs.display_names_only);
    let kcx = match links.cloned() {
        Some(links) => KitCx::new(fixtures, None, &message.ts).with_links(links),
        None => KitCx::new(fixtures, None, &message.ts),
    };
    // A row with a handler (the thread panel) opens its images through it too.
    let kcx = match handler {
        Some(handler) => {
            let open = handler.clone();
            kcx.dispatching_actions(handler.clone())
                .opening_images(Rc::new(move |origin, window, cx| {
                    open(MessageAction::OpenImage(origin), window, cx)
                }))
        }
        None => kcx,
    };
    let mut body = v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .text_sm()
        .when(grouped.is_none(), |body| {
            body.child(message_header(
                message,
                &author,
                card_stamp(&message.ts, prefs.clock_24h),
                fixtures,
                handler,
                cx,
            ))
        })
        .child(message_body(message, &kcx, cx));
    if let Some(files) = crate::files::attachments(fixtures, message, kcx.image_open.as_ref(), cx) {
        body = body.child(files);
    }
    let key = format!("row-{}", message.ts.as_str());
    body = body.children(actions::reaction_chips(
        &key,
        &message.reactions,
        fixtures,
        None,
        cx,
    ));
    let compact = crate::preferences::display_prefs(cx).compact;
    let avatar_size = message_avatar_size(compact);
    let gutter = match grouped {
        // Slack's continuation gutter: the time, shown while the row is hovered.
        Some(group) => div()
            .w(px(avatar_size))
            .flex_none()
            .flex()
            .justify_center()
            .pt_0p5()
            .opacity(0.)
            .group_hover(group, |slot| slot.opacity(1.))
            .whitespace_nowrap()
            .when(compact, |el| el.text_size(px(10.)))
            .when(!compact, |el| el.text_xs())
            .text_color(cx.theme().muted_foreground)
            .child(format_clock(&message.ts))
            .into_any_element(),
        None => avatar_gutter(
            AvatarGutterSpec {
                fixtures,
                message,
                author_key: &author_key,
                author: &author,
                key: &key,
                size: avatar_size,
                handler,
            },
            cx,
        ),
    };
    let row = h_flex()
        .items_start()
        .gap(if compact { px(8.) } else { px(12.) })
        .min_w_0()
        .child(gutter)
        .child(body);
    if message.is_ephemeral {
        v_flex()
            .min_w_0()
            .child(actions::ephemeral_banner(&key, handler, cx))
            .child(row)
    } else {
        row
    }
}

/// `element` as a link to `user`'s profile, like Slack's message avatars and names; plain
/// when there is no user or no action handler (search and pinned cards). The link also
/// drives the member hover card: hover reports carry its key, so the pointer crossing
/// straight onto another trigger keeps the newer one.
pub(super) fn profile_link(
    element: impl IntoElement,
    id: String,
    user: Option<&UserId>,
    handler: Option<&actions::ActionHandler>,
) -> AnyElement {
    let (Some(user), Some(handler)) = (user, handler) else {
        return element.into_any_element();
    };
    let (user, handler) = (user.clone(), handler.clone());
    let (hover_user, hover_key) = (user.clone(), SharedString::from(format!("name-{id}")));
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .hover(|link| link.underline())
        .child(element)
        .on_click(move |_, window, cx| {
            handler(MessageAction::OpenProfile(user.clone()), window, cx)
        })
        .on_hover(move |inside: &bool, window, cx| {
            if *inside {
                let at = window.mouse_position();
                hover_card::entered(
                    Trigger {
                        key: hover_key.clone(),
                        user: hover_user.clone(),
                        anchor: Bounds::new(at, size(px(40.), px(20.))),
                    },
                    cx,
                );
            } else {
                hover_card::left(&hover_key, cx);
            }
        })
        .into_any_element()
}

/// A message's avatar gutter: the author's avatar (badged when they are from another
/// organization) as a link to their profile, [`AVATAR_TOP`] down like Slack's.
/// The 36px left-gutter author avatar shared by transcript and thread editing rows.
pub(crate) fn editing_avatar(
    fixtures: &Fixtures,
    message: &Message,
    key: &str,
    cx: &App,
) -> AnyElement {
    let key_id = author_key(message);
    let author = author_name(message, fixtures);
    let avatar_size = message_avatar_size(crate::preferences::display_prefs(cx).compact);
    avatar_gutter(
        AvatarGutterSpec {
            fixtures,
            message,
            author_key: &key_id,
            author: &author,
            key,
            size: avatar_size,
            handler: None,
        },
        cx,
    )
}

struct AvatarGutterSpec<'a> {
    fixtures: &'a Fixtures,
    message: &'a Message,
    author_key: &'a str,
    author: &'a str,
    key: &'a str,
    size: f32,
    handler: Option<&'a actions::ActionHandler>,
}

fn avatar_gutter(spec: AvatarGutterSpec<'_>, cx: &App) -> AnyElement {
    let base_avatar = crate::avatars::author_avatar(
        spec.fixtures,
        spec.message,
        spec.author_key,
        spec.author,
        spec.size,
    );
    let ring = cx.theme().background;
    let avatar = if spec
        .message
        .user
        .as_ref()
        .and_then(|u| spec.fixtures.external_team(u))
        .is_some()
    {
        crate::avatars::with_external_badge(
            spec.fixtures,
            spec.message.user.as_ref(),
            base_avatar,
            spec.size,
            ring,
        )
    } else if let Some(bot_team) = spec.fixtures.external_bot_team(spec.message) {
        crate::avatars::with_external_team_badge(bot_team, base_avatar, spec.size, ring)
    } else {
        base_avatar
    };
    let top = if spec.size < AVATAR_SIZE {
        3.
    } else {
        AVATAR_TOP
    };
    div()
        .flex_none()
        .mt(px(top))
        .child(profile_link(
            avatar,
            format!("{}-avatar", spec.key),
            spec.message.user.as_ref(),
            spec.handler,
        ))
        .into_any_element()
}

/// Slack's `.c-app_badge` after an app's name: 10px bold "APP" in the secondary text
/// color on a 14px tile with 2px corners.
fn app_badge(cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .h(px(14.))
        .px(px(3.))
        .rounded(px(2.))
        .bg(theme.secondary)
        .text_color(theme.muted_foreground)
        .text_size(px(10.))
        .line_height(px(12.))
        .font_weight(FontWeight::BOLD)
        .child("APP")
}

/// Slack's external-app ownership chip (`F-COLLAB-804`): displays the external workspace's icon
/// and name beside the `APP` badge so users can tell which external org owns a bot posting in a
/// Slack Connect channel.
fn external_app_chip(team: &slackless_core::types::Team, cx: &App) -> Div {
    let theme = cx.theme();
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .h(px(16.))
        .px_1p5()
        .rounded(px(3.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.secondary.opacity(0.65))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.muted_foreground)
        .child(crate::channel_details::org_chip(team, 11.))
        .child(team.name.clone())
}

fn message_header(
    message: &Message,
    author: &str,
    stamp: String,
    fixtures: &Fixtures,
    handler: Option<&actions::ActionHandler>,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    // Like Slack, the author's current (unexpired) status emoji follows their name.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        });
    let status = message
        .user
        .as_ref()
        .and_then(|user| fixtures.user(user))
        .map(|user| &user.profile)
        .filter(|profile| {
            profile
                .status_expiration
                .is_none_or(|end| end == 0 || end > now)
        })
        .and_then(|profile| profile.status_emoji.as_deref())
        .map(|code| code.trim_matches(':'))
        .filter(|code| !code.is_empty())
        .map(|code| crate::emoji::emoji_element(code, fixtures, px(14.)));
    let is_app_post = message.bot_id.is_some()
        && message
            .user
            .as_ref()
            .and_then(|user| fixtures.user(user))
            .is_none_or(|user| user.is_bot);
    let external_bot_team = is_app_post
        .then(|| fixtures.external_bot_team(message))
        .flatten();
    h_flex()
        .items_baseline()
        .gap_2()
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(profile_link(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(author.to_owned()),
                    format!("author-{}", message.ts.as_str()),
                    message.user.as_ref(),
                    handler,
                ))
                // An app's own post; a person's post made through an app (`/giphy`) keeps
                // their name unbadged, as in Slack.
                .when(is_app_post, |name| name.child(app_badge(cx)))
                .when_some(external_bot_team, |name, team| {
                    name.child(external_app_chip(team, cx))
                })
                .children(status),
        )
        .child(div().text_xs().text_color(muted).child(stamp))
}

/// Slack's day divider: a full-width rule with the day's pill (label, then a chevron) on it.
/// The date divider's "Jump to…" targets.
#[derive(Clone, Copy)]
pub enum Jump {
    Recent,
    Yesterday,
    LastWeek,
    LastMonth,
    Beginning,
}

fn date_divider(label: String, key: String, list: WeakEntity<MessageList>, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, background, fg) = (theme.border, theme.background, theme.foreground);
    // Slack's pill: 28px high, 13px bold, 16px before the label and 8px after its small
    // chevron, a 13% foreground ring and a faint drop shadow.
    let pill = Button::new(SharedString::from(key))
        .outline()
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(999.)))
        .h(px(28.))
        .pl(px(16.))
        .pr(px(8.))
        .bg(background)
        .border_color(fg.opacity(0.13))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .text_color(fg)
                .child(label),
        )
        .child(
            Icon::new(IconName::ChevronDown)
                .size(px(10.))
                .text_color(fg),
        );
    // `Button::shadow` toggles the kit's own shadow; this is the element's.
    let pill = gpui_kit::gpui::Styled::shadow(
        pill,
        vec![gpui_kit::gpui::BoxShadow {
            color: Hsla::black().opacity(0.08),
            offset: gpui_kit::gpui::point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
            inset: false,
        }],
    );
    h_flex()
        .items_center()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(border))
        .child(
            pill.dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                menu = menu.label("Jump to…");
                for (jump, text) in [
                    (Jump::Recent, "Most recent"),
                    (Jump::Yesterday, "Yesterday"),
                    (Jump::LastWeek, "Last week"),
                    (Jump::LastMonth, "Last month"),
                    (Jump::Beginning, "The very beginning"),
                ] {
                    let list = list.clone();
                    menu = menu.item(PopupMenuItem::new(text).on_click(move |_, _, cx| {
                        let _ = list.update(cx, |list, cx| list.jump(jump, cx));
                    }));
                }
                menu
            }),
        )
        .child(div().flex_1().h(px(1.)).bg(border))
}

fn is_membership(message: &Message) -> bool {
    matches!(
        message.subtype,
        Some(
            MessageSubtype::ChannelJoin
                | MessageSubtype::ChannelLeave
                | MessageSubtype::GroupJoin
                | MessageSubtype::GroupLeave
        )
    )
}

/// Whether `next` continues `message`'s run of joins (or of leaves), which Slack shows as one
/// line.
fn membership_run(message: &Message, next: &Message) -> bool {
    let leave = |m: &Message| {
        matches!(
            m.subtype,
            Some(MessageSubtype::ChannelLeave | MessageSubtype::GroupLeave)
        )
    };
    is_membership(message) && is_membership(next) && leave(message) == leave(next)
}

/// Slack's compact join / leave line: a person icon in the avatar gutter, "<name> joined
/// #channel." ("<name> and N others left #channel." for a run of `others` more) and the
/// time, all muted. Like Slack, the name opens that member's profile and the channel the
/// conversation.
fn membership_line(
    message: &Message,
    author: &str,
    others: usize,
    store: &MessageData,
    links: &LinkHandler,
    cx: &App,
) -> Div {
    let muted = cx.theme().muted_foreground;
    let channel = store
        .fixtures
        .conversation(&store.channel)
        .map(|c| store.fixtures.conversation_label(c).replacen("# ", "#", 1))
        .unwrap_or_default();
    let verb = match message.subtype {
        Some(MessageSubtype::ChannelLeave | MessageSubtype::GroupLeave) => "left",
        _ => "joined",
    };
    // The clickable spans: the member who joined (or the first of a run), then the channel.
    let member_span = |name: &str| {
        let id = format!("member-{}", message.ts.as_str());
        message.user.clone().map_or_else(
            || div().child(name.to_owned()).into_any_element(),
            |user| {
                membership_link(id, name.to_owned(), LinkTarget::Profile(user), links)
                    .into_any_element()
            },
        )
    };
    let channel_span = membership_link(
        format!("channel-{}", message.ts.as_str()),
        channel.clone(),
        LinkTarget::Channel(store.channel.clone()),
        links,
    );
    let avatar_size = message_avatar_size(crate::preferences::display_prefs(cx).compact);
    h_flex()
        .items_center()
        .gap_1()
        .py_1()
        .text_sm()
        .text_color(muted)
        .child(
            div()
                .w(px(avatar_size))
                .flex_none()
                .flex()
                .justify_center()
                .child(
                    Icon::new(IconName::UserRound)
                        .text_color(muted)
                        .size(px(15.)),
                ),
        )
        .child(match others {
            0 => h_flex()
                .gap_1()
                .child(member_span(author))
                .child(verb)
                .child(channel_span)
                .child("."),
            1 => h_flex()
                .gap_1()
                .child(member_span(author))
                .child("and 1 other")
                .child(verb)
                .child(channel_span)
                .child("."),
            n => h_flex()
                .gap_1()
                .child(member_span(author))
                .child(format!("and {n} others"))
                .child(verb)
                .child(channel_span)
                .child("."),
        })
        .child(div().text_xs().child(format_clock(&message.ts)))
}

/// One muted, hover-underlined clickable span of a join/leave line.
fn membership_link(
    id: String,
    label: String,
    target: LinkTarget,
    links: &LinkHandler,
) -> Stateful<Div> {
    let links = links.clone();
    div()
        .id(SharedString::from(id))
        .cursor_pointer()
        .hover(|el| el.underline())
        .on_click(move |_, window, cx| links.clone()(target.clone(), window, cx))
        .child(label)
}

/// Slack's unread marker: a danger-colored line with the "New" label at its right end.
fn new_messages_divider(cx: &App) -> Div {
    let danger = cx.theme().danger;
    h_flex()
        .items_center()
        .gap_2()
        .py_1()
        .child(div().flex_1().h(px(1.)).bg(danger))
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(danger)
                .child("New"),
        )
}

// ---------------------------------------------------------------------------
// Rich text rendering (guide section 10): a paragraph is one `StyledText` with styled
// `TextRun`s; links are runs made clickable through `InteractiveText` ranges, so a
// paragraph flows as a single block of text. Only inline images split it.
// ---------------------------------------------------------------------------
