//! Channel creation, member invitations, channel details, and cleanup dialogs.

use super::super::*;

impl App {
    /// Open Slack's "Create a channel" dialog (`conversations.create` + optional purpose).
    pub(in crate::app) fn open_create_channel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let name_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("e.g. plan-budget")
                .auto_grow(1, 1)
        });
        let purpose_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("What is this channel about? (optional)")
                .auto_grow(1, 3)
        });
        let is_private = cx.new(|_| false);
        name_state.read(cx).focus_handle(cx).focus(window, cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, fg, accent) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.foreground,
                theme.sidebar_accent,
            );
            let private_now = *is_private.read(cx);
            let raw_name = name_state.read(cx).value().to_string();
            let slug = channel_details::normalize_channel_slug(&raw_name);
            let preview = if slug.is_empty() {
                String::new()
            } else if private_now {
                format!("Will create private channel #{slug}")
            } else {
                format!("Will create public channel #{slug}")
            };
            let (name_s, purpose_s, priv_s, this_create) = (
                name_state.clone(),
                purpose_state.clone(),
                is_private.clone(),
                this.clone(),
            );
            let priv_pub = is_private.clone();
            let priv_prv = is_private.clone();
            dialog.title("Create a channel").w(px(460.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Name"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .bg(bg)
                                    .child(
                                        Textarea::new(&name_state)
                                            .appearance(false)
                                            .bordered(false),
                                    ),
                            )
                            .when(!preview.is_empty(), |col| {
                                col.child(div().text_xs().text_color(muted).child(preview))
                            }),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Visibility"),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .id("create-ch-public")
                                            .flex_1()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(if !private_now { accent } else { border })
                                            .cursor_pointer()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(Icon::new(IconName::Hash).size(px(14.)))
                                            .child("Public")
                                            .on_click(move |_, _, cx| {
                                                priv_pub.update(cx, |v, cx| {
                                                    *v = false;
                                                    cx.notify();
                                                });
                                            }),
                                    )
                                    .child(
                                        h_flex()
                                            .id("create-ch-private")
                                            .flex_1()
                                            .items_center()
                                            .gap_2()
                                            .px_3()
                                            .py_2()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(if private_now { accent } else { border })
                                            .cursor_pointer()
                                            .text_sm()
                                            .text_color(fg)
                                            .child(Icon::new(IconName::Lock).size(px(14.)))
                                            .child("Private")
                                            .on_click(move |_, _, cx| {
                                                priv_prv.update(cx, |v, cx| {
                                                    *v = true;
                                                    cx.notify();
                                                });
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Description"),
                            )
                            .child(
                                div()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .bg(bg)
                                    .child(
                                        Textarea::new(&purpose_state)
                                            .appearance(false)
                                            .bordered(false),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                Button::new("create-ch-cancel")
                                    .outline()
                                    .with_size(Size::Small)
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("create-ch-submit")
                                    .primary()
                                    .with_size(Size::Small)
                                    .label("Create")
                                    .on_click(move |_, window, cx| {
                                        let slug = channel_details::normalize_channel_slug(
                                            name_s.read(cx).value().as_ref(),
                                        );
                                        if slug.is_empty() {
                                            actions::notify(
                                                "Enter a channel name",
                                                "Channel names use lowercase letters, numbers, and hyphens.",
                                                window,
                                                cx,
                                            );
                                            return;
                                        }
                                        let purpose =
                                            purpose_s.read(cx).value().trim().to_owned();
                                        let is_private = *priv_s.read(cx);
                                        window.close_dialog(cx);
                                        let _ = this_create.update(cx, |app, cx| {
                                            app.bridge.send(Command::CreateChannel {
                                                name: slug.clone(),
                                                is_private,
                                                purpose: (!purpose.is_empty()).then_some(purpose),
                                            });
                                            actions::notify(
                                                "Creating channel",
                                                format!("#{slug}"),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            ),
                    ),
            )
        });
    }

    /// Open a colleague multi-picker dialog to invite people into a channel (`conversations.invite`)
    /// or add people to a DM / start a group DM (`conversations.open`).
    pub(in crate::app) fn open_add_people(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        let fixtures = self.fixtures.clone();
        let conv = fixtures.conversation(&channel).cloned();
        let is_dm = conv.as_ref().is_some_and(|c| c.is_im || c.is_mpim);
        let existing: HashSet<UserId> = if is_dm {
            let mut set = HashSet::new();
            set.insert(fixtures.self_user.clone());
            if let Some(c) = &conv {
                if let Some(u) = &c.user {
                    set.insert(u.clone());
                }
                set.extend(c.members.iter().cloned());
            }
            if let Some((open_ch, members)) = &self.open_members {
                if *open_ch == channel {
                    set.extend(members.iter().cloned());
                }
            }
            set
        } else {
            self.open_members
                .as_ref()
                .filter(|(c, _)| *c == channel)
                .map(|(_, m)| m.iter().cloned().collect())
                .unwrap_or_default()
        };
        let candidates: Vec<(UserId, String, String)> = fixtures
            .users
            .iter()
            .filter(|u| !u.deleted && !u.is_bot && u.id.as_str() != "USLACKBOT")
            .filter(|u| !existing.contains(&u.id))
            .map(|u| {
                let name = fixtures.name_of(u).to_owned();
                let key = format!("{} {}", name.to_lowercase(), u.name.to_lowercase());
                (u.id.clone(), name, key)
            })
            .collect();
        let filter_state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Search people by name…")
                .auto_grow(1, 1)
        });
        let selected = cx.new(|_| Vec::<UserId>::new());
        filter_state.read(cx).focus_handle(cx).focus(window, cx);
        let title = if is_dm {
            "Add people to this DM".to_owned()
        } else {
            format!("Add people to {}", self.label(&channel))
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, bg, fg, hover, accent) = (
                theme.border,
                theme.muted_foreground,
                theme.background,
                theme.foreground,
                theme.accent,
                theme.sidebar_accent,
            );
            let needle = filter_state.read(cx).value().trim().to_lowercase();
            let picked = selected.read(cx).clone();
            let rows: Vec<_> = candidates
                .iter()
                .filter(|(_, _, key)| needle.is_empty() || key.contains(&needle))
                .take(25)
                .enumerate()
                .map(|(ix, (uid, name, _))| {
                    let on = picked.contains(uid);
                    let uid_click = uid.clone();
                    let sel_click = selected.clone();
                    h_flex()
                        .id(("add-people-row", ix))
                        .items_center()
                        .justify_between()
                        .px_3()
                        .py_1p5()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(on, |r| r.bg(accent.opacity(0.18)))
                        .hover(|r| r.bg(hover))
                        .child(div().text_sm().text_color(fg).child(name.clone()))
                        .when(on, |r| {
                            r.child(Icon::new(IconName::Check).size(px(14.)).text_color(accent))
                        })
                        .on_click(move |_, _, cx| {
                            sel_click.update(cx, |list, cx| {
                                if let Some(pos) = list.iter().position(|id| id == &uid_click) {
                                    list.remove(pos);
                                } else {
                                    list.push(uid_click.clone());
                                }
                                cx.notify();
                            });
                        })
                })
                .collect();
            let count = picked.len();
            let (sel_submit, ch_submit, this_submit, existing_submit) = (
                selected.clone(),
                channel.clone(),
                this.clone(),
                existing.clone(),
            );
            dialog.title(title.clone()).w(px(440.)).child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(
                        div()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(bg)
                            .child(
                                Textarea::new(&filter_state)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("add-people-list")
                            .max_h(px(280.))
                            .overflow_y_scroll()
                            .gap_0p5()
                            .when(rows.is_empty(), |col| {
                                col.child(
                                    div()
                                        .py_4()
                                        .text_center()
                                        .text_xs()
                                        .text_color(muted)
                                        .child("No matching people to add."),
                                )
                            })
                            .children(rows),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .items_center()
                            .child(div().text_xs().text_color(muted).child(if count == 0 {
                                "Select one or more people".to_owned()
                            } else {
                                format!("{count} selected")
                            }))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("add-people-cancel")
                                            .outline()
                                            .with_size(Size::Small)
                                            .label("Cancel")
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("add-people-confirm")
                                            .primary()
                                            .with_size(Size::Small)
                                            .label(if is_dm { "Open group DM" } else { "Add" })
                                            .on_click(move |_, window, cx| {
                                                let chosen = sel_submit.read(cx).clone();
                                                if chosen.is_empty() {
                                                    return;
                                                }
                                                window.close_dialog(cx);
                                                let _ = this_submit.update(cx, |app, cx| {
                                                    if is_dm {
                                                        let mut users: Vec<UserId> =
                                                            existing_submit
                                                                .iter()
                                                                .filter(|id| {
                                                                    **id != app.fixtures.self_user
                                                                })
                                                                .cloned()
                                                                .collect();
                                                        for u in chosen {
                                                            if !users.contains(&u) {
                                                                users.push(u);
                                                            }
                                                        }
                                                        app.bridge.send(Command::OpenDm { users });
                                                    } else {
                                                        let n = chosen.len();
                                                        app.bridge.send(Command::InviteToChannel {
                                                            channel: ch_submit.clone(),
                                                            users: chosen,
                                                        });
                                                        actions::notify(
                                                            "Added people",
                                                            format!(
                                                                "Invited {n} {} to channel",
                                                                if n == 1 {
                                                                    "person"
                                                                } else {
                                                                    "people"
                                                                }
                                                            ),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                });
                                            }),
                                    ),
                            ),
                    ),
            )
        });
    }

    /// Slack's channel details dialog, opened by clicking the channel name in the header or
    /// its members chip: the About and Members tabs over the channel's record and member
    /// list. A creator missing from the snapshot resolves through the core, like any other
    /// unknown person.
    pub(in crate::app) fn open_channel_details(
        &mut self,
        channel: &ChannelId,
        tab: channel_details::DetailsTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation) = self.fixtures.conversation(channel).cloned() else {
            return;
        };
        if let Some(creator) = &conversation.creator {
            self.resolve_users(std::iter::once(creator.clone()));
        }
        // Live mode without a member list yet keeps the Members tab on its placeholder until
        // `Event::ConversationInfo` lands.
        let stored = self.member_ids(channel, &conversation).to_vec();
        let loading = stored.is_empty() && self.bridge.store().is_some();
        let following = self
            .preferences
            .read(cx)
            .settings()
            .follow_all_threads_channels
            .iter()
            .any(|id| id == channel.as_str());
        self.details.update(cx, |details, cx| {
            details.open(
                self.fixtures.clone(),
                channel.clone(),
                stored,
                loading,
                tab,
                cx,
            );
            details.set_follow_all_threads(following, cx);
        });
        self.bridge.send(Command::FetchChannelManagers {
            channel: channel.clone(),
        });
        // Slack's title row: a private channel's lock, else the hash, beside the name in the
        // dialog's bold display size (the close X is the dialog's own).
        let title_name: SharedString = if conversation.is_private {
            conversation
                .name
                .as_deref()
                .unwrap_or_else(|| channel.as_str())
                .to_owned()
                .into()
        } else {
            format!(
                "# {}",
                conversation
                    .name
                    .as_deref()
                    .unwrap_or_else(|| channel.as_str())
            )
            .into()
        };
        let title_private = conversation.is_private;
        let details = self.details.clone();
        let focus_find_people = Rc::new(Cell::new(true));
        window.open_dialog(cx, move |dialog, window, cx| {
            // Slack puts the caret in "Find people" as the dialog opens.
            if focus_find_people.replace(false) {
                details.read(cx).find_people_focus(cx).focus(window, cx);
            }
            let mut title = h_flex()
                .items_center()
                .gap_1p5()
                .text_size(px(22.))
                .font_weight(FontWeight::BOLD);
            title = if title_private {
                title
                    .child(Icon::new(IconName::Lock))
                    .child(title_name.clone())
            } else {
                title.child(title_name.clone())
            };
            // Slack's 580pt dialog: its rules run edge to edge, so the content pads itself and
            // the title row carries the close X.
            let title_row = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .px(px(28.))
                .child(title)
                .child(
                    Button::new("details-close")
                        .ghost()
                        .icon(IconName::Close)
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                );
            dialog
                .title(title_row)
                .w(px(580.))
                .px_0()
                .pb_0()
                .close_button(false)
                .child(details.clone())
        });
    }

    /// The channel details dialog's actions.
    pub(in crate::app) fn details_event(
        &mut self,
        event: &ChannelDetailsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChannelDetailsEvent::Leave(channel) => {
                window.close_dialog(cx);
                self.leave(channel.clone(), window, cx);
            }
            ChannelDetailsEvent::Rename { channel, name } => {
                self.bridge.send(Command::RenameChannel {
                    channel: channel.clone(),
                    name: name.clone(),
                });
                actions::notify("Renamed channel", format!("#{name}"), window, cx);
            }
            ChannelDetailsEvent::Archive(channel) => {
                window.close_dialog(cx);
                let this = cx.weak_entity();
                let ch = channel.clone();
                let label = self.label(&ch);
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let (this, ch) = (this.clone(), ch.clone());
                    dialog.title(format!("Archive {label}?")).w(px(420.)).child(
                        v_flex()
                            .gap_3()
                            .p_4()
                            .child(div().text_sm().text_color(muted).child(
                                "Archiving stops new messages from being posted. Message history remains searchable and the channel can be unarchived later.",
                            ))
                            .child(
                                h_flex()
                                    .justify_end()
                                    .gap_2()
                                    .child(
                                        Button::new("archive-cancel")
                                            .outline()
                                            .with_size(Size::Small)
                                            .label("Cancel")
                                            .on_click(|_, window, cx| window.close_dialog(cx)),
                                    )
                                    .child(
                                        Button::new("archive-confirm")
                                            .danger()
                                            .with_size(Size::Small)
                                            .label("Archive channel")
                                            .on_click(move |_, window, cx| {
                                                window.close_dialog(cx);
                                                let _ = this.update(cx, |app, cx| {
                                                    app.bridge
                                                        .send(Command::ArchiveChannel(ch.clone()));
                                                    actions::notify(
                                                        "Channel archived",
                                                        app.label(&ch),
                                                        window,
                                                        cx,
                                                    );
                                                });
                                            }),
                                    ),
                            ),
                    )
                });
            }
            ChannelDetailsEvent::Unarchive(channel) => {
                window.close_dialog(cx);
                self.bridge.send(Command::UnarchiveChannel(channel.clone()));
                actions::notify("Channel unarchived", self.label(channel), window, cx);
            }
            ChannelDetailsEvent::ToggleFollowThreads(channel) => {
                let id = channel.as_str().to_owned();
                let mut enabled = false;
                self.preferences.update(cx, |prefs, cx| {
                    let mut next = prefs.settings().clone();
                    if let Some(pos) = next
                        .follow_all_threads_channels
                        .iter()
                        .position(|c| c == &id)
                    {
                        next.follow_all_threads_channels.remove(pos);
                        enabled = false;
                    } else {
                        next.follow_all_threads_channels.push(id.clone());
                        enabled = true;
                    }
                    prefs.load_settings(next, window, cx);
                });
                self.apply_preferences(window, cx);
                actions::notify(
                    if enabled {
                        "Following all new threads"
                    } else {
                        "Stopped following all threads"
                    },
                    self.label(channel),
                    window,
                    cx,
                );
            }
            ChannelDetailsEvent::Delete(channel) => {
                window.close_dialog(cx);
                let this = cx.weak_entity();
                let ch = channel.clone();
                let label = self.label(&ch);
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let (this, ch) = (this.clone(), ch.clone());
                    dialog
                        .title(format!("Delete {label}?"))
                        .w(px(420.))
                        .child(
                            v_flex()
                                .gap_3()
                                .p_4()
                                .child(div().text_sm().text_color(muted).child(
                                    "Deleting permanently removes this channel and its message history for everyone in the workspace. Requires workspace admin permissions.",
                                ))
                                .child(
                                    h_flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            Button::new("delete-ch-cancel")
                                                .outline()
                                                .with_size(Size::Small)
                                                .label("Cancel")
                                                .on_click(|_, window, cx| window.close_dialog(cx)),
                                        )
                                        .child(
                                            Button::new("delete-ch-confirm")
                                                .danger()
                                                .with_size(Size::Small)
                                                .label("Delete permanently")
                                                .on_click(move |_, window, cx| {
                                                    window.close_dialog(cx);
                                                    let _ = this.update(cx, |app, cx| {
                                                        if !app.fixtures.sample {
                                                            app.bridge.send(
                                                                Command::DeleteChannel(ch.clone()),
                                                            );
                                                        }
                                                        actions::notify(
                                                            "Delete requested",
                                                            app.label(&ch),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        ),
                                ),
                        )
                });
            }
            ChannelDetailsEvent::ConvertToPrivate(channel) => {
                window.close_dialog(cx);
                let this = cx.weak_entity();
                let ch = channel.clone();
                let label = self.label(&ch);
                window.open_dialog(cx, move |dialog, _, cx| {
                    let muted = cx.theme().muted_foreground;
                    let (this, ch) = (this.clone(), ch.clone());
                    dialog
                        .title(format!("Convert {label} to private?"))
                        .w(px(420.))
                        .child(
                            v_flex()
                                .gap_3()
                                .p_4()
                                .child(div().text_sm().text_color(muted).child(
                                    "Members stay in the conversation, and only invited people will be able to find or view it going forward.",
                                ))
                                .child(
                                    h_flex()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            Button::new("convert-priv-cancel")
                                                .outline()
                                                .with_size(Size::Small)
                                                .label("Cancel")
                                                .on_click(|_, window, cx| window.close_dialog(cx)),
                                        )
                                        .child(
                                            Button::new("convert-priv-confirm")
                                                .primary()
                                                .with_size(Size::Small)
                                                .label("Convert to private")
                                                .on_click(move |_, window, cx| {
                                                    window.close_dialog(cx);
                                                    let _ = this.update(cx, |app, cx| {
                                                        if !app.fixtures.sample {
                                                            app.bridge.send(
                                                                Command::ConvertChannelToPrivate(
                                                                    ch.clone(),
                                                                ),
                                                            );
                                                        }
                                                        actions::notify(
                                                            "Converted to private",
                                                            app.label(&ch),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        ),
                                ),
                        )
                });
            }
            ChannelDetailsEvent::AddPeople(channel) => {
                window.close_dialog(cx);
                self.open_add_people(channel.clone(), window, cx);
            }
            ChannelDetailsEvent::SetTopic { channel, topic } => {
                self.bridge.send(Command::SetChannelTopic {
                    channel: channel.clone(),
                    topic: topic.clone(),
                });
            }
            ChannelDetailsEvent::SetPurpose { channel, purpose } => {
                self.bridge.send(Command::SetChannelPurpose {
                    channel: channel.clone(),
                    purpose: purpose.clone(),
                });
            }
            ChannelDetailsEvent::StartHuddle => self.start_huddle(window, cx),
            ChannelDetailsEvent::SetNotifyLevel { channel, level } => {
                self.bridge.send(Command::SetNotifyLevel {
                    channel: channel.clone(),
                    level: *level,
                });
            }
            ChannelDetailsEvent::OpenProfile(user) => {
                window.close_dialog(cx);
                self.open_profile(user.clone(), cx);
            }
        }
    }

    /// "Clean up inactive channels" bulk-leave assistant (`F-CORE-023`, `F-SHELL-070`): lists
    /// joined non-general channels ordered by inactivity so users can leave stale channels at once.
    pub(in crate::app) fn open_inactive_channels_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = crate::messages::now_secs();
        let mut rows: Vec<(ChannelId, String, bool, Option<i64>)> = self
            .fixtures
            .channels
            .iter()
            .filter(|c| !c.is_general && !c.is_archived && !c.is_im && !c.is_mpim && c.is_member)
            .map(|c| {
                let latest_ts = self
                    .fixtures
                    .latest
                    .get(&c.id)
                    .and_then(|m| m.ts.secs())
                    .or_else(|| {
                        c.updated
                            .map(|u| if u > 10_000_000_000 { u / 1000 } else { u })
                    });
                let age_days = latest_ts.map(|ts| ((now - ts).max(0)) / 86_400);
                let name = c
                    .name
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        self.fixtures
                            .conversation_label(c)
                            .trim_start_matches('#')
                            .trim()
                            .to_owned()
                    });
                (c.id.clone(), name, c.is_private, age_days)
            })
            .collect();
        // Sort oldest / most inactive first (channels with no recent history at the top).
        rows.sort_by(|a, b| match (a.3, b.3) {
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(da), Some(db)) => db.cmp(&da),
            (None, None) => a.1.cmp(&b.1),
        });
        // Pre-select channels quiet for 30+ days so cleanup takes one click.
        let initial: HashSet<ChannelId> = rows
            .iter()
            .filter(|(_, _, _, days)| days.is_none_or(|d| d >= 30))
            .take(10)
            .map(|(id, _, _, _)| id.clone())
            .collect();
        let selected = cx.new(|_| initial);
        let this = cx.weak_entity();
        let rows = Arc::new(rows);
        window.open_dialog(cx, move |dialog, _, cx| {
            let (border, muted, hover, link) = (
                cx.theme().border,
                cx.theme().muted_foreground,
                cx.theme().accent,
                cx.theme().link,
            );
            let sel_count = selected.read(cx).len();
            let sel_for_leave = selected.clone();
            let this_leave = this.clone();
            let mut list = v_flex()
                .id("cleanup-channels-list")
                .max_h(px(340.))
                .overflow_y_scroll()
                .gap_1();
            if rows.is_empty() {
                list = list.child(
                    div()
                        .py_6()
                        .text_sm()
                        .text_color(muted)
                        .child("You have no joined channels eligible for cleanup."),
                );
            } else {
                for (ix, (ch_id, label, is_private, age_days)) in rows.iter().take(80).enumerate() {
                    let checked = selected.read(cx).contains(ch_id);
                    let sel_row = selected.clone();
                    let ch_toggle = ch_id.clone();
                    let age_text = match age_days {
                        None => "No recent activity".to_owned(),
                        Some(0) => "Active today".to_owned(),
                        Some(1) => "Quiet 1 day".to_owned(),
                        Some(d) => format!("Quiet {d} days"),
                    };
                    let icon = if *is_private {
                        IconName::Lock
                    } else {
                        IconName::Hash
                    };
                    list = list.child(
                        h_flex()
                            .id(("cleanup-ch-row", ix))
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2p5()
                            .py_1p5()
                            .rounded_md()
                            .border_1()
                            .border_color(if checked { link.opacity(0.5) } else { border })
                            .cursor_pointer()
                            .hover(move |el| el.bg(hover))
                            .on_click(move |_, window, cx| {
                                sel_row.update(cx, |set, cx| {
                                    if !set.remove(&ch_toggle) {
                                        set.insert(ch_toggle.clone());
                                    }
                                    cx.notify();
                                });
                                window.refresh();
                            })
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .min_w_0()
                                    .child(
                                        Icon::new(if checked {
                                            IconName::Check
                                        } else {
                                            IconName::Circle
                                        })
                                        .size(px(14.))
                                        .text_color(if checked { link } else { muted }),
                                    )
                                    .child(Icon::new(icon).size(px(13.)).text_color(muted))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(label.clone()),
                                    ),
                            )
                            .child(div().text_xs().text_color(muted).child(age_text)),
                    );
                }
            }
            dialog
                .title("Clean up inactive channels")
                .w(px(500.))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(div().text_xs().text_color(muted).child(
                            "Select quiet channels to leave in bulk. Channels with 30+ days of inactivity are pre-selected.",
                        ))
                        .child(list)
                        .child(
                            h_flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("{sel_count} selected")),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("cleanup-cancel")
                                                .outline()
                                                .with_size(Size::Small)
                                                .label("Cancel")
                                                .on_click(|_, window, cx| window.close_dialog(cx)),
                                        )
                                        .child(
                                            Button::new("cleanup-leave")
                                                .danger()
                                                .with_size(Size::Small)
                                                .disabled(sel_count == 0)
                                                .label(format!("Leave selected ({sel_count})"))
                                                .on_click(move |_, window, cx| {
                                                    let targets: Vec<ChannelId> = sel_for_leave
                                                        .read(cx)
                                                        .iter()
                                                        .cloned()
                                                        .collect();
                                                    window.close_dialog(cx);
                                                    let _ = this_leave.update(cx, |app, cx| {
                                                        let count = targets.len();
                                                        for ch in targets {
                                                            app.leave(ch, window, cx);
                                                        }
                                                        if count > 0 {
                                                            actions::notify(
                                                                "Left inactive channels",
                                                                format!(
                                                                    "Left {count} channel{}",
                                                                    if count == 1 { "" } else { "s" }
                                                                ),
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    });
                                                }),
                                        ),
                                ),
                        ),
                )
        });
    }
}
