//! Downloads viewer, file deletion, storage maintenance, revisions, and transcript export.

use super::super::*;

pub(in crate::app) struct HuddleTranscriptOpen {
    pub(in crate::app) call_id: String,
    pub(in crate::app) channel: ChannelId,
    pub(in crate::app) segments: Vec<slackless_core::store::HuddleTranscriptSegment>,
    pub(in crate::app) highlight_segment_id: Option<i64>,
    pub(in crate::app) highlight_query: Option<String>,
}

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
                        preferences::form_button(
                            Button::new("downloads-open-folder")
                                .outline()
                                .icon(IconName::Folder)
                                .label("Open folder"),
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            cx.open_url(&Self::file_url(&path));
                        }),
                    )
                })
                .when(has_finished, |row| {
                    row.child(
                        preferences::form_button(
                            Button::new("downloads-clear").ghost().label("Clear list"),
                            cx,
                        )
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
                                preferences::form_button(
                                    Button::new("delete-file-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .child(
                                preferences::form_button(
                                    Button::new("delete-file-confirm").danger().label("Delete"),
                                    cx,
                                )
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = this.update(cx, |app, _| {
                                        app.bridge.send(CollabCommand::DeleteFile {
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
                                .child(
                                    preferences::form_button(
                                        Button::new("reset-cache-cancel")
                                            .outline()
                                            .label("Cancel"),
                                        cx,
                                    )
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                                )
                                .child(
                                    preferences::form_button(
                                        Button::new("reset-cache-confirm")
                                            .danger()
                                            .label("Clear cache"),
                                        cx,
                                    )
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
                .spawn(async move { collect_storage_info(&store, &team) })
                .await;
            let _ = prefs.update(cx, |prefs, cx| prefs.set_storage_stats(info, cx));
        })
        .detach();
    }

    /// Gather SQLite & media cache metrics off the UI thread, push them to `Preferences`, and
    /// refresh `window` so an open Storage modal dialog repaints immediately on load.
    pub(in crate::app) fn refresh_storage_stats_in(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((store, team)) = self
            .bridge
            .read_store_clone()
            .zip(self.bridge.team_id().cloned())
        else {
            self.preferences
                .update(cx, |prefs, cx| prefs.set_storage_stats(None, cx));
            window.refresh();
            return;
        };
        let prefs = self.preferences.downgrade();
        cx.spawn_in(window, async move |_, cx| {
            let info = cx
                .background_executor()
                .spawn(async move { collect_storage_info(&store, &team) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = prefs.update(cx, |prefs, cx| prefs.set_storage_stats(info, cx));
                window.refresh();
            });
        })
        .detach();
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA optimize;` on the active workspace
    /// store off the UI thread, then refresh storage metrics and show a toast notification.
    pub(in crate::app) fn compact_storage(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, team)) = self
            .bridge
            .store()
            .cloned()
            .zip(self.bridge.team_id().cloned())
        else {
            notify(
                "No local database",
                "Compact is available in live workspaces.",
                window,
                cx,
            );
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let outcome: Result<u64, String> = cx
                .background_executor()
                .spawn(async move {
                    let before = store
                        .lock()
                        .ok()
                        .and_then(|s| s.stats(&team).ok())
                        .map(|s| s.db_bytes())
                        .unwrap_or(0);
                    let mut guard = store
                        .lock()
                        .map_err(|_| "Could not acquire database lock.".to_owned())?;
                    guard.vacuum().map_err(|err| err.to_string())?;
                    let after = guard.stats(&team).ok().map(|s| s.db_bytes()).unwrap_or(0);
                    Ok(before.saturating_sub(after))
                })
                .await;
            let _ = this.update_in(cx, |app, window, cx| match outcome {
                Ok(saved) => {
                    app.refresh_storage_stats_in(window, cx);
                    let msg = if saved > 0 {
                        format!("Reclaimed {} KiB and optimized FTS5 indexes.", saved / 1024)
                    } else {
                        "Database is already compact; FTS5 statistics refreshed.".to_owned()
                    };
                    notify("Database compacted", msg, window, cx);
                }
                Err(err) => notify("Compact failed", err, window, cx),
            });
        })
        .detach();
    }

    /// Prune temporary downloaded media cache files (`slackless-media` + `slackless-audio`) off
    /// the UI thread, then refresh storage metrics and show a toast notification.
    pub(in crate::app) fn prune_media_cache(&self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let removed = cx
                .background_executor()
                .spawn(async move {
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
                    removed
                })
                .await;
            let _ = this.update_in(cx, |app, window, cx| {
                app.refresh_storage_stats_in(window, cx);
                let msg = if removed > 0 {
                    format!(
                        "Removed {} KiB of cached media files.",
                        (removed / 1024).max(1)
                    )
                } else {
                    "Media cache was already clean.".to_owned()
                };
                notify("Media cache pruned", msg, window, cx);
            });
        })
        .detach();
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
        let revisions: Vec<(String, bool, String)> = if let (Some(store), Some(team)) =
            (self.bridge.read_store_clone(), self.bridge.team_id())
        {
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
        self.open_revision_history_with(subtitle, revisions, window, cx);
    }

    pub(in crate::app) fn open_revision_history_with(
        &self,
        subtitle: String,
        revisions: Vec<(String, bool, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                        let mut runs = Vec::with_capacity(segments.len() * 2);
                        let mut prev_kind: Option<slackless_core::DiffKind> = None;
                        for seg in segments {
                            let len = seg.text.len();
                            if len == 0 {
                                continue;
                            }
                            if matches!(
                                (prev_kind, seg.kind),
                                (
                                    Some(slackless_core::DiffKind::Removed),
                                    slackless_core::DiffKind::Added
                                ) | (
                                    Some(slackless_core::DiffKind::Added),
                                    slackless_core::DiffKind::Removed
                                )
                            ) {
                                combined.push(' ');
                                runs.push(gpui::TextRun {
                                    len: 1,
                                    font: gpui::Font {
                                        family: font.clone(),
                                        ..Default::default()
                                    },
                                    color: foreground,
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                });
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
                            prev_kind = Some(seg.kind);
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
    /// avatars, grouped turns, search-hit spotlighting, language badges, Copy Markdown, Go to
    /// channel, and Delete).
    pub(in crate::app) fn open_huddle_transcript(
        &self,
        call_id: String,
        channel: ChannelId,
        highlight_segment_id: Option<i64>,
        highlight_query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let segments: Vec<slackless_core::store::HuddleTranscriptSegment> =
            if let (Some(store), Some(team)) =
                (self.bridge.read_store_clone(), self.bridge.team_id())
            {
                store
                    .lock()
                    .ok()
                    .and_then(|s| s.huddle_transcript_for_call(team, &call_id).ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
        self.open_huddle_transcript_with(
            HuddleTranscriptOpen {
                call_id,
                channel,
                segments,
                highlight_segment_id,
                highlight_query,
            },
            window,
            cx,
        );
    }

    pub(in crate::app) fn open_huddle_transcript_with(
        &self,
        params: HuddleTranscriptOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let HuddleTranscriptOpen {
            call_id,
            channel,
            segments,
            highlight_segment_id,
            highlight_query,
        } = params;
        let fixtures = self.fixtures.clone();
        let ch_label = self.label(&channel);

        // Compute summary metadata: duration, unique speakers, unique languages, and provider.
        let mut unique_speakers: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut unique_langs: Vec<String> = Vec::new();
        for seg in &segments {
            let key = seg
                .speaker_id
                .as_ref()
                .map(|u| u.as_str().to_owned())
                .unwrap_or_else(|| "unknown".to_owned());
            unique_speakers.insert(key);
            if let Some(lang) = seg.lang.as_deref() {
                let code = lang.to_ascii_uppercase();
                if !unique_langs.contains(&code) {
                    unique_langs.push(code);
                }
            }
        }
        let duration_label = match (segments.first(), segments.last()) {
            (Some(first), Some(last)) if last.at_ms > first.at_ms => {
                let secs = ((last.at_ms - first.at_ms) / 1_000).max(1);
                if secs >= 3600 {
                    format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
                } else if secs >= 60 {
                    format!("{}m {}s", secs / 60, secs % 60)
                } else {
                    format!("{secs}s")
                }
            }
            _ => String::new(),
        };
        let started_stamp = segments.first().map(|s| {
            let ts = Ts::from(format!("{}.000000", s.at_ms.div_euclid(1_000)));
            crate::messages::list_stamp(&ts)
        });
        let engine_label = segments.first().map(|s| match s.source.as_str() {
            "parakeet-v3" => "Parakeet v3",
            "whisper-turbo" | "local-whisper" => "Whisper Turbo",
            _ => "Slack AWS",
        });

        let export_doc = {
            let ws = self
                .bridge
                .team_id()
                .cloned()
                .or_else(|| {
                    self.fixtures
                        .workspaces
                        .get(self.workspace)
                        .or_else(|| self.fixtures.workspaces.first())
                        .map(|t| t.id.clone())
                })
                .unwrap_or_default();
            let ws_name = self
                .fixtures
                .workspaces
                .iter()
                .find(|t| t.id == ws)
                .map(|t| t.name.clone())
                .filter(|n| !n.trim().is_empty());
            let mut users = HashMap::new();
            for u in &self.fixtures.users {
                users.insert(u.id.clone(), self.fixtures.name_of(u).to_owned());
            }
            slackless_core::export::build_huddle_transcript_doc(
                &ws, ws_name, &call_id, &channel, &ch_label, &segments, &users,
            )
        };
        let markdown_export =
            slackless_core::export::render_huddle_transcript_markdown(&export_doc);
        let json_export = slackless_core::export::render_huddle_transcript_json(&export_doc)
            .map_err(|e| e.to_string());
        let suggested_stem =
            slackless_core::export::sanitize_export_filename(&call_id, "huddle-transcript");

        // Parse search query terms (excluding filter tokens like `in:#ch` / `from:@user`) so we
        // can highlight matched words inside the transcript with Swedish åäö diacritic folding.
        let query_terms: Vec<Vec<char>> = highlight_query
            .as_deref()
            .unwrap_or("")
            .split_whitespace()
            .filter(|tok| !tok.contains(':') && !tok.starts_with('-'))
            .map(|tok| {
                tok.trim_matches(|c: char| !c.is_alphanumeric())
                    .chars()
                    .map(fold_search_char)
                    .collect::<Vec<_>>()
            })
            .filter(|v| v.len() >= 2)
            .collect();

        // Group consecutive segments from the same speaker within 90 seconds into conversation
        // turns so the transcript reads cleanly like a dialogue rather than stacked boxes.
        struct TurnGroup<'a> {
            speaker_id: Option<UserId>,
            first_at_ms: i64,
            primary_lang: Option<String>,
            items: Vec<&'a slackless_core::store::HuddleTranscriptSegment>,
        }
        let store_for_del = self.bridge.store().cloned();
        let team_for_del = self.bridge.team_id().cloned();
        let del_call_id = call_id.clone();
        let jump_channel = channel.clone();
        let this = cx.weak_entity();
        let speaker_count = unique_speakers.len();
        let save_dir = self.default_export_dir(cx);
        window.open_dialog(cx, move |dialog, _, cx| {
            let theme = cx.theme();
            let (border, muted, fg, accent, warn, card_bg, pill_bg, font_family) = (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.link,
                theme.warning,
                theme.secondary.opacity(0.32),
                theme.secondary.opacity(0.65),
                theme.font_family.clone(),
            );
            let copy_md = markdown_export.clone();
            let save_md = markdown_export.clone();
            let save_json = json_export.clone();
            let stem_md = suggested_stem.clone();
            let stem_json = suggested_stem.clone();
            let dir_md = save_dir.clone();
            let dir_json = save_dir.clone();
            let seg_count = segments.len();
            let del_store = store_for_del.clone();
            let del_team = team_for_del.clone();
            let del_id = del_call_id.clone();
            let del_this = this.clone();
            let jump_this = this.clone();
            let jump_ch = jump_channel.clone();

            let mut groups: Vec<TurnGroup<'_>> = Vec::new();
            for seg in &segments {
                let merge = groups.last().is_some_and(|g| {
                    g.speaker_id == seg.speaker_id
                        && seg.at_ms.saturating_sub(g.first_at_ms) <= 90_000
                });
                if merge && let Some(g) = groups.last_mut() {
                    g.items.push(seg);
                } else {
                    groups.push(TurnGroup {
                        speaker_id: seg.speaker_id.clone(),
                        first_at_ms: seg.at_ms,
                        primary_lang: seg.lang.as_deref().map(|l| l.to_ascii_uppercase()),
                        items: vec![seg],
                    });
                }
            }

            // Summary header bar: metadata chips + primary actions.
            let mut meta_pills = h_flex().items_center().flex_wrap().gap_1p5();
            if let Some(started) = &started_stamp {
                let time_text = if duration_label.is_empty() {
                    started.clone()
                } else {
                    format!("{started} \u{b7} {duration_label}")
                };
                meta_pills = meta_pills.child(
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(pill_bg)
                        .text_xs()
                        .text_color(fg)
                        .child(time_text),
                );
            }
            if !segments.is_empty() {
                let spk_label = if speaker_count == 1 {
                    "1 speaker".to_owned()
                } else {
                    format!("{speaker_count} speakers")
                };
                meta_pills = meta_pills.child(
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(pill_bg)
                        .text_xs()
                        .text_color(muted)
                        .child(format!("{spk_label} \u{b7} {} lines", segments.len())),
                );
            }
            for code in &unique_langs {
                meta_pills = meta_pills.child(
                    div()
                        .px_1p5()
                        .py(px(2.))
                        .rounded(px(5.))
                        .bg(accent.opacity(0.14))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(accent)
                        .child(code.clone()),
                );
            }
            if let Some(engine) = engine_label {
                meta_pills = meta_pills.child(
                    div()
                        .px_2()
                        .py(px(2.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(border)
                        .text_xs()
                        .text_color(muted)
                        .child(engine),
                );
            }

            let toolbar = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap_3()
                .pb_3()
                .border_b_1()
                .border_color(border)
                .child(div().flex_1().min_w_0().child(meta_pills))
                .child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1p5()
                        .child(
                            preferences::form_button(
                                Button::new("transcript-go-channel")
                                    .outline()
                                    .icon(IconName::Hash)
                                    .label("Channel"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = jump_this.update(cx, |app, cx| {
                                    app.select(jump_ch.clone(), cx);
                                });
                            }),
                        )
                        .child(
                            preferences::form_button(
                                Button::new("transcript-copy-md")
                                    .outline()
                                    .icon(IconName::Copy)
                                    .label("Copy MD")
                                    .disabled(segments.is_empty()),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_md.clone()));
                                notify(
                                    "Transcript copied",
                                    "Copied Markdown transcript to clipboard.",
                                    window,
                                    cx,
                                );
                            }),
                        )
                        .child(
                            preferences::form_button(
                                Button::new("transcript-export-md")
                                    .outline()
                                    .icon(IconName::Download)
                                    .label("Save MD\u{2026}")
                                    .disabled(segments.is_empty()),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let suggested = format!("{stem_md}.md");
                                let content = save_md.clone();
                                let rx = cx.prompt_for_new_path(&dir_md, Some(&suggested));
                                window
                                    .spawn(cx, async move |cx| {
                                        let Ok(Ok(Some(path))) = rx.await else {
                                            return;
                                        };
                                        let res = cx
                                            .background_executor()
                                            .spawn(async move {
                                                std::fs::write(&path, content).map(|()| path)
                                            })
                                            .await;
                                        let _ = cx.update(|window, cx| match res {
                                            Ok(path) => notify(
                                                "Transcript exported",
                                                format!(
                                                    "Saved {seg_count} lines to {}",
                                                    path.display()
                                                ),
                                                window,
                                                cx,
                                            ),
                                            Err(err) => {
                                                notify("Export failed", err.to_string(), window, cx)
                                            }
                                        });
                                    })
                                    .detach();
                            }),
                        )
                        .child(
                            preferences::form_button(
                                Button::new("transcript-export-json")
                                    .outline()
                                    .icon(IconName::FileCode)
                                    .label("Save JSON\u{2026}")
                                    .disabled(segments.is_empty()),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let content = match save_json.clone() {
                                    Ok(c) => c,
                                    Err(err) => {
                                        notify("Export failed", err, window, cx);
                                        return;
                                    }
                                };
                                let suggested = format!("{stem_json}.json");
                                let rx = cx.prompt_for_new_path(&dir_json, Some(&suggested));
                                window
                                    .spawn(cx, async move |cx| {
                                        let Ok(Ok(Some(path))) = rx.await else {
                                            return;
                                        };
                                        let res = cx
                                            .background_executor()
                                            .spawn(async move {
                                                std::fs::write(&path, content).map(|()| path)
                                            })
                                            .await;
                                        let _ = cx.update(|window, cx| match res {
                                            Ok(path) => notify(
                                                "Transcript exported",
                                                format!(
                                                    "Saved {seg_count} lines to {}",
                                                    path.display()
                                                ),
                                                window,
                                                cx,
                                            ),
                                            Err(err) => {
                                                notify("Export failed", err.to_string(), window, cx)
                                            }
                                        });
                                    })
                                    .detach();
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
                                            && let Ok(mut guard) = st.lock()
                                        {
                                            let _ = guard.delete_huddle_transcript(tm, &del_id);
                                        }
                                        window.close_dialog(cx);
                                        let _ = del_this.update(cx, |app, cx| {
                                            if app.page == MainPage::Huddles {
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

            let mut list = v_flex()
                .id("huddle-transcript-lines")
                .w_full()
                .max_h(px(460.))
                .overflow_y_scroll()
                .gap_3()
                .py_1();

            if groups.is_empty() {
                list = list.child(
                    v_flex()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .py_8()
                        .child(
                            Icon::new(IconName::Headphones)
                                .size(px(22.))
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("No archived transcript lines found for this huddle."),
                        ),
                );
            } else {
                for (g_ix, group) in groups.into_iter().enumerate() {
                    let speaker = group
                        .speaker_id
                        .as_ref()
                        .map(|u| fixtures.user_name(u).to_owned())
                        .unwrap_or_else(|| "Speaker".to_owned());
                    let avatar = match &group.speaker_id {
                        Some(uid) => crate::avatars::user_chip(&fixtures, uid, 30.),
                        None => div()
                            .size(px(30.))
                            .rounded_md()
                            .bg(pill_bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(15.))
                                    .text_color(muted),
                            ),
                    };
                    let ts = Ts::from(format!("{}.000000", group.first_at_ms.div_euclid(1_000)));
                    let clock = crate::messages::list_stamp(&ts);
                    let turn_lang = group.primary_lang.clone();

                    let mut turn_col = v_flex().flex_1().min_w_0().gap_1().child(
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
                            .when_some(turn_lang.clone(), |row, code| {
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
                    );

                    for (u_ix, seg) in group.items.into_iter().enumerate() {
                        let is_hit = highlight_segment_id == Some(seg.id);
                        let seg_lang = seg.lang.as_deref().map(|l| l.to_ascii_uppercase());
                        let switched_lang = u_ix > 0 && seg_lang.is_some() && seg_lang != turn_lang;
                        let styled_text = render_highlighted_transcript_line(
                            &seg.text,
                            &query_terms,
                            fg,
                            warn.opacity(0.28),
                            &font_family,
                        );
                        let line_row = h_flex()
                            .w_full()
                            .min_w_0()
                            .items_start()
                            .gap_2()
                            .when(is_hit, |row| {
                                row.px_2p5()
                                    .py_1p5()
                                    .rounded_md()
                                    .bg(accent.opacity(0.12))
                                    .border_l_2()
                                    .border_color(accent)
                            })
                            .when(!is_hit, |row| row.px_1().py(px(1.)))
                            .when(switched_lang, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .mt(px(3.))
                                        .px_1()
                                        .py(px(1.))
                                        .rounded(px(3.))
                                        .bg(accent.opacity(0.14))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(accent)
                                        .child(seg_lang.clone().unwrap_or_default()),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .line_height(px(21.))
                                    .text_color(fg)
                                    .child(styled_text),
                            )
                            .when(is_hit, |row| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .mt(px(3.))
                                        .px_1p5()
                                        .py(px(1.))
                                        .rounded(px(4.))
                                        .bg(accent.opacity(0.20))
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(accent)
                                        .child("Match"),
                                )
                            });
                        turn_col = turn_col.child(line_row);
                    }

                    list = list.child(
                        h_flex()
                            .id(("transcript-turn", g_ix))
                            .w_full()
                            .min_w_0()
                            .items_start()
                            .gap_3()
                            .px_3()
                            .py_2p5()
                            .rounded_lg()
                            .bg(card_bg)
                            .border_1()
                            .border_color(border.opacity(0.6))
                            .child(div().flex_none().mt(px(2.)).child(avatar))
                            .child(turn_col),
                    );
                }
            }

            dialog
                .title(format!("Huddle transcript \u{b7} {ch_label}"))
                .w(px(680.))
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .gap_3()
                        .child(toolbar)
                        .child(list),
                )
        });
    }

    /// Default folder for save/export dialogs (configured download folder, else `~/Downloads`).
    pub(in crate::app) fn default_export_dir(&self, cx: &gpui::App) -> std::path::PathBuf {
        let custom = self.preferences.read(cx).settings().download_dir.clone();
        if !custom.is_empty() {
            std::path::PathBuf::from(custom)
        } else {
            std::env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join("Downloads"))
                .unwrap_or_else(std::env::temp_dir)
        }
    }

    /// Start an explicit background backfill (history + thread replies) for one channel.
    pub(in crate::app) fn start_channel_backfill(
        &mut self,
        channel: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.label(&channel);
        if self.fixtures.sample || self.bridge.store().is_none() {
            notify(
                "Live workspace required",
                format!("Sign in to backfill full Slack history for {label}."),
                window,
                cx,
            );
            return;
        }
        self.bridge.send(WorkspaceCommand::BackfillChannel {
            channel,
            include_threads: true,
        });
        notify(
            "Backfill started",
            format!("Syncing history and threads for {label} into local SQLite."),
            window,
            cx,
        );
    }

    /// Start background archive backfill across all member channels in the active workspace.
    pub(in crate::app) fn start_workspace_backfill(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample || self.bridge.store().is_none() {
            notify(
                "Live workspace required",
                "Sign in to backfill workspace history into local SQLite.",
                window,
                cx,
            );
            return;
        }
        self.bridge.send(WorkspaceCommand::BackfillWorkspace {
            include_threads: true,
        });
        notify(
            "Workspace backfill started",
            "Walking channel history and threads in the background.",
            window,
            cx,
        );
    }

    /// Pause any running background archive backfill cleanly between pages.
    pub(in crate::app) fn pause_backfill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixtures.sample || self.bridge.store().is_none() {
            return;
        }
        self.bridge.send(WorkspaceCommand::PauseBackfill);
        notify(
            "Backfill paused",
            "Archive sync paused; click Resume to continue where it left off.",
            window,
            cx,
        );
    }

    /// Resume a paused background archive backfill.
    pub(in crate::app) fn resume_backfill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.fixtures.sample || self.bridge.store().is_none() {
            return;
        }
        self.bridge.send(WorkspaceCommand::ResumeBackfill);
        notify(
            "Backfill resumed",
            "Continuing background archive sync into local SQLite.",
            window,
            cx,
        );
    }

    /// Open the Archive & Storage modal dialog showing local SQLite metrics, live workspace &
    /// per-channel backfill progress, Start / Pause / Resume controls, and maintenance actions.
    pub(in crate::app) fn open_storage_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_storage_stats_in(window, cx);
        if !self.fixtures.sample && self.bridge.store().is_some() {
            self.bridge.send(WorkspaceCommand::QueryBackfillProgress);
        }
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let theme = cx.theme();
            let (border, muted, fg, accent, success, danger, card_bg, pill_bg) = (
                theme.border,
                theme.muted_foreground,
                theme.foreground,
                theme.link,
                theme.success,
                theme.danger,
                theme.secondary.opacity(0.32),
                theme.secondary.opacity(0.65),
            );
            let snapshot = this.upgrade().map(|app| {
                let a = app.read(cx);
                let stats = a.preferences.read(cx).storage_stats_snapshot();
                let progress = a.ws.backfill_progress.clone();
                let active_channel = a.channel.clone();
                let active_label = a.label(&active_channel);
                let sample = a.fixtures.sample;
                (stats, progress, active_channel, active_label, sample)
            });
            let (stats, progress, active_channel, active_label, sample) = match snapshot {
                Some(s) => s,
                None => {
                    return dialog.title("Archive & Storage").w(px(620.)).child(div());
                }
            };

            let status = progress
                .as_ref()
                .map(|p| p.status)
                .unwrap_or(slackless_core::BackfillStatus::Idle);
            let status_color = match status {
                slackless_core::BackfillStatus::Running => accent,
                slackless_core::BackfillStatus::Complete => success,
                slackless_core::BackfillStatus::Error => danger,
                slackless_core::BackfillStatus::Paused | slackless_core::BackfillStatus::Idle => {
                    muted
                }
            };
            let (completed, total, msgs, replies, pages) = progress
                .as_ref()
                .map(|p| {
                    (
                        p.completed_channels,
                        p.total_channels,
                        p.messages_stored,
                        p.replies_stored,
                        p.pages_fetched,
                    )
                })
                .unwrap_or((0, 0, 0, 0, 0));
            let pct = if total > 0 {
                ((completed as f32) / (total as f32)).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let this_ws = this.clone();
            let this_ch = this.clone();
            let this_pause = this.clone();
            let this_resume = this.clone();
            let this_compact = this.clone();
            let this_prune = this.clone();
            let this_reset = this.clone();
            let active_ch_for_btn = active_channel.clone();

            let stats_card = v_flex()
                .gap_1p5()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(fg)
                        .child("Local SQLite archive"),
                )
                .child(match stats {
                    Some(st) => div().text_xs().text_color(muted).child(format!(
                        "{} SQLite · {} media cache · {} messages ({} revisions) · {} conversations · {} users",
                        slackless_core::types::time::format_bytes(st.db_bytes),
                        slackless_core::types::time::format_bytes(st.media_cache_bytes),
                        st.messages,
                        st.revisions,
                        st.conversations,
                        st.users,
                    )),
                    None => div().text_xs().text_color(muted).child(
                        "Connect a live workspace to inspect SQLite database metrics and backfill Slack history.",
                    ),
                });

            let mut controls = h_flex().items_center().flex_wrap().gap_2();
            match status {
                slackless_core::BackfillStatus::Running => {
                    controls = controls.child(
                        preferences::form_button(
                            Button::new("storage-bf-pause")
                                .outline()
                                .label("Pause backfill"),
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            let _ = this_pause.update(cx, |app, cx| app.pause_backfill(window, cx));
                        }),
                    );
                }
                slackless_core::BackfillStatus::Paused => {
                    controls = controls.child(
                        preferences::form_button(
                            Button::new("storage-bf-resume")
                                .primary()
                                .label("Resume backfill"),
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            let _ =
                                this_resume.update(cx, |app, cx| app.resume_backfill(window, cx));
                        }),
                    );
                }
                _ => {
                    controls = controls.child(
                        preferences::form_button(
                            Button::new("storage-bf-workspace")
                                .primary()
                                .disabled(sample)
                                .label("Backfill workspace history"),
                            cx,
                        )
                        .on_click(move |_, window, cx| {
                            let _ = this_ws
                                .update(cx, |app, cx| app.start_workspace_backfill(window, cx));
                        }),
                    );
                }
            }
            controls = controls.child(
                preferences::form_button(
                    Button::new("storage-bf-current")
                        .outline()
                        .disabled(sample)
                        .label(format!("Backfill {active_label}")),
                    cx,
                )
                .on_click(move |_, window, cx| {
                    let _ = this_ch.update(cx, |app, cx| {
                        app.start_channel_backfill(active_ch_for_btn.clone(), window, cx);
                    });
                }),
            );

            let backfill_card = v_flex()
                .gap_2()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(card_bg)
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(fg)
                                .child("Background history backfill"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py(px(1.))
                                .rounded(px(4.))
                                .bg(status_color.opacity(0.16))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(status_color)
                                .child(status.label()),
                        ),
                )
                .child(
                    div().text_xs().text_color(muted).child(format!(
                        "{completed}/{total} channels complete · {msgs} messages · {replies} thread replies · {pages} pages"
                    )),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(6.))
                        .rounded_full()
                        .bg(pill_bg)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(pct))
                                .bg(if status == slackless_core::BackfillStatus::Complete {
                                    success
                                } else {
                                    accent
                                }),
                        ),
                )
                .child(controls);

            let channel_rows: Vec<_> = progress
                .as_ref()
                .map(|p| {
                    p.channels
                        .iter()
                        .enumerate()
                        .map(|(ix, ch)| {
                            let ch_id = ch.channel.clone();
                            let ch_name = ch
                                .channel_name
                                .clone()
                                .unwrap_or_else(|| ch.channel.to_string());
                            let ch_status_color = match ch.status {
                                slackless_core::BackfillStatus::Running => accent,
                                slackless_core::BackfillStatus::Complete => success,
                                slackless_core::BackfillStatus::Error => danger,
                                _ => muted,
                            };
                            let detail = if let Some(err) = &ch.last_error {
                                format!("Error: {err}")
                            } else {
                                format!(
                                    "{} msgs · {} replies",
                                    ch.messages_stored, ch.replies_stored
                                )
                            };
                            let this_row = this.clone();
                            h_flex()
                                .id(("storage-bf-ch", ix))
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .px_2p5()
                                .py_1p5()
                                .border_b_1()
                                .border_color(border.opacity(0.6))
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(fg)
                                                .child(ch_name),
                                        )
                                        .child(
                                            div()
                                                .truncate()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(detail),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .flex_none()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(ch_status_color.opacity(0.14))
                                                .text_xs()
                                                .text_color(ch_status_color)
                                                .child(ch.status.label()),
                                        )
                                        .child(
                                            Button::new(("storage-bf-row-go", ix))
                                                .ghost()
                                                .with_size(Size::XSmall)
                                                .icon(IconName::RotateCcw)
                                                .tooltip("Backfill this channel")
                                                .disabled(sample)
                                                .on_click(move |_, window, cx| {
                                                    let _ = this_row.update(cx, |app, cx| {
                                                        app.start_channel_backfill(
                                                            ch_id.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }),
                                        ),
                                )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let maintenance = h_flex()
                .items_center()
                .justify_between()
                .flex_wrap()
                .gap_2()
                .pt_2()
                .border_t_1()
                .border_color(border)
                .child(
                    h_flex()
                        .items_center()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            preferences::form_button(
                                Button::new("storage-dlg-compact")
                                    .outline()
                                    .label("Compact SQLite"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let _ = this_compact
                                    .update(cx, |app, cx| app.compact_storage(window, cx));
                            }),
                        )
                        .child(
                            preferences::form_button(
                                Button::new("storage-dlg-prune")
                                    .outline()
                                    .label("Prune media cache"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let _ = this_prune
                                    .update(cx, |app, cx| app.prune_media_cache(window, cx));
                            }),
                        ),
                )
                .child(
                    preferences::form_button(
                        Button::new("storage-dlg-reset")
                            .danger()
                            .label("Clear local cache"),
                        cx,
                    )
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = this_reset
                            .update(cx, |app, cx| app.confirm_reset_cache(window, cx));
                    }),
                );

            let max_body_h = (window.viewport_size().height * 0.78).max(px(360.));
            dialog.title("Archive & Storage").w(px(620.)).child(
                v_flex()
                    .id("storage-dialog-body")
                    .max_h(max_body_h)
                    .overflow_y_scroll()
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .gap_3()
                    .child(stats_card)
                    .child(backfill_card)
                    .when(!channel_rows.is_empty(), |col| {
                        col.child(
                            v_flex()
                                .id("storage-bf-channel-list")
                                .max_h(px(240.))
                                .overflow_y_scroll()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .children(channel_rows),
                        )
                    })
                    .child(maintenance),
            )
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
        self.export_channel_transcript(self.channel.clone(), markdown, window, cx);
    }

    /// Export any channel's locally archived messages using `slackless_core::export` (with
    /// fixture-mode fallback so sample workspaces export identically).
    pub(in crate::app) fn export_channel_transcript(
        &self,
        channel: ChannelId,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.label(&channel);
        let fmt = if markdown {
            slackless_core::export::ExportFormat::Markdown
        } else {
            slackless_core::export::ExportFormat::Json
        };
        let stem = slackless_core::export::sanitize_export_filename(&label, channel.as_str());
        let suggested = format!("{stem}-export.{}", fmt.extension());
        let start_dir = self.default_export_dir(cx);
        let fixtures = self.fixtures.clone();
        let workspace_index = self.workspace;
        let store = self.bridge.read_store_clone();
        let team = self.bridge.team_id().cloned();
        let include_deleted = self.preferences.read(cx).settings().show_deleted_messages;
        let rx = cx.prompt_for_new_path(&start_dir, Some(&suggested));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let result: std::io::Result<(usize, std::path::PathBuf)> = cx
                .background_executor()
                .spawn(async move {
                    let opts = slackless_core::export::ExportOptions {
                        format: fmt,
                        limit: 10_000,
                        include_deleted,
                        include_threads: true,
                        include_transcripts: true,
                    };
                    let (count, body) = if let (Some(store), Some(team)) = (store, team) {
                        let artifact = store
                            .lock()
                            .map_err(|_| std::io::Error::other("local store unavailable"))?
                            .export_channel_via_core(&team, &channel, &opts)
                            .map_err(std::io::Error::other)?;
                        (artifact.item_count, artifact.content)
                    } else {
                        let doc = build_fixture_channel_export(
                            &fixtures,
                            workspace_index,
                            &channel,
                            &label,
                        );
                        let content = match fmt {
                            slackless_core::export::ExportFormat::Markdown => {
                                slackless_core::export::render_messages_markdown(&doc)
                            }
                            slackless_core::export::ExportFormat::Json => {
                                slackless_core::export::render_messages_json(&doc)
                                    .map_err(std::io::Error::other)?
                            }
                        };
                        (doc.count, content)
                    };
                    std::fs::write(&path, body).map(|()| (count, path))
                })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok((count, path)) => notify(
                    "Channel exported",
                    format!("Saved {count} messages to {}", path.display()),
                    window,
                    cx,
                ),
                Err(err) => notify("Export failed", err.to_string(), window, cx),
            });
        })
        .detach();
    }

    /// Export a single thread (root + replies) as Markdown or JSON using `slackless_core::export`.
    pub(in crate::app) fn export_thread_transcript(
        &self,
        channel: ChannelId,
        thread_ts: Ts,
        markdown: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let label = self.label(&channel);
        let fmt = if markdown {
            slackless_core::export::ExportFormat::Markdown
        } else {
            slackless_core::export::ExportFormat::Json
        };
        let stem = slackless_core::export::sanitize_export_filename(&label, channel.as_str());
        let ts_slug =
            slackless_core::export::sanitize_export_filename(thread_ts.as_str(), "thread");
        let suggested = format!("{stem}-thread-{ts_slug}.{}", fmt.extension());
        let start_dir = self.default_export_dir(cx);
        let fixtures = self.fixtures.clone();
        let workspace_index = self.workspace;
        let store = self.bridge.read_store_clone();
        let team = self.bridge.team_id().cloned();
        let include_deleted = self.preferences.read(cx).settings().show_deleted_messages;
        let rx = cx.prompt_for_new_path(&start_dir, Some(&suggested));
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            let result: std::io::Result<(usize, std::path::PathBuf)> = cx
                .background_executor()
                .spawn(async move {
                    let opts = slackless_core::export::ExportOptions {
                        format: fmt,
                        limit: 10_000,
                        include_deleted,
                        include_threads: false,
                        include_transcripts: true,
                    };
                    let (count, body) = if let (Some(store), Some(team)) = (store, team) {
                        let guard = store
                            .lock()
                            .map_err(|_| std::io::Error::other("local store unavailable"))?;
                        let artifact = slackless_core::export::export_thread(
                            &guard, &team, &channel, &thread_ts, &opts,
                        )
                        .map_err(std::io::Error::other)?;
                        (artifact.item_count, artifact.content)
                    } else {
                        let doc = build_fixture_thread_export(
                            &fixtures,
                            workspace_index,
                            &channel,
                            &label,
                            &thread_ts,
                        );
                        let content = match fmt {
                            slackless_core::export::ExportFormat::Markdown => {
                                slackless_core::export::render_messages_markdown(&doc)
                            }
                            slackless_core::export::ExportFormat::Json => {
                                slackless_core::export::render_messages_json(&doc)
                                    .map_err(std::io::Error::other)?
                            }
                        };
                        (doc.count, content)
                    };
                    std::fs::write(&path, body).map(|()| (count, path))
                })
                .await;
            let _ = cx.update(|window, cx| match result {
                Ok((count, path)) => notify(
                    "Thread exported",
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

trait StoreExportExt {
    fn export_channel_via_core(
        &self,
        team: &slackless_core::types::TeamId,
        channel: &ChannelId,
        opts: &slackless_core::export::ExportOptions,
    ) -> slackless_core::store::Result<slackless_core::export::ExportArtifact>;
}

impl StoreExportExt for slackless_core::store::Store {
    fn export_channel_via_core(
        &self,
        team: &slackless_core::types::TeamId,
        channel: &ChannelId,
        opts: &slackless_core::export::ExportOptions,
    ) -> slackless_core::store::Result<slackless_core::export::ExportArtifact> {
        slackless_core::export::export_channel(self, team, channel, opts)
    }
}

fn collect_storage_info(
    store: &SharedStore,
    team: &slackless_core::types::TeamId,
) -> Option<crate::preferences::StorageInfo> {
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
    let stats = store.lock().ok()?.stats(team).ok()?;
    let mut media_cache_bytes = dir_bytes(&std::env::temp_dir().join("slackless-media"))
        .saturating_add(dir_bytes(&std::env::temp_dir().join("slackless-audio")));
    if let Some(cache) = slackless_core::Fetcher::disk_cache_dir() {
        media_cache_bytes = media_cache_bytes.saturating_add(dir_bytes(&cache));
    }
    Some(crate::preferences::StorageInfo {
        messages: stats.messages,
        revisions: stats.revisions,
        conversations: stats.conversations,
        users: stats.users,
        offline_queued: stats.offline_queued,
        db_bytes: stats.db_bytes(),
        media_cache_bytes,
    })
}

pub(in crate::app) fn build_fixture_channel_export(
    fixtures: &Fixtures,
    workspace_index: usize,
    channel: &ChannelId,
    label: &str,
) -> slackless_core::export::ExportedMessagesDoc {
    let active_ws = fixtures
        .workspaces
        .get(workspace_index)
        .or_else(|| fixtures.workspaces.first());
    let ws = active_ws
        .map(|t| t.id.as_str().to_owned())
        .unwrap_or_default();
    let ws_name = active_ws
        .map(|t| t.name.clone())
        .filter(|n| !n.trim().is_empty());
    let mut users = HashMap::new();
    for u in &fixtures.users {
        users.insert(u.id.clone(), fixtures.name_of(u).to_owned());
    }
    let transcripts = &fixtures.file_transcripts;
    let messages: Vec<slackless_core::export::ExportedMessage> = fixtures
        .messages
        .get(channel)
        .into_iter()
        .flatten()
        .map(|msg| {
            let stored = slackless_core::store::StoredMessage {
                channel: channel.clone(),
                message: msg.clone(),
                deleted: false,
            };
            let mut row =
                slackless_core::export::build_exported_message(&stored, &users, transcripts, true);
            if let Some(replies) = fixtures.threads.get(&(channel.clone(), msg.ts.clone())) {
                for r in replies {
                    if r.ts == msg.ts {
                        continue;
                    }
                    let rs = slackless_core::store::StoredMessage {
                        channel: channel.clone(),
                        message: r.clone(),
                        deleted: false,
                    };
                    row.replies
                        .push(slackless_core::export::build_exported_message(
                            &rs,
                            &users,
                            transcripts,
                            true,
                        ));
                }
            }
            row
        })
        .collect();
    slackless_core::export::ExportedMessagesDoc {
        kind: "channel".to_owned(),
        workspace: ws,
        workspace_name: ws_name,
        channel: slackless_core::export::ExportedChannelMeta {
            id: channel.as_str().to_owned(),
            label: label.to_owned(),
        },
        thread_ts: None,
        count: messages.len(),
        messages,
    }
}

pub(in crate::app) fn build_fixture_thread_export(
    fixtures: &Fixtures,
    workspace_index: usize,
    channel: &ChannelId,
    label: &str,
    thread_ts: &Ts,
) -> slackless_core::export::ExportedMessagesDoc {
    let active_ws = fixtures
        .workspaces
        .get(workspace_index)
        .or_else(|| fixtures.workspaces.first());
    let ws = active_ws
        .map(|t| t.id.as_str().to_owned())
        .unwrap_or_default();
    let ws_name = active_ws
        .map(|t| t.name.clone())
        .filter(|n| !n.trim().is_empty());
    let mut users = HashMap::new();
    for u in &fixtures.users {
        users.insert(u.id.clone(), fixtures.name_of(u).to_owned());
    }
    let transcripts = &fixtures.file_transcripts;
    let mut raw: Vec<Message> = Vec::new();
    if let Some(root) = fixtures
        .messages
        .get(channel)
        .and_then(|list| list.iter().find(|m| m.ts == *thread_ts))
    {
        raw.push(root.clone());
    }
    if let Some(replies) = fixtures.threads.get(&(channel.clone(), thread_ts.clone())) {
        for r in replies {
            if !raw.iter().any(|m| m.ts == r.ts) {
                raw.push(r.clone());
            }
        }
    }
    let messages: Vec<slackless_core::export::ExportedMessage> = raw
        .into_iter()
        .map(|msg| {
            let stored = slackless_core::store::StoredMessage {
                channel: channel.clone(),
                message: msg,
                deleted: false,
            };
            slackless_core::export::build_exported_message(&stored, &users, transcripts, true)
        })
        .collect();
    slackless_core::export::ExportedMessagesDoc {
        kind: "thread".to_owned(),
        workspace: ws,
        workspace_name: ws_name,
        channel: slackless_core::export::ExportedChannelMeta {
            id: channel.as_str().to_owned(),
            label: label.to_owned(),
        },
        thread_ts: Some(thread_ts.as_str().to_owned()),
        count: messages.len(),
        messages,
    }
}

/// Fold accented Latin characters (including Swedish `å`/`ä`/`ö`) to ASCII lowercase for
/// diacritic-insensitive search-term highlighting inside transcript lines.
fn fold_search_char(c: char) -> char {
    match c.to_lowercase().next().unwrap_or(c) {
        'å' | 'ä' | 'à' | 'á' | 'â' | 'ã' => 'a',
        'ö' | 'ø' | 'ó' | 'ò' | 'ô' | 'õ' => 'o',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'ü' | 'ú' | 'ù' | 'û' => 'u',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        other => other,
    }
}

/// Render one transcript line as `StyledText`, highlighting any `query_terms` matches with a
/// subtle background tint while keeping every UTF-8 byte range strictly on char boundaries.
fn render_highlighted_transcript_line(
    text: &str,
    query_terms: &[Vec<char>],
    fg: gpui::Hsla,
    hl_bg: gpui::Hsla,
    font_family: &SharedString,
) -> gpui::StyledText {
    if query_terms.is_empty() || text.is_empty() {
        return gpui::StyledText::new(text.to_owned());
    }
    let chars: Vec<(usize, usize, char)> = text
        .char_indices()
        .map(|(byte_ix, ch)| (byte_ix, byte_ix + ch.len_utf8(), fold_search_char(ch)))
        .collect();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in query_terms {
        let tlen = term.len();
        if tlen == 0 || tlen > chars.len() {
            continue;
        }
        for i in 0..=(chars.len() - tlen) {
            if chars[i..i + tlen]
                .iter()
                .zip(term.iter())
                .all(|((_, _, c), q)| c == q)
            {
                ranges.push((chars[i].0, chars[i + tlen - 1].1));
            }
        }
    }
    if ranges.is_empty() {
        return gpui::StyledText::new(text.to_owned());
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (s, e) in ranges {
        if let Some(last) = merged.last_mut()
            && s <= last.1
        {
            last.1 = last.1.max(e);
        } else {
            merged.push((s, e));
        }
    }
    let mut runs = Vec::with_capacity(merged.len() * 2 + 1);
    let mut cursor = 0usize;
    let make_run = |len: usize, bg: Option<gpui::Hsla>| gpui::TextRun {
        len,
        font: gpui::Font {
            family: font_family.clone(),
            ..Default::default()
        },
        color: fg,
        background_color: bg,
        underline: None,
        strikethrough: None,
    };
    for (s, e) in merged {
        if s > cursor {
            runs.push(make_run(s - cursor, None));
        }
        if e > s {
            runs.push(make_run(e - s, Some(hl_bg)));
        }
        cursor = e;
    }
    if cursor < text.len() {
        runs.push(make_run(text.len() - cursor, None));
    }
    gpui::StyledText::new(text.to_owned()).with_runs(runs)
}
