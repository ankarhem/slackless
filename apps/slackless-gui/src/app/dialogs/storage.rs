//! Downloads viewer, file deletion, storage maintenance, revisions, and transcript export.

use super::super::*;

impl App {
    /// A `file://` URL for `path`, percent-encoded (Finder opens folder URLs).
    pub(in crate::app) fn file_url(path: &std::path::Path) -> String {
        let mut url = String::from("file://");
        for byte in path.as_os_str().to_string_lossy().bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                    url.push(byte as char)
                }
                _ => url.push_str(&format!("%{byte:02X}")),
            }
        }
        url
    }

    /// Open Slack's in-app Downloads list (`Cmd+Shift+J`): shows every file downloaded in this
    /// session with Open / Show in Finder / Retry controls plus a shortcut to `~/Downloads`.
    pub(in crate::app) fn open_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let downloads_dir = std::env::var_os("HOME").map(|home| {
            let mut p = std::path::PathBuf::from(home);
            p.push("Downloads");
            p
        });
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, foreground, danger) = (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.danger,
            );
            let items = crate::media::downloads(cx);
            let has_finished = items
                .iter()
                .any(|d| !matches!(d.status, crate::media::DownloadStatus::InProgress));
            let folder = downloads_dir.clone();
            let header_actions = h_flex()
                .items_center()
                .gap_2()
                .when_some(folder, |row, path| {
                    row.child(
                        Button::new("downloads-open-folder")
                            .outline()
                            .with_size(Size::Small)
                            .icon(IconName::Folder)
                            .label("Open folder")
                            .on_click(move |_, _, cx| {
                                cx.open_url(&Self::file_url(&path));
                            }),
                    )
                })
                .when(has_finished, |row| {
                    row.child(
                        Button::new("downloads-clear")
                            .ghost()
                            .with_size(Size::Small)
                            .label("Clear list")
                            .on_click(|_, window, cx| {
                                crate::media::clear_finished_downloads(cx);
                                window.refresh();
                            }),
                    )
                });
            let rows: Vec<_> = items
                .into_iter()
                .map(|item| {
                    let (status_line, is_err) = match &item.status {
                        crate::media::DownloadStatus::InProgress => {
                            ("Downloading\u{2026}".to_owned(), false)
                        }
                        crate::media::DownloadStatus::Completed(_) => {
                            let size = item
                                .bytes
                                .map(|b| crate::files::human_size(b as i64))
                                .unwrap_or_default();
                            if size.is_empty() {
                                ("Completed".to_owned(), false)
                            } else {
                                (format!("{size} \u{b7} Completed"), false)
                            }
                        }
                        crate::media::DownloadStatus::Failed(err) => {
                            (format!("Failed: {err}"), true)
                        }
                    };
                    let actions = match &item.status {
                        crate::media::DownloadStatus::InProgress => h_flex().into_any_element(),
                        crate::media::DownloadStatus::Completed(path) => {
                            let open_path = path.clone();
                            let reveal_path = path.clone();
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .child(
                                    Button::new(("dl-open", item.id))
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .label("Open")
                                        .on_click(move |_, _, _| {
                                            crate::media::open_local_file(&open_path);
                                        }),
                                )
                                .child(
                                    Button::new(("dl-reveal", item.id))
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Folder)
                                        .tooltip("Show in Finder")
                                        .on_click(move |_, _, _| {
                                            crate::media::reveal_in_finder(&reveal_path);
                                        }),
                                )
                                .into_any_element()
                        }
                        crate::media::DownloadStatus::Failed(_) => {
                            let url = item.url.clone();
                            let name = item.name.clone();
                            Button::new(("dl-retry", item.id))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::RotateCcw)
                                .label("Retry")
                                .on_click(move |_, window, cx| {
                                    crate::media::download(url.clone(), name.clone(), window, cx);
                                })
                                .into_any_element()
                        }
                    };
                    h_flex()
                        .id(("dl-row", item.id))
                        .items_center()
                        .gap_3()
                        .py_2()
                        .border_b_1()
                        .border_color(border)
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(16.))
                                .text_color(if is_err { danger } else { muted }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .truncate()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(foreground)
                                        .child(item.name),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(if is_err { danger } else { muted })
                                        .child(status_line),
                                ),
                        )
                        .child(actions)
                })
                .collect();
            let empty = rows.is_empty().then(|| {
                v_flex()
                    .items_center()
                    .gap_2()
                    .py_8()
                    .text_sm()
                    .text_color(muted)
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(28.))
                            .text_color(muted),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child("No downloads yet"),
                    )
                    .child("Files you download from messages appear here.")
            });
            dialog.title("Downloads").w(px(560.)).child(
                v_flex().gap_3().child(header_actions).child(
                    v_flex()
                        .id("downloads-list")
                        .max_h(px(380.))
                        .overflow_y_scroll()
                        .children(rows)
                        .children(empty),
                ),
            )
        });
    }
    /// Confirm permanent deletion of a user-owned file/clip (`files.delete`, `F-CORE-130`).
    pub(in crate::app) fn confirm_delete_file(
        &self,
        file_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            let this = this.clone();
            let file_id = file_id.clone();
            dialog.title("Delete file?").w(px(420.)).child(
                v_flex()
                    .gap_4()
                    .child(div().text_color(muted).child(format!(
                        "Permanently delete \"{name}\"? This cannot be undone."
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("delete-file-cancel")
                                    .label("Cancel")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                Button::new("delete-file-confirm")
                                    .danger()
                                    .label("Delete")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = this.update(cx, |app, _| {
                                            app.bridge.send(Command::DeleteFile {
                                                file_id: file_id.clone(),
                                            });
                                        });
                                    }),
                            ),
                    ),
            )
        });
    }

    /// Confirm and clear local SQLite message cache (`Preferences -> Advanced -> Clear local cache`).
    pub(in crate::app) fn confirm_reset_cache(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.open_dialog(cx, move |dialog, _, cx| {
            let muted = cx.theme().muted_foreground;
            dialog
                .title("Clear local cache?")
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_4()
                        .child(div().text_color(muted).child(
                            "Removes cached search & message data from disk (your signed-in sessions stay intact). Relaunch slackless to re-sync fresh history.",
                        ))
                        .child(
                            h_flex()
                                .gap_2()
                                .justify_end()
                                .child(Button::new("reset-cache-cancel").label("Cancel").on_click(
                                    |_, window, cx| window.close_dialog(cx),
                                ))
                                .child(
                                    Button::new("reset-cache-confirm")
                                        .danger()
                                        .label("Clear cache")
                                        .on_click(|_, window, cx| {
                                            window.close_dialog(cx);
                                            if let Some(dir) = crate::bridge::store_dir()
                                                && let Ok(entries) = std::fs::read_dir(&dir) {
                                                    for entry in entries.flatten() {
                                                        let path = entry.path();
                                                        let name = path
                                                            .file_name()
                                                            .and_then(|n| n.to_str())
                                                            .unwrap_or_default();
                                                        if name.ends_with(".sqlite")
                                                            || name.ends_with(".sqlite-wal")
                                                            || name.ends_with(".sqlite-shm")
                                                        {
                                                            let _ = std::fs::remove_file(path);
                                                        }
                                                    }
                                                }
                                            notify(
                                                "Cache cleared",
                                                "Restart slackless to re-sync from Slack.".to_owned(),
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                )
        });
    }

    /// Gather SQLite & media cache metrics for the active workspace off the UI thread and push
    /// them into `Preferences -> Advanced -> Local storage & maintenance`.
    pub(in crate::app) fn refresh_storage_stats(&self, cx: &mut Context<Self>) {
        fn dir_bytes(dir: &std::path::Path) -> u64 {
            std::fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        }
        let Some((store, team)) = self
            .bridge
            .read_store_clone()
            .zip(self.bridge.team_id().cloned())
        else {
            self.preferences
                .update(cx, |prefs, cx| prefs.set_storage_stats(None, cx));
            return;
        };
        let prefs = self.preferences.downgrade();
        cx.spawn(async move |_, cx| {
            let info = cx
                .background_executor()
                .spawn(async move {
                    let stats = store.lock().ok()?.stats(&team).ok()?;
                    let media_cache_bytes = dir_bytes(
                        &std::env::temp_dir().join("slackless-media"),
                    )
                    .saturating_add(dir_bytes(&std::env::temp_dir().join("slackless-audio")));
                    Some(crate::preferences::StorageInfo {
                        messages: stats.messages,
                        revisions: stats.revisions,
                        conversations: stats.conversations,
                        users: stats.users,
                        offline_queued: stats.offline_queued,
                        db_bytes: stats.db_bytes(),
                        media_cache_bytes,
                    })
                })
                .await;
            let _ = prefs.update(cx, |prefs, cx| prefs.set_storage_stats(info, cx));
        })
        .detach();
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA optimize;` on the active workspace store.
    pub(in crate::app) fn compact_storage(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, team)) = self.bridge.store().zip(self.bridge.team_id()) else {
            notify(
                "No local database",
                "Compact is available in live workspaces.",
                window,
                cx,
            );
            return;
        };
        let before = store
            .lock()
            .ok()
            .and_then(|s| s.stats(team).ok())
            .map(|s| s.db_bytes())
            .unwrap_or(0);
        let res = store.lock().ok().map(|s| s.vacuum());
        match res {
            Some(Ok(())) => {
                let after = store
                    .lock()
                    .ok()
                    .and_then(|s| s.stats(team).ok())
                    .map(|s| s.db_bytes())
                    .unwrap_or(0);
                self.refresh_storage_stats(cx);
                let saved = before.saturating_sub(after);
                let msg = if saved > 0 {
                    format!("Reclaimed {} KiB and optimized FTS5 indexes.", saved / 1024)
                } else {
                    "Database is already compact; FTS5 statistics refreshed.".to_owned()
                };
                notify("Database compacted", msg, window, cx);
            }
            Some(Err(err)) => {
                notify("Compact failed", err.to_string(), window, cx);
            }
            None => {
                notify(
                    "Compact failed",
                    "Could not acquire database lock.",
                    window,
                    cx,
                );
            }
        }
    }

    /// Prune temporary downloaded media cache files (`slackless-media` + `slackless-audio`).
    pub(in crate::app) fn prune_media_cache(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut removed = 0u64;
        let mut dirs: Vec<std::path::PathBuf> = ["slackless-media", "slackless-audio"]
            .into_iter()
            .map(|folder| std::env::temp_dir().join(folder))
            .collect();
        if let Some(cache) = slackless_core::Fetcher::disk_cache_dir() {
            dirs.push(cache);
        }
        for dir in dirs {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata()
                        && meta.is_file()
                        && std::fs::remove_file(entry.path()).is_ok()
                    {
                        removed = removed.saturating_add(meta.len());
                    }
                }
            }
        }
        self.refresh_storage_stats(cx);
        let msg = if removed > 0 {
            format!(
                "Removed {} KiB of cached media files.",
                (removed / 1024).max(1)
            )
        } else {
            "Media cache was already clean.".to_owned()
        };
        notify("Media cache pruned", msg, window, cx);
    }

    /// Open a modal dialog showing every locally recorded revision of `(channel, ts)` from
    /// SQLite's immutable append-only `message_revision` log.
    pub(in crate::app) fn open_revision_history(
        &self,
        channel: ChannelId,
        ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fixtures = self.fixtures.clone();
        let revisions: Vec<(String, bool, String)> =
            if let (Some(store), Some(team)) = (self.bridge.store(), self.bridge.team_id()) {
                store
                    .lock()
                    .ok()
                    .and_then(|s| s.revisions(team, &channel, &ts).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|rev| {
                        let stamp = if rev.edit_ts.is_empty() {
                            format!("Original · {}", rev.message.ts)
                        } else if rev.deleted {
                            format!("Deleted · {}", rev.edit_ts)
                        } else {
                            format!("Edited · {}", rev.edit_ts)
                        };
                        let text = fixtures.plain_text(&rev.message);
                        (stamp, rev.deleted, text)
                    })
                    .collect()
            } else {
                fixtures
                    .messages
                    .get(&channel)
                    .and_then(|list| list.iter().find(|m| m.ts == ts))
                    .map(|m| {
                        vec![(
                            format!("Original · {}", m.ts),
                            false,
                            fixtures.plain_text(m),
                        )]
                    })
                    .unwrap_or_default()
            };
        let subtitle = format!("{} · {}", self.label(&channel), ts);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, danger, success, foreground, card_bg, font) = (
                theme.border,
                theme.muted_foreground,
                theme.danger,
                theme.success,
                theme.foreground,
                theme.secondary.opacity(0.35),
                theme.font_family.clone(),
            );
            let mut body = v_flex()
                .id("revision-history-list")
                .max_h(px(440.))
                .overflow_y_scroll()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(subtitle.clone()));
            if revisions.is_empty() {
                body = body.child(
                    div()
                        .py_6()
                        .text_sm()
                        .text_color(muted)
                        .child("No stored revisions found for this message."),
                );
            } else {
                for (ix, (stamp, deleted, text)) in revisions.iter().enumerate() {
                    let copy_text = text.clone();
                    let content_el: AnyElement = if ix > 0 && revisions[ix - 1].2 != *text {
                        let segments = slackless_core::diff_words(&revisions[ix - 1].2, text);
                        let mut combined = String::new();
                        let mut runs = Vec::with_capacity(segments.len());
                        for seg in segments {
                            let len = seg.text.len();
                            if len == 0 {
                                continue;
                            }
                            combined.push_str(&seg.text);
                            let (color, background_color, strikethrough) = match seg.kind {
                                slackless_core::DiffKind::Equal => (foreground, None, None),
                                slackless_core::DiffKind::Removed => (
                                    danger,
                                    Some(danger.opacity(0.14)),
                                    Some(gpui::StrikethroughStyle {
                                        thickness: px(1.),
                                        color: Some(danger),
                                    }),
                                ),
                                slackless_core::DiffKind::Added => {
                                    (success, Some(success.opacity(0.14)), None)
                                }
                            };
                            runs.push(gpui::TextRun {
                                len,
                                font: gpui::Font {
                                    family: font.clone(),
                                    ..Default::default()
                                },
                                color,
                                background_color,
                                underline: None,
                                strikethrough,
                            });
                        }
                        if combined.is_empty() {
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("(empty)")
                                .into_any_element()
                        } else {
                            div()
                                .text_sm()
                                .child(gpui::StyledText::new(combined).with_runs(runs))
                                .into_any_element()
                        }
                    } else {
                        div()
                            .text_sm()
                            .child(if text.is_empty() {
                                "(empty)".to_owned()
                            } else {
                                text.clone()
                            })
                            .into_any_element()
                    };
                    body = body.child(
                        v_flex()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .border_1()
                            .border_color(if *deleted {
                                danger.opacity(0.45)
                            } else {
                                border
                            })
                            .bg(if *deleted {
                                danger.opacity(0.08)
                            } else {
                                card_bg
                            })
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(if *deleted { danger } else { muted })
                                            .child(format!("#{} · {stamp}", ix + 1)),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!("rev-copy-{ix}")))
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .label("Copy")
                                            .on_click(move |_, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_text.clone(),
                                                ));
                                                notify(
                                                    "Revision copied",
                                                    "Copied message text to clipboard.",
                                                    window,
                                                    cx,
                                                );
                                            }),
                                    ),
                            )
                            .child(content_el),
                    );
                }
            }
            dialog
                .title(format!("Edit history ({} revisions)", revisions.len()))
                .w(px(560.))
                .child(body)
        });
    }

    /// Open a modal viewer for one locally archived text-only huddle transcript (with speaker
    /// attribution, per-line EN/SV language badges, Copy Markdown, and Delete).
    pub(in crate::app) fn open_huddle_transcript(
        &self,
        call_id: String,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let fixtures = self.fixtures.clone();
        let segments: Vec<slackless_core::store::HuddleTranscriptSegment> =
            if let (Some(store), Some(team)) = (self.bridge.store(), self.bridge.team_id()) {
                store
                    .lock()
                    .ok()
                    .and_then(|s| s.huddle_transcript_for_call(team, &call_id).ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        let ch_label = self.label(&channel);
        let subtitle = format!(
            "{ch_label} \u{b7} {call_id} \u{b7} {} lines",
            segments.len()
        );
        let markdown_export = {
            let mut md = format!("# Huddle Transcript \u{2014} {ch_label} (`{call_id}`)\n\n");
            for seg in &segments {
                let speaker = seg
                    .speaker_id
                    .as_ref()
                    .map(|u| fixtures.user_name(u).to_owned())
                    .unwrap_or_else(|| "Speaker".to_owned());
                let ts = Ts::from(format!("{}.000000", seg.at_ms.div_euclid(1_000)));
                let clock = crate::messages::list_stamp(&ts);
                let lang_tag = seg
                    .lang
                    .as_deref()
                    .map(|l| format!(" `[{l}]`"))
                    .unwrap_or_default();
                md.push_str(&format!(
                    "- **{speaker}** ({clock}){lang_tag}: {}\n",
                    seg.text
                ));
            }
            md
        };
        let store_for_del = self.bridge.store().cloned();
        let team_for_del = self.bridge.team_id().cloned();
        let del_call_id = call_id.clone();
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, fg, accent, danger, card_bg) = (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.link,
                theme.danger,
                theme.secondary.opacity(0.35),
            );
            let copy_md = markdown_export.clone();
            let del_store = store_for_del.clone();
            let del_team = team_for_del.clone();
            let del_id = del_call_id.clone();
            let del_this = this.clone();
            let mut list = v_flex()
                .id("huddle-transcript-lines")
                .max_h(px(420.))
                .overflow_y_scroll()
                .gap_1p5();
            if segments.is_empty() {
                list = list.child(
                    div()
                        .py_6()
                        .text_sm()
                        .text_color(muted)
                        .child("No archived transcript lines found for this huddle."),
                );
            } else {
                for seg in &segments {
                    let speaker = seg
                        .speaker_id
                        .as_ref()
                        .map(|u| fixtures.user_name(u).to_owned())
                        .unwrap_or_else(|| "Speaker".to_owned());
                    let ts = Ts::from(format!("{}.000000", seg.at_ms.div_euclid(1_000)));
                    let clock = crate::messages::list_stamp(&ts);
                    let lang_badge = seg.lang.as_deref().map(|l| l.to_ascii_uppercase());
                    list = list.child(
                        v_flex()
                            .gap_0p5()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(card_bg)
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .child(speaker),
                                    )
                                    .child(div().text_xs().text_color(muted).child(clock))
                                    .when_some(lang_badge, |row, code| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(accent.opacity(0.14))
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(accent)
                                                .child(code),
                                        )
                                    }),
                            )
                            .child(div().text_sm().text_color(fg).child(seg.text.clone())),
                    );
                }
            }
            let actions = h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().text_xs().text_color(muted).child(subtitle.clone()))
                .child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Button::new("transcript-copy-md")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::Copy)
                                .label("Copy Markdown")
                                .disabled(segments.is_empty())
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        copy_md.clone(),
                                    ));
                                    notify(
                                        "Transcript copied",
                                        "Copied Markdown transcript to clipboard.",
                                        window,
                                        cx,
                                    );
                                }),
                        )
                        .when(!segments.is_empty(), |row| {
                            row.child(
                                Button::new("transcript-delete")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::Trash)
                                    .tooltip("Delete local transcript")
                                    .on_click(move |_, window, cx| {
                                        if let (Some(st), Some(tm)) = (&del_store, &del_team)
                                            && let Ok(guard) = st.lock()
                                        {
                                            let _ = guard.delete_huddle_transcript(tm, &del_id);
                                        }
                                        window.close_dialog(cx);
                                        let _ = del_this.update(cx, |app, cx| {
                                            if app.huddles_open {
                                                app.show_huddles(cx);
                                            }
                                        });
                                        notify(
                                            "Transcript deleted",
                                            "Removed local huddle transcript from SQLite.",
                                            window,
                                            cx,
                                        );
                                    }),
                            )
                        }),
                );
            let _ = danger;
            dialog
                .title(format!("Huddle transcript \u{b7} {ch_label}"))
                .w(px(620.))
                .child(v_flex().gap_3().child(actions).child(list))
        });
    }

    /// Export the active conversation's locally archived messages as Markdown or pretty JSON
    /// using a native save dialog.
    pub(in crate::app) fn export_conversation_transcript(
        &self,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self.channel.clone();
        let label = self.label(&channel);
        let safe_name: String = label
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let safe_name = safe_name.trim_matches('-');
        let stem = if safe_name.is_empty() {
            channel.as_str()
        } else {
            safe_name
        };
        let ext = if markdown { "md" } else { "json" };
        let suggested = format!("{stem}-transcript.{ext}");
        let start_dir = {
            let custom = self.preferences.read(cx).settings().download_dir.clone();
            if !custom.is_empty() {
                std::path::PathBuf::from(custom)
            } else {
                std::env::var_os("HOME")
                    .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                    .unwrap_or_else(std::env::temp_dir)
            }
        };
        let fixtures = self.fixtures.clone();
        let store = self.bridge.store().cloned();
        let team = self.bridge.team_id().cloned();
        let rx = cx.prompt_for_new_path(&start_dir, Some(&suggested));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    let messages: Vec<slackless_core::types::Message> =
                        if let (Some(store), Some(team)) = (store, team) {
                            store
                                .lock()
                                .ok()
                                .and_then(|s| s.channel_history(&team, &channel, None, 10_000).ok())
                                .map(|rows| rows.into_iter().map(|r| r.message).collect())
                                .unwrap_or_default()
                        } else {
                            fixtures.messages.get(&channel).cloned().unwrap_or_default()
                        };
                    let count = messages.len();
                    let body = if markdown {
                        let mut md = format!("# {label}\n\n");
                        for msg in &messages {
                            let author = msg
                                .user
                                .as_ref()
                                .map(|u| fixtures.user_name(u).to_owned())
                                .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()))
                                .unwrap_or_else(|| "unknown".to_owned());
                            md.push_str(&format!("**{author}** (`{}`)\n\n", msg.ts));
                            let text = fixtures.plain_text(msg);
                            if !text.is_empty() {
                                md.push_str(&text);
                                md.push_str("\n\n");
                            }
                            if !msg.files.is_empty() {
                                let names: Vec<&str> = msg
                                    .files
                                    .iter()
                                    .filter_map(|f| f.name.as_deref().or(f.title.as_deref()))
                                    .collect();
                                if !names.is_empty() {
                                    md.push_str(&format!("_Files:_ {}\n\n", names.join(", ")));
                                }
                            }
                            md.push_str("---\n\n");
                        }
                        md
                    } else {
                        let rows: Vec<_> = messages
                            .iter()
                            .map(|msg| {
                                let author = msg
                                    .user
                                    .as_ref()
                                    .map(|u| fixtures.user_name(u).to_owned())
                                    .or_else(|| msg.bot_profile.as_ref().map(|b| b.name.clone()));
                                crate::blockkit::serde_json::json!({
                                    "ts": msg.ts.as_str(),
                                    "user": msg.user.as_ref().map(|u| u.as_str()),
                                    "author": author,
                                    "text": fixtures.plain_text(msg),
                                    "thread_ts": msg.thread_ts.as_ref().map(|t| t.as_str()),
                                })
                            })
                            .collect();
                        crate::blockkit::serde_json::to_string_pretty(
                            &crate::blockkit::serde_json::json!({
                                "channel": channel.as_str(),
                                "label": label,
                                "count": count,
                                "messages": rows,
                            }),
                        )
                        .unwrap_or_default()
                    };
                    std::fs::write(&path, body).map(|()| (count, path))
                })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok((count, path)) => notify(
                    "Transcript exported",
                    format!("Saved {count} messages to {}", path.display()),
                    window,
                    cx,
                ),
                Err(err) => notify("Export failed", err.to_string(), window, cx),
            });
        })
        .detach();
    }
}
