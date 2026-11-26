//! Workflow trigger details dialog (`workflows.triggers.info` + `.trip`, `F-COLLAB-404..435`)
//! and third-party calling app launcher dialog (`F-COLLAB-078..094`).

use super::*;
use gpui_kit::gpui::WeakEntity;
use slackless_core::api::WorkflowTriggerInfo;
use slackless_core::domain::calling::{CallMode, CallingApp, build_calling_command};

/// Reactive state for the Workflow Details dialog (`workflows.triggers.info`).
pub(in crate::app) struct WorkflowDetailsView {
    app: WeakEntity<App>,
    trigger_id: Option<String>,
    fallback_url: Option<String>,
    fallback_title: String,
    loading: bool,
    info: Option<WorkflowTriggerInfo>,
    error: Option<String>,
}

impl WorkflowDetailsView {
    pub(in crate::app) fn new(
        app: WeakEntity<App>,
        req: crate::blockkit::WorkflowDetailsRequest,
        sample: bool,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        let info = sample.then(|| WorkflowTriggerInfo {
            trigger_id: req
                .trigger_id
                .clone()
                .unwrap_or_else(|| "Ft0SAMPLE01".to_owned()),
            name: req.title.clone(),
            description: "Runs this Slack workflow directly from your channel or message."
                .to_owned(),
            trigger_type: "shortcut".to_owned(),
            url: req.url.clone(),
            workflow_id: Some("Wf0SAMPLE01".to_owned()),
            app_id: None,
            steps: vec![
                slackless_core::api::WorkflowStepSummary {
                    title: "Collect workflow inputs".to_owned(),
                    description: Some("Opens a modal form if inputs are required.".to_owned()),
                    function_id: None,
                },
                slackless_core::api::WorkflowStepSummary {
                    title: "Post confirmation in conversation".to_owned(),
                    description: None,
                    function_id: None,
                },
            ],
            collaborators: Vec::new(),
            is_published: Some(true),
            date_updated: None,
        });
        let loading = !sample && req.trigger_id.is_some();
        cx.new(|_| Self {
            app,
            trigger_id: req.trigger_id,
            fallback_url: req.url,
            fallback_title: req.title,
            loading,
            info,
            error: None,
        })
    }

    pub(in crate::app) fn apply_loaded(
        &mut self,
        trigger_id: &str,
        result: Result<WorkflowTriggerInfo, String>,
        cx: &mut Context<Self>,
    ) {
        if self.trigger_id.as_deref() != Some(trigger_id) {
            return;
        }
        self.loading = false;
        match result {
            Ok(info) => {
                self.info = Some(info);
                self.error = None;
            }
            Err(err) => {
                self.error = Some(err);
            }
        }
        cx.notify();
    }
}

impl Render for WorkflowDetailsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, fg, link, danger) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
            theme.danger,
        );
        let title = self
            .info
            .as_ref()
            .map(|i| i.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.fallback_title.clone());
        let description = self
            .info
            .as_ref()
            .map(|i| i.description.clone())
            .unwrap_or_default();
        let trigger_type = self
            .info
            .as_ref()
            .map(|i| i.trigger_type.clone())
            .unwrap_or_else(|| "shortcut".to_owned());
        let url = self
            .info
            .as_ref()
            .and_then(|i| i.url.clone())
            .or_else(|| self.fallback_url.clone());
        let steps = self
            .info
            .as_ref()
            .map(|i| i.steps.clone())
            .unwrap_or_default();

        let app_run = self.app.clone();
        let run_url = url.clone();
        let copy_url = url.clone();

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .size(px(32.))
                            .rounded_md()
                            .bg(link.opacity(0.14))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::new(IconName::Zap).size(px(16.)).text_color(link)),
                    )
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("Workflow · {trigger_type} trigger")),
                            ),
                    ),
            )
            .when(self.loading, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Loading workflow details\u{2026}"),
                )
            })
            .when_some(self.error.clone(), |col, err| {
                let retry_trigger = self.trigger_id.clone();
                col.child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2p5()
                        .py_2()
                        .rounded_md()
                        .border_1()
                        .border_color(danger.opacity(0.4))
                        .bg(danger.opacity(0.1))
                        .child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .items_start()
                                .gap_1p5()
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .flex_none()
                                        .size(px(14.))
                                        .text_color(danger),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(danger)
                                        .child(format!("Could not load workflow details: {err}")),
                                ),
                        )
                        .when_some(retry_trigger, |row, tid| {
                            row.child(
                                Button::new("wf-details-retry")
                                    .outline()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::RotateCcw)
                                    .label("Retry")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.loading = true;
                                        this.error = None;
                                        let _ = this.app.update(cx, |app, _| {
                                            app.bridge.send(
                                                CollabCommand::FetchWorkflowTriggerInfo {
                                                    trigger_id: tid.clone(),
                                                },
                                            );
                                        });
                                        cx.notify();
                                    })),
                            )
                        }),
                )
            })
            .when(
                !self.loading && self.error.is_none() && self.info.is_none() && url.is_none(),
                |col| {
                    col.child(
                        div().text_xs().text_color(muted).child(
                            "No additional workflow details are available for this trigger.",
                        ),
                    )
                },
            )
            .when(!description.is_empty(), |col| {
                col.child(div().text_sm().text_color(fg).child(description))
            })
            .when(!steps.is_empty(), |col| {
                col.child(
                    v_flex()
                        .gap_1p5()
                        .p_2p5()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(theme.muted)
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child(format!("Steps ({})", steps.len())),
                        )
                        .children(steps.into_iter().enumerate().map(|(ix, step)| {
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(fg)
                                        .child(format!("{}. {}", ix + 1, step.title)),
                                )
                                .when_some(step.description, |s, desc| {
                                    s.child(div().pl_3().text_xs().text_color(muted).child(desc))
                                })
                        })),
                )
            })
            .when_some(url.clone(), |col, href| {
                col.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(muted)
                                .child("Workflow URL"),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1p5()
                                .rounded_md()
                                .border_1()
                                .border_color(border)
                                .bg(theme.secondary)
                                .text_xs()
                                .text_color(fg)
                                .truncate()
                                .child(href),
                        ),
                )
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .when_some(copy_url, |row, href| {
                        row.child(
                            preferences::form_button(
                                Button::new("wf-details-copy")
                                    .outline()
                                    .icon(IconName::Link)
                                    .label("Copy URL"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(href.clone()));
                                notify(
                                    "Workflow URL copied",
                                    "Copied workflow trigger URL to clipboard.",
                                    window,
                                    cx,
                                );
                            }),
                        )
                    })
                    .child(
                        preferences::form_button(
                            Button::new("wf-details-close").outline().label("Close"),
                            cx,
                        )
                        .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .when_some(run_url, |row, href| {
                        row.child(
                            preferences::form_button(
                                Button::new("wf-details-run")
                                    .primary()
                                    .icon(IconName::Zap)
                                    .label("Run workflow"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = app_run.update(cx, |app, cx| {
                                    app.trip_workflow_url(&href, "details_dialog", window, cx);
                                });
                            }),
                        )
                    }),
            )
    }
}

/// Interactive third-party calling app launcher dialog (`F-COLLAB-078..085`, `F-COLLAB-092..094`).
pub(in crate::app) struct CallingAppLauncher {
    app: WeakEntity<App>,
    apps: Vec<CallingApp>,
    selected_ix: usize,
    mode: CallMode,
    topic: Entity<TextareaState>,
}

impl CallingAppLauncher {
    pub(in crate::app) fn new(
        app: WeakEntity<App>,
        apps: Vec<CallingApp>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        let topic = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Optional meeting title or topic\u{2026}")
                .auto_grow(1, 1)
        });
        cx.new(|_| Self {
            app,
            apps,
            selected_ix: 0,
            mode: CallMode::Video,
            topic,
        })
    }
}

impl Render for CallingAppLauncher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, fg, link) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.link,
        );
        let selected = self.apps.get(self.selected_ix).cloned();
        let topic_input = self.topic.clone();
        let selected_for_run = selected.clone();
        let mode = self.mode;
        let app_run = self.app.clone();
        let app_logout = self.app.clone();
        let logout_cmd = selected.as_ref().and_then(|a| a.logout_command.clone());

        v_flex()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Choose a calling app to start a meeting in this conversation."),
            )
            .child(
                v_flex()
                    .id("call-app-choices")
                    .max_h(px(260.))
                    .overflow_y_scroll()
                    .gap_1p5()
                    .children(self.apps.iter().enumerate().map(|(ix, calling_app)| {
                        let active = ix == self.selected_ix;
                        let hover_bg = theme.accent;
                        h_flex()
                            .id(("call-app-choice", ix))
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(if active { link } else { border })
                            .bg(if active {
                                link.opacity(0.1)
                            } else {
                                theme.muted
                            })
                            .when(!active, |row| row.hover(move |el| el.bg(hover_bg)))
                            .cursor_pointer()
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Icon::new(IconName::Video)
                                            .flex_none()
                                            .size(px(15.))
                                            .text_color(if active { link } else { muted }),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_sm()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(fg)
                                                    .child(calling_app.name.clone()),
                                            )
                                            .child(
                                                div()
                                                    .min_w_0()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(muted)
                                                    .child(calling_app.command.clone()),
                                            ),
                                    ),
                            )
                            .when(active, |row| {
                                row.child(
                                    Icon::new(IconName::Check)
                                        .flex_none()
                                        .size(px(14.))
                                        .text_color(link),
                                )
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_ix = ix;
                                if !this.apps[ix].supports_audio_only {
                                    this.mode = CallMode::Video;
                                }
                                cx.notify();
                            }))
                    })),
            )
            .when(
                selected.as_ref().is_some_and(|a| a.supports_audio_only),
                |col| {
                    col.child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(muted)
                                    .child("Call type"),
                            )
                            .child(
                                Button::new("call-mode-video")
                                    .with_size(Size::XSmall)
                                    .icon(IconName::Video)
                                    .label("Video")
                                    .when(mode == CallMode::Video, Button::primary)
                                    .when(mode != CallMode::Video, Button::outline)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode = CallMode::Video;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("call-mode-audio")
                                    .with_size(Size::XSmall)
                                    .icon(IconName::Phone)
                                    .label("Audio only")
                                    .when(mode == CallMode::Audio, Button::primary)
                                    .when(mode != CallMode::Audio, Button::outline)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.mode = CallMode::Audio;
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child("Meeting title (optional)"),
                    )
                    .child(crate::preferences::text_input_box(&self.topic, false, cx)),
            )
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .flex_wrap()
                    .gap_2()
                    .pt_2()
                    .child(div().when_some(logout_cmd, |slot, cmd| {
                        slot.child(
                            Button::new("call-app-logout")
                                .ghost()
                                .with_size(Size::XSmall)
                                .label("Disconnect account")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = app_logout.update(cx, |app, cx| {
                                        if let Some(inv) = crate::slash::parse(&cmd) {
                                            app.run_slash(&inv, None, window, cx);
                                        }
                                    });
                                }),
                        )
                    }))
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .child(
                                preferences::form_button(
                                    Button::new("call-launch-cancel").outline().label("Cancel"),
                                    cx,
                                )
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                            )
                            .when_some(selected_for_run, |row, chosen| {
                                let label = match mode {
                                    CallMode::Video => "Start video call",
                                    CallMode::Audio => "Start audio call",
                                };
                                row.child(
                                    preferences::form_button(
                                        Button::new("call-launch-go")
                                            .primary()
                                            .icon(if mode == CallMode::Video {
                                                IconName::Video
                                            } else {
                                                IconName::Phone
                                            })
                                            .label(label),
                                        cx,
                                    )
                                    .on_click(
                                        move |_, window, cx| {
                                            let topic = topic_input.read(cx).value().to_string();
                                            window.close_dialog(cx);
                                            let _ = app_run.update(cx, |app, cx| {
                                                app.launch_calling_app(
                                                    &chosen, mode, &topic, window, cx,
                                                );
                                            });
                                        },
                                    ),
                                )
                            }),
                    ),
            )
    }
}

impl App {
    /// Return installed third-party calling apps (or sample providers in fixture mode).
    pub(in crate::app) fn calling_apps(&self) -> Vec<CallingApp> {
        if self.fixtures.sample && self.fixtures.app_commands.is_empty() {
            return slackless_core::domain::calling::sample_calling_apps();
        }
        let bot_names: Vec<&str> = self
            .fixtures
            .users
            .iter()
            .filter(|u| u.is_bot && !u.deleted)
            .map(|u| u.display_name())
            .collect();
        slackless_core::domain::calling::detect_calling_apps(
            &self.fixtures.app_commands,
            &bot_names,
        )
    }

    /// Launch a third-party calling app in the active conversation via `chat.command`
    /// (`F-COLLAB-078..085`, `F-COLLAB-092`).
    pub(in crate::app) fn launch_calling_app(
        &mut self,
        calling_app: &CallingApp,
        mode: CallMode,
        topic: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (cmd, args) = build_calling_command(calling_app, mode, topic);
        let command_str = format!("/{cmd}");
        if self.fixtures.sample {
            let mode_str = match mode {
                CallMode::Video => "video",
                CallMode::Audio => "audio",
            };
            let detail = if args.is_empty() {
                format!(
                    "Launched {} {mode_str} call ({command_str}).",
                    calling_app.name
                )
            } else {
                format!(
                    "Launched {} {mode_str} call ({command_str} {args}).",
                    calling_app.name
                )
            };
            notify(calling_app.name.as_str(), detail, window, cx);
            return;
        }
        self.bridge.send(MessagingCommand::RunCommand {
            channel: self.channel.clone(),
            command: command_str,
            args,
            thread_ts: None,
            app_id: None,
        });
        notify(
            calling_app.name.as_str(),
            format!("Starting {} call\u{2026}", calling_app.name),
            window,
            cx,
        );
    }

    /// Open the calling app selector dialog (`F-COLLAB-079`, `F-COLLAB-084`).
    pub(in crate::app) fn open_calling_app_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let apps = self.calling_apps();
        if apps.is_empty() {
            notify(
                "No calling apps installed",
                "Install a calling app such as Zoom, Google Meet, or Microsoft Teams in this workspace.",
                window,
                cx,
            );
            return;
        }
        let launcher = CallingAppLauncher::new(cx.entity().downgrade(), apps, window, cx);
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Start a call")
                .w(px(460.))
                .child(launcher.clone())
        });
    }

    /// Open the Workflow Details dialog and fetch `workflows.triggers.info` when live
    /// (`F-COLLAB-432`).
    pub(in crate::app) fn open_workflow_details(
        &mut self,
        req: crate::blockkit::WorkflowDetailsRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let trigger_id = req.trigger_id.clone().or_else(|| {
            req.url
                .as_deref()
                .and_then(slackless_core::api::extract_workflow_trigger_id)
        });
        let req = crate::blockkit::WorkflowDetailsRequest {
            trigger_id: trigger_id.clone(),
            ..req
        };
        let view = WorkflowDetailsView::new(cx.entity().downgrade(), req, self.fixtures.sample, cx);
        self.workflow_details = Some(view.downgrade());
        if !self.fixtures.sample
            && let Some(tid) = trigger_id
        {
            self.bridge
                .send(CollabCommand::FetchWorkflowTriggerInfo { trigger_id: tid });
        }
        let view_id = view.entity_id();
        let app_close = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let app_close = app_close.clone();
            dialog
                .title("Workflow details")
                .w(px(480.))
                .on_close(move |_, _, cx| {
                    let _ = app_close.update(cx, |app, _| {
                        if app.workflow_details.as_ref().map(|w| w.entity_id()) == Some(view_id) {
                            app.workflow_details = None;
                        }
                    });
                })
                .child(view.clone())
        });
    }

    /// Dispatch a workflow link trigger (`workflows.triggers.trip`, `F-COLLAB-404`).
    pub(in crate::app) fn trip_workflow_url(
        &mut self,
        url: &str,
        location: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.fixtures.sample {
            notify("Workflow started", format!("Triggered {url}"), window, cx);
        } else {
            self.bridge.send(CollabCommand::TripWorkflowTrigger {
                url: url.to_owned(),
                context: crate::blockkit::serde_json::json!({
                    "location": location,
                    "channel_id": self.channel.as_str(),
                }),
            });
        }
    }
}
