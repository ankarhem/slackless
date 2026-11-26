//! Preferences > Advanced section view and network diagnostics.

use super::*;

impl Preferences {
    pub(super) fn advanced(&mut self, cx: &mut Context<Self>) -> Div {
        let unsend = self.checkbox(
            "adv-unsend",
            "Ask before deleting a message",
            |s| &mut s.confirm_unsend,
            cx,
        );
        let external = self.checkbox(
            "adv-external",
            "Warn me about potentially malicious links",
            |s| &mut s.warn_external,
            cx,
        );
        let login = self.checkbox(
            "adv-login",
            "Launch slackless at login",
            |s| &mut s.launch_at_login,
            cx,
        );
        let menu_bar = self.checkbox(
            "adv-menu-bar",
            "Show slackless icon in the menu bar",
            |s| &mut s.show_menu_bar_icon,
            cx,
        );
        let close_tray = self.checkbox(
            "adv-close-tray",
            "Keep slackless running in the menu bar when closing the window",
            |s| &mut s.close_to_tray,
            cx,
        );
        let hardware = self.checkbox(
            "adv-hw",
            "Use hardware acceleration (Metal GPU — active on macOS)",
            |s| &mut s.hardware_acceleration,
            cx,
        );
        let debug = self.checkbox(
            "adv-debug",
            "Enable debug logging",
            |s| &mut s.debug_logging,
            cx,
        );
        let links = self.select_w("adv-links", 220., &OPEN_LINKS, |s| &mut s.open_links, cx);
        let remote_search = self.checkbox(
            "adv-remote-search",
            "Also search Slack remotely by default (slower; local FTS5 stays instant otherwise)",
            |s| &mut s.remote_search_auto,
            cx,
        );
        let stealth_mode = self.checkbox(
            "adv-stealth-mode",
            "Stealth mode \u{2014} hide typing indicators, read receipts, and active presence",
            |s| &mut s.stealth_mode,
            cx,
        );
        let show_deleted = self.checkbox(
            "adv-show-deleted",
            "Expand deleted messages by default (otherwise show a minimal toggle row)",
            |s| &mut s.show_deleted_messages,
            cx,
        );
        let strip_tracking = self.checkbox(
            "adv-strip-tracking",
            "Strip tracking parameters (utm_*, fbclid, gclid, si, etc.) from links",
            |s| &mut s.strip_tracking_params,
            cx,
        );
        let warn_secrets = self.checkbox(
            "adv-warn-secrets",
            "Warn before sending API keys, tokens, or private keys",
            |s| &mut s.warn_before_sending_secrets,
            cx,
        );
        let search_in_ch = self.checkbox(
            "adv-search-in-channel",
            "\u{2318}F searches the current conversation instead of all of Slack",
            |s| &mut s.search_in_channel,
            cx,
        );
        let m = self.metrics();
        let excluded_input = text_input_box_for(&self.excluded_channels, true, false, m, cx)
            .w_full()
            .max_w(px(420.));
        let open_storage_dialog = self
            .btn(
                Button::new("adv-open-storage")
                    .outline()
                    .label("Archive & backfill\u{2026}"),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::OpenStorageDialog)));
        let backfill_btn = {
            let status = self
                .backfill_progress
                .as_ref()
                .map(|p| p.status)
                .unwrap_or_default();
            match status {
                slackless_core::BackfillStatus::Running => self
                    .btn(
                        Button::new("adv-backfill-pause")
                            .outline()
                            .label("Pause backfill"),
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::PauseBackfill))),
                slackless_core::BackfillStatus::Paused => self
                    .btn(
                        Button::new("adv-backfill-resume")
                            .primary()
                            .label("Resume backfill"),
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ResumeBackfill))),
                _ => self
                    .btn(
                        Button::new("adv-backfill-start")
                            .outline()
                            .label("Backfill workspace"),
                    )
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(PreferencesEvent::StartWorkspaceBackfill);
                    })),
            }
        };
        let backfill_note = self.backfill_progress.as_ref().map(|p| {
            format!(
                "Backfill: {} · {}/{} channels complete · {} messages ({} thread replies)",
                p.status.label(),
                p.completed_channels,
                p.total_channels,
                p.messages_stored,
                p.replies_stored,
            )
        });
        let compact_db = self
            .btn(
                Button::new("adv-compact-db")
                    .outline()
                    .label("Compact database"),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::CompactStorage)));
        let prune_media = self
            .btn(
                Button::new("adv-prune-media")
                    .outline()
                    .label("Prune media cache"),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::PruneMediaCache)));
        let reset_cache = self
            .btn(
                Button::new("adv-reset-cache")
                    .danger()
                    .label("Clear local cache & reload"),
            )
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ResetCache)));
        let storage_summary = {
            let theme = cx.theme();
            let fg = theme.foreground;
            let muted = theme.muted_foreground;
            let border = theme.border;
            let card_bg = theme.secondary.opacity(0.35);
            let body =
                match self.storage_stats.clone() {
                    Some(st) => {
                        let mut metrics = h_flex()
                            .items_center()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child("SQLite"),
                            )
                            .child(self.mono_chip(format_bytes(st.db_bytes), cx))
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child("Media cache"),
                            )
                            .child(self.mono_chip(format_bytes(st.media_cache_bytes), cx));
                        if st.offline_queued > 0 {
                            metrics = metrics.child(
                                self.mono_chip(format!("{} queued offline", st.offline_queued), cx),
                            );
                        }
                        v_flex().gap_1p5().child(metrics).child(
                            div().text_xs().text_color(muted).child(format!(
                                "{} messages ({} revisions) · {} conversations · {} users",
                                st.messages, st.revisions, st.conversations, st.users,
                            )),
                        )
                    }
                    None => v_flex().child(div().text_xs().text_color(muted).child(
                        "Local SQLite metrics are shown when connected to a live workspace.",
                    )),
                };
            v_flex()
                .gap_3()
                .px_3p5()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(body)
                .when_some(backfill_note, |col, note| {
                    col.child(div().text_xs().text_color(muted).child(note))
                })
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .flex_wrap()
                                .child(open_storage_dialog)
                                .child(backfill_btn)
                                .child(compact_db)
                                .child(prune_media),
                        )
                        .child(reset_cache),
                )
        };
        let dl_label = if self.settings.download_dir.is_empty() {
            "~/Downloads (default)".to_owned()
        } else {
            self.settings.download_dir.clone()
        };
        let has_custom_dl = !self.settings.download_dir.is_empty();
        let dl_muted = cx.theme().muted_foreground;
        let dl_fg = cx.theme().foreground;
        let dl_border = cx.theme().input;
        let dl_bg = cx.theme().secondary.opacity(0.35);
        let dl_mono = cx.theme().mono_font_family.clone();
        let download_row = h_flex()
            .items_center()
            .gap_2()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h(m.height)
                    .items_center()
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(dl_border)
                    .bg(dl_bg)
                    .font_family(dl_mono)
                    .text_xs()
                    .text_color(if has_custom_dl { dl_fg } else { dl_muted })
                    .child(div().flex_1().min_w_0().truncate().child(dl_label)),
            )
            .child(
                self.btn(
                    Button::new("adv-dl-choose")
                        .outline()
                        .label("Choose folder\u{2026}"),
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    let rx = cx.prompt_for_paths(PathPromptOptions {
                        files: false,
                        directories: true,
                        multiple: false,
                        prompt: Some("Choose download folder".into()),
                    });
                    cx.spawn(async move |this, cx| {
                        if let Ok(Ok(Some(paths))) = rx.await
                            && let Some(path) = paths.into_iter().next()
                        {
                            let _ = this.update(cx, |prefs, cx| {
                                prefs.settings.download_dir = path.to_string_lossy().into_owned();
                                prefs.changed(cx);
                            });
                        }
                    })
                    .detach();
                })),
            )
            .when(has_custom_dl, |row| {
                row.child(
                    self.btn(Button::new("adv-dl-reset").ghost().label("Reset"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings.download_dir.clear();
                            this.changed(cx);
                        })),
                )
            });
        let ask_dl = self.checkbox(
            "adv-dl-ask",
            "Ask where to save each file before downloading",
            |s| &mut s.ask_download_location,
            cx,
        );
        let protocol_input =
            text_input_box_for(&self.protocol_allowlist_input, false, false, m, cx)
                .w_full()
                .max_w(px(420.));
        let semantic_search_card = self.semantic_search_card(cx);
        let ai_summaries_group = self.ai_summaries_group(cx);
        let network_group = self.network_diagnostics_group(cx);
        v_flex()
            .child(self.group(
                "Privacy & archive",
                Some("Local-only controls that go beyond official Slack: stay invisible while reading, inspect locally preserved deleted messages, scrub tracking parameters, and block accidental secret leaks."),
                v_flex()
                    .gap_2()
                    .child(stealth_mode)
                    .child(show_deleted)
                    .child(strip_tracking)
                    .child(warn_secrets),
                cx,
            ))
            .child(ai_summaries_group)
            .child(self.group(
                "Search",
                Some("Local search is instant and works offline. Enable remote search when you want older history, unjoined channels, and server-indexed files automatically."),
                v_flex()
                    .gap_2()
                    .child(remote_search)
                    .child(search_in_ch)
                    .child(self.labeled("Exclude these channels from local search", excluded_input))
                    .child(semantic_search_card),
                cx,
            ))
            .child(self.group(
                "Download location",
                Some("Where files downloaded from Slack are saved."),
                v_flex().gap_2().child(download_row).child(ask_dl),
                cx,
            ))
            .child(network_group)
            .child(self.group(
                "Other options",
                None,
                v_flex()
                    .gap_2()
                    .child(unsend)
                    .child(external)
                    .child(self.labeled("Allowed URL protocols (comma-separated)", protocol_input)),
                cx,
            ))
            .child(self.group("Open links in", None, links, cx))
            .child(self.group(
                "App",
                None,
                v_flex()
                    .gap_2()
                    .child(login)
                    .child(menu_bar)
                    .child(close_tray)
                    .child(hardware)
                    .child(debug),
                cx,
            ))
            .child(self.software_updates_group(cx))
            .child(self.cli_install_group(cx))
            .child(self.group(
                "Local storage & maintenance",
                Some("Durable local-first SQLite history & FTS5 search index for the active workspace."),
                storage_summary,
                cx,
            ))
    }

    fn semantic_search_card(&mut self, cx: &mut Context<Self>) -> Div {
        let model = slackless_core::embeddings::EmbeddingModel::default();
        let (setup_ready, disk_bytes, download_hint, model_path_label, idx_stats) =
            match &self.semantic_snapshot {
                Some(snap) => (
                    snap.setup.model_ready,
                    snap.setup.disk_bytes,
                    snap.setup.download_size_hint.clone(),
                    snap.setup.model_path.display().to_string(),
                    snap.stats.clone(),
                ),
                None => (
                    false,
                    0,
                    model.download_size_hint().to_owned(),
                    "app data models directory".to_owned(),
                    slackless_core::store::SemanticIndexStats::default(),
                ),
            };
        let downloading = self.semantic_downloading || crate::search::semantic_download_in_flight();
        let indexing = self.semantic_indexing || crate::search::semantic_index_in_flight();
        let (link_color, success_color, warn_color) = {
            let theme = cx.theme();
            (theme.link, theme.success, theme.warning)
        };
        let dot_color = if downloading || indexing {
            link_color
        } else if setup_ready {
            success_color
        } else {
            warn_color
        };
        let (title, badge, detail) = if downloading {
            (
                format!("Downloading {}…", model.slug()),
                Some("Downloading"),
                self.semantic_status_note
                    .clone()
                    .unwrap_or_else(|| "Fetching quantized ONNX weights (~23 MB)…".to_owned()),
            )
        } else if indexing {
            (
                "Indexing local semantic vectors…".to_owned(),
                Some("Indexing"),
                self.semantic_status_note.clone().unwrap_or_else(|| {
                    "Embedding archived messages, snippets, canvases & transcripts…".to_owned()
                }),
            )
        } else if setup_ready {
            (
                format!("Semantic search · {}", model.slug()),
                Some("Ready"),
                self.semantic_status_note.clone().unwrap_or_else(|| {
                    format!(
                        "{} model · {} docs indexed ({}) across {} workspace(s).",
                        format_bytes(disk_bytes),
                        idx_stats.total_docs,
                        format_bytes(idx_stats.db_bytes),
                        idx_stats.workspaces,
                    )
                }),
            )
        } else {
            (
                format!("Local semantic model ({download_hint})"),
                Some("Opt-in"),
                self.semantic_status_note.clone().unwrap_or_else(|| {
                    format!(
                        "0 MB on disk until downloaded. Powers local Hybrid & Semantic search in {model_path_label}."
                    )
                }),
            )
        };

        let actions = h_flex()
            .items_center()
            .gap_1p5()
            .when(!setup_ready, |row| {
                row.child(
                    self.btn(
                        Button::new("prefs-sem-download")
                            .primary()
                            .icon(IconName::Download)
                            .disabled(downloading)
                            .label(if downloading {
                                "Downloading…".to_owned()
                            } else {
                                format!("Download ({download_hint})")
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_semantic_model_download(cx);
                    })),
                )
            })
            .when(setup_ready && !downloading, |row| {
                row.child(
                    self.btn(
                        Button::new("prefs-sem-index")
                            .outline()
                            .icon(IconName::RefreshCw)
                            .disabled(indexing)
                            .label(if indexing {
                                "Indexing…"
                            } else {
                                "Index archive"
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_semantic_archive_index(cx);
                    })),
                )
                .child(
                    self.btn(
                        Button::new("prefs-sem-delete")
                            .ghost()
                            .icon(IconName::Trash)
                            .disabled(indexing)
                            .label("Remove model"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        crate::search::clear_cached_onnx_embedder();
                        match slackless_core::embeddings::delete_embedding_model(
                            slackless_core::embeddings::EmbeddingModel::default(),
                        ) {
                            Ok(freed) => {
                                this.semantic_status_note = Some(format!(
                                    "Removed local embedding model (reclaimed {}).",
                                    format_bytes(freed)
                                ));
                            }
                            Err(err) => {
                                this.semantic_status_note =
                                    Some(format!("Could not remove model: {err}"));
                            }
                        }
                        this.refresh_semantic_snapshot(cx);
                        cx.notify();
                    })),
                )
            });

        self.status_card(
            super::StatusCard {
                dot_color,
                title: title.into(),
                chip: badge.map(SharedString::from),
                detail: detail.into(),
                detail_is_err: false,
                actions,
            },
            cx,
        )
    }

    fn start_semantic_model_download(&mut self, cx: &mut Context<Self>) {
        if self.semantic_downloading {
            return;
        }
        let Some(task_guard) = crate::search::try_begin_semantic_download() else {
            self.semantic_status_note = Some("Model download is already in progress…".to_owned());
            cx.notify();
            return;
        };
        let model = slackless_core::embeddings::EmbeddingModel::default();
        self.semantic_downloading = true;
        self.semantic_status_note = Some("Connecting to Hugging Face…".to_owned());
        cx.notify();

        let progress_note: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let done_result: std::sync::Arc<
            std::sync::Mutex<Option<Result<std::path::PathBuf, String>>>,
        > = std::sync::Arc::new(std::sync::Mutex::new(None));
        let prog_writer = std::sync::Arc::clone(&progress_note);
        let done_writer = std::sync::Arc::clone(&done_result);

        if let Err(err) = std::thread::Builder::new()
            .name("slackless-prefs-embed-dl".to_owned())
            .spawn(move || {
                let _guard = task_guard;
                let res = slackless_core::embeddings::download_embedding_model(model, move |p| {
                    let pct = p
                        .total_bytes
                        .filter(|&t| t > 0)
                        .map(|t| format!(" · {}%", (p.downloaded_bytes * 100 / t).min(100)))
                        .unwrap_or_default();
                    let line = format!(
                        "[{}/{}] {} ({}{})",
                        p.file_index,
                        p.file_count,
                        p.file_name,
                        format_bytes(p.downloaded_bytes),
                        pct
                    );
                    if let Ok(mut g) = prog_writer.lock() {
                        *g = Some(line);
                    }
                });
                if let Ok(mut g) = done_writer.lock() {
                    *g = Some(res);
                }
            })
        {
            self.semantic_downloading = false;
            self.semantic_status_note = Some(format!("Failed to start download thread: {err}"));
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(180))
                    .await;
                let latest = progress_note.lock().ok().and_then(|g| g.clone());
                let finished = done_result.lock().ok().and_then(|mut g| g.take());
                let keep = this
                    .update(cx, |prefs, cx| {
                        if let Some(line) = latest {
                            prefs.semantic_status_note = Some(line);
                        }
                        if let Some(res) = finished {
                            prefs.semantic_downloading = false;
                            match res {
                                Ok(_) => {
                                    prefs.semantic_status_note = Some(
                                        "Model downloaded. Click 'Index archive' to build semantic_index.sqlite.".to_owned(),
                                    );
                                }
                                Err(err) => {
                                    prefs.semantic_status_note =
                                        Some(format!("Download failed: {err}"));
                                }
                            }
                            prefs.refresh_semantic_snapshot(cx);
                            cx.notify();
                            false
                        } else {
                            cx.notify();
                            true
                        }
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_semantic_archive_index(&mut self, cx: &mut Context<Self>) {
        if self.semantic_indexing {
            return;
        }
        let Some(task_guard) = crate::search::try_begin_semantic_index() else {
            self.semantic_status_note = Some("Semantic indexing is already running…".to_owned());
            cx.notify();
            return;
        };
        self.semantic_indexing = true;
        self.semantic_status_note =
            Some("Embedding archived messages across workspaces…".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome: Result<(String, super::SemanticStorageSnapshot), String> = cx
                .background_executor()
                .spawn(async move {
                    let _guard = task_guard;
                    let embedder = crate::search::load_or_get_onnx_embedder()?;
                    let archives = slackless_core::search::discover_local_archives();
                    let mut idx = slackless_core::semantic_index::open_default_semantic_index()?;
                    let report = slackless_core::semantic_index::index_disk_archives(
                        &archives,
                        &mut idx,
                        embedder.as_ref(),
                        |_| {},
                    )?;
                    let msg = format!(
                        "Indexed {} workspace(s): {} embedded, {} unchanged, {} pruned.",
                        report.workspaces_indexed.len(),
                        report.embedded,
                        report.skipped_unchanged,
                        report.pruned
                    );
                    Ok((msg, super::SemanticStorageSnapshot::collect()))
                })
                .await;
            let _ = this.update(cx, |prefs, cx| {
                prefs.semantic_indexing = false;
                match outcome {
                    Ok((msg, snap)) => {
                        prefs.semantic_status_note = Some(msg);
                        prefs.set_semantic_snapshot(snap, cx);
                    }
                    Err(err) => {
                        prefs.semantic_status_note = Some(format!("Indexing failed: {err}"));
                        prefs.refresh_semantic_snapshot(cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn network_diagnostics_group(&mut self, cx: &mut Context<Self>) -> Div {
        let m = self.metrics();
        let theme = cx.theme();
        let dot_color = if self.settings.net_log_enabled {
            theme.success
        } else {
            theme.muted_foreground
        };
        let proxy_field = text_input_box_for(&self.proxy_input, false, true, m, cx)
            .w_full()
            .max_w(px(420.));
        let net_log_toggle = self.checkbox(
            "adv-net-log",
            "Capture HTTP net-log diagnostics (credentials redacted; last 500 requests)",
            |s| &mut s.net_log_enabled,
            cx,
        );
        let net_log_count = slackless_core::api::net_log_snapshot().len();
        let net_log_card = self.status_card(
            super::StatusCard {
                dot_color,
                title: if self.settings.net_log_enabled {
                    "Capturing requests"
                } else {
                    "Capture paused"
                }
                .into(),
                chip: Some(SharedString::from(format!(
                    "{net_log_count} {}",
                    if net_log_count == 1 {
                        "request"
                    } else {
                        "requests"
                    }
                ))),
                detail: "Credentials and tokens are redacted before recording.".into(),
                detail_is_err: false,
                actions: h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        self.btn(
                            Button::new("adv-net-log-clear")
                                .ghost()
                                .disabled(net_log_count == 0)
                                .label("Clear"),
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            slackless_core::api::clear_net_log();
                            cx.notify();
                        })),
                    )
                    .child(
                        self.btn(
                            Button::new("adv-net-log-export")
                                .outline()
                                .disabled(net_log_count == 0)
                                .icon(IconName::Download)
                                .label("Export"),
                        )
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PreferencesEvent::ExportNetLog)),
                        ),
                    ),
            },
            cx,
        );
        self.group(
            "Network & diagnostics",
            Some("Optional HTTP/HTTPS/SOCKS proxy (leave empty to use system environment variables) and redacted HTTP net-log capture."),
            v_flex()
                .gap_2p5()
                .child(self.labeled("Proxy URL (applies to new connections)", proxy_field))
                .child(net_log_toggle)
                .child(net_log_card),
            cx,
        )
    }

    fn ai_summaries_group(&mut self, cx: &mut Context<Self>) -> Div {
        use slackless_core::summarize::AiProvider;

        let enabled_cb = self.checkbox(
            "adv-ai-enabled",
            "Enable AI summaries (local LLM or BYO OpenAI-compatible API key)",
            |s| &mut s.ai_summary.enabled,
            cx,
        );
        let provider = self.settings.ai_summary.provider;
        let m = self.metrics();
        let theme = cx.theme();
        let (fg, muted, success, danger, primary, border, secondary) = (
            theme.foreground,
            theme.muted_foreground,
            theme.success,
            theme.danger,
            theme.primary,
            theme.border,
            theme.secondary,
        );

        let provider_pills = h_flex().items_center().gap_1p5().flex_wrap().children(
            AiProvider::ALL.into_iter().map(|preset| {
                let active = preset == provider;
                let id = SharedString::from(format!("adv-ai-provider-{}", preset.key()));
                let btn = self.btn(if active {
                    Button::new(id).primary().label(preset.short_label())
                } else {
                    Button::new(id).outline().label(preset.short_label())
                });
                btn.on_click(cx.listener(move |this, _, window, cx| {
                    let prev = this.settings.ai_summary.provider;
                    if prev == preset {
                        return;
                    }
                    this.settings.ai_summary.provider = preset;
                    let cur_url = this.settings.ai_summary.base_url.trim();
                    if cur_url.is_empty() || cur_url == prev.default_base_url() {
                        let next_url = preset.default_base_url().to_owned();
                        this.settings.ai_summary.base_url = next_url.clone();
                        this.ai_base_url_input
                            .update(cx, |st, cx| st.set_value(next_url, window, cx));
                    }
                    let cur_model = this.settings.ai_summary.model.trim();
                    if cur_model.is_empty() || cur_model == prev.default_model() {
                        let next_model = preset.default_model().to_owned();
                        this.settings.ai_summary.model = next_model.clone();
                        this.ai_model_input
                            .update(cx, |st, cx| st.set_value(next_model, window, cx));
                    }
                    this.ai_test_status = None;
                    this.changed(cx);
                }))
            }),
        );

        let url_field = text_input_box_for(&self.ai_base_url_input, false, true, m, cx)
            .w_full()
            .max_w(px(460.));
        let model_field = text_input_box_for(&self.ai_model_input, false, true, m, cx)
            .w_full()
            .max_w(px(320.));

        let has_stored_key = self.ai_api_key_stored == Some(true);
        let key_badge_label = if has_stored_key {
            "Stored in Keychain"
        } else if provider.requires_api_key() {
            "Required for OpenAI"
        } else {
            "Optional (not set)"
        };
        let key_badge_color = if has_stored_key {
            success
        } else if provider.requires_api_key() {
            danger
        } else {
            muted
        };
        let key_input = text_input_box_for(&self.ai_api_key_input, false, true, m, cx)
            .flex_1()
            .min_w_0();
        let save_key_btn = self
            .btn(Button::new("adv-ai-key-save").outline().label("Save key"))
            .on_click(cx.listener(|this, _, window, cx| {
                this.save_ai_api_key(window, cx);
            }));
        let clear_key_btn = self
            .btn(Button::new("adv-ai-key-clear").ghost().label("Clear"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.clear_ai_api_key(cx);
            }));
        let key_row = v_flex()
            .gap_1p5()
            .max_w(px(460.))
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(fg)
                            .child("API key (stored in Keychain)"),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py(px(1.))
                            .rounded_full()
                            .border_1()
                            .border_color(border)
                            .bg(secondary.opacity(0.5))
                            .text_xs()
                            .text_color(key_badge_color)
                            .child(key_badge_label),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1p5()
                    .child(key_input)
                    .child(save_key_btn)
                    .when(has_stored_key, |row| row.child(clear_key_btn)),
            )
            .when_some(self.ai_key_status_note.clone(), |col, note| {
                col.child(div().text_xs().text_color(muted).child(note))
            });

        let (dot_color, title, chip, detail, detail_is_err) = if self.ai_test_running {
            (
                primary,
                "Testing connection\u{2026}".to_owned(),
                Some(SharedString::from(provider.short_label())),
                format!(
                    "Probing {} ({})…",
                    self.settings.ai_summary.effective_base_url(),
                    self.settings.ai_summary.effective_model()
                ),
                false,
            )
        } else if let Some((ok, msg)) = &self.ai_test_status {
            (
                if *ok { success } else { danger },
                if *ok {
                    "Endpoint reachable".to_owned()
                } else {
                    "Connection failed".to_owned()
                },
                Some(SharedString::from(format!(
                    "{} · {}",
                    provider.short_label(),
                    self.settings.ai_summary.effective_model()
                ))),
                msg.clone(),
                !*ok,
            )
        } else {
            let eff_url = self.settings.ai_summary.effective_base_url();
            let eff_model = self.settings.ai_summary.effective_model();
            let status_detail = if eff_url.is_empty() || eff_model.is_empty() {
                "Set an OpenAI-compatible base URL and model name, then click Test connection."
                    .to_owned()
            } else {
                format!("Ready to summarize via {eff_url} using \"{eff_model}\".")
            };
            (
                if self.settings.ai_summary.is_ready_to_run() {
                    success
                } else {
                    muted
                },
                if self.settings.ai_summary.enabled {
                    "AI summaries enabled".to_owned()
                } else {
                    "AI summaries disabled".to_owned()
                },
                Some(SharedString::from(provider.short_label())),
                status_detail,
                false,
            )
        };

        let test_card = self.status_card(
            super::StatusCard {
                dot_color,
                title: title.into(),
                chip,
                detail: detail.into(),
                detail_is_err,
                actions: h_flex().items_center().gap_1p5().child(
                    self.btn(
                        Button::new("adv-ai-test-conn")
                            .outline()
                            .icon(IconName::Sparkles)
                            .disabled(self.ai_test_running)
                            .label(if self.ai_test_running {
                                "Testing\u{2026}"
                            } else {
                                "Test connection"
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.start_ai_connection_test(cx);
                    })),
                ),
            },
            cx,
        );

        self.group(
            "AI Summaries (Local / BYO-key)",
            Some("Summarize threads, channels, Catch-Up briefings, and transcripts with local Ollama / LM Studio or your own OpenAI-compatible endpoint. API keys are kept in the OS Keychain and never logged."),
            v_flex()
                .gap_2p5()
                .child(enabled_cb)
                .child(self.labeled("Provider preset", provider_pills))
                .child(self.labeled("Endpoint base URL (http:// or https://)", url_field))
                .child(self.labeled("Model", model_field))
                .child(key_row)
                .child(test_card),
            cx,
        )
    }

    fn save_ai_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.ai_api_key_input.read(cx).value().trim().to_owned();
        if raw.is_empty() {
            self.ai_key_status_note = Some("Enter an API key first, or click Clear.".to_owned());
            cx.notify();
            return;
        }
        // Clear the input buffer immediately so the secret does not sit in UI memory.
        self.ai_api_key_input
            .update(cx, |st, cx| st.set_value(String::new(), window, cx));
        self.ai_key_status_note = Some("Saving API key to Keychain\u{2026}".to_owned());
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome: Result<(), String> = cx
                .background_executor()
                .spawn(async move {
                    let store =
                        slackless_core::auth::SecretStore::new().map_err(|e| e.to_string())?;
                    store
                        .set(slackless_core::auth::AI_SUMMARY_API_KEY_ACCOUNT, &raw)
                        .map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |prefs, cx| {
                match outcome {
                    Ok(()) => {
                        prefs.ai_api_key_stored = Some(true);
                        prefs.ai_key_status_note =
                            Some("API key saved securely in Keychain.".to_owned());
                        prefs.ai_test_status = None;
                    }
                    Err(err) => {
                        prefs.ai_key_status_note = Some(format!("Could not save API key: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clear_ai_api_key(&mut self, cx: &mut Context<Self>) {
        self.ai_key_status_note = Some("Removing API key from Keychain\u{2026}".to_owned());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let outcome: Result<(), String> = cx
                .background_executor()
                .spawn(async move {
                    let store =
                        slackless_core::auth::SecretStore::new().map_err(|e| e.to_string())?;
                    store
                        .delete(slackless_core::auth::AI_SUMMARY_API_KEY_ACCOUNT)
                        .map_err(|e| e.to_string())
                })
                .await;
            let _ = this.update(cx, |prefs, cx| {
                match outcome {
                    Ok(()) => {
                        prefs.ai_api_key_stored = Some(false);
                        prefs.ai_key_status_note =
                            Some("API key removed from Keychain.".to_owned());
                        prefs.ai_test_status = None;
                    }
                    Err(err) => {
                        prefs.ai_key_status_note = Some(format!("Could not remove API key: {err}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_ai_connection_test(&mut self, cx: &mut Context<Self>) {
        if self.ai_test_running {
            return;
        }
        let ai_settings = self.settings.ai_summary.clone();
        self.ai_test_running = true;
        self.ai_test_status = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let outcome: Result<slackless_core::summarize::ConnectionTestResult, String> = cx
                .background_executor()
                .spawn(async move {
                    let api_key = slackless_core::auth::SecretStore::new().ok().and_then(|s| {
                        s.get(slackless_core::auth::AI_SUMMARY_API_KEY_ACCOUNT)
                            .ok()
                            .flatten()
                    });
                    let cfg = ai_settings.to_runtime_config(api_key);
                    slackless_core::summarize::test_connection_blocking(&cfg)
                        .map_err(|e| e.to_string())
                })
                .await;

            let _ = this.update(cx, |prefs, cx| {
                prefs.ai_test_running = false;
                prefs.ai_test_status = Some(match outcome {
                    Ok(res) => (true, res.detail),
                    Err(err) => (false, err),
                });
                cx.notify();
            });
        })
        .detach();
    }
}
