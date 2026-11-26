//! Auto-updater controls/banner, connection status banner, and failed offline mutations banner.

use super::super::*;

impl App {
    fn sync_update_ui(&mut self, cx: &mut Context<Self>) {
        let state = self.update_state.clone();
        let available = self.available_update.clone();
        self.preferences
            .update(cx, |p, cx| p.set_update_state(state, available, cx));
        self.update_menus(cx);
        cx.notify();
    }

    /// Check GitHub Releases (`ankarhem/slackless`) for a newer `YYYY.MM.DD-xx` macOS build.
    /// Silent when `manual == false`; surfaces "up to date" and errors when triggered via
    /// `Slackless -> Check for Updates…` or Preferences -> Advanced.
    pub(in crate::app) fn check_for_updates(&mut self, manual: bool, cx: &mut Context<Self>) {
        use crate::updater::UpdateState;
        if matches!(
            self.update_state,
            UpdateState::Checking | UpdateState::Downloading(_)
        ) {
            return;
        }
        if manual {
            self.update_dismissed = None;
            self.update_state = UpdateState::Checking;
            self.sync_update_ui(cx);
        }
        let channel = self.preferences.read(cx).settings().update_channel();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::updater::check_for_update_on(channel) })
                .await;
            let _ = this.update(cx, |app, cx| {
                match result {
                    Ok(Some(release)) => {
                        app.available_update = Some(release.clone());
                        if !manual && app.update_dismissed.as_deref() == Some(&release.version) {
                            app.sync_update_ui(cx);
                            return;
                        }
                        app.update_state = UpdateState::Available(release);
                    }
                    Ok(None) => {
                        app.available_update = None;
                        app.update_state = if manual {
                            UpdateState::UpToDate
                        } else {
                            UpdateState::Idle
                        };
                        if manual {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(Duration::from_secs(5)).await;
                                let _ = this.update(cx, |app, cx| {
                                    if matches!(app.update_state, UpdateState::UpToDate) {
                                        app.update_state = UpdateState::Idle;
                                        app.sync_update_ui(cx);
                                    }
                                });
                            })
                            .detach();
                        }
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "auto-update check failed");
                        app.update_state = if manual {
                            UpdateState::Error(err)
                        } else {
                            UpdateState::Idle
                        };
                    }
                }
                app.sync_update_ui(cx);
            });
        })
        .detach();
    }

    /// Download, verify SHA-256 and the code signature, swap the `.app` in-process (rolling back on
    /// failure), then relaunch once this app has quit (`F-SHELL-163`).
    pub(in crate::app) fn install_available_update(&mut self, cx: &mut Context<Self>) {
        use crate::updater::UpdateState;
        let Some(release) = (match &self.update_state {
            UpdateState::Available(r) => Some(r.clone()),
            _ => self.available_update.clone(),
        }) else {
            return;
        };
        let version = release.version.clone();
        self.update_state = UpdateState::Downloading(version);
        self.sync_update_ui(cx);
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move { crate::updater::install_update(&release) })
                .await;
            let _ = this.update(cx, |app, cx| match res {
                Ok(()) => {
                    cx.quit();
                }
                Err(err) => {
                    tracing::warn!(error = %err, "failed to install update");
                    app.update_state = UpdateState::Error(err);
                    app.sync_update_ui(cx);
                }
            });
        })
        .detach();
    }

    /// Slim banner shown at the top of the window when a newer GitHub Release is available
    /// (or while checking / downloading via `Slackless -> Check for Updates…`).
    pub(in crate::app) fn update_banner(&self, cx: &Context<Self>) -> Option<Div> {
        use crate::updater::{UpdateState, installed_version};
        let theme = cx.theme();
        let (primary, fg, muted, danger, success) = (
            theme.primary,
            theme.foreground,
            theme.muted_foreground,
            theme.danger,
            theme.success,
        );
        match &self.update_state {
            UpdateState::Idle => None,
            UpdateState::Checking => Some(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .py_1p5()
                    .bg(primary.opacity(0.12))
                    .border_b_1()
                    .border_color(primary.opacity(0.28))
                    .child(
                        Icon::new(IconName::RefreshCw)
                            .size(px(13.))
                            .text_color(primary),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(fg)
                            .child("Checking for Slackless updates…"),
                    ),
            ),
            UpdateState::UpToDate => Some(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .py_1p5()
                    .bg(success.opacity(0.14))
                    .border_b_1()
                    .border_color(success.opacity(0.30))
                    .child(Icon::new(IconName::Check).size(px(13.)).text_color(success))
                    .child(
                        div()
                            .text_xs()
                            .text_color(fg)
                            .child(format!("Slackless {} is up to date.", installed_version())),
                    )
                    .child(
                        Button::new("update-banner-dismiss")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_state = UpdateState::Idle;
                                cx.notify();
                            })),
                    ),
            ),
            UpdateState::Downloading(ver) => Some(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .py_1p5()
                    .bg(primary.opacity(0.16))
                    .border_b_1()
                    .border_color(primary.opacity(0.32))
                    .child(
                        Icon::new(IconName::Download)
                            .size(px(13.))
                            .text_color(primary),
                    )
                    .child(div().text_xs().text_color(fg).child(format!(
                        "Downloading and verifying Slackless {ver}… will restart automatically."
                    ))),
            ),
            UpdateState::Available(rel) => {
                let ver = rel.version.clone();
                let banner_text = if rel.is_prerelease {
                    format!("Slackless {ver} (Beta) is available")
                } else {
                    format!("Slackless {ver} is available")
                };
                let notes_url = rel.html_url.clone();
                let dismiss_ver = ver.clone();
                Some(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .gap_2p5()
                        .px_4()
                        .py_1p5()
                        .bg(primary.opacity(0.16))
                        .border_b_1()
                        .border_color(primary.opacity(0.32))
                        .child(
                            Icon::new(IconName::Download)
                                .size(px(14.))
                                .text_color(primary),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(fg)
                                .child(banner_text),
                        )
                        .child(
                            Button::new("update-banner-install")
                                .primary()
                                .with_size(Size::XSmall)
                                .label("Update & Restart")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.install_available_update(cx);
                                })),
                        )
                        .child(
                            Button::new("update-banner-notes")
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("Release notes")
                                .on_click(move |_, _, cx| {
                                    cx.open_url(&notes_url);
                                }),
                        )
                        .child(
                            Button::new("update-banner-dismiss")
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::Close)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_dismissed = Some(dismiss_ver.clone());
                                    this.update_state = UpdateState::Idle;
                                    cx.notify();
                                })),
                        ),
                )
            }
            UpdateState::Error(msg) => Some(
                h_flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .px_4()
                    .py_1p5()
                    .bg(danger.opacity(0.14))
                    .border_b_1()
                    .border_color(danger.opacity(0.32))
                    .child(
                        Icon::new(IconName::TriangleAlert)
                            .size(px(13.))
                            .text_color(danger),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(fg)
                            .child(format!("Update check failed: {msg}")),
                    )
                    .child(
                        Button::new("update-banner-retry")
                            .outline()
                            .with_size(Size::XSmall)
                            .label("Retry")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.check_for_updates(true, cx);
                            })),
                    )
                    .child(
                        Button::new("update-banner-close-err")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::Close)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.update_state = UpdateState::Idle;
                                cx.notify();
                            })),
                    )
                    .child(div().text_xs().text_color(muted)),
            ),
        }
    }

    /// Offline / reconnecting notice at the top of the conversation column while Slack's RTM
    /// socket is dropped (`F-SHELL-169`). Also displays the number of queued outgoing mutations
    /// that will automatically flush on reconnect. Hidden once connected and drained.
    pub(in crate::app) fn connection_banner(&self, cx: &Context<Self>) -> Option<Div> {
        use slackless_core::rtm::ConnectionState;
        let queued = self.ws.offline_queued;
        let queue_suffix = if queued == 1 {
            " (1 queued change will send when reconnected)".to_owned()
        } else if queued > 1 {
            format!(" ({queued} queued changes will send when reconnected)")
        } else {
            String::new()
        };
        let text = match self.ws.connection.as_ref() {
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
    pub(in crate::app) fn failed_mutations_banner(&self, cx: &Context<Self>) -> Option<Div> {
        if self.ws.failed_mutations.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (danger, fg, muted) = (theme.danger, theme.foreground, theme.muted_foreground);
        let rows: Vec<_> =
            self.ws
                .failed_mutations
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
                                        this.ws.failed_mutations.retain(|m| m.id != retry_id);
                                        this.bridge
                                            .send(MessagingCommand::RetryOfflineMutation(retry_id));
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
                                        this.ws.failed_mutations.retain(|m| m.id != discard_id);
                                        this.bridge.send(MessagingCommand::DiscardOfflineMutation(
                                            discard_id,
                                        ));
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
}
