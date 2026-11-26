//! Root `Render` implementation for `App`: window chrome, workspace rail, resizable panes, and overlays.

use super::*;

impl App {
    /// Shared Canvas callbacks used by both live and sample canvases (profile/channel links,
    /// media links, print/PDF export, lookups, and pop-out).
    fn build_common_canvas_actions(
        &self,
        live_file_id: Option<String>,
        cx: &Context<Self>,
    ) -> crate::canvas::CanvasActions {
        let this_pop = cx.weak_entity();
        let pop_fid = live_file_id.clone();
        let on_pop_out: crate::canvas::CanvasPopOutFn = Rc::new(move |doc, window, cx| {
            let _ = this_pop.update(cx, |app, cx| {
                app.pop_out_canvas(doc, pop_fid.clone(), window, cx);
            });
        });
        let user_lookup = crate::canvas::user_lookup_from_fixtures(self.fixtures.clone());
        let channel_lookup = crate::canvas::channel_lookup_from_fixtures(self.fixtures.clone());
        let this_prof = cx.weak_entity();
        let on_open_profile: crate::canvas::CanvasOpenProfileFn =
            Rc::new(move |user_id, _window, cx| {
                let _ = this_prof.update(cx, |app, cx| {
                    app.open_profile(user_id, cx);
                });
            });
        let this_ch = cx.weak_entity();
        let on_open_channel: crate::canvas::CanvasOpenChannelFn =
            Rc::new(move |channel_id, _window, cx| {
                let _ = this_ch.update(cx, |app, cx| {
                    app.select(channel_id, cx);
                });
            });
        let is_live = live_file_id.is_some();
        let this_emb = cx.weak_entity();
        let on_open_embed: crate::canvas::CanvasOpenEmbedFn =
            Rc::new(move |kind, fid, url, _window, cx| {
                if !is_live {
                    let _ = this_emb.update(cx, |app, cx| match kind {
                        crate::canvas::CanvasEmbedKind::Canvas => {
                            app.select_tab(ConversationTab::Canvas, cx);
                        }
                        crate::canvas::CanvasEmbedKind::List => {
                            app.select_tab(ConversationTab::List, cx);
                        }
                    });
                } else if !fid.is_empty() {
                    let _ = this_emb.update(cx, |app, cx| match kind {
                        crate::canvas::CanvasEmbedKind::Canvas => {
                            app.open_live_canvas(fid, cx);
                        }
                        crate::canvas::CanvasEmbedKind::List => {
                            app.open_live_list(fid, cx);
                        }
                    });
                } else if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                }
            });
        let on_open_media: crate::canvas::CanvasOpenMediaFn =
            Rc::new(move |url, _kind, window, cx| {
                if url.starts_with("https://") || url.starts_with("http://") {
                    cx.open_url(&url);
                } else {
                    notify(
                        "Media link",
                        format!("Cannot open unsupported media URL: {url}"),
                        window,
                        cx,
                    );
                }
            });
        let on_print_canvas: crate::canvas::CanvasExportFn =
            Rc::new(
                move |doc, window, cx| match crate::canvas::trigger_canvas_print(&doc) {
                    Ok(_) => notify(
                        "Print canvas",
                        "Opened printable canvas preview.",
                        window,
                        cx,
                    ),
                    Err(err) => notify("Print canvas", err, window, cx),
                },
            );
        let on_save_pdf: crate::canvas::CanvasExportFn = Rc::new(move |doc, window, cx| {
            let stem = crate::canvas::canvas_export_stem(&doc.title);
            let target = std::env::temp_dir().join(format!("{stem}.pdf"));
            match crate::canvas::save_canvas_pdf_to_path(&doc, &target) {
                Ok(saved) => notify(
                    "Saved PDF",
                    format!("Exported to {}", saved.display()),
                    window,
                    cx,
                ),
                Err(err) => notify("Save as PDF", err, window, cx),
            }
        });
        crate::canvas::CanvasActions {
            on_pop_out: Some(on_pop_out),
            on_open_media: Some(on_open_media),
            user_lookup: Some(user_lookup),
            channel_lookup: Some(channel_lookup),
            on_open_profile: Some(on_open_profile),
            on_open_channel: Some(on_open_channel),
            on_open_embed: Some(on_open_embed),
            on_print_canvas: Some(on_print_canvas),
            on_save_pdf: Some(on_save_pdf),
            ..Default::default()
        }
    }

    fn render_canvas_tab(
        &self,
        conversation: Option<&Conversation>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        if self.fixtures.sample {
            let mut actions = self.build_common_canvas_actions(None, cx);
            actions.on_pin_tab = Some(Rc::new(move |window, cx| {
                notify(
                    "Pinned to channel tabs",
                    "Canvas pinned to channel header tabs.",
                    window,
                    cx,
                );
            }));
            actions.on_upload_file = Some(Rc::new(move |on_done, window, cx| {
                let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Upload file to canvas".into()),
                });
                window
                    .spawn(cx, async move |cx| {
                        let Ok(Ok(Some(paths))) = rx.await else {
                            return;
                        };
                        let Some(path) = paths.into_iter().next() else {
                            return;
                        };
                        let filename = path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("attachment")
                            .to_owned();
                        let url = format!("file://{}", path.display());
                        let _ = cx.update(|window, cx| {
                            on_done(
                                crate::canvas::UploadedCanvasFile {
                                    filename: filename.clone(),
                                    url,
                                },
                                window,
                                cx,
                            );
                            notify(
                                "Inserted file",
                                format!("Added {filename} to canvas."),
                                window,
                                cx,
                            );
                        });
                    })
                    .detach();
            }));
            return canvas_view(&self.canvas_query, Some(&self.canvas_search), actions, cx);
        }

        let file_id = self.ws.active_canvas.clone().or_else(|| {
            conversation
                .and_then(|c| c.properties.as_ref())
                .and_then(|p| p.canvas.as_ref())
                .and_then(|cv| cv.file_id.as_ref())
                .map(|f| f.to_string())
        });
        let loaded = file_id.as_deref().and_then(|id| self.ws.canvases.get(id));
        let mut actions = self.build_common_canvas_actions(file_id.clone(), cx);
        if file_id.is_none() {
            actions.on_pop_out = None;
        }
        actions.open_browser = file_id.as_ref().map(|id| {
            let url = self.canvas_link(id);
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                cx.open_url(&url);
            }) as crate::canvas::OpenBrowserFn
        });
        actions.on_edit = file_id.as_ref().map(|id| {
            let fid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |op, _: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::EditCanvas {
                        file_id: fid.clone(),
                        op,
                    });
                });
            }) as crate::canvas::CanvasEditFn
        });
        actions.on_request_access = file_id.as_ref().map(|id| {
            let fid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::RequestFilePermission {
                        file_id: fid.clone(),
                        request_type: "upgrade".to_owned(),
                    });
                });
            }) as crate::canvas::OpenBrowserFn
        });
        actions.canvas_url = file_id.as_ref().map(|id| self.canvas_link(id));
        let tab_limit_msg = conversation.and_then(|conv| {
            let specs = self.conversation_tab_specs(conv);
            tabs::tab_limit_message(&specs)
        });
        actions.on_pin_tab = file_id.as_ref().map(|_| {
            let ch = self.channel.clone();
            let label = loaded
                .and_then(|res| res.as_ref().ok())
                .map(|doc| doc.title.clone());
            let limit_err = tab_limit_msg.clone();
            let this = cx.weak_entity();
            Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
                if let Some(msg) = &limit_err {
                    notify("Tab limit reached", msg.clone(), window, cx);
                    return;
                }
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(ChannelCommand::AddConversationTab {
                        channel: ch.clone(),
                        tab_type: "canvas".to_owned(),
                        label: label.clone(),
                    });
                });
                notify(
                    "Pinned to channel tabs",
                    "Canvas added to the channel header tabs.",
                    window,
                    cx,
                );
            }) as crate::canvas::OpenBrowserFn
        });
        actions.on_upload_file = file_id.as_ref().map(|_| {
            let ch = self.channel.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |on_done: crate::canvas::CanvasUploadCompleteFn,
                      window: &mut Window,
                      cx: &mut gpui::App| {
                    let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: Some("Upload file to canvas".into()),
                    });
                    let ch_u = ch.clone();
                    let this_u = this.clone();
                    window
                        .spawn(cx, async move |cx| {
                            let Ok(Ok(Some(paths))) = rx.await else {
                                return;
                            };
                            let Some(path) = paths.into_iter().next() else {
                                return;
                            };
                            // A picked file can be gigabytes: read it off the UI thread.
                            let read_path = path.clone();
                            let bytes = cx
                                .background_executor()
                                .spawn(async move { std::fs::read(&read_path) })
                                .await;
                            let bytes = match bytes {
                                Ok(bytes) if !bytes.is_empty() => bytes,
                                Ok(_) => return,
                                Err(err) => {
                                    let _ = cx.update(|window, cx| {
                                        notify("Could not read file", err.to_string(), window, cx)
                                    });
                                    return;
                                }
                            };
                            let filename = path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("attachment")
                                .to_owned();
                            let url = format!("file://{}", path.display());
                            let _ = cx.update(|window, cx| {
                                let _ = this_u.update(cx, |app, _| {
                                    app.bridge.send(CollabCommand::UploadFiles {
                                        channel: ch_u.clone(),
                                        thread_ts: None,
                                        content: None,
                                        files: vec![slackless_core::api::UploadFile {
                                            filename: filename.clone(),
                                            bytes,
                                        }],
                                    });
                                });
                                on_done(
                                    crate::canvas::UploadedCanvasFile {
                                        filename: filename.clone(),
                                        url,
                                    },
                                    window,
                                    cx,
                                );
                                notify(
                                    "Uploaded file to canvas",
                                    format!("Uploaded and inserted {filename}."),
                                    window,
                                    cx,
                                );
                            });
                        })
                        .detach();
                },
            ) as crate::canvas::CanvasUploadFileFn
        });
        actions.meta = file_id
            .as_deref()
            .and_then(|id| self.ws.canvas_metas.get(id).cloned());
        actions.comment_threads = file_id
            .as_deref()
            .and_then(|id| self.ws.canvas_comments.get(id).cloned());
        actions.status_banner = file_id
            .as_deref()
            .and_then(|id| self.ws.canvas_status.get(id).cloned());
        actions.updated_note = file_id
            .as_deref()
            .and_then(|id| self.ws.canvas_updated_notes.get(id).cloned());
        actions.on_load_comments = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::LoadCanvasComments {
                        canvas_id: cid.clone(),
                    });
                });
            }) as crate::canvas::CanvasCommentsLoadFn
        });
        actions.on_post_comment = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |text, thread_ts, _: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::PostCanvasComment {
                        canvas_id: cid.clone(),
                        text,
                        thread_ts,
                    });
                });
            }) as crate::canvas::CanvasCommentPostFn
        });
        actions.on_resolve_comment = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |thread_ts, resolved, _: &mut Window, cx: &mut gpui::App| {
                    let _ = this.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::ResolveCanvasComment {
                            canvas_id: cid.clone(),
                            thread_ts,
                            resolved,
                        });
                    });
                },
            ) as crate::canvas::CanvasCommentResolveFn
        });
        actions.on_resolve_all_comments = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::ResolveAllCanvasComments {
                        canvas_id: cid.clone(),
                    });
                });
            }) as crate::canvas::CanvasCommentResolveAllFn
        });
        actions.on_set_access = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |req: crate::canvas::ShareAccessRequest,
                      _: &mut Window,
                      cx: &mut gpui::App| {
                    let _ = this.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::SetCanvasAccess {
                            canvas_id: cid.clone(),
                            access_level: req.access_level,
                            channel_ids: req.channel_ids,
                            user_ids: req.user_ids,
                            post_permalink: req.post_permalink,
                        });
                    });
                },
            ) as crate::canvas::CanvasAccessSetFn
        });
        actions.on_delete_access = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |channel_ids, user_ids, _: &mut Window, cx: &mut gpui::App| {
                    let _ = this.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::DeleteCanvasAccess {
                            canvas_id: cid.clone(),
                            channel_ids,
                            user_ids,
                        });
                    });
                },
            ) as crate::canvas::CanvasAccessDeleteFn
        });
        actions.on_delete_canvas = file_id.as_ref().map(|id| {
            let cid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::DeleteCanvas {
                        canvas_id: cid.clone(),
                    });
                });
            }) as crate::canvas::CanvasDeleteFn
        });
        actions.on_refresh_canvas = file_id.as_ref().map(|id| {
            let fid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, cx| {
                    app.ws.canvas_updated_notes.remove(&fid);
                    app.bridge.send(CollabCommand::LoadCanvas {
                        file_id: fid.clone(),
                    });
                    cx.notify();
                });
            }) as crate::canvas::OpenBrowserFn
        });
        actions.on_dismiss_updated = file_id.as_ref().map(|id| {
            let fid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, cx| {
                    app.ws.canvas_updated_notes.remove(&fid);
                    cx.notify();
                });
            }) as crate::canvas::OpenBrowserFn
        });
        crate::canvas::live_canvas_view(
            file_id.as_deref().unwrap_or("live-canvas"),
            loaded,
            &self.canvas_query,
            Some(&self.canvas_search),
            actions,
            cx,
        )
    }

    fn render_list_tab(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        if self.fixtures.sample {
            return list_view(cx);
        }
        let file_id = self.ws.active_list.clone();
        let loaded = file_id.as_deref().and_then(|id| self.ws.lists.get(id));
        let open_browser = file_id.as_ref().map(|id| {
            let url = self.canvas_link(id);
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                cx.open_url(&url);
            }) as crate::canvas::OpenBrowserFn
        });
        let this = cx.weak_entity();
        let on_sort: crate::lists::SortColumnFn = Rc::new(move |col_ix, _window, cx| {
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
                    app.bridge.send(CollabCommand::CreateListItem {
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
                    app.bridge.send(CollabCommand::UpdateListCell {
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
                    app.bridge.send(CollabCommand::DeleteListItem {
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
                    app.bridge.send(CollabCommand::ArchiveListItem {
                        list_id: lid.clone(),
                        row_id,
                    });
                });
            }) as crate::lists::DeleteListItemFn
        });
        let on_set_access = file_id.as_ref().map(|id| {
            let lid = id.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |req: crate::lists::ListShareAccessRequest,
                      _: &mut Window,
                      cx: &mut gpui::App| {
                    let _ = this.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::SetListAccess {
                            list_id: lid.clone(),
                            access_level: req.access_level,
                            channel_ids: req.channel_ids,
                            user_ids: req.user_ids,
                            post_permalink: req.post_permalink,
                        });
                    });
                },
            ) as crate::lists::ListAccessSetFn
        });
        let on_delete_access = file_id.as_ref().map(|id| {
            let lid = id.clone();
            let this = cx.weak_entity();
            Rc::new(
                move |channel_ids: Vec<ChannelId>,
                      user_ids: Vec<UserId>,
                      _: &mut Window,
                      cx: &mut gpui::App| {
                    let _ = this.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::DeleteListAccess {
                            list_id: lid.clone(),
                            channel_ids,
                            user_ids,
                        });
                    });
                },
            ) as crate::lists::ListAccessDeleteFn
        });
        let on_load_comments = file_id.as_ref().map(|id| {
            let lid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |record_id, _: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, _| {
                    app.bridge.send(CollabCommand::FetchListRecordComments {
                        list_id: lid.clone(),
                        record_id,
                    });
                });
            }) as crate::lists::DeleteListItemFn
        });
        let on_open_in_threads = file_id.as_ref().map(|id| {
            let lid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |record_id, _: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, cx| {
                    app.bridge.send(CollabCommand::FetchListRecordComments {
                        list_id: lid.clone(),
                        record_id,
                    });
                    app.open_threads(cx);
                    app.threads.update(cx, |view, cx| {
                        view.set_filter(crate::threads_view::ThreadsFilter::ListItems, cx);
                    });
                });
            }) as crate::lists::DeleteListItemFn
        });
        let on_add_tab = file_id.as_ref().map(|_| {
            let ch = self.channel.clone();
            let title = loaded
                .and_then(|res| res.as_ref().ok())
                .map(|data| data.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "List".to_owned());
            let this = cx.weak_entity();
            Rc::new(move |window: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, cx| {
                    app.bridge.send(ChannelCommand::AddConversationTab {
                        channel: ch.clone(),
                        tab_type: "list".to_owned(),
                        label: Some(title.clone()),
                    });
                    crate::actions::notify(
                        "Tab added",
                        format!("Pinned \"{title}\" as a conversation tab."),
                        window,
                        cx,
                    );
                });
            }) as crate::lists::ListCommandFn
        });
        let on_delete_list = file_id.as_ref().map(|id| {
            let fid = id.clone();
            let this = cx.weak_entity();
            Rc::new(move |_: &mut Window, cx: &mut gpui::App| {
                let _ = this.update(cx, |app, cx| {
                    app.bridge.send(CollabCommand::DeleteFile {
                        file_id: fid.clone(),
                    });
                    app.select_tab(ConversationTab::Messages, cx);
                });
            }) as crate::lists::ListCommandFn
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
                on_delete_access,
                on_load_comments,
                on_open_in_threads,
                on_add_tab,
                on_delete_list,
                open_browser,
            },
            cx,
        )
    }

    fn render_channel_column(
        &self,
        title: &str,
        topic: &str,
        conversation: Option<&Conversation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let header = self.conversation_header(title, topic, conversation, cx);
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
            workflow_run: Rc::new({
                let this = this.clone();
                move |url, window, cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.trip_workflow_url(url, "channel_bookmark", window, cx);
                    });
                }
            }),
            workflow_details: Rc::new({
                let this = this.clone();
                move |req, window, cx| {
                    let _ = this.update(cx, |app, cx| app.open_workflow_details(req, window, cx));
                }
            }),
            add: Rc::new(move |window, cx| {
                let _ = this.update(cx, |app, cx| app.open_link_dialog(window, cx));
            }),
        };
        let bookmark_bar =
            (!bookmark_list.is_empty() || pin_count.is_some_and(|n| n > 0)).then(|| {
                bookmarks::bar(
                    bookmark_list,
                    pin_count,
                    self.right_pane.is_pins(),
                    clicks,
                    cx,
                )
            });
        let column = v_flex()
            .size_full()
            .children(self.update_banner(cx))
            .children(self.connection_banner(cx))
            .children(self.failed_mutations_banner(cx))
            .child(header)
            .child(tabs)
            .children(bookmark_bar);
        match &self.tab {
            ConversationTab::Home => {
                let view = self.app_home_view(window, cx);
                column.child(div().flex_1().min_h_0().child(view))
            }
            // The Pins tab keeps the transcript under it; its panel is the right column.
            ConversationTab::Messages | ConversationTab::Pins => {
                let outbox_strip = crate::outbox::inline_outbox_strip(
                    &self.ws.outbox,
                    &self.channel,
                    None,
                    crate::outbox::OutboxActions {
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
                    "channel",
                    cx,
                );
                let column = column
                    .children(self.app_dm_banner(conversation, cx))
                    .child(div().flex_1().min_h_0().child(self.messages.clone()))
                    .children(outbox_strip);
                // In a public channel the user is not in, Slack's Join card takes the
                // composer's place.
                match conversation.filter(|c| joinable(c)) {
                    Some(joinable) => column.child(self.join_card(joinable, cx)),
                    None => column
                        .children(self.dnd_banner(conversation, cx))
                        .children(self.scheduled_banner(cx))
                        .child(self.composer.clone()),
                }
            }
            ConversationTab::Canvas => {
                let view = self.render_canvas_tab(conversation, cx);
                column.child(div().flex_1().min_h_0().child(view))
            }
            ConversationTab::List => {
                let view = self.render_list_tab(cx);
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
                    let add_fn: tab_views::BookmarkAddFn = Rc::new(move |window, cx| {
                        let this_cb = this_add.clone();
                        let folder_cb = folder_for_add.clone();
                        let ch_cb = ch_for_add.clone();
                        bookmarks::open_add_dialog(window, cx, move |title, url, cx| {
                            let _ = this_cb.update(cx, |app, _| {
                                app.bridge.send(MessagingCommand::AddBookmarkInFolder {
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
                    let rm_fn: tab_views::BookmarkRemoveFn = Rc::new(move |bm_id, cx| {
                        let _ = this_rm.update(cx, |app, _| {
                            app.bridge.send(MessagingCommand::RemoveBookmark {
                                channel: ch_for_rm.clone(),
                                bookmark_id: bm_id,
                            });
                        });
                    });
                    (Some(add_fn), Some(rm_fn))
                } else {
                    (None, None)
                };
                let view = tab_views::folder_view(bookmark_id, bookmarks, on_add, on_remove, cx);
                column.child(div().flex_1().min_h_0().child(view))
            }
        }
    }

    fn render_main_column(
        &self,
        title: &str,
        topic: &str,
        conversation: Option<&Conversation>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        match self.page {
            MainPage::CatchUp => v_flex().size_full().child(self.catchup.clone()),
            MainPage::Search => v_flex().size_full().child(self.results.clone()),
            MainPage::Drafts => v_flex().size_full().child(self.drafts.clone()),
            MainPage::Later => v_flex().size_full().child(self.later.clone()),
            MainPage::Threads => v_flex().size_full().child(self.threads.clone()),
            MainPage::Unreads => v_flex().size_full().child(self.unreads.clone()),
            MainPage::Huddles => v_flex().size_full().child(self.huddles.clone()),
            MainPage::Directory => v_flex().size_full().child(self.directory.clone()),
            MainPage::Tools => v_flex().size_full().child(self.tools_page.clone()),
            MainPage::Compose => v_flex().size_full().child(self.compose.clone()),
            MainPage::Activity => v_flex().size_full().child(self.activity.clone()),
            MainPage::Files => v_flex().size_full().child(self.files_page.clone()),
            MainPage::Channel => {
                if self.sidebar.read(cx).active_nav() == NavItem::Files {
                    v_flex().size_full().child(self.files_page.clone())
                } else {
                    self.render_channel_column(title, topic, conversation, window, cx)
                }
            }
        }
    }

    fn render_side_panel(
        &self,
        title: &str,
        dark: bool,
        outline: Hsla,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let wrap = |pane: AnyElement| {
            div()
                .size_full()
                .when(dark, |pane| pane.border_l_1().border_color(outline))
                .child(pane)
                .into_any_element()
        };
        match self.right_pane {
            RightPane::None => None,
            RightPane::Profile { .. } => Some(wrap(self.profile.clone().into_any_element())),
            RightPane::Thread => Some(wrap(self.thread.clone().into_any_element())),
            RightPane::Pins => {
                let this = cx.weak_entity();
                let close = move |_: &mut Window, cx: &mut gpui::App| {
                    this.update(cx, |app, cx| {
                        app.set_right_pane(RightPane::None, cx);
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
                let panel = pins::pins_panel(&self.fixtures, pinned, title, close, cx);
                Some(wrap(panel.into_any_element()))
            }
        }
    }

    fn bind_root_actions(&self, root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        root.on_action(
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
        .on_action(cx.listener(|this, _: &OpenCatchUp, window, cx| this.open_catchup(window, cx)))
        .on_action(
            cx.listener(|this, _: &OpenChannelBrowser, window, cx| this.open_browser(window, cx)),
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
                cx.listener(|this, _: &NavigateForward, window, cx| this.go_forward(window, cx)),
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
        .on_action(cx.listener(|this, _: &OpenUnreads, window, cx| this.open_unreads(window, cx)))
        .on_action(cx.listener(|this, _: &OpenLater, _, cx| this.open_later(cx)))
        .on_action(cx.listener(|this, _: &OpenFiles, _, cx| this.open_files(cx)))
        .on_action(cx.listener(|this, _: &OpenPeople, _, cx| this.open_directory(cx)))
        .on_action(cx.listener(|this, _: &OpenOwnProfile, _, cx| {
            this.open_profile(this.fixtures.self_user.clone(), cx)
        }))
        .on_action(cx.listener(|this, open: &OpenRecent, _, cx| this.select(open.0.clone(), cx)))
        .on_action(cx.listener(|this, sign_out: &SignOut, _, cx| this.sign_out(sign_out.0, cx)))
        .on_action(cx.listener(|this, _: &PreviousConversation, _, cx| {
            this.step_conversation(false, false, cx)
        }))
        .on_action(
            cx.listener(|this, _: &NextConversation, _, cx| {
                this.step_conversation(true, false, cx)
            }),
        )
        .on_action(
            cx.listener(|this, _: &PreviousUnread, _, cx| this.step_conversation(false, true, cx)),
        )
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
            cx.listener(|this, _: &FocusRemindMe, window, cx| this.open_remind_menu(window, cx)),
        )
        .on_action(cx.listener(|this, _: &FocusReact, window, cx| {
            this.focus_message(
                MessageAction::PickReaction(window.mouse_position()),
                window,
                cx,
            )
        }))
        .on_action(
            cx.listener(|this, _: &MarkAllRead, window, cx| this.request_mark_all_read(window, cx)),
        )
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
            if this.image_viewer.is_some() {
                return;
            }
            if this.page == MainPage::Activity {
                this.activity.update(cx, |a, cx| a.step(false, cx));
            } else if this.showing_transcript() {
                this.messages.update(cx, |m, cx| m.step_focus(false, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &BrowseDown, _, cx| {
            if this.image_viewer.is_some() {
                return;
            }
            if this.page == MainPage::Activity {
                this.activity.update(cx, |a, cx| a.step(true, cx));
            } else if this.showing_transcript() {
                this.messages.update(cx, |m, cx| m.step_focus(true, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &BrowseOpen, window, cx| {
            if this.image_viewer.is_some() {
                return;
            }
            if this.page == MainPage::Activity {
                this.activity.update(cx, |a, cx| a.open_selected(cx));
            } else if this.showing_transcript() {
                this.focus_message(MessageAction::ReplyInThread, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &ExitThread, window, cx| {
            if this.image_viewer.is_some() {
                return;
            }
            if this.right_pane.is_thread() {
                this.close_thread(cx);
                this.focus.focus(window, cx);
                cx.notify();
            }
        }))
        .on_action(cx.listener(|this, _: &JumpToOldest, _, cx| {
            if this.image_viewer.is_some() {
                return;
            }
            if this.showing_transcript() {
                this.messages
                    .update(cx, |m, cx| m.jump(crate::messages::Jump::Beginning, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &JumpToNewest, _, cx| {
            if this.image_viewer.is_some() {
                return;
            }
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
        .on_action(cx.listener(|this, _: &CrossWorkspaceSummary, window, cx| {
            let _ = this.announce_cross_workspace_summary(window, cx);
        }))
        .on_action(cx.listener(|this, _: &ReplayFocusedMessage, window, cx| {
            let _ = this.replay_focused_message(window, cx);
        }))
    }

    fn render_overlays(
        &self,
        root: Stateful<Div>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.when(self.user_menu_open, |root| {
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
                        .on_click(cx.listener(|this, _, window, cx| this.close_picker(window, cx))),
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
        .when_some(
            crate::media::active_pip_video(self.image_viewer.is_some(), cx),
            |root, pip| {
                let chrome = crate::theme::viewer_chrome();
                let fg = crate::theme::media_overlay_foreground();
                let bottom_offset = if self.huddle_dock.is_some() {
                    px(84.)
                } else {
                    px(18.)
                };
                let fid = pip.meta.file_id.clone();
                let fid_play = fid.clone();
                let fid_mute = fid.clone();
                let fid_scrub = fid.clone();
                let fid_expand = fid.clone();
                let expand_meta = pip.meta.clone();
                let expand_meta_stage = pip.meta.clone();
                let fid_expand_stage = fid.clone();
                let jump_target = pip.meta.channel.clone().zip(pip.meta.message_ts.clone());
                let jump_target_btn = jump_target.clone();
                let poster = pip
                    .meta
                    .poster_url
                    .clone()
                    .map(|u| gpui::ImageSource::from(SharedString::from(u)));
                let dur = pip.snapshot.duration_secs;
                let pos = pip.snapshot.position_secs;
                let progress = crate::media::progress_fraction(pos, dur);
                let time_label = if dur > 0.0 {
                    format!(
                        "{} / {}",
                        crate::media::format_video_time(pos),
                        crate::media::format_video_time(dur)
                    )
                } else {
                    crate::media::format_video_time(pos)
                };
                let playing = pip.snapshot.playing;
                let muted = pip.snapshot.muted;
                let loading = pip.snapshot.loading;
                let (pip_w, pip_stage_h) =
                    crate::media::pip_stage_dimensions(pip.snapshot.width, pip.snapshot.height);
                let title = SharedString::from(pip.meta.title.clone());

                root.child(
                    v_flex()
                        .id("video-pip-dock")
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_scroll(|_, _, cx| cx.stop_propagation())
                        .absolute()
                        .right(px(18.))
                        .bottom(bottom_offset)
                        .w(px(pip_w))
                        .rounded_lg()
                        .overflow_hidden()
                        .bg(chrome.panel)
                        .border_1()
                        .border_color(chrome.border)
                        .shadow(vec![shadow(8., 0.35, 24.)])
                        // Header bar: title (click jumps to message when known) + Expand + Close.
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_1()
                                .px_2()
                                .py_1()
                                .bg(chrome.glass)
                                .border_b_1()
                                .border_color(chrome.border)
                                .child(
                                    h_flex()
                                        .id("video-pip-title")
                                        .flex_1()
                                        .min_w_0()
                                        .items_center()
                                        .gap_1()
                                        .when(jump_target.is_some(), |el| {
                                            el.cursor_pointer()
                                                .hover(move |h| h.text_color(chrome.text))
                                        })
                                        .child(
                                            Icon::new(IconName::Video)
                                                .size(px(12.))
                                                .text_color(chrome.text_muted),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .truncate()
                                                .text_xs()
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(chrome.text)
                                                .child(title),
                                        )
                                        .when_some(jump_target, |el, (ch, ts)| {
                                            el.on_click(cx.listener(move |this, _, _, cx| {
                                                this.close_full_column_views(cx);
                                                this.select(ch.clone(), cx);
                                                this.messages
                                                    .update(cx, |list, cx| list.reveal(&ts, cx));
                                            }))
                                        }),
                                )
                                .child(
                                    h_flex()
                                        .flex_none()
                                        .items_center()
                                        .gap_0p5()
                                        .when_some(jump_target_btn, |row, (ch, ts)| {
                                            row.child(
                                                div()
                                                    .id("video-pip-jump")
                                                    .size(px(22.))
                                                    .rounded_sm()
                                                    .flex()
                                                    .items_center()
                                                    .justify_center()
                                                    .cursor_pointer()
                                                    .hover(move |b| b.bg(chrome.button_hover))
                                                    .child(
                                                        Icon::new(IconName::ArrowUpRight)
                                                            .size(px(12.))
                                                            .text_color(chrome.text_muted),
                                                    )
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.close_full_column_views(cx);
                                                            this.select(ch.clone(), cx);
                                                            this.messages.update(cx, |list, cx| {
                                                                list.reveal(&ts, cx)
                                                            });
                                                        },
                                                    )),
                                            )
                                        })
                                        .child(
                                            div()
                                                .id("video-pip-expand")
                                                .size(px(22.))
                                                .rounded_sm()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .hover(move |b| b.bg(chrome.button_hover))
                                                .child(
                                                    Icon::new(IconName::Maximize2)
                                                        .size(px(12.))
                                                        .text_color(chrome.text),
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        crate::media::stop_inline_video_for_expand(
                                                            &fid_expand,
                                                            window,
                                                            cx,
                                                        );
                                                        let viewer_img =
                                                            ViewerImage::from_inline_video_meta(
                                                                &this.fixtures,
                                                                &expand_meta,
                                                            );
                                                        let ch = expand_meta
                                                            .channel
                                                            .clone()
                                                            .unwrap_or_else(|| {
                                                                this.channel.clone()
                                                            });
                                                        this.open_image_viewer(
                                                            ch,
                                                            Rc::new(vec![viewer_img]),
                                                            0,
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                        .child(
                                            div()
                                                .id("video-pip-close")
                                                .size(px(22.))
                                                .rounded_sm()
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .cursor_pointer()
                                                .hover(move |b| b.bg(chrome.button_hover))
                                                .child(
                                                    Icon::new(IconName::X)
                                                        .size(px(12.))
                                                        .text_color(chrome.text_muted),
                                                )
                                                .on_click(|_, window, cx| {
                                                    crate::media::stop_active_inline_video(
                                                        window, cx,
                                                    );
                                                }),
                                        ),
                                ),
                        )
                        // Video surface stage: clicking expands cleanly into ImageViewer.
                        .child(
                            div()
                                .id("video-pip-stage")
                                .relative()
                                .w_full()
                                .h(px(pip_stage_h))
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    crate::media::stop_inline_video_for_expand(
                                        &fid_expand_stage,
                                        window,
                                        cx,
                                    );
                                    let viewer_img = ViewerImage::from_inline_video_meta(
                                        &this.fixtures,
                                        &expand_meta_stage,
                                    );
                                    let ch = expand_meta_stage
                                        .channel
                                        .clone()
                                        .unwrap_or_else(|| this.channel.clone());
                                    this.open_image_viewer(
                                        ch,
                                        Rc::new(vec![viewer_img]),
                                        0,
                                        window,
                                        cx,
                                    );
                                }))
                                .child(crate::media::render_video_surface(
                                    pip.snapshot.surface,
                                    poster,
                                    px(0.),
                                ))
                                .when(loading, |stage| {
                                    stage.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                div()
                                                    .px_2p5()
                                                    .py_1()
                                                    .rounded_full()
                                                    .bg(chrome.glass)
                                                    .border_1()
                                                    .border_color(chrome.border)
                                                    .text_xs()
                                                    .text_color(fg)
                                                    .child("Loading\u{2026}"),
                                            ),
                                    )
                                }),
                        )
                        // Compact bottom transport bar: play/pause, scrub bar, time, mute.
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .px_2()
                                .py_1()
                                .bg(chrome.glass)
                                .border_t_1()
                                .border_color(chrome.border)
                                .child(
                                    div()
                                        .id("video-pip-play")
                                        .size(px(24.))
                                        .flex_none()
                                        .rounded_sm()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(move |b| b.bg(chrome.button_hover))
                                        .child(
                                            Icon::new(if playing {
                                                IconName::Pause
                                            } else {
                                                IconName::Play
                                            })
                                            .size(px(13.))
                                            .text_color(fg),
                                        )
                                        .on_click(move |_, window, cx| {
                                            crate::media::toggle_inline_video_pause(
                                                &fid_play, window, cx,
                                            );
                                        }),
                                )
                                .child(crate::media::video_scrub_bar(
                                    SharedString::from("video-pip-scrub"),
                                    progress,
                                    dur,
                                    chrome.border,
                                    fg,
                                    move |secs, window, cx| {
                                        crate::media::seek_inline_video(
                                            &fid_scrub, secs, window, cx,
                                        );
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(chrome.text_muted)
                                        .child(time_label),
                                )
                                .child(
                                    div()
                                        .id("video-pip-mute")
                                        .size(px(24.))
                                        .flex_none()
                                        .rounded_sm()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(move |b| b.bg(chrome.button_hover))
                                        .child(
                                            Icon::new(if muted {
                                                IconName::VolumeX
                                            } else {
                                                IconName::Volume2
                                            })
                                            .size(px(13.))
                                            .text_color(fg),
                                        )
                                        .on_click(move |_, window, cx| {
                                            crate::media::toggle_inline_video_mute(
                                                &fid_mute, window, cx,
                                            );
                                        }),
                                ),
                        ),
                )
            },
        )
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

        let conversation = self.fixtures.conversation(&self.channel).cloned();
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
        let main_column =
            self.render_main_column(&title, &topic, conversation.as_ref(), window, cx);

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
        let side_panel_el = self.render_side_panel(&title, dark, outline, cx);
        let side_panel = side_panel_el.is_some();
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
                    .on_drop::<gpui::ExternalPaths>(cx.listener(
                        |this, paths: &gpui::ExternalPaths, _, cx| {
                            if !paths.paths().is_empty() {
                                this.composer.update(cx, |composer, cx| {
                                    composer.attach_paths(paths.paths().to_vec(), cx);
                                });
                            }
                        },
                    ))
                    .drag_over::<gpui::ExternalPaths>(|style, _, _, cx| {
                        style.border_2().border_color(cx.theme().link)
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
        if let Some(pane) = side_panel_el {
            panels = panels.child(
                resizable_panel()
                    .size(FLEXPANE_WIDTH)
                    .size_range(px(340.)..px(720.))
                    .fixed()
                    .child(pane),
            );
        }

        let (switcher, rail) = self.sidebar.update(cx, |sidebar, cx| {
            (sidebar.workspace_switcher(cx), sidebar.tab_rail(cx))
        });

        let root = v_flex()
            .id("root")
            .key_context("App")
            .track_focus(&self.focus);
        let root = self
            .bind_root_actions(root, cx)
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
                    |_, _, cx| crate::media::inline_video_begin_paint(cx),
                    move |_, paint_epoch, window, cx| {
                        crate::media::inline_video_finish_paint(paint_epoch, window, cx);
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
            });
        self.render_overlays(root, window, cx)
    }
}
