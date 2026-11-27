//! Conversation header row and conversation tab bar.

use super::super::*;
use super::chrome_icon;

impl App {
    /// The conversation header row: star, peer avatar for DMs (presence dot for IMs, stacked
    /// chips for group DMs), the conversation name, the member count for channels, and the
    /// huddle / notifications / search / details icons on the right.
    pub(in crate::app) fn conversation_header(
        &self,
        title: &str,
        topic: &str,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted_foreground = cx.theme().muted_foreground;
        let accent = cx.theme().accent;
        let background = cx.theme().background;
        let success = cx.theme().success;
        let huddle_here = self.huddle_here();

        // Channels: the label already carries `# name`, which Slack keeps beside the channel
        // icon tile; a private channel's lock replaces the hash.
        let channel = conversation.filter(|c| {
            matches!(
                c.kind(),
                ConversationKind::PublicChannel | ConversationKind::PrivateChannel
            )
        });
        let section_emoji = channel.and_then(|c| {
            self.fixtures
                .sections
                .iter()
                .find(|section| !section.emoji.is_empty() && section.channel_ids.contains(&c.id))
                .map(|section| section.emoji.as_str())
                .filter(|emoji| {
                    crate::emoji::display_glyph(emoji).is_some()
                        || self.fixtures.custom_emoji(emoji).is_some()
                })
        });
        // Slack's tile beside the name shows the emoji of the channel's sidebar section.
        let channel_tile = section_emoji.map(|emoji| {
            let glyph = crate::emoji::emoji_element(emoji, &self.fixtures, px(18.));
            h_flex()
                .id("hdr-channel-tile")
                .size(px(28.))
                .flex_none()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .bg(cx.theme().secondary)
                .text_color(muted_foreground)
                .text_size(px(16.))
                .child(glyph)
        });
        let channel_lock = channel.filter(|c| c.is_private).map(|_| {
            Icon::new(IconName::Lock)
                .text_color(muted_foreground)
                .size(px(16.))
        });
        let header_title = if channel.is_some_and(|c| c.is_private) {
            title.strip_prefix("# ").unwrap_or(title).to_owned()
        } else {
            title.to_owned()
        };
        let peer = conversation
            .map(|c| conversation_avatar(&self.fixtures, c, 24., background, cx))
            .unwrap_or_else(div);
        // Slack's members chip: hover shows the "View all members" popover, a click opens
        // the details dialog straight onto its Members tab.
        let members_channel = channel.map(|c| c.id.clone());
        let members = conversation
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| self.ws.member_counts.get(&c.id).copied().or(c.num_members))
            .map(|count| {
                let popover_open = self.members_hover.0 || self.members_hover.1;
                div()
                    .id("hdr-members")
                    .relative()
                    .flex_none()
                    .cursor_pointer()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .px_1()
                            .h(px(28.))
                            .rounded_sm()
                            .text_sm()
                            .text_color(muted_foreground)
                            .child(
                                Icon::new(IconName::User)
                                    .text_color(muted_foreground)
                                    .size(px(16.)),
                            )
                            .child(count.to_string()),
                    )
                    // Slack keeps the pill lit while its tooltip is open, not only under
                    // the pointer.
                    .when(popover_open, |chip| chip.bg(accent))
                    .hover(|el| el.bg(accent))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.members_hover.0 = *hovered;
                        if *hovered {
                            this.load_channel_people(cx);
                        }
                        cx.notify();
                    }))
                    .when_some(members_channel.clone(), |chip, channel| {
                        chip.on_click(cx.listener(move |this, _, window, cx| {
                            this.members_hover = (false, false);
                            this.open_channel_details(
                                &channel,
                                channel_details::DetailsTab::Members,
                                window,
                                cx,
                            );
                        }))
                    })
                    .when(popover_open, |chip| {
                        chip.child(deferred(self.members_popover(cx)).with_priority(1))
                    })
            });
        // Slack counts the channel's apps apart: "👤 13  ⊞ 4".
        let apps = conversation
            .and_then(|c| self.ws.app_counts.get(&c.id).copied())
            .filter(|&apps| apps > 0)
            .map(|apps| {
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(
                        Icon::new(IconName::LayoutGrid)
                            .text_color(muted_foreground)
                            .size(px(16.)),
                    )
                    .child(apps.to_string())
            });

        // IMs: the peer avatar + name open the peer's profile, as in Slack.
        let peer_user = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.clone());
        // Channels: the name tile opens the details dialog, as in Slack.
        let details_channel = channel.map(|c| c.id.clone());
        // Slack Connect / Enterprise Grid external-org & channel-owner header badges
        // (`F-COLLAB-800`, `806`, `828`).
        let connect_badge: Option<AnyElement> = if let Some(c) = channel {
            if c.is_external_connect() {
                let ext_teams = self.fixtures.shared_teams(c);
                let owner = self.fixtures.channel_owner_team(c);
                let org_summary = match ext_teams.as_slice() {
                    [] => "External".to_owned(),
                    [only] => only.name.clone(),
                    [first, rest @ ..] => format!("{} +{}", first.name, rest.len()),
                };
                let label = match owner {
                    Some(host) => format!("{org_summary} · Owned by {}", host.name),
                    None => format!("External · {org_summary}"),
                };
                let ch_id = c.id.clone();
                let secondary = cx.theme().secondary;
                let border = cx.theme().border;
                Some(
                    h_flex()
                        .id("hdr-connect-badge")
                        .flex_none()
                        .max_w(px(240.))
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(2.))
                        .rounded_full()
                        .border_1()
                        .border_color(border)
                        .bg(secondary.opacity(0.6))
                        .cursor_pointer()
                        .hover(|el| el.bg(accent))
                        .child(if let Some(first) = ext_teams.first() {
                            div()
                                .flex_none()
                                .child(crate::channel_details::org_chip(first, 14.))
                                .into_any_element()
                        } else {
                            Icon::new(IconName::Building2)
                                .flex_none()
                                .size(px(12.))
                                .text_color(muted_foreground)
                                .into_any_element()
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted_foreground)
                                .child(label),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_channel_details(
                                &ch_id,
                                channel_details::DetailsTab::About,
                                window,
                                cx,
                            );
                        }))
                        .into_any_element(),
                )
            } else if c.is_org_shared {
                let border = cx.theme().border;
                let secondary = cx.theme().secondary;
                let text = if c.is_org_mandatory {
                    "Org-wide · Mandatory"
                } else {
                    "Org-wide"
                };
                Some(
                    h_flex()
                        .id("hdr-org-badge")
                        .flex_none()
                        .max_w(px(200.))
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py(px(2.))
                        .rounded_full()
                        .border_1()
                        .border_color(border)
                        .bg(secondary.opacity(0.5))
                        .child(
                            Icon::new(IconName::Building2)
                                .flex_none()
                                .size(px(12.))
                                .text_color(muted_foreground),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(muted_foreground)
                                .child(text),
                        )
                        .into_any_element(),
                )
            } else {
                None
            }
        } else if let Some(ref uid) = peer_user
            && self.fixtures.is_external_user(uid)
        {
            let ext_team = self.fixtures.external_team(uid);
            let label = match ext_team {
                Some(team) => format!("External · {}", team.name),
                None => "External".to_owned(),
            };
            let border = cx.theme().border;
            let secondary = cx.theme().secondary;
            let open_uid = uid.clone();
            Some(
                h_flex()
                    .id("hdr-external-dm-badge")
                    .flex_none()
                    .max_w(px(220.))
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py(px(2.))
                    .rounded_full()
                    .border_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.6))
                    .cursor_pointer()
                    .hover(|el| el.bg(accent))
                    .child(if let Some(team) = ext_team {
                        div()
                            .flex_none()
                            .child(crate::channel_details::org_chip(team, 14.))
                            .into_any_element()
                    } else {
                        Icon::new(IconName::Building2)
                            .flex_none()
                            .size(px(12.))
                            .text_color(muted_foreground)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted_foreground)
                            .child(label),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_profile(open_uid.clone(), cx);
                    }))
                    .into_any_element(),
            )
        } else {
            None
        };
        // Slack offers no huddle with yourself.
        let can_huddle = peer_user.as_ref() != Some(&self.fixtures.self_user);
        // Slack keeps the whole name; the topic after it truncates instead.
        let name = h_flex()
            .id("hdr-peer")
            .flex_none()
            .items_center()
            .gap_2()
            .px_1()
            .rounded_sm()
            .child(peer)
            .children(channel_tile)
            .children(channel_lock)
            .child(
                div()
                    .truncate()
                    .text_size(px(17.))
                    .font_weight(FontWeight::BOLD)
                    .child(header_title),
            )
            .when_some(peer_user, |name, user| {
                name.cursor_pointer().hover(|el| el.bg(accent)).on_click(
                    cx.listener(move |this, _, _, cx| this.open_profile(user.clone(), cx)),
                )
            })
            .when_some(details_channel, |name, channel| {
                name.cursor_pointer()
                    .hover(|el| el.bg(accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_channel_details(
                            &channel,
                            channel_details::DetailsTab::About,
                            window,
                            cx,
                        )
                    }))
            });

        h_flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(50.))
            .flex_none()
            .child(name)
            .children(connect_badge)
            .when(!topic.is_empty(), |header| {
                // Slack renders the topic's mrkdwn: mentions by name, links, emoji.
                header.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child(crate::messages::render_preview(
                            &crate::messages::memoized_preview(topic, &self.fixtures, cx),
                            muted_foreground,
                            format!("topic-{title}"),
                            cx,
                        )),
                )
            })
            .child(div().flex_1())
            .children(members)
            .children(apps)
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .when(can_huddle, |actions| {
                        let live_people = self.ws.active_huddles.get(&self.channel);
                        let live_count = live_people.map(Vec::len).unwrap_or(0);
                        let tooltip_text: SharedString = match live_people {
                            Some(users) if !users.is_empty() => {
                                let names: Vec<_> = users
                                    .iter()
                                    .take(5)
                                    .map(|u| self.fixtures.user_name(u))
                                    .collect();
                                let extra = users.len().saturating_sub(names.len());
                                if extra > 0 {
                                    format!("Join huddle: {} +{extra} more", names.join(", "))
                                        .into()
                                } else {
                                    format!("Join huddle: {}", names.join(", ")).into()
                                }
                            }
                            _ if huddle_here => "Open huddle window".into(),
                            _ => "Start a huddle".into(),
                        };
                        actions
                            .child(if live_count > 0 {
                                h_flex()
                                    .id("hdr-huddle")
                                    .h(px(28.))
                                    .px_2()
                                    .gap_1()
                                    .rounded_sm()
                                    .items_center()
                                    .bg(success.opacity(0.16))
                                    .cursor_pointer()
                                    .hover(|el| el.bg(success.opacity(0.24)))
                                    .tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(
                                            tooltip_text.clone(),
                                        )
                                        .build(window, cx)
                                    })
                                    .child(
                                        Icon::new(IconName::Headphones)
                                            .text_color(success)
                                            .size(px(14.)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(success)
                                            .child(live_count.to_string()),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.start_huddle(window, cx)
                                    }))
                                    .into_any_element()
                            } else {
                                chrome_icon(
                                    "hdr-huddle",
                                    IconName::Headphones,
                                    if huddle_here {
                                        success
                                    } else {
                                        muted_foreground
                                    },
                                    accent,
                                )
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(tooltip_text.clone())
                                        .build(window, cx)
                                })
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.start_huddle(window, cx)
                                    }),
                                )
                                .into_any_element()
                            })
                            .child(self.huddle_menu(cx))
                    })
                    .child(self.notify_menu(conversation, cx))
                    .child(
                        chrome_icon(
                            "hdr-summarize",
                            IconName::Sparkles,
                            muted_foreground,
                            accent,
                        )
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("Summarize channel")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_summary_for_channel(window, cx);
                        })),
                    )
                    .child(
                        chrome_icon("hdr-search", IconName::Search, muted_foreground, accent)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search_in_conversation(window, cx)
                            })),
                    )
                    .child(self.conversation_menu(conversation, cx)),
            )
    }

    /// Effective tab specs for `conversation`, prepending the `Home` tab for app/bot DMs.
    pub(in crate::app) fn conversation_tab_specs(
        &self,
        conversation: &Conversation,
    ) -> Vec<tabs::TabSpec> {
        let is_app_dm = conversation.is_im
            && conversation
                .user
                .as_ref()
                .and_then(|u| self.fixtures.user(u))
                .is_some_and(|u| u.is_bot);
        let specs = tabs::derive(conversation, self.fixtures.sample);
        if is_app_dm {
            tabs::with_app_home(specs)
        } else {
            specs
        }
    }

    /// The tab row under the header, derived from the conversation's `properties.tabs` in
    /// live mode ([`tabs`] for the rules) or the sample workspace's fixed set, with the
    /// active tab underlined. The trailing "+" opens Slack's new-tab menu, restricted to
    /// the actions slackless can run: Canvas (live only) and Link.
    pub(in crate::app) fn conversation_tabs(&self, cx: &mut Context<Self>) -> Div {
        let border = cx.theme().border;
        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;

        let fallback = Conversation::default();
        let conversation = self
            .fixtures
            .conversation(&self.channel)
            .unwrap_or(&fallback);
        let specs = self.conversation_tab_specs(conversation);
        let at_tab_limit = !tabs::can_add_tab(&specs);
        let canvas_item_label = if tabs::has_canvas_tab(&specs) {
            "Canvas"
        } else {
            "Add canvas"
        };
        let tab_limit_tip = tabs::tab_limit_message(&specs).unwrap_or_else(|| "New tab".to_owned());
        let this = cx.entity().downgrade();
        let sample = self.fixtures.sample;
        let ch_for_add = self.channel.clone();
        let add_menu = Button::new("tab-add")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Plus)
            .disabled(at_tab_limit)
            .tooltip(tab_limit_tip)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if !sample {
                    let this_canvas = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(canvas_item_label)
                            .icon(IconName::LayoutTemplate)
                            .disabled(at_tab_limit)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this_canvas.update(cx, |app, cx| app.open_or_add_canvas(cx));
                            }),
                    );
                    let this_files = this.clone();
                    let ch_files = ch_for_add.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Files & links")
                            .icon(IconName::Files)
                            .disabled(at_tab_limit)
                            .on_click(move |_, _, cx| {
                                let _ = this_files.update(cx, |app, _| {
                                    app.bridge.send(ChannelCommand::AddConversationTab {
                                        channel: ch_files.clone(),
                                        tab_type: "files".to_owned(),
                                        label: None,
                                    });
                                });
                            }),
                    );
                    let this_pins = this.clone();
                    let ch_pins = ch_for_add.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Pins & bookmarks")
                            .icon(IconName::Pin)
                            .disabled(at_tab_limit)
                            .on_click(move |_, _, cx| {
                                let _ = this_pins.update(cx, |app, _| {
                                    app.bridge.send(ChannelCommand::AddConversationTab {
                                        channel: ch_pins.clone(),
                                        tab_type: "bookmarks".to_owned(),
                                        label: None,
                                    });
                                });
                            }),
                    );
                }
                let this = this.clone();
                menu.item(
                    PopupMenuItem::new("Link")
                        .icon(IconName::Link)
                        .disabled(at_tab_limit)
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
                        }),
                )
            });
        let tab_ids: Vec<Option<String>> = specs.iter().map(|s| s.tab_id.clone()).collect();
        let ch_for_tabs = self.channel.clone();
        let this_for_tabs = cx.entity().downgrade();
        h_flex()
            .items_stretch()
            .flex_none()
            .px_4()
            .gap_5()
            .h(px(40.))
            .border_b_1()
            .border_color(border)
            .children(specs.into_iter().enumerate().map(|(ix, spec)| {
                let active = match &spec.action {
                    TabAction::Switch(target) => {
                        *target == self.tab
                            && !(*target == ConversationTab::Pins && !self.right_pane.is_pins())
                    }
                    _ => false,
                };
                let tab_id = spec.tab_id;
                let entry = tab_views::tab(
                    spec.id,
                    spec.label,
                    spec.icon,
                    active,
                    foreground,
                    muted_foreground,
                    foreground,
                );
                let clickable = match spec.action {
                    TabAction::Switch(target) => {
                        entry.on_click(cx.listener(move |this, _, _, cx| {
                            this.select_tab(target.clone(), cx);
                        }))
                    }
                    action => entry.on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_tab(action.clone(), cx);
                    })),
                };
                if !sample && let Some(tid) = tab_id {
                    let prev_after = if ix >= 2 {
                        tab_ids.get(ix - 2).cloned().flatten()
                    } else {
                        None
                    };
                    let can_left = ix > 1 && tab_ids.get(ix - 1).is_some_and(Option::is_some);
                    let next_after = tab_ids.get(ix + 1).cloned().flatten();
                    let ch_menu = ch_for_tabs.clone();
                    let app_menu = this_for_tabs.clone();
                    return clickable
                        .context_menu(move |mut menu, _, _| {
                            if can_left {
                                let ch = ch_menu.clone();
                                let tid = tid.clone();
                                let after = prev_after.clone();
                                let app = app_menu.clone();
                                menu = menu.item(
                                    PopupMenuItem::new("Move left")
                                        .icon(IconName::ArrowLeft)
                                        .on_click(move |_, _, cx| {
                                            let _ = app.update(cx, |app, _| {
                                                app.bridge.send(
                                                    ChannelCommand::MoveConversationTab {
                                                        channel: ch.clone(),
                                                        tab_id: tid.clone(),
                                                        after: after.clone(),
                                                    },
                                                );
                                            });
                                        }),
                                );
                            }
                            if let Some(after) = next_after.clone() {
                                let ch = ch_menu.clone();
                                let tid = tid.clone();
                                let app = app_menu.clone();
                                menu = menu.item(
                                    PopupMenuItem::new("Move right")
                                        .icon(IconName::ArrowRight)
                                        .on_click(move |_, _, cx| {
                                            let _ = app.update(cx, |app, _| {
                                                app.bridge.send(
                                                    ChannelCommand::MoveConversationTab {
                                                        channel: ch.clone(),
                                                        tab_id: tid.clone(),
                                                        after: Some(after.clone()),
                                                    },
                                                );
                                            });
                                        }),
                                );
                            }
                            let ch = ch_menu.clone();
                            let tid = tid.clone();
                            let app = app_menu.clone();
                            menu.item(
                                PopupMenuItem::new("Remove tab")
                                    .icon(IconName::Trash)
                                    .on_click(move |_, _, cx| {
                                        let _ = app.update(cx, |app, _| {
                                            app.bridge.send(
                                                ChannelCommand::RemoveConversationTab {
                                                    channel: ch.clone(),
                                                    tab_id: tid.clone(),
                                                },
                                            );
                                        });
                                    }),
                            )
                        })
                        .into_any_element();
                }
                clickable.into_any_element()
            }))
            // The stretched row would top-align the small button: center it, as the tab
            // labels are.
            .child(h_flex().items_center().child(add_menu))
    }
}
