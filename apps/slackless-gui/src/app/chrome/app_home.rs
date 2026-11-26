//! Huddle thread toggle, App Home Block Kit tab, app DM banner, and peer DND banner.

use super::super::*;

impl App {
    /// Open or toggle the active huddle's thread in the right-hand thread panel
    /// (`F-COLLAB-014`, `F-COLLAB-015`). Targets the huddle's own channel (even when browsing
    /// another channel while the huddle bar is open), using its newest `huddle_thread` trail
    /// message (falling back to the newest message in sample mode).
    pub(in crate::app) fn toggle_huddle_thread(&mut self, cx: &mut Context<Self>) {
        // The dock's session: the core's snapshot with the room's meta merged in, which is
        // where a thread started after joining arrives.
        let session = self.huddle_state.as_ref().map(|state| {
            let session = state.read(cx).session();
            (session.channel.clone(), session.thread_root_ts.clone())
        });
        let huddle_channel = (session.as_ref())
            .map(|(channel, _)| channel.clone())
            .or_else(|| huddle_state::fixture_huddle(&self.fixtures).map(|h| h.channel))
            .unwrap_or_else(|| self.channel.clone());
        let huddle_ts = session
            .and_then(|(_, thread_root_ts)| thread_root_ts)
            .or_else(|| {
                if huddle_channel == self.channel {
                    self.messages
                        .read(cx)
                        .latest_huddle_thread_ts(self.fixtures.sample, cx)
                } else {
                    self.fixtures
                        .messages
                        .get(&huddle_channel)
                        .and_then(|msgs| {
                            msgs.iter()
                                .rev()
                                .find(|m| {
                                    m.subtype
                                        == Some(slackless_core::types::MessageSubtype::HuddleThread)
                                })
                                .or_else(|| self.fixtures.sample.then(|| msgs.last()).flatten())
                                .map(|m| m.ts.clone())
                        })
                }
            });
        let Some(ts) = huddle_ts else {
            return;
        };
        if self.right_pane.is_thread()
            && self.thread.read(cx).open_thread() == Some((huddle_channel.clone(), ts.clone()))
        {
            self.close_thread(cx);
        } else {
            self.open_thread(huddle_channel, ts, cx);
        }
    }

    /// Render the active app DM's Home tab (`F-COLLAB-480`): displays published Block Kit
    /// `home_view.blocks` with interactive `blocks.actions` (`container: {"type":"view",...}`),
    /// interactive `BlockKitState` inputs/selects, link routing, and a Refresh button.
    pub(in crate::app) fn app_home_view(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, muted, fg) = (theme.border, theme.muted_foreground, theme.foreground);
        let channel = self.channel.clone();
        let app_name = self
            .fixtures
            .conversation(&channel)
            .and_then(|c| c.user.as_ref())
            .map(|u| self.fixtures.user_name(u).to_owned())
            .unwrap_or_else(|| "App".to_owned());

        if self.fixtures.sample && !self.ws.app_homes.contains_key(&channel) {
            return div()
                .id("app-home-scroll")
                .size_full()
                .overflow_y_scroll()
                .child(
                v_flex()
                    .px_6()
                    .py_5()
                    .gap_3()
                    .max_w(px(760.))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child(format!("{app_name} Home")),
                    )
                    .child(div().text_sm().text_color(muted).child(
                        "App Home tabs load live Block Kit dashboards published by workspace apps.",
                    )),
            );
        }

        let header = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::new(IconName::House).size(px(16.)).text_color(muted))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(format!("{app_name} Home")),
                    ),
            )
            .child(
                Button::new("app-home-refresh")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::RefreshCw)
                    .label("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ws.app_homes.remove(&this.channel);
                        this.bridge
                            .send(CollabCommand::LoadAppHome(this.channel.clone()));
                        cx.notify();
                    })),
            );

        let body: AnyElement = match self.ws.app_homes.get(&channel) {
            None => div()
                .py_8()
                .text_sm()
                .text_color(muted)
                .child("Loading Home tab\u{2026}")
                .into_any_element(),
            Some(Err(err)) => div()
                .py_8()
                .text_sm()
                .text_color(muted)
                .child(format!("Could not load Home tab: {err}"))
                .into_any_element(),
            Some(Ok(home))
                if home.home_tab_enabled
                    && let Some(view) = home.home_view.as_ref() =>
            {
                let view_id = view["id"].as_str().unwrap_or_default().to_owned();
                let app_id = view["app_id"].as_str().unwrap_or_default().to_owned();
                let bot_id = view["bot_id"]
                    .as_str()
                    .map(slackless_core::types::BotId::from)
                    .unwrap_or_else(|| slackless_core::types::BotId::from(app_id.as_str()));
                let team = view["app_installed_team_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(slackless_core::types::TeamId::from);
                let raw_blocks: Vec<crate::blockkit::Json> =
                    view["blocks"].as_array().cloned().unwrap_or_default();
                if raw_blocks.is_empty() {
                    div()
                        .py_8()
                        .text_sm()
                        .text_color(muted)
                        .child("This app's Home tab is currently empty.")
                        .into_any_element()
                } else {
                    let ts = Ts::from(format!("home-{view_id}"));
                    crate::blockkit::prepare_raw_inputs(
                        &self.app_home_kit,
                        &ts,
                        &raw_blocks,
                        window,
                        cx,
                    );
                    let this_action = cx.entity().downgrade();
                    let ch_action = channel.clone();
                    let (act_bot, act_app, act_team, act_view) = (
                        bot_id.clone(),
                        app_id.clone(),
                        team.clone(),
                        view_id.clone(),
                    );
                    let on_action: crate::actions::ActionHandler =
                        Rc::new(move |action, _window, cx| {
                            if let crate::actions::MessageAction::BlockAction { action, .. } =
                                action
                            {
                                let _ = this_action.update(cx, |app, _| {
                                    app.bridge.send(CollabCommand::BlockAction {
                                        bot: act_bot.clone(),
                                        app_id: act_app.clone(),
                                        team: act_team.clone(),
                                        action,
                                        container: slackless_core::api::ActionContainer {
                                            channel: ch_action.clone(),
                                            message_ts: Ts::default(),
                                            attachment_id: None,
                                            is_ephemeral: false,
                                            view_id: Some(act_view.clone()),
                                        },
                                    });
                                });
                            }
                        });
                    let this_suggest = cx.entity().downgrade();
                    let ch_suggest = channel.clone();
                    let on_suggest: crate::blockkit::SuggestHandler = Rc::new(move |req, cx| {
                        let _ = this_suggest.update(cx, |app, cx| {
                            if app.fixtures.sample {
                                let opts =
                                    crate::app::dialogs::sample_external_suggestions(&req.value);
                                app.app_home_kit.update(cx, |kit, cx| {
                                    kit.set_suggestions(req.key.clone(), opts);
                                    cx.notify();
                                });
                            } else {
                                let team = team.clone().unwrap_or_else(|| app.workspace_team_id());
                                app.bridge.send(CollabCommand::BlockSuggestions {
                                    key: req.key,
                                    bot: bot_id.clone(),
                                    app_id: app_id.clone(),
                                    team,
                                    action_id: req.action_id,
                                    block_id: req.block_id,
                                    value: req.value,
                                    container: slackless_core::api::ActionContainer {
                                        channel: ch_suggest.clone(),
                                        message_ts: Ts::default(),
                                        attachment_id: req.attachment,
                                        is_ephemeral: false,
                                        view_id: Some(view_id.clone()),
                                    },
                                });
                            }
                        });
                    });
                    let this_wf = cx.entity().downgrade();
                    let on_wf: crate::blockkit::WorkflowDetailsHandler =
                        Rc::new(move |req, window, cx| {
                            let _ = this_wf
                                .update(cx, |app, cx| app.open_workflow_details(req, window, cx));
                        });
                    let this_call = cx.entity().downgrade();
                    let on_call: crate::blockkit::CallRefreshHandler =
                        Rc::new(move |call_id, cx| {
                            let _ = this_call.update(cx, |app, _| {
                                if !app.fixtures.sample {
                                    app.bridge.send(CollabCommand::FetchCallInfo { call_id });
                                }
                            });
                        });
                    let this_link = cx.entity().downgrade();
                    let links: crate::messages::LinkHandler = Rc::new(move |target, window, cx| {
                        let _ = this_link.update(cx, |app, cx| {
                            app.open_link(target, window, cx);
                        });
                    });
                    let kcx =
                        crate::blockkit::KitCx::new(&self.fixtures, Some(&self.app_home_kit), &ts)
                            .with_links(links)
                            .dispatching_actions(on_action)
                            .with_suggestions(on_suggest)
                            .with_workflow_details(on_wf)
                            .with_call_refresh(on_call);
                    crate::blockkit::render_raw_blocks(&raw_blocks, &kcx, cx).into_any_element()
                }
            }
            Some(Ok(_)) => div()
                .py_8()
                .text_sm()
                .text_color(muted)
                .child(format!("{app_name} has not published a Home tab."))
                .into_any_element(),
        };

        div()
            .id("app-home-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                v_flex()
                    .px_6()
                    .py_5()
                    .gap_4()
                    .max_w(px(860.))
                    .child(header)
                    .child(body),
            )
    }

    /// App DM info & quick-action strip rendered above the transcript when the active
    /// conversation is a direct message with an app / bot user (`F-COLLAB-482`).
    pub(in crate::app) fn app_dm_banner(
        &self,
        conversation: Option<&Conversation>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let peer_id = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.as_ref())?;
        let user = self.fixtures.user(peer_id)?;
        if !user.is_bot {
            return None;
        }
        let name = self.fixtures.name_of(user).to_owned();
        let desc = user
            .profile
            .title
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let peer_for_profile = peer_id.clone();
        let peer_for_copy = peer_id.as_str().to_owned();
        let theme = cx.theme();
        let (border, muted, fg, badge_bg) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.secondary,
        );
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_1p5()
                .bg(badge_bg.opacity(0.45))
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .min_w_0()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(IconName::Bot).size(px(14.)).text_color(muted))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child(name),
                        )
                        .child(
                            div()
                                .px_1()
                                .py(px(1.))
                                .rounded_sm()
                                .bg(badge_bg)
                                .text_size(px(10.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(muted)
                                .child("APP"),
                        )
                        .children(desc.map(|title| {
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(title)
                        })),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("app-dm-copy-id")
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("Copy ID")
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        peer_for_copy.clone(),
                                    ));
                                    notify("App ID copied", peer_for_copy.clone(), window, cx);
                                }),
                        )
                        .child(
                            Button::new("app-dm-about")
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("About app")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_profile(peer_for_profile.clone(), cx);
                                })),
                        ),
                ),
        )
    }

    /// Slack's paused-notifications strip for a DM whose peer has do-not-disturb on: a
    /// muted row between the message list and the composer.
    pub(in crate::app) fn dnd_banner(
        &self,
        conversation: Option<&Conversation>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let peer = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.as_ref())?;
        if !self.ws.peer_dnd_paused.get(peer).copied().unwrap_or(false) {
            return None;
        }
        let muted = cx.theme().muted;
        let muted_foreground = cx.theme().muted_foreground;
        // Slack's notice: a rounded box aligned with the composer, worded for the reader.
        Some(
            h_flex()
                .items_center()
                .gap_2()
                .mx_4()
                .mb_1()
                .px_3()
                .py_1p5()
                .flex_none()
                .rounded_md()
                .bg(muted)
                .child(
                    Icon::new(IconName::BellOff)
                        .size(px(14.))
                        .text_color(muted_foreground),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("They will see your messages when they're back."),
                ),
        )
    }

    /// Compact, subtle strip right above the composer listing any messages scheduled for the
    /// active conversation, with inline `Send now` and `Cancel` actions.
    pub(in crate::app) fn scheduled_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let items = self
            .drafts
            .read(cx)
            .scheduled_for_channel(self.workspace, &self.channel);
        if items.is_empty() {
            return None;
        }
        let now = now_secs();
        let theme = cx.theme();
        let (border, muted_fg, fg, bg) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.secondary.opacity(0.32),
        );
        let rows = items
            .into_iter()
            .enumerate()
            .map(|(row_ix, (sched_ix, entry))| {
                let when = crate::drafts::when_label(entry.at, now);
                let preview = entry
                    .text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2p5()
                    .px_3()
                    .py_1p5()
                    .when(row_ix > 0, |row| {
                        row.border_t_1().border_color(border.opacity(0.55))
                    })
                    .child(
                        Icon::new(IconName::Clock)
                            .flex_none()
                            .size(px(13.))
                            .text_color(muted_fg),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(format!("Scheduled \u{b7} sends {when}")),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .truncate()
                                    .text_sm()
                                    .text_color(fg.opacity(0.88))
                                    .child(preview),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new(("chat-sched-send", row_ix))
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .label("Send now")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let taken = this
                                            .drafts
                                            .update(cx, |d, cx| d.take_scheduled(sched_ix, cx));
                                        if let Some(entry) = taken {
                                            this.deliver(entry, window, cx);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new(("chat-sched-cancel", row_ix))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .label("Cancel")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let taken = this
                                            .drafts
                                            .update(cx, |d, cx| d.take_scheduled(sched_ix, cx));
                                        if let Some(entry) = taken {
                                            if let Some(id) = entry.id {
                                                this.bridge.send(
                                                    MessagingCommand::DeleteScheduledMessage {
                                                        channel: entry.channel,
                                                        scheduled_message_id: id,
                                                    },
                                                );
                                            }
                                            notify(
                                                "Scheduled message cancelled",
                                                "Removed from the send queue.",
                                                window,
                                                cx,
                                            );
                                            cx.notify();
                                        }
                                    })),
                            ),
                    )
            });
        Some(
            div()
                .flex_none()
                .mx_4()
                .mb_1p5()
                .rounded_md()
                .border_1()
                .border_color(border.opacity(0.65))
                .bg(bg)
                .overflow_hidden()
                .child(
                    v_flex()
                        .id("chat-scheduled-list")
                        .max_h(px(144.))
                        .overflow_y_scroll()
                        .children(rows),
                ),
        )
    }
}
