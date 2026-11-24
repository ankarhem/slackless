//! Root `Render` implementation for `App`: window chrome, workspace rail, resizable panes, and overlays.

use super::*;

impl Render for App {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Owned color copies: `cx.theme()` borrows `cx`, so nothing may outlive the
        // statement that reads it across the `cx.listener` calls in the helpers below.
        let background = cx.theme().background;
        let foreground = cx.theme().foreground;
        // The chrome (and its corner glow) is painted once here; the title bar and rail are
        // transparent over it.
        let chrome = cx.theme().title_bar;
        let (glow_side, glow) = crate::theme::chrome_glow(cx);
        self.sync_badge(cx);

        let conversation = self.fixtures.conversation(&self.channel).cloned();
        self.sync_window_title(conversation.as_ref(), window);
        let title = conversation
            .as_ref()
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| self.channel.to_string());
        let topic = conversation
            .as_ref()
            .and_then(|c| c.topic.as_ref())
            .map(|t| t.value.clone())
            .unwrap_or_default();

        let title_bar = self.global_bar(cx);
        let main_column = if self.searching {
            v_flex().size_full().child(self.results.clone())
        } else if self.drafts_open {
            v_flex().size_full().child(self.drafts.clone())
        } else if self.later_open {
            v_flex().size_full().child(self.later.clone())
        } else if self.threads_open {
            v_flex().size_full().child(self.threads.clone())
        } else if self.unreads_open {
            v_flex().size_full().child(self.unreads.clone())
        } else if self.huddles_open {
            v_flex().size_full().child(self.huddles.clone())
        } else if self.directory_open {
            v_flex().size_full().child(self.directory.clone())
        } else if self.tools_open {
            v_flex().size_full().child(self.tools_page.clone())
        } else if self.compose_open {
            v_flex().size_full().child(self.compose.clone())
        } else if self.activity_open {
            // The Activity page spans the whole main area; the sidebar renders only its
            // rail while this destination is active.
            v_flex().size_full().child(self.activity.clone())
        } else if self.sidebar.read(cx).active_nav() == NavItem::Files {
            // The Files view owns the main area beside the sidebar's Files list column.
            v_flex().size_full().child(self.files_page.clone())
        } else {
            let header = self.conversation_header(&title, &topic, conversation.as_ref(), cx);
            let tabs = self.conversation_tabs(cx);
            let this = cx.weak_entity();
            // The bookmarks bar: the sample workspace's fixture set (plus links added in the
            // dialog), or the live channel's `bookmarks.list` once fetched. Like Slack, no
            // bar for a conversation with neither bookmarks nor pins.
            let (mut bookmark_list, pin_count) = if self.fixtures.sample {
                (
                    bookmarks::fixture(&self.fixtures, &self.channel),
                    Some(pins::pinned(&self.fixtures, &self.channel).len()),
                )
            } else {
                // The pin count is unknown until the channel's pins have been fetched.
                let pinned = self
                    .ws
                    .pins
                    .get(&self.channel)
                    .and_then(|loaded| loaded.as_ref().ok());
                (self.live_bookmarks(), pinned.map(Vec::len))
            };
            if let Some(added) = self.fixture_bookmarks.get(&self.channel) {
                bookmark_list.extend(added.iter().cloned());
            }
            let clicks = bookmarks::BarClicks {
                pins: Rc::new({
                    let this = this.clone();
                    move |_window, cx| {
                        let _ = this.update(cx, |app, cx| app.toggle_pins(cx));
                    }
                }),
                folder: Rc::new({
                    let this = this.clone();
                    move |folder, _window, cx| {
                        let _ = this.update(cx, |app, cx| app.open_folder_tab(folder, cx));
                    }
                }),
                add: Rc::new(move |window, cx| {
                    let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
                }),
            };
            let bookmark_bar = (!bookmark_list.is_empty() || pin_count.is_some_and(|n| n > 0))
                .then(|| bookmarks::bar(bookmark_list, pin_count, self.pins_open, clicks, cx));
            let column = v_flex()
                .size_full()
                .children(self.update_banner(cx))
                .children(self.connection_banner(cx))
                .children(self.failed_mutations_banner(cx))
                .child(header);
            let column = match (&self.huddle, self.huddle_open) {
                (Some((_, bar)), true) => column.child(bar.clone()),
                _ => column,
            };
            let column = column.child(tabs).children(bookmark_bar);
            match &self.tab {
                ConversationTab::Home => {
                    let view = self.app_home_view(window, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
                // The Pins tab keeps the transcript under it; its panel is the right column.
                ConversationTab::Messages | ConversationTab::Pins => {
                    let column = column
                        .children(self.app_dm_banner(conversation.as_ref(), cx))
                        .child(div().flex_1().min_h_0().child(self.messages.clone()));
                    // In a public channel the user is not in, Slack's Join card takes the
                    // composer's place.
                    match conversation.as_ref().filter(|c| joinable(c)) {
                        Some(joinable) => column.child(self.join_card(joinable, cx)),
                        None => column
                            .children(self.dnd_banner(conversation.as_ref(), cx))
                            .child(self.composer.clone()),
                    }
                }
                ConversationTab::Canvas => {
                    let view = if self.fixtures.sample {
                        let this_pop = cx.weak_entity();
                        let on_pop_out: crate::canvas::CanvasPopOutFn =
                            Rc::new(move |doc, window, cx| {
                                let _ = this_pop.update(cx, |app, cx| {
                                    app.pop_out_canvas(doc, None, window, cx);
                                });
                            });
                        canvas_view(
                            &self.canvas_query,
                            Some(&self.canvas_search),
                            crate::canvas::CanvasActions {
                                on_pop_out: Some(on_pop_out),
                                ..Default::default()
                            },
                            cx,
                        )
                    } else {
                        let file_id = self.ws.active_canvas.clone().or_else(|| {
                            conversation
                                .as_ref()
                                .and_then(|c| c.properties.as_ref())
                                .and_then(|p| p.canvas.as_ref())
                                .and_then(|cv| cv.file_id.as_ref())
                                .map(|f| f.to_string())
                        });
                        let loaded = file_id.as_deref().and_then(|id| self.ws.canvases.get(id));
                        let open_browser = file_id.as_ref().map(|id| {
                            let url = self.canvas_link(id);
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                cx.open_url(&url);
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let on_edit = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |op, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::EditCanvas {
                                        file_id: fid.clone(),
                                        op,
                                    });
                                });
                            }) as crate::canvas::CanvasEditFn
                        });
                        let on_request_access = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::RequestFilePermission {
                                        file_id: fid.clone(),
                                        request_type: "upgrade".to_owned(),
                                    });
                                });
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let on_pop_out = file_id.as_ref().map(|id| {
                            let fid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |doc, window: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, cx| {
                                    app.pop_out_canvas(doc, Some(fid.clone()), window, cx);
                                });
                            }) as crate::canvas::CanvasPopOutFn
                        });
                        let canvas_url = file_id.as_ref().map(|id| self.canvas_link(id));
                        let on_pin_tab = file_id.as_ref().map(|_| {
                            let ch = self.channel.clone();
                            let label = loaded
                                .and_then(|res| res.as_ref().ok())
                                .map(|doc| doc.title.clone());
                            let this = cx.weak_entity();
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::AddConversationTab {
                                        channel: ch.clone(),
                                        tab_type: "canvas".to_owned(),
                                        label: label.clone(),
                                    });
                                });
                            }) as crate::canvas::OpenBrowserFn
                        });
                        crate::canvas::live_canvas_view(
                            file_id.as_deref().unwrap_or("live-canvas"),
                            loaded,
                            &self.canvas_query,
                            Some(&self.canvas_search),
                            crate::canvas::CanvasActions {
                                open_browser,
                                on_edit,
                                on_request_access,
                                on_pop_out,
                                on_pin_tab,
                                canvas_url,
                            },
                            cx,
                        )
                    };
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::List => {
                    let view = if self.fixtures.sample {
                        list_view(cx)
                    } else {
                        let file_id = self.ws.active_list.clone();
                        let loaded = file_id.as_deref().and_then(|id| self.ws.lists.get(id));
                        let open_browser = file_id.as_ref().map(|id| {
                            let url = self.canvas_link(id);
                            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                                cx.open_url(&url);
                            }) as crate::canvas::OpenBrowserFn
                        });
                        let this = cx.weak_entity();
                        let on_sort: crate::lists::SortColumnFn =
                            Rc::new(move |col_ix, _window, cx| {
                                let _ = this.update(cx, |app, cx| {
                                    app.list_sort = match app.list_sort {
                                        Some((c, true)) if c == col_ix => Some((col_ix, false)),
                                        Some((c, false)) if c == col_ix => None,
                                        _ => Some((col_ix, true)),
                                    };
                                    cx.notify();
                                });
                            });
                        let on_create_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |initial_fields, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::CreateListItem {
                                        list_id: lid.clone(),
                                        initial_fields,
                                    });
                                });
                            }) as crate::lists::CreateListItemFn
                        });
                        let on_update_cell = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, cell, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::UpdateListCell {
                                        list_id: lid.clone(),
                                        row_id,
                                        cells: vec![cell],
                                    });
                                });
                            }) as crate::lists::UpdateListCellFn
                        });
                        let on_delete_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::DeleteListItem {
                                        list_id: lid.clone(),
                                        row_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        let on_archive_item = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |row_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::ArchiveListItem {
                                        list_id: lid.clone(),
                                        row_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        let on_set_access = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let ch = self.channel.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |access_level, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::SetListAccess {
                                        list_id: lid.clone(),
                                        channel: Some(ch.clone()),
                                        access_level,
                                    });
                                });
                            }) as crate::lists::SetListAccessFn
                        });
                        let on_load_comments = file_id.as_ref().map(|id| {
                            let lid = id.clone();
                            let this = cx.weak_entity();
                            Rc::new(move |record_id, _: &mut Window, cx: &mut gpui::App| {
                                let _ = this.update(cx, |app, _| {
                                    app.bridge.send(Command::FetchListRecordComments {
                                        list_id: lid.clone(),
                                        record_id,
                                    });
                                });
                            }) as crate::lists::DeleteListItemFn
                        });
                        crate::lists::live_list_view(
                            loaded,
                            &self.fixtures,
                            &self.list_query,
                            Some(&self.list_search),
                            self.list_sort,
                            crate::lists::ListActions {
                                on_sort: Some(on_sort),
                                on_create_item,
                                on_update_cell,
                                on_delete_item,
                                on_archive_item,
                                on_set_access,
                                on_load_comments,
                                open_browser,
                            },
                            cx,
                        )
                    };
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::Files => {
                    // The sample workspace lists its messages' files, newest first.
                    let sample_files: Loaded<File>;
                    let files = if self.fixtures.sample {
                        let history = self.fixtures.messages.get(&self.channel);
                        sample_files = Ok(history
                            .into_iter()
                            .flat_map(|history| history.iter().rev())
                            .flat_map(|message| message.files.iter().cloned())
                            .collect());
                        Some(&sample_files)
                    } else {
                        self.ws.files.get(&self.channel)
                    };
                    let view = tab_views::files_view(&self.fixtures, files, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
                ConversationTab::Folder { bookmark_id, .. } => {
                    let bookmarks = self.ws.bookmarks.get(&self.channel);
                    let (on_add, on_remove): (
                        Option<tab_views::BookmarkAddFn>,
                        Option<tab_views::BookmarkRemoveFn>,
                    ) = if !self.fixtures.sample {
                        let this_add = cx.weak_entity();
                        let folder_for_add = bookmark_id.clone();
                        let ch_for_add = self.channel.clone();
                        let add_fn: tab_views::BookmarkAddFn =
                            std::rc::Rc::new(move |window, cx| {
                                let this_cb = this_add.clone();
                                let folder_cb = folder_for_add.clone();
                                let ch_cb = ch_for_add.clone();
                                bookmarks::open_add_dialog(window, cx, move |title, url, cx| {
                                    let _ = this_cb.update(cx, |app, _| {
                                        app.bridge.send(Command::AddBookmarkInFolder {
                                            channel: ch_cb.clone(),
                                            title,
                                            url,
                                            parent_id: folder_cb.clone(),
                                        });
                                    });
                                });
                            });
                        let this_rm = cx.weak_entity();
                        let ch_for_rm = self.channel.clone();
                        let rm_fn: tab_views::BookmarkRemoveFn =
                            std::rc::Rc::new(move |bm_id, cx| {
                                let _ = this_rm.update(cx, |app, _| {
                                    app.bridge.send(Command::RemoveBookmark {
                                        channel: ch_for_rm.clone(),
                                        bookmark_id: bm_id,
                                    });
                                });
                            });
                        (Some(add_fn), Some(rm_fn))
                    } else {
                        (None, None)
                    };
                    let view =
                        tab_views::folder_view(bookmark_id, bookmarks, on_add, on_remove, cx);
                    column.child(div().flex_1().min_h_0().child(view))
                }
            }
        };

        // Slack's DM list is wider than its channel sidebar, and each keeps the width it is
        // dragged to: one resizable layout (and so one remembered width) per destination.
        // The Activity page replaces the sidebar's list column.
        let nav = self.sidebar.read(cx).active_nav();
        let (layout, sidebar_width) = match nav {
            NavItem::Dms => ("workspace-dms", DM_LIST_WIDTH),
            NavItem::Activity => ("workspace-activity", SIDEBAR_WIDTH),
            _ => ("workspace", SIDEBAR_WIDTH),
        };
        let dark = cx.theme().mode.is_dark();
        let outline = cx.theme().sidebar_border;
        let side_panel = self.thread_open || self.profile_open || self.pins_open;
        // Like Slack, opening the thread / profile / pins panel narrows the conversation,
        // never the sidebar, and resizing the window only resizes the conversation: the
        // sidebar and the side panel are fixed at the widths Slack gives them.
        // Slack draws its own dividers (hairlines in dark mode, shadows in light), so the kit's
        // handle lines stay unpainted; the handles still resize.
        let mut panels = h_resizable(layout).with_handle_appearance(Rc::new(
            |_: &gpui_kit::base::ResizeHandleContext, _: &mut Window, _: &mut gpui::App| {
                Some(div().into_any_element())
            },
        ));
        if nav != NavItem::Activity {
            panels = panels.child(
                resizable_panel()
                    .size(sidebar_width)
                    .size_range(px(288.)..px(728.))
                    .fixed()
                    .child(self.sidebar.clone()),
            );
        }
        let mut panels = panels.child(
            resizable_panel().child(
                div()
                    .relative()
                    .size_full()
                    .bg(background)
                    .when(!side_panel, |column| {
                        column
                            .rounded_tr(px(PANEL_RADIUS))
                            .rounded_br(px(PANEL_RADIUS))
                    })
                    .child(main_column)
                    // Light Slack's side panel casts its shadow over the conversation's edge.
                    .when(!dark && side_panel, |column| {
                        column.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(16.))
                                .bg(crate::theme::panel_shadow_edge()),
                        )
                    }),
            ),
        );
        // Dark Slack's side panel starts with a hairline; light Slack's is lifted by its shadow.
        let side_pane = |pane: AnyElement| {
            div()
                .size_full()
                .when(dark, |pane| pane.border_l_1().border_color(outline))
                .child(pane)
        };
        // A profile opened from the thread stacks over it.
        if self.profile_open {
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(self.profile.clone().into_any_element())),
            );
        } else if self.thread_open {
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(self.thread.clone().into_any_element())),
            );
        } else if self.pins_open {
            let this = cx.weak_entity();
            let close = move |_: &mut Window, cx: &mut gpui::App| {
                this.update(cx, |app, cx| {
                    app.pins_open = false;
                    if app.tab == ConversationTab::Pins {
                        app.tab = ConversationTab::Messages;
                    }
                    cx.notify();
                })
                .ok();
            };
            let sample_pins: Loaded<Message>;
            let pinned = if self.fixtures.sample {
                sample_pins = Ok(pins::pinned(&self.fixtures, &self.channel)
                    .into_iter()
                    .cloned()
                    .collect());
                Some(&sample_pins)
            } else {
                self.ws.pins.get(&self.channel)
            };
            let panel = pins::pins_panel(&self.fixtures, pinned, &title, close, cx);
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(side_pane(panel.into_any_element())),
            );
        }

        let (switcher, rail) = self.sidebar.update(cx, |sidebar, cx| {
            (sidebar.workspace_switcher(cx), sidebar.tab_rail(cx))
        });

        v_flex()
            .id("root")
            .key_context("App")
            .track_focus(&self.focus)
            .on_action(
                cx.listener(|this, _: &QuickSwitch, window, cx| this.open_switcher(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenPreferences, window, cx| {
                this.open_preferences(None, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::menu_bar::CheckForUpdates, _, cx| {
                    if this.available_update.is_some() {
                        this.install_available_update(cx);
                    } else {
                        this.check_for_updates(true, cx);
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &ShowShortcuts, window, cx| this.open_shortcuts(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenDms, _, cx| this.show_nav(NavItem::Dms, cx)))
            .on_action(
                cx.listener(|this, _: &OpenActivity, _, cx| this.show_nav(NavItem::Activity, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenDrafts, _, cx| this.open_drafts(cx)))
            .on_action(
                cx.listener(|this, _: &OpenChannelBrowser, window, cx| {
                    this.open_browser(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &OpenDownloads, window, cx| this.open_downloads(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SetStatus, window, cx| this.open_status_picker(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.search.read(cx).focus_handle(cx).focus(window, cx);
                this.open_search_menu(cx);
            }))
            // Only with somewhere to go, so the menu bar greys out Back / Forward as Slack's does.
            .when(!self.back.is_empty(), |root| {
                root.on_action(
                    cx.listener(|this, _: &NavigateBack, window, cx| this.go_back(window, cx)),
                )
            })
            .when(!self.forward.is_empty(), |root| {
                root.on_action(
                    cx.listener(|this, _: &NavigateForward, window, cx| {
                        this.go_forward(window, cx)
                    }),
                )
            })
            .on_action(cx.listener(|this, switch: &SwitchWorkspace, window, cx| {
                this.switch_workspace(switch.0, window, cx)
            }))
            .on_action(cx.listener(|this, tab: &SelectConversationTab, _, cx| {
                this.select_conversation_tab(tab.0, cx)
            }))
            .on_action(cx.listener(|this, _: &NextWorkspace, window, cx| {
                let count = this.parked.len().max(1);
                this.switch_workspace((this.workspace + 1) % count, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousWorkspace, window, cx| {
                let count = this.parked.len().max(1);
                this.switch_workspace((this.workspace + count - 1) % count, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NewMessage, window, cx| this.open_compose("", window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenThreads, _, cx| this.open_threads(cx)))
            .on_action(
                cx.listener(|this, _: &OpenUnreads, window, cx| this.open_unreads(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenLater, _, cx| this.open_later(cx)))
            .on_action(cx.listener(|this, _: &OpenFiles, _, cx| this.open_files(cx)))
            .on_action(cx.listener(|this, _: &OpenPeople, _, cx| this.open_directory(cx)))
            .on_action(cx.listener(|this, _: &OpenOwnProfile, _, cx| {
                this.open_profile(this.fixtures.self_user.clone(), cx)
            }))
            .on_action(
                cx.listener(|this, open: &OpenRecent, _, cx| this.select(open.0.clone(), cx)),
            )
            .on_action(cx.listener(|this, sign_out: &SignOut, _, cx| this.sign_out(sign_out.0, cx)))
            .on_action(cx.listener(|this, _: &PreviousConversation, _, cx| {
                this.step_conversation(false, false, cx)
            }))
            .on_action(cx.listener(|this, _: &NextConversation, _, cx| {
                this.step_conversation(true, false, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousUnread, _, cx| {
                this.step_conversation(false, true, cx)
            }))
            .on_action(
                cx.listener(|this, _: &NextUnread, _, cx| this.step_conversation(true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &MarkConversationRead, _, cx| this.mark_open_read(cx)))
            // A focused field passes Esc on when it has nothing to close (the composer's menu,
            // extra cursors): that marks the conversation read too, as in Slack, and Esc still
            // reaches the listeners outside the root (an open dialog's).
            .on_action(cx.listener(|this, _: &InputEscape, _, cx| {
                this.mark_open_read(cx);
                cx.propagate();
            }))
            // Slack's one-key message shortcuts (E, Delete, T/Right, F, P, A, U, M, R), all
            // through the transcript's shared [`MessageAction`] handlers.
            .on_action(cx.listener(|this, _: &FocusEditMessage, window, cx| {
                this.focus_message(MessageAction::Edit, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusDeleteMessage, window, cx| {
                this.focus_message(MessageAction::Delete, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusReplyInThread, window, cx| {
                this.focus_message(MessageAction::ReplyInThread, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusForwardMessage, window, cx| {
                this.focus_message(MessageAction::Forward, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusTogglePin, window, cx| {
                this.focus_message(MessageAction::TogglePin, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusSaveMessage, window, cx| {
                this.focus_message(MessageAction::SaveForLater, window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusMarkMessageUnread, window, cx| {
                this.focus_message(MessageAction::MarkUnread, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &FocusRemindMe, window, cx| {
                    this.open_remind_menu(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &FocusReact, window, cx| {
                this.focus_message(
                    MessageAction::PickReaction(window.mouse_position()),
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &MarkAllRead, window, cx| {
                this.request_mark_all_read(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_zoom(Some(1), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_zoom(Some(-1), cx)))
            .on_action(cx.listener(|this, _: &ActualSize, _, cx| this.step_zoom(None, cx)))
            .on_action(cx.listener(|this, _: &SearchConversation, window, cx| {
                this.search_in_conversation(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleWorkspaceRail, window, cx| {
                this.toggle_workspace_rail(window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleRightPane, _, cx| this.toggle_right_pane(cx)))
            .on_action(cx.listener(|this, _: &JumpToUnread, _, cx| this.jump_to_unread(cx)))
            .on_action(cx.listener(|this, _: &OpenHome, _, cx| this.open_home(cx)))
            .on_action(cx.listener(|this, _: &CreateChannelAction, window, cx| {
                this.open_create_channel(window, cx)
            }))
            .on_action(cx.listener(|this, _: &PopOutConversation, window, cx| {
                let ch = this.channel.clone();
                this.pop_out_conversation(ch, window, cx);
            }))
            .on_action(cx.listener(|this, _: &UnsendLastMessage, window, cx| {
                this.unsend_last_message(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextNavTab, _, cx| {
                this.sidebar.update(cx, |s, cx| s.cycle_nav(true, cx));
            }))
            .on_action(cx.listener(|this, _: &PrevNavTab, _, cx| {
                this.sidebar.update(cx, |s, cx| s.cycle_nav(false, cx));
            }))
            .on_action(cx.listener(|this, _: &FocusNextPane, window, cx| {
                this.cycle_pane_focus(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusPrevPane, window, cx| {
                this.cycle_pane_focus(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &BrowseUp, _, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.step(false, cx));
                } else if this.showing_transcript() {
                    this.messages.update(cx, |m, cx| m.step_focus(false, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &BrowseDown, _, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.step(true, cx));
                } else if this.showing_transcript() {
                    this.messages.update(cx, |m, cx| m.step_focus(true, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &BrowseOpen, window, cx| {
                if this.activity_open {
                    this.activity.update(cx, |a, cx| a.open_selected(cx));
                } else if this.showing_transcript() {
                    this.focus_message(MessageAction::ReplyInThread, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ExitThread, window, cx| {
                if this.thread_open {
                    this.thread_open = false;
                    this.messages
                        .update(cx, |list, cx| list.set_open_thread(None, cx));
                    this.focus.focus(window, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &JumpToOldest, _, cx| {
                if this.showing_transcript() {
                    this.messages
                        .update(cx, |m, cx| m.jump(crate::messages::Jump::Beginning, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &JumpToNewest, _, cx| {
                if this.showing_transcript() {
                    this.messages
                        .update(cx, |m, cx| m.jump(crate::messages::Jump::Recent, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &OpenMore, _, cx| {
                this.open_directory(cx);
            }))
            .on_action(cx.listener(|this, _: &NextSection, _, cx| {
                if let Some(next) = this.sidebar.read(cx).step_section(&this.channel, true) {
                    this.select(next, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &PrevSection, _, cx| {
                if let Some(prev) = this.sidebar.read(cx).step_section(&this.channel, false) {
                    this.select(prev, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleHuddleMute, _, cx| {
                this.toggle_huddle_mute(cx);
            }))
            .relative()
            .size_full()
            .bg(chrome)
            .text_color(foreground)
            // Window-level capture for hardware mouse Back/Forward buttons (buttons 3 & 4 and
            // swipe-ended events). Registered via `window.on_mouse_event` rather than an element
            // hitbox so it works unconditionally even over `.occlude()` overlays or dialogs.
            .child({
                let nav_app = cx.weak_entity();
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let nav_app = nav_app.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            match event.button {
                                MouseButton::Navigate(NavigationDirection::Back) => {
                                    let _ = nav_app.update(cx, |app, cx| app.go_back(window, cx));
                                    cx.stop_propagation();
                                }
                                MouseButton::Navigate(NavigationDirection::Forward) => {
                                    let _ =
                                        nav_app.update(cx, |app, cx| app.go_forward(window, cx));
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        });
                    },
                )
                .absolute()
                .size_0()
            })
            .child(div().absolute().top_0().left_0().size(glow_side).bg(glow))
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(
                        {
                            let s = self.preferences.read(cx).settings();
                            s.show_workspace_rail && !s.simplified_layout
                        },
                        |row| row.child(switcher),
                    )
                    // Slack's workspace panel: the tab rail and the tab panel (sidebar,
                    // conversation, side panel) in one rounded card inset from the window's
                    // right and bottom edges, lifted by a shadow in light mode and outlined in
                    // dark mode.
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .mr_1()
                            .mb_1()
                            .rounded(px(PANEL_RADIUS))
                            .when(!dark, |panel| panel.shadow(vec![shadow(-4., 0.09, 16.)]))
                            .child(rail)
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .rounded_tr(px(PANEL_RADIUS))
                                    .rounded_br(px(PANEL_RADIUS))
                                    .overflow_hidden()
                                    .when(dark, |panel| panel.border_1().border_color(outline))
                                    .child(panels)
                                    .when_some(self.huddle_invite.clone(), |panel, banner| {
                                        panel.child(
                                            div()
                                                .id("huddle-invite-float")
                                                .occlude()
                                                .absolute()
                                                .top_4()
                                                .right_4()
                                                .child(banner),
                                        )
                                    })
                                    .when(side_panel, |panel| {
                                        let outline = dark.then_some(outline);
                                        panel
                                            .children(corner_cap(true, chrome, outline))
                                            .children(corner_cap(false, chrome, outline))
                                    }),
                            ),
                    ),
            )
            // Official Slack full-width bottom huddle strip spanning across the main window.
            .when_some(self.huddle_dock.clone(), |root, dock| {
                root.child(div().flex_none().w_full().px_1().pb_1().child(dock))
            })
            .when(self.user_menu_open, |root| {
                root.child(
                    div()
                        .id("user-menu-scrim")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.user_menu_open = false;
                            this.user_menu.update(cx, |menu, cx| menu.closed(cx));
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("user-menu")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        // Where Slack drops it: beside the tab rail's avatar.
                        .bottom(px(24.))
                        .left(px(124.))
                        .child(self.user_menu.clone()),
                )
            })
            .when_some(
                self.picker.as_ref().map(|(_, position)| *position),
                |root, position| {
                    // Grow away from the nearest window edges: up from low clicks (composer),
                    // leftwards from clicks on the right (hover bars).
                    let viewport = window.viewport_size();
                    let anchor = match (
                        position.y > viewport.height / 2.,
                        position.x > viewport.width / 2.,
                    ) {
                        (true, true) => Anchor::BottomRight,
                        (true, false) => Anchor::BottomLeft,
                        (false, true) => Anchor::TopRight,
                        (false, false) => Anchor::TopLeft,
                    };
                    root.child(
                        div()
                            .id("emoji-picker-scrim")
                            .occlude()
                            .on_scroll(|_, _, cx| cx.stop_propagation())
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_picker(window, cx)),
                            ),
                    )
                    .child(deferred(
                        anchored()
                            .position(position)
                            .anchor(anchor)
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .id("emoji-picker-overlay")
                                    .occlude()
                                    .on_scroll(|_, _, cx| cx.stop_propagation())
                                    .child(self.emoji_picker.clone()),
                            ),
                    ))
                },
            )
            .when_some(
                self.remind_menu
                    .as_ref()
                    .map(|(menu, position, _)| (menu.clone(), *position)),
                |root, (menu, position)| {
                    // Slack drops the reminder menu beside the message; the mouse is the
                    // nearest fixed point the focused message has.
                    root.child(deferred(
                        anchored()
                            .position(position)
                            .anchor(Anchor::TopLeft)
                            .snap_to_window_with_margin(Edges::all(px(8.)))
                            .child(
                                div()
                                    .id("remind-menu-overlay")
                                    .occlude()
                                    .on_scroll(|_, _, cx| cx.stop_propagation())
                                    .child(menu),
                            ),
                    ))
                },
            )
            .when_some(self.profile_hover.card.clone(), |root, trigger| {
                // Slack's member hover card, anchored at its trigger: above one in the
                // lower half of the viewport, below one in the upper half.
                let this = cx.weak_entity();
                let user = trigger.user.clone();
                let actions = hover_card::CardActions {
                    open_profile: {
                        let this = this.clone();
                        Rc::new(move |_window, cx| {
                            let _ = this.update(cx, |app, cx| {
                                app.close_hover_card(cx);
                                app.open_profile(user.clone(), cx);
                            });
                        })
                    },
                    message: {
                        let (this, user) = (this.clone(), trigger.user.clone());
                        Rc::new(move |window, cx| {
                            let _ = this.update(cx, |app, cx| app.message_user(&user, window, cx));
                        })
                    },
                    set_status: {
                        let this = this.clone();
                        Rc::new(move |window, cx| {
                            let _ = this.update(cx, |app, cx| {
                                app.close_hover_card(cx);
                                app.open_status_picker(window, cx);
                            });
                        })
                    },
                    hovered: {
                        let this = this.clone();
                        Rc::new(move |inside, _window, cx| {
                            let _ = this.update(cx, |app, cx| app.hover_card_hovered(inside, cx));
                        })
                    },
                    dismiss: {
                        let this = this.clone();
                        Rc::new(move |_window, cx| {
                            let _ = this.update(cx, |app, cx| app.close_hover_card(cx));
                        })
                    },
                };
                let (anchor, position) =
                    hover_card::anchor_for(&trigger.anchor, window.viewport_size());
                root.child(deferred(
                    anchored()
                        .position(position)
                        .anchor(anchor)
                        .snap_to_window_with_margin(Edges::all(px(8.)))
                        .child(hover_card::card(
                            &self.fixtures,
                            &self.self_state,
                            &trigger.user,
                            &actions,
                            cx,
                        )),
                ))
            })
            .when(self.search_menu_open, |root| {
                let bounds = self.search_menu.read(cx).pill_bounds();
                root.when_some(
                    (bounds.size.width > px(0.)).then_some(bounds),
                    |root, bounds| {
                        // Slack's search panel: hangs over the pill from its top-left
                        // corner, just as wide, above every pane. A press outside it (and
                        // outside the pill) folds it.
                        root.child(deferred(
                            anchored()
                                .position(bounds.origin)
                                .anchor(Anchor::TopLeft)
                                .snap_to_window_with_margin(Edges::all(px(8.)))
                                .child(
                                    div()
                                        .id("search-menu-overlay")
                                        .occlude()
                                        .on_scroll(|_, _, cx| cx.stop_propagation())
                                        .w(bounds.size.width)
                                        .on_mouse_down_out(cx.listener(
                                            |this, event: &MouseDownEvent, window, cx| {
                                                if !this
                                                    .search_menu
                                                    .read(cx)
                                                    .pill_bounds()
                                                    .contains(&event.position)
                                                {
                                                    this.close_search_menu(window, cx);
                                                }
                                            },
                                        ))
                                        .child(self.search_menu.clone()),
                                ),
                        ))
                    },
                )
            })
            .when_some(self.image_viewer.clone(), |root, viewer| {
                // Slack's image viewer: an opaque gray surface covering the window below
                // the title bar; the panel itself renders the viewer.
                root.child(
                    div()
                        .id("image-viewer-overlay")
                        .occlude()
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        .top(px(40.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(viewer),
                )
            })
    }
}
