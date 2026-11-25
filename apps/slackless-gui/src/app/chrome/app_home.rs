//! Huddle thread toggle, App Home Block Kit tab, app DM banner, and peer DND banner.

use super::super::*;

impl App {
    /// Open or toggle the active huddle's thread in the right-hand thread panel
    /// (`F-COLLAB-014`, `F-COLLAB-015`). Targets the huddle's own channel (even when browsing
    /// another channel while the huddle bar is open), using its newest `huddle_thread` trail
    /// message (falling back to the newest message in sample mode).
    pub(in crate::app) fn toggle_huddle_thread(&mut self, cx: &mut Context<Self>) {
        let huddle_channel = self
            .huddle_session
            .as_ref()
            .map(|s| s.channel.clone())
            .or_else(|| self.huddle.as_ref().map(|(ch, _)| ch.clone()))
            .unwrap_or_else(|| self.channel.clone());
        let huddle_ts = self
            .huddle_session
            .as_ref()
            .and_then(|s| s.thread_root_ts.clone())
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
        if self.thread_open
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
                        this.bridge.send(Command::LoadAppHome(this.channel.clone()));
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
                    let on_action: crate::actions::ActionHandler =
                        Rc::new(move |action, _window, cx| {
                            if let crate::actions::MessageAction::BlockAction { action, .. } =
                                action
                            {
                                let _ = this_action.update(cx, |app, _| {
                                    app.bridge.send(Command::BlockAction {
                                        bot: bot_id.clone(),
                                        app_id: app_id.clone(),
                                        team: team.clone(),
                                        action,
                                        container: slackless_core::api::ActionContainer {
                                            channel: ch_action.clone(),
                                            message_ts: Ts::default(),
                                            attachment_id: None,
                                            is_ephemeral: false,
                                            view_id: Some(view_id.clone()),
                                        },
                                    });
                                });
                            }
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
                            .dispatching_actions(on_action);
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
}
