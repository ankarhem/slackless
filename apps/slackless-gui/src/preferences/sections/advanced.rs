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
            .child(self.group(
                "Search",
                Some("Local search is instant and works offline. Enable remote search when you want older history, unjoined channels, and server-indexed files automatically."),
                v_flex()
                    .gap_2()
                    .child(remote_search)
                    .child(search_in_ch)
                    .child(self.labeled("Exclude these channels from local search", excluded_input)),
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
}
