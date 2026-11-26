//! Global title bar, history navigation, search pill, and conversation header menus.

use super::super::*;
use super::chrome_icon;

impl App {
    /// The huddle button's chevron: Slack's huddle & third-party calling app menu
    /// (`F-COLLAB-078..085`, `F-COLLAB-092..094`).
    pub(in crate::app) fn huddle_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let link = self.huddle_link();
        let start = if self.huddle_here() {
            "Show huddle"
        } else {
            "Start huddle"
        };
        let calling_apps = self.calling_apps();
        Button::new("hdr-huddle-menu")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::ChevronDown)
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                let this_huddle = this.clone();
                let link = link.clone();
                menu = menu
                    .item(
                        PopupMenuItem::new(start)
                            .icon(IconName::Headphones)
                            .on_click(move |_, window, cx| {
                                let _ =
                                    this_huddle.update(cx, |app, cx| app.start_huddle(window, cx));
                            }),
                    )
                    .item(
                        PopupMenuItem::new("Copy huddle link")
                            .icon(IconName::Link)
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                            }),
                    );
                if !calling_apps.is_empty() {
                    menu = menu.separator().label("Calling apps");
                    for calling_app in &calling_apps {
                        let app_v = calling_app.clone();
                        let this_v = this.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Start {} call", calling_app.name))
                                .icon(IconName::Video)
                                .on_click(move |_, window, cx| {
                                    let _ = this_v.update(cx, |app, cx| {
                                        app.launch_calling_app(
                                            &app_v,
                                            slackless_core::domain::calling::CallMode::Video,
                                            "",
                                            window,
                                            cx,
                                        );
                                    });
                                }),
                        );
                        if calling_app.supports_audio_only {
                            let app_a = calling_app.clone();
                            let this_a = this.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("{} audio call", calling_app.name))
                                    .icon(IconName::Phone)
                                    .on_click(move |_, window, cx| {
                                        let _ = this_a.update(cx, |app, cx| {
                                            app.launch_calling_app(
                                                &app_a,
                                                slackless_core::domain::calling::CallMode::Audio,
                                                "",
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            );
                        }
                    }
                    let this_dialog = this.clone();
                    menu = menu.separator().item(
                        PopupMenuItem::new("More calling options\u{2026}")
                            .icon(IconName::PhoneCall)
                            .on_click(move |_, window, cx| {
                                let _ = this_dialog.update(cx, |app, cx| {
                                    app.open_calling_app_dialog(window, cx);
                                });
                            }),
                    );
                }
                menu
            })
    }

    /// The header bell: Slack's "Notify you about…" menu for this conversation, checked at
    /// its current level; the icon is a crossed-out bell while muted.
    pub(in crate::app) fn notify_menu(
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
                                app.bridge.send(ChannelCommand::SetNotifyLevel {
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
    pub(in crate::app) fn conversation_menu(
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
                if let Some(ch_id) = channel.clone() {
                    let this_md = this.clone();
                    let this_json = this.clone();
                    let this_bf = this.clone();
                    let this_storage = this.clone();
                    let ch_bf = ch_id.clone();
                    menu = menu
                        .separator()
                        .item(
                            PopupMenuItem::new("Backfill channel history")
                                .icon(IconName::RotateCcw)
                                .on_click(move |_, window, cx| {
                                    let _ = this_bf.update(cx, |app, cx| {
                                        app.start_channel_backfill(ch_bf.clone(), window, cx);
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new("Archive & storage\u{2026}")
                                .icon(IconName::HardDrive)
                                .on_click(move |_, window, cx| {
                                    let _ = this_storage.update(cx, |app, cx| {
                                        app.open_storage_dialog(window, cx);
                                    });
                                }),
                        )
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
    pub(in crate::app) fn global_bar(&mut self, cx: &mut Context<Self>) -> TitleBar {
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
                    .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx))),
            )
            .child(
                Button::new("nav-forward")
                    .ghost()
                    .with_size(Size::Small)
                    .icon(Icon::new(IconName::ArrowRight).text_color(chrome_muted))
                    .tooltip("Forward")
                    .disabled(self.forward.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx))),
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
            .gap_1()
            .px_1p5()
            .h(px(26.))
            .rounded(px(6.))
            .border_1()
            .border_color(chrome_fg.opacity(0.14))
            .bg(chrome_fg.opacity(0.06))
            .cursor_pointer()
            .hover(move |el| el.bg(chrome_hover))
            .child(div().text_sm().child(status_glyph))
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(chrome_muted)
                    .size(px(12.)),
            )
            .on_click(cx.listener(|this, _, window, cx| this.open_status_picker(window, cx)));

        let stealth_active = self.preferences.read(cx).settings().stealth_mode;
        let stealth_pill = stealth_active.then(|| {
            let warn = cx.theme().warning;
            h_flex()
                .id("stealth-mode-pill")
                .flex_none()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(24.))
                .rounded_full()
                .border_1()
                .border_color(warn.opacity(0.5))
                .bg(warn.opacity(0.18))
                .cursor_pointer()
                .hover(move |el| el.bg(warn.opacity(0.28)))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(
                        "Stealth mode active: typing, read receipts & active presence suppressed. Click to turn off.",
                    )
                    .build(window, cx)
                })
                .child(Icon::new(IconName::Eye).text_color(warn).size(px(12.)))
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(chrome_fg)
                        .child("Stealth"),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.preferences.update(cx, |prefs, cx| {
                        prefs.set_stealth_mode(false, cx);
                    });
                    this.apply_preferences(window, cx);
                    notify(
                        "Stealth mode off",
                        "Typing indicators, read receipts, and active presence resumed.",
                        window,
                        cx,
                    );
                }))
        });

        let outbox_badge = crate::outbox::top_bar_outbox_badge(
            &self.ws.outbox,
            &self.fixtures,
            chrome_fg,
            crate::outbox::TopBarOutboxActions {
                on_flush: {
                    let this = cx.weak_entity();
                    Rc::new(move |_window, cx| {
                        let _ = this.update(cx, |app, _| {
                            app.bridge.send(MessagingCommand::FlushOutbox);
                        });
                    })
                },
                on_open: {
                    let this = cx.weak_entity();
                    Rc::new(move |channel, thread_ts, _window, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.select(channel.clone(), cx);
                            if let Some(root_ts) = thread_ts {
                                app.open_thread(channel, root_ts, cx);
                            }
                        });
                    })
                },
                on_retry: {
                    let this = cx.weak_entity();
                    Rc::new(move |id, _window, cx| {
                        let _ = this.update(cx, |app, _| {
                            app.bridge.send(MessagingCommand::RetryOutboxItem(id));
                        });
                    })
                },
                on_discard: {
                    let this = cx.weak_entity();
                    Rc::new(move |id, _window, cx| {
                        let _ = this.update(cx, |app, _| {
                            app.bridge.send(MessagingCommand::DiscardOutboxItem(id));
                        });
                    })
                },
            },
            cx,
        );

        let rail_toggle = chrome_icon(
            "toggle-workspace-rail",
            IconName::PanelLeft,
            chrome_muted,
            chrome_hover,
        )
        .tooltip(|window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(
                "Toggle workspace switcher (\u{2318}\u{21E7}S)",
            )
            .build(window, cx)
        })
        .on_click(cx.listener(|this, _, window, cx| this.toggle_workspace_rail(window, cx)));

        // Slack's 40px top bar: workspace-rail toggle next to the traffic lights, the history
        // arrows hugging the left of a search pill centred in the window, with status pill and
        // help pinned to the right edge.
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
                    .child(
                        h_flex()
                            .absolute()
                            .left_1()
                            .top_0()
                            .h_full()
                            .items_center()
                            .child(rail_toggle),
                    )
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
                            .children(outbox_badge)
                            .children(stealth_pill)
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
}
