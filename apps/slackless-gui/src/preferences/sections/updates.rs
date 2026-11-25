//! Preferences > Advanced: software updates and CLI installation groups.

use super::*;

impl Preferences {
    pub(super) fn software_updates_group(&mut self, cx: &mut Context<Self>) -> Div {
        use crate::updater::{UpdateState, installed_version, is_dev_build};
        let theme = cx.theme();
        let success = theme.success;
        let primary = theme.primary;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let current = installed_version();
        let dev = is_dev_build();

        let (dot_color, title, chip, detail, is_err, actions) = match &self.update_state {
            UpdateState::Checking => (
                muted,
                "Checking for updates\u{2026}".to_owned(),
                Some(SharedString::from(current)),
                "Contacting GitHub Releases.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    self.btn(
                        Button::new("adv-upd-checking")
                            .outline()
                            .disabled(true)
                            .label("Checking\u{2026}"),
                    ),
                ),
            ),
            UpdateState::Downloading(ver) => (
                primary,
                "Downloading update\u{2026}".to_owned(),
                Some(SharedString::from(ver.clone())),
                "Verifying package and preparing restart.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    self.btn(
                        Button::new("adv-upd-downloading")
                            .primary()
                            .disabled(true)
                            .label("Downloading\u{2026}"),
                    ),
                ),
            ),
            UpdateState::Available(rel) => {
                let ver = rel.version.clone();
                let notes_url = rel.html_url.clone();
                (
                    primary,
                    "Update available".to_owned(),
                    Some(SharedString::from(ver)),
                    format!("Currently running {current}."),
                    false,
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            self.btn(Button::new("adv-upd-notes").ghost().label("Release notes"))
                                .on_click(move |_, _, cx| cx.open_url(&notes_url)),
                        )
                        .child(
                            self.btn(
                                Button::new("adv-upd-download")
                                    .primary()
                                    .label("Download & restart"),
                            )
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(PreferencesEvent::InstallUpdate);
                            })),
                        ),
                )
            }
            UpdateState::UpToDate => (
                success,
                "Up to date".to_owned(),
                Some(SharedString::from(current)),
                "You have the latest release of Slackless.".to_owned(),
                false,
                h_flex().gap_1p5().child(
                    self.btn(
                        Button::new("adv-upd-check-again")
                            .outline()
                            .label("Check for updates"),
                    )
                    .on_click(cx.listener(|_, _, _, cx| {
                        cx.emit(PreferencesEvent::CheckForUpdates);
                    })),
                ),
            ),
            UpdateState::Error(err) => (
                danger,
                "Update check failed".to_owned(),
                Some(SharedString::from(current)),
                err.clone(),
                true,
                h_flex().gap_1p5().child(
                    self.btn(Button::new("adv-upd-retry").outline().label("Try again"))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(PreferencesEvent::CheckForUpdates);
                        })),
                ),
            ),
            UpdateState::Idle => match &self.available_update {
                Some(rel) => {
                    let ver = rel.version.clone();
                    let notes_url = rel.html_url.clone();
                    (
                        primary,
                        "Update available".to_owned(),
                        Some(SharedString::from(ver)),
                        format!("Currently running {current}."),
                        false,
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                self.btn(
                                    Button::new("adv-upd-notes-cached")
                                        .ghost()
                                        .label("Release notes"),
                                )
                                .on_click(move |_, _, cx| cx.open_url(&notes_url)),
                            )
                            .child(
                                self.btn(
                                    Button::new("adv-upd-download-cached")
                                        .primary()
                                        .label("Download & restart"),
                                )
                                .on_click(cx.listener(
                                    |_, _, _, cx| {
                                        cx.emit(PreferencesEvent::InstallUpdate);
                                    },
                                )),
                            ),
                    )
                }
                None => (
                    if dev { muted } else { success },
                    if dev {
                        "Development build"
                    } else {
                        "Current version"
                    }
                    .to_owned(),
                    Some(SharedString::from(current)),
                    if dev {
                        "Automatic startup checks are paused on dev builds."
                    } else {
                        "Checks automatically on startup."
                    }
                    .to_owned(),
                    false,
                    h_flex().gap_1p5().child(
                        self.btn(
                            Button::new("adv-upd-check")
                                .outline()
                                .label("Check for updates"),
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(PreferencesEvent::CheckForUpdates);
                        })),
                    ),
                ),
            },
        };

        let card = self.status_card(
            super::StatusCard {
                dot_color,
                title: title.into(),
                chip,
                detail: detail.into(),
                detail_is_err: is_err,
                actions,
            },
            cx,
        );
        let release_notes = match &self.update_state {
            UpdateState::Available(rel) => Some(rel),
            UpdateState::Idle => self.available_update.as_ref(),
            _ => None,
        }
        .filter(|rel| !rel.notes.is_empty())
        .map(|rel| (rel.version.clone(), rel.notes.clone()));

        let body = if let Some((ver, notes)) = release_notes {
            let theme = cx.theme();
            let (border, card_bg, fg, muted) = (
                theme.border,
                theme.secondary.opacity(0.25),
                theme.foreground,
                theme.muted_foreground,
            );
            v_flex().gap_2().child(card).child(
                v_flex()
                    .gap_1()
                    .px_3p5()
                    .py_2p5()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .bg(card_bg)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(format!("What's new in {ver}")),
                    )
                    .children(notes.into_iter().map(move |item| {
                        h_flex()
                            .items_start()
                            .gap_1p5()
                            .text_xs()
                            .text_color(muted)
                            .child(div().flex_none().child("\u{2022}"))
                            .child(div().flex_1().min_w_0().child(item))
                    })),
            )
        } else {
            v_flex().child(card)
        };
        self.group(
            "Software updates",
            Some("Checks for new releases on startup or when requested; updates both Slackless.app and the CLI together."),
            body,
            cx,
        )
    }

    pub(super) fn cli_install_group(&mut self, cx: &mut Context<Self>) -> Div {
        use crate::updater::{
            CliInstallStatus, cli_install_status, cli_link_path, dir_in_path, installed_version,
        };
        let status = cli_install_status();
        let target_link = cli_link_path();
        let ver = installed_version();
        let theme = cx.theme();
        let success = theme.success;
        let warning = theme.warning;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let card_bg = theme.secondary.opacity(0.35);

        let effective_dir = match &status {
            CliInstallStatus::Installed(p) | CliInstallStatus::OutOfSync(p) => {
                p.parent().map(std::path::Path::to_path_buf)
            }
            _ => target_link.parent().map(std::path::Path::to_path_buf),
        };
        let in_path = effective_dir.as_deref().is_none_or(dir_in_path);

        let (dot_color, title, chip, detail, actions) = match status {
            CliInstallStatus::Installed(path) => (
                success,
                "Installed".to_owned(),
                Some(SharedString::from(path.display().to_string())),
                format!("Version {ver} · stays in sync with app updates"),
                h_flex().gap_1p5().child(
                    self.btn(
                        Button::new("adv-cli-uninstall")
                            .outline()
                            .label("Uninstall"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        match crate::updater::uninstall_cli_symlink() {
                            Ok(()) => {
                                this.cli_status_note =
                                    Some("Removed slackless CLI link.".to_owned());
                            }
                            Err(err) => {
                                this.cli_status_note = Some(format!("Error: {err}"));
                            }
                        }
                        cx.notify();
                    })),
                ),
            ),
            CliInstallStatus::OutOfSync(path) => (
                warning,
                "Out of sync".to_owned(),
                Some(SharedString::from(path.display().to_string())),
                format!("Points to a different build. Re-link to match Slackless {ver}."),
                h_flex().gap_1p5().child(
                    self.btn(Button::new("adv-cli-relink").primary().label("Update link"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            match crate::updater::install_cli_symlink() {
                                Ok(p) => {
                                    this.cli_status_note =
                                        Some(format!("Linked {} to this app.", p.display()));
                                }
                                Err(err) => {
                                    this.cli_status_note = Some(format!("Error: {err}"));
                                }
                            }
                            cx.notify();
                        })),
                ),
            ),
            CliInstallStatus::NotInstalled => (
                muted,
                "Not installed".to_owned(),
                Some(SharedString::from(target_link.display().to_string())),
                format!(
                    "Links the bundled CLI (version {ver}) so it stays in sync with app updates."
                ),
                h_flex().gap_1p5().child(
                    self.btn(
                        Button::new("adv-cli-install")
                            .primary()
                            .label("Install CLI"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        match crate::updater::install_cli_symlink() {
                            Ok(p) => {
                                this.cli_status_note =
                                    Some(format!("Installed CLI at {}.", p.display()));
                            }
                            Err(err) => {
                                this.cli_status_note = Some(format!("Error: {err}"));
                            }
                        }
                        cx.notify();
                    })),
                ),
            ),
            CliInstallStatus::Unavailable => (
                muted,
                "Unavailable".to_owned(),
                None,
                "Launch from an installed Slackless.app bundle to install the matching CLI."
                    .to_owned(),
                h_flex(),
            ),
        };

        let card = self.status_card(
            super::StatusCard {
                dot_color,
                title: title.into(),
                chip,
                detail: detail.into(),
                detail_is_err: false,
                actions,
            },
            cx,
        );
        let mut body = v_flex().gap_2().child(card);

        if !in_path && let Some(dir) = effective_dir {
            let export_cmd = format!("export PATH=\"{}:$PATH\"", dir.display());
            let copy_cmd = export_cmd.clone();
            body = body.child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
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
                            .flex_wrap()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Add to your shell profile:"),
                            )
                            .child(self.mono_chip(export_cmd, cx)),
                    )
                    .child(
                        self.btn(
                            Button::new("adv-cli-copy-path")
                                .ghost()
                                .icon(IconName::Copy)
                                .label("Copy"),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                copy_cmd.clone(),
                            ));
                            this.cli_status_note = Some("Copied PATH export command.".to_owned());
                            cx.notify();
                        })),
                    ),
            );
        }
        if let Some(note) = &self.cli_status_note {
            let note_color = if note.starts_with("Error:") {
                danger
            } else {
                muted
            };
            body = body.child(div().text_xs().text_color(note_color).child(note.clone()));
        }

        self.group(
            "Command-line interface",
            Some("Use the slackless CLI from your terminal with your local archive and session."),
            body,
            cx,
        )
    }
}
