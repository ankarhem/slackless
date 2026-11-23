//! Window chrome, top bar, banners, conversation header, menus, join card, and tabs.

use super::*;

/// A black box shadow (Slack's `box-shadow: <offset> <offset> <blur> rgba(0,0,0,<alpha>)`).
pub(super) fn shadow(offset: f32, alpha: f32, blur: f32) -> BoxShadow {
    BoxShadow {
        color: hsla(0., 0., 0., alpha),
        offset: point(px(offset), px(offset)),
        blur_radius: px(blur),
        spread_radius: px(0.),
        inset: false,
    }
}

/// One right corner (`top` or bottom) of the workspace panel over a side panel that paints a
/// square background: GPUI clips rectangularly, so a band whose inner edge is the corner's arc
/// repaints the window chrome outside it, and dark mode's `outline` is redrawn along it.
pub(super) fn corner_cap(top: bool, chrome: Hsla, outline: Option<Hsla>) -> Vec<Div> {
    let band = PANEL_RADIUS;
    // Absolute insets start inside the panel's outline.
    let edge = if outline.is_some() { 1. } else { 0. };
    let cap = div()
        .absolute()
        .right(px(-band - edge))
        .size(px(2. * (PANEL_RADIUS + band)))
        .border_r(px(band))
        .border_color(chrome);
    let mut caps = vec![if top {
        cap.top(px(-band - edge))
            .border_t(px(band))
            .rounded_tr(px(PANEL_RADIUS + band))
    } else {
        cap.bottom(px(-band - edge))
            .border_b(px(band))
            .rounded_br(px(PANEL_RADIUS + band))
    }];
    if let Some(outline) = outline {
        let arc = div()
            .absolute()
            .right(px(-edge))
            .size(px(2. * PANEL_RADIUS))
            .border_r_1()
            .border_color(outline);
        caps.push(if top {
            arc.top(px(-edge)).border_t_1().rounded_tr(px(PANEL_RADIUS))
        } else {
            arc.bottom(px(-edge))
                .border_b_1()
                .rounded_br(px(PANEL_RADIUS))
        });
    }
    caps
}

impl App {
    /// The huddle button's chevron: Slack's huddle menu.
    pub(super) fn huddle_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let link = self.huddle_link();
        let start = if self.huddle_here() {
            "Show huddle"
        } else {
            "Start huddle"
        };
        Button::new("hdr-huddle-menu")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::ChevronDown)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let this = this.clone();
                let link = link.clone();
                menu.item(
                    PopupMenuItem::new(start)
                        .icon(IconName::Headphones)
                        .on_click(move |_, window, cx| {
                            let _ = this.update(cx, |app, cx| app.start_huddle(window, cx));
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy huddle link")
                        .icon(IconName::Link)
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                        }),
                )
            })
    }

    /// The header bell: Slack's "Notify you about…" menu for this conversation, checked at
    /// its current level; the icon is a crossed-out bell while muted.
    pub(super) fn notify_menu(
        &self,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let channel = conversation.map(|c| c.id.clone());
        let level = channel
            .as_ref()
            .and_then(|c| self.fixtures.notify_levels.get(c).copied())
            .unwrap_or(NotifyLevel::Everything);
        let icon = if level == NotifyLevel::Muted {
            IconName::BellOff
        } else {
            IconName::Bell
        };
        Button::new("hdr-bell")
            .ghost()
            .with_size(Size::Small)
            .icon(icon)
            .disabled(channel.is_none())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                menu = menu.label("Notify you about…");
                for (choice, label) in [
                    (NotifyLevel::Everything, "All new posts"),
                    (NotifyLevel::Mentions, "Just mentions"),
                    (NotifyLevel::Muted, "Mute and hide"),
                ] {
                    let (this, channel) = (this.clone(), channel.clone());
                    menu = menu.item(PopupMenuItem::new(label).checked(choice == level).on_click(
                        move |_, _, cx| {
                            let Some(channel) = channel.clone() else {
                                return;
                            };
                            let _ = this.update(cx, |app, _| {
                                app.bridge.send(Command::SetNotifyLevel {
                                    channel,
                                    level: choice,
                                })
                            });
                        },
                    ));
                }
                menu
            })
    }

    /// The header's ⋮ menu: what slackless can do for this conversation (Slack's menu minus
    /// the workflow, template, split-view and window items it has no backend for).
    pub(super) fn conversation_menu(
        &self,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let channel = conversation.map(|c| c.id.clone());
        let channel_name = conversation
            .filter(|c| {
                matches!(
                    c.kind(),
                    ConversationKind::PublicChannel | ConversationKind::PrivateChannel
                )
            })
            .and_then(|c| c.name.as_deref())
            .map(|name| format!("#{name}"));
        let peer = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.clone());
        let link = channel.as_ref().map(|id| self.conversation_link(id));
        Button::new("hdr-more")
            .ghost()
            .with_size(Size::Small)
            .icon(IconName::EllipsisVertical)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if let Some(user) = peer.clone() {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new("View full profile")
                            .icon(IconName::CircleUser)
                            .on_click(move |_, _, cx| {
                                let _ =
                                    this.update(cx, |app, cx| app.open_profile(user.clone(), cx));
                            }),
                    );
                }
                if let Some(link) = link.clone() {
                    let label = if channel_name.is_some() {
                        "Copy link to channel"
                    } else {
                        "Copy link to conversation"
                    };
                    menu = menu.item(PopupMenuItem::new(label).icon(IconName::Link).on_click(
                        move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                        },
                    ));
                }
                if let Some(name) = channel_name.clone() {
                    menu = menu.item(
                        PopupMenuItem::new("Copy channel name")
                            .icon(IconName::Copy)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
                            }),
                    );
                }
                if channel.is_some() {
                    let this_md = this.clone();
                    let this_json = this.clone();
                    menu = menu
                        .separator()
                        .item(
                            PopupMenuItem::new("Export transcript (Markdown…)")
                                .icon(IconName::Download)
                                .on_click(move |_, window, cx| {
                                    let _ = this_md.update(cx, |app, cx| {
                                        app.export_conversation_transcript(true, window, cx)
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Export transcript (JSON…)")
                                .icon(IconName::FileCode)
                                .on_click(move |_, window, cx| {
                                    let _ = this_json.update(cx, |app, cx| {
                                        app.export_conversation_transcript(false, window, cx)
                                    });
                                }),
                        );
                }
                if let (Some(_), Some(channel)) = (&channel_name, channel.clone()) {
                    let this = this.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("Leave channel")
                            .icon(IconName::LogOut)
                            .on_click(move |_, window, cx| {
                                let _ = this
                                    .update(cx, |app, cx| app.leave(channel.clone(), window, cx));
                            }),
                    );
                }
                menu
            })
    }

    /// The global bar (gpui-kit `TitleBar`): history navigation on the left, the wide
    /// centered search pill, and the workspace chip, theme toggle and help on the right —
    /// the reference screenshot's top row.
    pub(super) fn global_bar(&mut self, cx: &mut Context<Self>) -> TitleBar {
        // The top bar sits directly over `title_bar` chrome (which may be dark-tinted even in
        // Light mode under Aubergine/Jade/Clementine/etc. or Darker sidebars), so all icons,
        // text, and translucent pill washes derive from `sidebar_foreground`.
        let chrome_fg = cx.theme().sidebar_foreground;
        let chrome_muted = chrome_fg.opacity(0.75);
        let pill = chrome_fg.opacity(0.14);
        let chrome_hover = chrome_fg.opacity(0.12);

        let this = cx.entity().downgrade();
        let recent = self.recent_entries();
        let nav = h_flex()
            .items_center()
            .gap_1()
            .text_color(chrome_fg)
            .child(
                Button::new("nav-back")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(Icon::new(IconName::ArrowLeft).text_color(chrome_muted))
                    .tooltip("Back")
                    .disabled(self.back.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_back(cx))),
            )
            .child(
                Button::new("nav-forward")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(Icon::new(IconName::ArrowRight).text_color(chrome_muted))
                    .tooltip("Forward")
                    .disabled(self.forward.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.go_forward(cx))),
            )
            .child(
                Button::new("nav-history")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(Icon::new(IconName::Clock).text_color(chrome_muted))
                    .tooltip("History")
                    .dropdown_menu_with_anchor(Anchor::TopLeft, move |mut menu, _, _| {
                        menu = menu.label("Recent");
                        for (id, label, icon) in &recent {
                            let (this, id) = (this.clone(), id.clone());
                            menu =
                                menu.item(PopupMenuItem::new(label.clone()).icon(*icon).on_click(
                                    move |_, _, cx| {
                                        let _ =
                                            this.update(cx, |app, cx| app.select(id.clone(), cx));
                                    },
                                ));
                        }
                        menu
                    }),
            );

        // The dropdown hangs under the pill: capture its window bounds as it paints
        // (stored only, never notified — the panel's rows are computed on open and query
        // change, never per frame).
        let menu = self.search_menu.downgrade();
        let search_pill = div()
            .w(px(820.))
            .min_w(px(200.))
            .on_children_prepainted(move |children, _, cx| {
                if let Some(bounds) = children.first() {
                    let _ = menu.update(cx, |menu, _| menu.set_pill_bounds(*bounds));
                }
            })
            .child(
                h_flex()
                    .w_full()
                    .h(px(26.))
                    .items_center()
                    .gap_2()
                    .px_2()
                    .rounded(px(6.))
                    .bg(pill)
                    .text_color(chrome_fg)
                    // Like Slack, a click anywhere on the pill focuses search (and never
                    // drags the window, as the rest of the title bar does) and opens the
                    // dropdown under it.
                    .cursor_text()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            window.prevent_default();
                            this.search.read(cx).focus_handle(cx).focus(window, cx);
                            this.open_search_menu(cx);
                        }),
                    )
                    .child(
                        Icon::new(IconName::Search)
                            .text_color(chrome_muted)
                            .size(px(14.)),
                    )
                    .child(
                        // While the dropdown is open the field renders inside the panel;
                        // when closed, show the query or placeholder directly in chrome colors so
                        // Light mode + tinted dark chrome never shows low-contrast dark placeholder text.
                        {
                            let current = self.search.read(cx).value().to_string();
                            let empty = current.is_empty();
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .text_color(if empty { chrome_muted } else { chrome_fg })
                                .when(!self.search_menu_open, |field| {
                                    field.child(if empty {
                                        SEARCH_PLACEHOLDER.to_owned()
                                    } else {
                                        current
                                    })
                                })
                        },
                    ),
            );

        // Slack's top-right status button: the status emoji (a plain face while none is set)
        // and a chevron, opening the status picker. The avatar lives at the rail's foot.
        let status_glyph = self
            .self_state
            .status
            .as_ref()
            .map_or("\u{1F642}", |status| status.glyph());
        let status_button = h_flex()
            .id("status-button")
            .flex_none()
            .items_center()
            .gap_0p5()
            .px_1()
            .h(px(28.))
            .rounded_sm()
            .cursor_pointer()
            .hover(move |el| el.bg(chrome_hover))
            .child(div().text_base().child(status_glyph))
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(chrome_muted)
                    .size(px(12.)),
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_status_picker(window, cx)));

        // Slack's 40px top bar: the history arrows hug the left of a search pill centred in
        // the window (the right padding mirrors the traffic lights' inset on the left), with
        // status and help pinned to the right edge.
        // Transparent over the root's chrome (and its corner glow), like the rail, and without
        // the kit's bottom rule: Slack's chrome runs unbroken into the workspace column.
        TitleBar::new()
            .h(px(40.))
            .bg(gpui_kit::gpui::transparent_black())
            .border_b_0()
            .child(
                h_flex()
                    .relative()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .pr(px(80.))
                    .child(h_flex().flex_1().min_w_0().justify_end().pr_2().child(nav))
                    .child(search_pill)
                    .child(div().flex_1().min_w_0())
                    .child(
                        h_flex()
                            .absolute()
                            .right_2()
                            .top_0()
                            .h_full()
                            .items_center()
                            .gap_1()
                            .child(status_button)
                            .child(
                                chrome_icon(
                                    "help",
                                    IconName::CircleQuestionMark,
                                    chrome_muted,
                                    chrome_hover,
                                )
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.open_shortcuts(window, cx),
                                )),
                            ),
                    ),
            )
    }

    /// Offline / reconnecting notice at the top of the conversation column while Slack's RTM
    /// socket is dropped (`F-SHELL-169`). Also displays the number of queued outgoing mutations
    /// that will automatically flush on reconnect. Hidden once connected and drained.
    pub(super) fn connection_banner(&self, cx: &Context<Self>) -> Option<Div> {
        use slackless_core::rtm::ConnectionState;
        let queued = self.offline_queued;
        let queue_suffix = if queued == 1 {
            " (1 queued change will send when reconnected)".to_owned()
        } else if queued > 1 {
            format!(" ({queued} queued changes will send when reconnected)")
        } else {
            String::new()
        };
        let text = match self.connection.as_ref() {
            Some(ConnectionState::Reconnecting { attempt, delay, .. }) => {
                let secs = delay.as_secs();
                if secs > 0 {
                    format!(
                        "Offline — reconnecting to Slack in {secs}s (attempt {attempt})…{queue_suffix}"
                    )
                } else {
                    format!("Offline — reconnecting to Slack (attempt {attempt})…{queue_suffix}")
                }
            }
            Some(ConnectionState::Disconnected { reason }) => {
                format!("Disconnected from Slack ({reason}){queue_suffix}")
            }
            Some(ConnectionState::Connected { .. } | ConnectionState::Connecting) | None => {
                if queued == 0 {
                    return None;
                }
                let word = if queued == 1 { "change" } else { "changes" };
                format!("Reconnected — sending {queued} queued {word}…")
            }
        };
        let theme = cx.theme();
        Some(
            h_flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap_2()
                .px_4()
                .py_1p5()
                .bg(theme.warning.opacity(0.18))
                .border_b_1()
                .border_color(theme.warning.opacity(0.35))
                .child(
                    Icon::new(IconName::WifiOff)
                        .size(px(14.))
                        .text_color(theme.warning),
                )
                .child(div().text_xs().text_color(theme.foreground).child(text)),
        )
    }

    /// Clickable Retry / Discard bar shown when one or more persisted offline mutations failed
    /// while flushing after reconnect.
    pub(super) fn failed_mutations_banner(&self, cx: &Context<Self>) -> Option<Div> {
        if self.failed_mutations.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (danger, fg, muted) = (theme.danger, theme.foreground, theme.muted_foreground);
        let rows: Vec<_> =
            self.failed_mutations
                .iter()
                .map(|item| {
                    let retry_id = item.id;
                    let discard_id = item.id;
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            h_flex()
                                .min_w_0()
                                .items_center()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(14.))
                                        .text_color(danger),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(fg)
                                        .child(format!("Failed to send: {}", item.summary)),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("({})", item.error)),
                                ),
                        )
                        .child(
                            h_flex()
                                .flex_none()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "offline-retry-{retry_id}"
                                    )))
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .label("Retry")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.failed_mutations.retain(|m| m.id != retry_id);
                                        this.bridge.send(Command::RetryOfflineMutation(retry_id));
                                        cx.notify();
                                    })),
                                )
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "offline-discard-{discard_id}"
                                    )))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .label("Discard")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.failed_mutations.retain(|m| m.id != discard_id);
                                        this.bridge
                                            .send(Command::DiscardOfflineMutation(discard_id));
                                        cx.notify();
                                    })),
                                ),
                        )
                })
                .collect();
        Some(
            v_flex()
                .flex_none()
                .gap_1()
                .px_4()
                .py_1p5()
                .bg(danger.opacity(0.14))
                .border_b_1()
                .border_color(danger.opacity(0.35))
                .children(rows),
        )
    }

    /// App DM info & quick-action strip rendered above the transcript when the active
    /// conversation is a direct message with an app / bot user (`F-COLLAB-482`).
    pub(super) fn app_dm_banner(
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
    pub(super) fn dnd_banner(
        &self,
        conversation: Option<&Conversation>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let peer = conversation
            .filter(|c| c.kind() == ConversationKind::Im)
            .and_then(|c| c.user.as_ref())?;
        if !self.peer_dnd_paused.get(peer).copied().unwrap_or(false) {
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

    /// Slack's context bar for an externally shared channel, drawn inside the composer's
    /// frame: the first organizations' icons stacked, then "<N> external people are from
    /// <Org>" (two organizations by name, more as a count), the count linking to the channel's
    /// details. Built when the members or the snapshot change, never per frame.
    pub(super) fn connect_bar(&self, cx: &Context<Self>) -> Option<ContextBar> {
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
    pub(super) fn connect_line(&self) -> Option<(Vec<Team>, String, String)> {
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
        teams.truncate(3);
        Some((teams, count, rest))
    }

    /// Recompute [`Self::connect_orgs`] for the open conversation: only a Slack Connect
    /// channel has other organizations to count.
    pub(super) fn sync_connect_orgs(&mut self, cx: &mut Context<Self>) {
        self.connect_orgs = match self.fixtures.conversation(&self.channel) {
            Some(conversation) if conversation.is_ext_shared => channel_details::external_orgs(
                &channel_details::own_team(&self.fixtures),
                self.member_ids(&self.channel, conversation),
                &self.fixtures.users,
                &self.fixtures.teams,
            ),
            _ => Vec::new(),
        };
        let bar = self.connect_bar(cx);
        self.composer
            .update(cx, |composer, cx| composer.set_context_bar(bar, cx));
        self.sync_thread_bar(cx);
    }

    /// Slack shows the channel's Slack Connect bar over the thread's reply box too.
    pub(super) fn sync_thread_bar(&mut self, cx: &mut Context<Self>) {
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
    /// channel, else the conversation row (the sample workspace's lists).
    pub(super) fn member_ids<'a>(
        &'a self,
        channel: &ChannelId,
        conversation: &'a Conversation,
    ) -> &'a [UserId] {
        match &self.open_members {
            Some((open, members)) if open == channel => members,
            _ => &conversation.members,
        }
    }

    /// Slack's dark tooltip hanging under the header members chip (`.c-tooltip__tip`: the
    /// inverse surface, 200px wide, 13px text on a 1.25 line, 8x12 padding, radius 8, the
    /// arrow filling the 8px gap up to the chip).
    pub(super) fn members_popover(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = cx.theme();
        let (surface, ink) = (theme.foreground, theme.background);
        div()
            .id("hdr-members-popover")
            .absolute()
            .top(relative(1.))
            .left_1_2()
            .ml(px(-100.))
            .pt(px(8.))
            .occlude()
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
    pub(super) fn load_channel_people(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn people(&self, channel: &ChannelId, history: &[Message]) -> Vec<UserId> {
        let conversation = self.fixtures.conversation(channel);
        channel_people(
            &self.fixtures,
            self.member_ids(channel, conversation.unwrap_or(&Conversation::default())),
            history,
        )
    }

    /// The names of [`Self::channel_people`], as the members popover lists them.
    pub(super) fn people_names(&self) -> Vec<String> {
        self.channel_people
            .iter()
            .flatten()
            .map(|user| self.fixtures.user_name(user).to_owned())
            .collect()
    }

    /// Load the open channel's people when it shows the Join card, which names them.
    pub(super) fn load_join_people(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn join_card(&self, conversation: &Conversation, cx: &mut Context<Self>) -> Div {
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
    pub(super) fn join_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            .send(Command::JoinConversation(self.channel.clone()));
    }

    /// The conversation header row: star, peer avatar for DMs (presence dot for IMs, stacked
    /// chips for group DMs), the conversation name, the member count for channels, and the
    /// huddle / notifications / search / details icons on the right.
    pub(super) fn conversation_header(
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
            .and_then(|c| self.member_counts.get(&c.id).copied().or(c.num_members))
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
            .and_then(|c| self.app_counts.get(&c.id).copied())
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
            .when(!topic.is_empty(), |header| {
                // Slack renders the topic's mrkdwn: mentions by name, links, emoji.
                header.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child(crate::messages::mrkdwn_preview(
                            topic,
                            &self.fixtures,
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
                        let live_count = self
                            .active_huddles
                            .get(&self.channel)
                            .map(Vec::len)
                            .unwrap_or(0);
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
                        chrome_icon("hdr-search", IconName::Search, muted_foreground, accent)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.search_in_conversation(window, cx)
                            })),
                    )
                    .child(self.conversation_menu(conversation, cx)),
            )
    }

    /// The tab row under the header, derived from the conversation's `properties.tabs` in
    /// live mode ([`tabs`] for the rules) or the sample workspace's fixed set, with the
    /// active tab underlined. The trailing "+" opens Slack's new-tab menu, restricted to
    /// the actions slackless can run: Canvas (live only) and Link.
    pub(super) fn conversation_tabs(&self, cx: &mut Context<Self>) -> Div {
        let border = cx.theme().border;
        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;

        let fallback = Conversation::default();
        let specs = tabs::derive(
            self.fixtures
                .conversation(&self.channel)
                .unwrap_or(&fallback),
            self.fixtures.sample,
        );
        let this = cx.entity().downgrade();
        let sample = self.fixtures.sample;
        let ch_for_add = self.channel.clone();
        let add_menu = Button::new("tab-add")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::Plus)
            .tooltip("New tab")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                if !sample {
                    let this_canvas = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new("Canvas")
                            .icon(IconName::LayoutTemplate)
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
                            .on_click(move |_, _, cx| {
                                let _ = this_files.update(cx, |app, _| {
                                    app.bridge.send(Command::AddConversationTab {
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
                            .on_click(move |_, _, cx| {
                                let _ = this_pins.update(cx, |app, _| {
                                    app.bridge.send(Command::AddConversationTab {
                                        channel: ch_pins.clone(),
                                        tab_type: "bookmarks".to_owned(),
                                        label: None,
                                    });
                                });
                            }),
                    );
                }
                let this = this.clone();
                menu.item(PopupMenuItem::new("Link").icon(IconName::Link).on_click(
                    move |_, window, cx| {
                        let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
                    },
                ))
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
                            && !(*target == ConversationTab::Pins && !self.pins_open)
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
                if !sample {
                    if let Some(tid) = tab_id {
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
                                                    app.bridge.send(Command::MoveConversationTab {
                                                        channel: ch.clone(),
                                                        tab_id: tid.clone(),
                                                        after: after.clone(),
                                                    });
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
                                                    app.bridge.send(Command::MoveConversationTab {
                                                        channel: ch.clone(),
                                                        tab_id: tid.clone(),
                                                        after: Some(after.clone()),
                                                    });
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
                                                app.bridge.send(Command::RemoveConversationTab {
                                                    channel: ch.clone(),
                                                    tab_id: tid.clone(),
                                                });
                                            });
                                        }),
                                )
                            })
                            .into_any_element();
                    }
                }
                clickable.into_any_element()
            }))
            // The stretched row would top-align the small button: center it, as the tab
            // labels are.
            .child(h_flex().items_center().child(add_menu))
    }
}

/// A 28px square chrome icon button.
/// The members popover's "Includes …" line for one to three names, Slack's list
/// punctuation ("Includes A", "Includes A and B", "Includes A, B, and C").
pub(super) fn includes_line(names: &[String]) -> Option<String> {
    match names {
        [] => None,
        [a] => Some(format!("Includes {a}")),
        [a, b] => Some(format!("Includes {a} and {b}")),
        [a, b, c, ..] => Some(format!("Includes {a}, {b}, and {c}")),
    }
}

/// Slack's stacked organization icons (Slack Connect bar and Join card), left to right, each
/// ringed in `ring`, the surface behind them.
pub(super) fn org_stack(teams: &[Team], ring: Hsla) -> Div {
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
pub(super) fn joinable(conversation: &Conversation) -> bool {
    conversation.kind() == ConversationKind::PublicChannel
        && !conversation.is_member
        && !conversation.is_archived
}

/// The Join card's "who is here": up to two names, then how many more people.
pub(super) fn who_is_here(names: &[&str], more: usize) -> Option<String> {
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
pub(super) fn apps_also_here(apps: &[&str]) -> Option<String> {
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
pub(super) fn channel_people(
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
pub(super) fn push_person(
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

pub(super) fn chrome_icon(
    id: &'static str,
    icon: IconName,
    color: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(px(28.))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(color)
        .hover(|el| el.bg(hover_bg))
        .child(Icon::new(icon).text_color(color).size(px(16.)))
}
