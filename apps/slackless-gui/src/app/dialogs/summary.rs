//! Local / BYO-key AI Summary dialog (`SummaryDialogView`) and `App` entry points for
//! summarizing threads, channels, Catch-Up briefings, and video/audio transcripts.

use std::sync::atomic::{AtomicU64, Ordering};

use gpui_kit::gpui::WeakEntity;
use slackless_core::summarize::{AiProvider, SummaryMessage, SummaryResult, SummaryTarget};
use slackless_core::types::{FileId, TranscriptCue};

use super::*;

/// Disjoint selection document order for selectable summary text inside the dialog.
const SUMMARY_DIALOG_ORDER: u64 = (1u64 << 62) + 1_000_000;

/// Monotonic counter guarding async summary completions against stale writes.
static SUMMARY_GEN: AtomicU64 = AtomicU64::new(1);

/// Current UI state of [`SummaryDialogView`].
#[derive(Clone, Debug)]
pub(in crate::app) enum SummaryDialogState {
    /// AI summaries are disabled or missing endpoint/model configuration.
    NotConfigured {
        target_title: String,
        reason: String,
    },
    /// Summary request is in flight off the UI thread.
    Loading {
        target_title: String,
        provider: AiProvider,
        model: String,
        source_items: usize,
    },
    /// Summary completed successfully.
    Ready(SummaryResult),
    /// Summary failed (network, HTTP, auth, or empty input).
    Error {
        target_title: String,
        provider: AiProvider,
        model: String,
        message: String,
    },
}

/// Reactive GPUI dialog card presenting an AI summary with selectable text, Copy button,
/// provider/model pill, and a "Configure in Preferences" CTA when unconfigured or errored.
pub(in crate::app) struct SummaryDialogView {
    app: WeakEntity<App>,
    request: SummaryRequest,
    state: SummaryDialogState,
    generation: u64,
    copied: bool,
}

impl SummaryDialogView {
    pub(in crate::app) fn new(
        app: WeakEntity<App>,
        request: SummaryRequest,
        state: SummaryDialogState,
        generation: u64,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        cx.new(|_| Self {
            app,
            request,
            state,
            generation,
            copied: false,
        })
    }

    pub(in crate::app) fn apply_result(
        &mut self,
        generation: u64,
        result: Result<SummaryResult, String>,
        provider: AiProvider,
        model: String,
        cx: &mut Context<Self>,
    ) {
        if self.generation != generation {
            return;
        }
        self.copied = false;
        self.state = match result {
            Ok(res) => SummaryDialogState::Ready(res),
            Err(message) => SummaryDialogState::Error {
                target_title: self.request.display_title(),
                provider,
                model,
                message,
            },
        };
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::app) fn state(&self) -> &SummaryDialogState {
        &self.state
    }
}

impl Render for SummaryDialogView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted, border, secondary, primary, danger) = (
            theme.foreground,
            theme.muted_foreground,
            theme.border,
            theme.secondary,
            theme.primary,
            theme.danger,
        );

        let (title, pill): (String, Option<String>) = match &self.state {
            SummaryDialogState::NotConfigured { target_title, .. } => (target_title.clone(), None),
            SummaryDialogState::Loading {
                target_title,
                provider,
                model,
                ..
            } => (
                target_title.clone(),
                Some(format!("{} · {model}", provider.short_label())),
            ),
            SummaryDialogState::Ready(res) => (
                res.title.clone(),
                Some(format!("{} · {}", res.provider.short_label(), res.model)),
            ),
            SummaryDialogState::Error {
                target_title,
                provider,
                model,
                ..
            } => (
                target_title.clone(),
                Some(format!("{} · {model}", provider.short_label())),
            ),
        };

        let header = h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap_3()
            .pb_2p5()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(28.))
                            .flex_none()
                            .rounded_md()
                            .bg(primary.opacity(0.14))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Sparkles)
                                    .size(px(15.))
                                    .text_color(primary),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(fg)
                                    .truncate()
                                    .child(format!("Summary · {title}")),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Local / BYO-key AI summary"),
                            ),
                    ),
            )
            .when_some(pill, |row, pill_text| {
                row.child(
                    div()
                        .id("summary-provider-pill")
                        .flex_none()
                        .max_w(px(220.))
                        .truncate()
                        .px_2()
                        .py(px(3.))
                        .rounded_full()
                        .border_1()
                        .border_color(border)
                        .bg(secondary.opacity(0.6))
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(fg)
                        .child(pill_text),
                )
            });

        let app_prefs = self.app.clone();
        let app_retry = self.app.clone();
        let retry_request = self.request.clone();

        let (body, footer): (AnyElement, AnyElement) = match &self.state {
            SummaryDialogState::NotConfigured { reason, .. } => {
                let body = v_flex()
                    .gap_2p5()
                    .py_3()
                    .px_3p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(secondary.opacity(0.35))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child("AI summaries are not configured yet"),
                    )
                    .child(div().text_sm().text_color(muted).child(reason.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Point Slackless at local Ollama (http://localhost:11434/v1), LM Studio, OpenAI, or any OpenAI-compatible server in Preferences -> Advanced."),
                    )
                    .into_any_element();

                let footer = h_flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("summary-close-unconfigured")
                            .ghost()
                            .with_size(Size::Small)
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("summary-open-prefs")
                            .primary()
                            .with_size(Size::Small)
                            .icon(IconName::SlidersHorizontal)
                            .label("Configure in Preferences")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = app_prefs.update(cx, |app, cx| {
                                    app.open_preferences(Some(Section::Advanced), window, cx);
                                });
                            }),
                    )
                    .into_any_element();
                (body, footer)
            }
            SummaryDialogState::Loading {
                provider,
                model,
                source_items,
                ..
            } => {
                let item_note = if *source_items > 0 {
                    format!(
                        "Summarizing {source_items} item(s) with {} ({model})\u{2026}",
                        provider.short_label()
                    )
                } else {
                    format!(
                        "Generating summary with {} ({model})\u{2026}",
                        provider.short_label()
                    )
                };
                let body = v_flex()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .py_8()
                    .px_4()
                    .child(
                        Icon::new(IconName::Sparkles)
                            .size(px(22.))
                            .text_color(primary),
                    )
                    .child(div().text_sm().text_color(fg).child(item_note))
                    .child(div().text_xs().text_color(muted).child(
                        "Running off the UI thread — you can keep browsing or close this dialog.",
                    ))
                    .into_any_element();

                let footer = h_flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("summary-cancel-loading")
                            .outline()
                            .with_size(Size::Small)
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .into_any_element();
                (body, footer)
            }
            SummaryDialogState::Ready(res) => {
                let summary_text = res.summary.clone();
                let copy_text = res.summary.clone();
                let copied = self.copied;
                let mut meta_parts = vec![format!("{} item(s)", res.source_items)];
                if let Some(tokens) = res.total_tokens {
                    meta_parts.push(format!("{tokens} tokens"));
                }
                meta_parts.push(format!("{} ms", res.elapsed_ms));
                if res.truncated {
                    meta_parts.push("input truncated to fit budget".to_owned());
                }
                let meta_line = meta_parts.join(" · ");

                let body = v_flex()
                    .gap_2()
                    .child(
                        div()
                            .id("summary-body-scroll")
                            .w_full()
                            .max_h(px(380.))
                            .overflow_y_scroll()
                            .px_3p5()
                            .py_3()
                            .rounded_md()
                            .border_1()
                            .border_color(border)
                            .bg(secondary.opacity(0.28))
                            .text_sm()
                            .line_height(px(21.))
                            .text_color(fg)
                            .child(crate::messages::selectable_text(
                                "summary-dialog-text",
                                0,
                                summary_text,
                                Some(fg),
                                SUMMARY_DIALOG_ORDER,
                                cx,
                            )),
                    )
                    .child(div().text_xs().text_color(muted).child(meta_line))
                    .into_any_element();

                let footer = h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("summary-copy")
                                    .outline()
                                    .with_size(Size::Small)
                                    .icon(if copied {
                                        IconName::Check
                                    } else {
                                        IconName::Copy
                                    })
                                    .label(if copied { "Copied" } else { "Copy" })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy_text.clone(),
                                        ));
                                        this.copied = true;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("summary-regenerate")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .icon(IconName::RefreshCw)
                                    .label("Regenerate")
                                    .on_click(move |_, window, cx| {
                                        let _ = app_retry.update(cx, |app, cx| {
                                            app.run_summary_request(
                                                retry_request.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("summary-prefs-link")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .label("Preferences")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        let _ = app_prefs.update(cx, |app, cx| {
                                            app.open_preferences(
                                                Some(Section::Advanced),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .child(
                                Button::new("summary-close-ready")
                                    .primary()
                                    .with_size(Size::Small)
                                    .label("Done")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                    )
                    .into_any_element();
                (body, footer)
            }
            SummaryDialogState::Error { message, .. } => {
                let body = v_flex()
                    .gap_2()
                    .py_3()
                    .px_3p5()
                    .rounded_md()
                    .border_1()
                    .border_color(danger.opacity(0.45))
                    .bg(danger.opacity(0.08))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::TriangleAlert)
                                    .size(px(15.))
                                    .text_color(danger),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(danger)
                                    .child("Could not generate summary"),
                            ),
                    )
                    .child(div().text_sm().text_color(fg).child(message.clone()))
                    .into_any_element();

                let footer = h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .pt_2()
                    .child(
                        Button::new("summary-error-prefs")
                            .outline()
                            .with_size(Size::Small)
                            .icon(IconName::SlidersHorizontal)
                            .label("Configure in Preferences")
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = app_prefs.update(cx, |app, cx| {
                                    app.open_preferences(Some(Section::Advanced), window, cx);
                                });
                            }),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("summary-error-retry")
                                    .primary()
                                    .with_size(Size::Small)
                                    .icon(IconName::RefreshCw)
                                    .label("Retry")
                                    .on_click(move |_, window, cx| {
                                        let _ = app_retry.update(cx, |app, cx| {
                                            app.run_summary_request(
                                                retry_request.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                    }),
                            )
                            .child(
                                Button::new("summary-error-close")
                                    .ghost()
                                    .with_size(Size::Small)
                                    .label("Close")
                                    .on_click(|_, window, cx| window.close_dialog(cx)),
                            ),
                    )
                    .into_any_element();
                (body, footer)
            }
        };

        v_flex()
            .w_full()
            .min_w_0()
            .gap_3()
            .child(header)
            .child(body)
            .child(footer)
    }
}

fn fixture_message_to_summary(
    fixtures: &Fixtures,
    msg: &Message,
    is_thread_root: bool,
) -> Option<SummaryMessage> {
    let text = slackless_core::domain::mrkdwn::mrkdwn_to_plain(&msg.text)
        .trim()
        .to_owned();
    if text.is_empty() {
        return None;
    }
    let author = msg
        .user
        .as_ref()
        .map(|u| fixtures.user_name(u).to_owned())
        .or_else(|| {
            msg.username
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .or_else(|| {
            msg.bot_profile
                .as_ref()
                .map(|bp| bp.name.clone())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "Unknown".to_owned());
    Some(SummaryMessage {
        author,
        timestamp: msg.ts.secs().map(|secs| crate::messages::clock(secs, None)),
        text,
        is_thread_root,
    })
}

/// Specification of what to summarize: either an already-materialized [`SummaryTarget`] or a
/// lazy live SQLite store loader run off the UI thread so channel/thread reads never block frames.
#[derive(Clone)]
pub(in crate::app) enum SummaryRequest {
    Direct(SummaryTarget),
    LiveChannel {
        store: SharedStore,
        workspace: slackless_core::types::TeamId,
        channel: ChannelId,
        channel_label: String,
    },
    LiveThread {
        store: SharedStore,
        workspace: slackless_core::types::TeamId,
        channel: ChannelId,
        thread_ts: Ts,
        channel_label: String,
    },
}

impl SummaryRequest {
    fn display_title(&self) -> String {
        match self {
            Self::Direct(target) => target.display_title(),
            Self::LiveChannel { channel_label, .. } => channel_label.clone(),
            Self::LiveThread { channel_label, .. } => format!("Thread in {channel_label}"),
        }
    }

    fn known_item_count(&self) -> usize {
        match self {
            Self::Direct(SummaryTarget::Thread { messages, .. })
            | Self::Direct(SummaryTarget::Channel { messages, .. }) => messages.len(),
            Self::Direct(SummaryTarget::CatchUp { briefing }) => {
                briefing.mentions.len()
                    + briefing.unread_channels.len()
                    + briefing.active_threads.len()
                    + briefing.transcripts.len()
            }
            Self::Direct(SummaryTarget::Transcript { cues, text, .. }) => {
                if !cues.is_empty() {
                    cues.len()
                } else if !text.trim().is_empty() {
                    1
                } else {
                    0
                }
            }
            Self::LiveChannel { .. } | Self::LiveThread { .. } => 0,
        }
    }

    fn resolve(self) -> Result<SummaryTarget, String> {
        match self {
            Self::Direct(target) => Ok(target),
            Self::LiveChannel {
                store,
                workspace,
                channel,
                ..
            } => {
                let guard = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                slackless_core::summarize::load_channel_target(
                    &guard,
                    &workspace,
                    &channel,
                    slackless_core::summarize::DEFAULT_SUMMARY_MESSAGE_LIMIT,
                )
                .map_err(|e| e.to_string())
            }
            Self::LiveThread {
                store,
                workspace,
                channel,
                thread_ts,
                ..
            } => {
                let guard = store
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                slackless_core::summarize::load_thread_target(
                    &guard, &workspace, &channel, &thread_ts,
                )
                .map_err(|e| e.to_string())
            }
        }
    }
}

impl App {
    /// Summarize recent messages in the currently open conversation.
    pub(in crate::app) fn open_summary_for_channel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel = self.channel.clone();
        let conv = self.fixtures.conversation(&channel);
        let channel_label = conv
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| channel.as_str().to_owned());

        if let Some(store) = self.bridge.store()
            && let Some(workspace) = self
                .fixtures
                .workspaces
                .get(self.workspace)
                .map(|t| t.id.clone())
        {
            self.run_summary_request(
                SummaryRequest::LiveChannel {
                    store: store.clone(),
                    workspace,
                    channel,
                    channel_label,
                },
                window,
                cx,
            );
            return;
        }

        let topic = conv
            .and_then(|c| c.topic.as_ref())
            .map(|t| t.value.trim().to_owned())
            .filter(|s| !s.is_empty());
        let messages: Vec<SummaryMessage> = self
            .fixtures
            .messages
            .get(&channel)
            .map(|list| {
                list.iter()
                    .rev()
                    .take(slackless_core::summarize::DEFAULT_SUMMARY_MESSAGE_LIMIT)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .filter_map(|m| fixture_message_to_summary(&self.fixtures, m, false))
                    .collect()
            })
            .unwrap_or_default();

        self.run_summary_request(
            SummaryRequest::Direct(SummaryTarget::Channel {
                channel_label,
                topic,
                messages,
            }),
            window,
            cx,
        );
    }

    /// Summarize an open thread (root + replies).
    pub(in crate::app) fn open_summary_for_thread(
        &mut self,
        channel: ChannelId,
        thread_ts: Ts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let channel_label = self
            .fixtures
            .conversation(&channel)
            .map(|c| self.fixtures.conversation_label(c))
            .unwrap_or_else(|| channel.as_str().to_owned());

        if let Some(store) = self.bridge.store()
            && let Some(workspace) = self
                .fixtures
                .workspaces
                .get(self.workspace)
                .map(|t| t.id.clone())
        {
            self.run_summary_request(
                SummaryRequest::LiveThread {
                    store: store.clone(),
                    workspace,
                    channel,
                    thread_ts,
                    channel_label,
                },
                window,
                cx,
            );
            return;
        }

        let mut combined = Vec::new();
        if let Some(root) = self
            .fixtures
            .messages
            .get(&channel)
            .and_then(|list| list.iter().find(|m| m.ts == thread_ts))
        {
            combined.push(root.clone());
        }
        if let Some(replies) = self
            .fixtures
            .threads
            .get(&(channel.clone(), thread_ts.clone()))
        {
            for r in replies {
                if r.ts != thread_ts {
                    combined.push(r.clone());
                }
            }
        }
        let messages: Vec<SummaryMessage> = combined
            .iter()
            .filter_map(|m| fixture_message_to_summary(&self.fixtures, m, m.ts == thread_ts))
            .collect();

        self.run_summary_request(
            SummaryRequest::Direct(SummaryTarget::Thread {
                channel_label,
                thread_ts: Some(thread_ts.as_str().to_owned()),
                messages,
            }),
            window,
            cx,
        );
    }

    /// Summarize the currently loaded local Catch-Up briefing.
    pub(in crate::app) fn open_summary_for_catchup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let briefing = self.catchup.read(cx).current_briefing();
        self.run_summary_request(
            SummaryRequest::Direct(SummaryTarget::CatchUp { briefing }),
            window,
            cx,
        );
    }

    /// Summarize a video/audio transcript (with timestamped highlights when cues exist).
    pub(in crate::app) fn open_summary_for_transcript(
        &mut self,
        file_id: FileId,
        title: String,
        text: String,
        cues: Vec<TranscriptCue>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_summary_request(
            SummaryRequest::Direct(SummaryTarget::Transcript {
                title,
                file_id: Some(file_id.as_str().to_owned()),
                text,
                cues,
            }),
            window,
            cx,
        );
    }

    /// Core dialog launcher shared by all 4 summary entry points.
    pub(in crate::app) fn run_summary_request(
        &mut self,
        request: SummaryRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ai_settings = self.preferences.read(cx).settings().ai_summary.clone();
        let target_title = request.display_title();
        let generation = SUMMARY_GEN.fetch_add(1, Ordering::Relaxed);
        let app_weak = cx.weak_entity();

        if !ai_settings.enabled {
            let view = SummaryDialogView::new(
                app_weak,
                request,
                SummaryDialogState::NotConfigured {
                    target_title,
                    reason: "Enable AI summaries in Preferences -> Advanced to summarize locally or with your own API key.".to_owned(),
                },
                generation,
                cx,
            );
            window.open_dialog(cx, move |dialog, _, _| {
                dialog
                    .w(px(580.))
                    .overlay_closable(true)
                    .child(view.clone())
            });
            return;
        }

        let eff_url = ai_settings.effective_base_url().to_owned();
        let eff_model = ai_settings.effective_model().to_owned();
        if eff_url.is_empty() || eff_model.is_empty() {
            let view = SummaryDialogView::new(
                app_weak,
                request,
                SummaryDialogState::NotConfigured {
                    target_title,
                    reason: "Configure an OpenAI-compatible endpoint URL and model name in Preferences -> Advanced.".to_owned(),
                },
                generation,
                cx,
            );
            window.open_dialog(cx, move |dialog, _, _| {
                dialog
                    .w(px(580.))
                    .overlay_closable(true)
                    .child(view.clone())
            });
            return;
        }

        let source_items = request.known_item_count();
        let provider = ai_settings.provider;
        let view = SummaryDialogView::new(
            app_weak,
            request.clone(),
            SummaryDialogState::Loading {
                target_title,
                provider,
                model: eff_model.clone(),
                source_items,
            },
            generation,
            cx,
        );
        let view_weak = view.downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(620.))
                .overlay_closable(true)
                .child(view.clone())
        });

        cx.spawn(async move |_, cx| {
            let result: Result<SummaryResult, String> = cx
                .background_executor()
                .spawn(async move {
                    let target = request.resolve()?;
                    let api_key = slackless_core::auth::SecretStore::new()
                        .ok()
                        .and_then(|store| {
                            store
                                .get(slackless_core::auth::AI_SUMMARY_API_KEY_ACCOUNT)
                                .ok()
                                .flatten()
                        });
                    let cfg = ai_settings.to_runtime_config(api_key);
                    slackless_core::summarize::summarize_blocking(&cfg, &target)
                        .map_err(|err| err.to_string())
                })
                .await;

            let _ = view_weak.update(cx, |dlg, cx| {
                dlg.apply_result(generation, result, provider, eff_model, cx);
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::gpui::TestAppContext;

    use super::*;

    #[gpui_kit::gpui::test]
    fn summary_dialog_transitions_loading_to_ready_and_ignores_stale_gen(cx: &mut TestAppContext) {
        let target = SummaryTarget::Channel {
            channel_label: "#engineering".to_owned(),
            topic: None,
            messages: vec![SummaryMessage {
                author: "Alice".to_owned(),
                timestamp: Some("10:00".to_owned()),
                text: "Shipped v2".to_owned(),
                is_thread_root: false,
            }],
        };

        let view = cx.update(|cx| {
            SummaryDialogView::new(
                WeakEntity::new_invalid(),
                SummaryRequest::Direct(target),
                SummaryDialogState::Loading {
                    target_title: "#engineering".to_owned(),
                    provider: AiProvider::Ollama,
                    model: "llama3.2".to_owned(),
                    source_items: 1,
                },
                7,
                cx,
            )
        });

        // Stale generation must be ignored.
        view.update(cx, |dlg, cx| {
            dlg.apply_result(
                6,
                Err("stale error".to_owned()),
                AiProvider::Ollama,
                "llama3.2".to_owned(),
                cx,
            );
        });
        view.read_with(cx, |dlg, _| {
            assert!(matches!(dlg.state(), SummaryDialogState::Loading { .. }));
        });

        // Matching generation transitions cleanly to Ready.
        let ready = SummaryResult {
            summary: "Highlights:\n- Shipped v2".to_owned(),
            provider: AiProvider::Ollama,
            endpoint: "http://localhost:11434/v1".to_owned(),
            model: "llama3.2".to_owned(),
            target_kind: "channel".to_owned(),
            title: "#engineering".to_owned(),
            source_items: 1,
            truncated: false,
            prompt_tokens: Some(40),
            completion_tokens: Some(15),
            total_tokens: Some(55),
            elapsed_ms: 120,
        };
        view.update(cx, |dlg, cx| {
            dlg.apply_result(
                7,
                Ok(ready.clone()),
                AiProvider::Ollama,
                "llama3.2".to_owned(),
                cx,
            );
        });
        view.read_with(cx, |dlg, _| match dlg.state() {
            SummaryDialogState::Ready(res) => {
                assert_eq!(res.summary, "Highlights:\n- Shipped v2");
                assert_eq!(res.model, "llama3.2");
            }
            other => panic!("expected Ready, got {other:?}"),
        });
    }

    #[gpui_kit::gpui::test]
    fn summary_dialog_transitions_loading_to_error(cx: &mut TestAppContext) {
        let target = SummaryTarget::Transcript {
            title: "demo.mp4".to_owned(),
            file_id: Some("F1".to_owned()),
            text: "Hello world".to_owned(),
            cues: Vec::new(),
        };
        let view = cx.update(|cx| {
            SummaryDialogView::new(
                WeakEntity::new_invalid(),
                SummaryRequest::Direct(target),
                SummaryDialogState::Loading {
                    target_title: "Transcript · demo.mp4".to_owned(),
                    provider: AiProvider::OpenAi,
                    model: "gpt-4o-mini".to_owned(),
                    source_items: 1,
                },
                9,
                cx,
            )
        });
        view.update(cx, |dlg, cx| {
            dlg.apply_result(
                9,
                Err("endpoint returned HTTP 401: Invalid API key".to_owned()),
                AiProvider::OpenAi,
                "gpt-4o-mini".to_owned(),
                cx,
            );
        });
        view.read_with(cx, |dlg, _| match dlg.state() {
            SummaryDialogState::Error {
                provider,
                model,
                message,
                ..
            } => {
                assert_eq!(*provider, AiProvider::OpenAi);
                assert_eq!(model, "gpt-4o-mini");
                assert!(message.contains("401"));
            }
            other => panic!("expected Error, got {other:?}"),
        });
    }
}
