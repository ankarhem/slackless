//! Multi-view Block Kit modal stack (`F-COLLAB-600..616`):
//! manages `view_opened` / `view_updated` / `view_closed` lifecycle, `views.submit`
//! (`push` / `update` / `clear` / `errors`), back navigation across stacked views, inline
//! server validation errors keyed by `block_id`, and async `blocks.suggestions` inside modals.

use super::*;
use gpui_kit::gpui::WeakEntity;
use slackless_core::api::{ActionContainer, ViewSubmitResponse};
use slackless_core::types::{BotId, TeamId};

/// One frame in the app modal stack.
pub(in crate::app) struct ModalFrame {
    pub(in crate::app) view: crate::blockkit::Json,
    pub(in crate::app) view_id: String,
    pub(in crate::app) root_view_id: String,
    pub(in crate::app) previous_view_id: Option<String>,
    pub(in crate::app) hash: Option<String>,
    pub(in crate::app) app_id: String,
    pub(in crate::app) bot_id: BotId,
    pub(in crate::app) team_id: Option<TeamId>,
    pub(in crate::app) clear_on_close: bool,
    pub(in crate::app) notify_on_close: bool,
    pub(in crate::app) ts: Ts,
    pub(in crate::app) kit: Entity<crate::blockkit::BlockKitState>,
    pub(in crate::app) submitting: bool,
    pub(in crate::app) submit_error: Option<String>,
}

impl ModalFrame {
    fn from_view(
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Option<Self> {
        let view_id = view["id"].as_str().unwrap_or_default().to_owned();
        if view_id.is_empty() {
            return None;
        }
        let root_view_id = view["root_view_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(&view_id)
            .to_owned();
        let previous_view_id = view["previous_view_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let hash = view["hash"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let app_id = view["app_id"].as_str().unwrap_or_default().to_owned();
        let bot_id = BotId::from(
            view["bot_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("B01"),
        );
        let team_id = view["app_installed_team_id"]
            .as_str()
            .or_else(|| view["team_id"].as_str())
            .filter(|s| !s.is_empty())
            .map(TeamId::from);
        let clear_on_close = view["clear_on_close"].as_bool().unwrap_or(false);
        let notify_on_close = view["notify_on_close"].as_bool().unwrap_or(false);
        let raw_blocks: Vec<crate::blockkit::Json> =
            view["blocks"].as_array().cloned().unwrap_or_default();
        let ts = Ts::from(format!("modal-{view_id}"));
        let kit = cx.new(|_| crate::blockkit::BlockKitState::default());
        crate::blockkit::prepare_raw_inputs(&kit, &ts, &raw_blocks, window, cx);
        Some(Self {
            view,
            view_id,
            root_view_id,
            previous_view_id,
            hash,
            app_id,
            bot_id,
            team_id,
            clear_on_close,
            notify_on_close,
            ts,
            kit,
            submitting: false,
            submit_error: None,
        })
    }

    fn apply_in_place_update(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        if let Some(h) = view["hash"].as_str().filter(|s| !s.is_empty()) {
            self.hash = Some(h.to_owned());
        }
        if let Some(clear) = view["clear_on_close"].as_bool() {
            self.clear_on_close = clear;
        }
        if let Some(notify) = view["notify_on_close"].as_bool() {
            self.notify_on_close = notify;
        }
        let raw_blocks: Vec<crate::blockkit::Json> =
            view["blocks"].as_array().cloned().unwrap_or_default();
        crate::blockkit::prepare_raw_inputs(&self.kit, &self.ts, &raw_blocks, window, cx);
        self.kit.update(cx, |state, cx| {
            state.set_block_errors(HashMap::new());
            cx.notify();
        });
        self.view = view;
        self.submitting = false;
        self.submit_error = None;
    }

    fn title(&self) -> String {
        self.view["title"]["text"]
            .as_str()
            .or_else(|| self.view["title"].as_str())
            .unwrap_or("App modal")
            .to_owned()
    }

    fn submit_label(&self) -> Option<String> {
        self.view["submit"]["text"]
            .as_str()
            .or_else(|| self.view["submit"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    }

    fn close_label(&self, has_previous: bool) -> String {
        self.view["close"]["text"]
            .as_str()
            .or_else(|| self.view["close"].as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if has_previous {
                    "Back".to_owned()
                } else {
                    "Cancel".to_owned()
                }
            })
    }

    fn blocks(&self) -> Vec<crate::blockkit::Json> {
        self.view["blocks"].as_array().cloned().unwrap_or_default()
    }
}

/// Reactive GPUI view hosting Slack's multi-view modal stack (`views.open` / `push` / `update` /
/// `close` / `errors`).
pub(in crate::app) struct AppModalStack {
    fixtures: Arc<Fixtures>,
    app: WeakEntity<App>,
    pub(in crate::app) frames: Vec<ModalFrame>,
    dialog_open: bool,
    kit_subs: Vec<Subscription>,
}

impl AppModalStack {
    pub(in crate::app) fn new(
        fixtures: Arc<Fixtures>,
        app: WeakEntity<App>,
        cx: &mut gpui::App,
    ) -> Entity<Self> {
        cx.new(|_| Self {
            fixtures,
            app,
            frames: Vec::new(),
            dialog_open: false,
            kit_subs: Vec::new(),
        })
    }

    pub(in crate::app) fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        cx.notify();
    }

    #[cfg(test)]
    pub(in crate::app) fn depth(&self) -> usize {
        self.frames.len()
    }

    #[cfg(test)]
    pub(in crate::app) fn top_view_id(&self) -> Option<&str> {
        self.frames.last().map(|f| f.view_id.as_str())
    }

    #[cfg(test)]
    pub(in crate::app) fn top_kit(&self) -> Option<Entity<crate::blockkit::BlockKitState>> {
        self.frames.last().map(|f| f.kit.clone())
    }

    fn observe_kits(&mut self, cx: &mut Context<Self>) {
        self.kit_subs = self
            .frames
            .iter()
            .map(|f| cx.observe(&f.kit, |_, _, cx| cx.notify()))
            .collect();
    }

    fn ensure_dialog_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let stack = cx.entity();
        let stack_on_close = stack.downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let stack_close = stack_on_close.clone();
            dialog
                .w(px(560.))
                .overlay_closable(true)
                .on_close(move |_, _, cx| {
                    let _ = stack_close.update(cx, |s, cx| s.on_dialog_dismissed(cx));
                })
                .child(stack.clone())
        });
    }

    fn on_dialog_dismissed(&mut self, cx: &mut Context<Self>) {
        self.dialog_open = false;
        let frames = std::mem::take(&mut self.frames);
        self.kit_subs.clear();
        for frame in frames.into_iter().rev() {
            if frame.notify_on_close {
                self.send_close(&frame, cx);
            }
        }
        cx.notify();
    }

    fn send_close(&self, frame: &ModalFrame, cx: &mut Context<Self>) {
        let view_id = frame.view_id.clone();
        let root_view_id = Some(frame.root_view_id.clone());
        let hash = frame.hash.clone();
        let _ = self.app.update(cx, |app, _| {
            if !app.fixtures.sample {
                app.bridge.send(CollabCommand::CloseModal {
                    view_id,
                    root_view_id,
                    hash,
                });
            }
        });
    }

    /// Handle `view_opened`: push onto stack if `previous_view_id` matches the current top view
    /// (or shares `root_view_id` with a new `view_id`), update in place if same `view_id`, else
    /// start a fresh modal stack.
    pub(in crate::app) fn open_view(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(frame) = ModalFrame::from_view(view.clone(), window, cx) else {
            return;
        };
        if let Some(existing) = self.frames.iter_mut().find(|f| f.view_id == frame.view_id) {
            existing.apply_in_place_update(view, window, cx);
            self.ensure_dialog_open(window, cx);
            cx.notify();
            return;
        }
        let should_push = self.frames.last().is_some_and(|top| {
            frame.previous_view_id.as_deref() == Some(top.view_id.as_str())
                || (frame.root_view_id == top.root_view_id && frame.view_id != top.view_id)
        });
        if should_push {
            if let Some(top) = self.frames.last_mut() {
                top.submitting = false;
            }
            self.frames.push(frame);
        } else {
            self.frames = vec![frame];
        }
        self.observe_kits(cx);
        self.ensure_dialog_open(window, cx);
        cx.notify();
    }

    /// Handle `view_updated` (websocket) or `ViewSubmitResponse::Update`: update the matching
    /// view in place while preserving user input drafts, or push/open if unseen.
    pub(in crate::app) fn update_view(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view_id = view["id"].as_str().unwrap_or_default();
        if view_id.is_empty() {
            return;
        }
        if let Some(existing) = self.frames.iter_mut().find(|f| f.view_id == view_id) {
            existing.apply_in_place_update(view, window, cx);
            self.ensure_dialog_open(window, cx);
            cx.notify();
            return;
        }
        self.open_view(view, window, cx);
    }

    /// Push a new view onto the modal stack (`ViewSubmitResponse::Push`).
    pub(in crate::app) fn push_view(
        &mut self,
        view: crate::blockkit::Json,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(frame) = ModalFrame::from_view(view.clone(), window, cx) else {
            return;
        };
        if let Some(top) = self.frames.last_mut() {
            top.submitting = false;
            top.submit_error = None;
        }
        if let Some(existing) = self.frames.iter_mut().find(|f| f.view_id == frame.view_id) {
            existing.apply_in_place_update(view, window, cx);
        } else {
            self.frames.push(frame);
            self.observe_kits(cx);
        }
        self.ensure_dialog_open(window, cx);
        cx.notify();
    }

    /// Pop the top modal view back to its predecessor (`Back` button on pushed modals), closing
    /// the dialog if the stack becomes empty.
    pub(in crate::app) fn pop_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popped) = self.frames.pop() {
            if popped.notify_on_close {
                self.send_close(&popped, cx);
            }
            if popped.clear_on_close || self.frames.is_empty() {
                self.dismiss_all(window, cx);
                return;
            }
            if let Some(top) = self.frames.last_mut() {
                top.submitting = false;
            }
            self.observe_kits(cx);
            cx.notify();
        }
    }

    /// Dismiss the entire modal stack (X button, Cancel on root, or `response_action: "clear"`).
    pub(in crate::app) fn dismiss_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let frames = std::mem::take(&mut self.frames);
        self.kit_subs.clear();
        for frame in frames.into_iter().rev() {
            if frame.notify_on_close {
                self.send_close(&frame, cx);
            }
        }
        if self.dialog_open {
            self.dialog_open = false;
            window.close_dialog(cx);
        }
        cx.notify();
    }

    /// Handle websocket `view_closed`.
    pub(in crate::app) fn apply_modal_closed(
        &mut self,
        view_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if view_id.is_empty()
            || self
                .frames
                .first()
                .is_some_and(|root| root.root_view_id == view_id || root.view_id == view_id)
        {
            self.frames.clear();
            self.kit_subs.clear();
            if self.dialog_open {
                self.dialog_open = false;
                window.close_dialog(cx);
            }
            cx.notify();
            return;
        }
        self.frames.retain(|f| f.view_id != view_id);
        if self.frames.is_empty() {
            self.kit_subs.clear();
            if self.dialog_open {
                self.dialog_open = false;
                window.close_dialog(cx);
            }
        } else {
            self.observe_kits(cx);
        }
        cx.notify();
    }

    /// Fold a `views.submit` outcome into the modal stack (`push`, `update`, `clear`, `close`,
    /// inline `errors` keyed by `block_id`, or transport error).
    pub(in crate::app) fn apply_submit_result(
        &mut self,
        view_id: &str,
        outcome: Result<ViewSubmitResponse, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(ViewSubmitResponse::Clear) => {
                self.frames.clear();
                self.kit_subs.clear();
                if self.dialog_open {
                    self.dialog_open = false;
                    window.close_dialog(cx);
                }
                cx.notify();
            }
            Ok(ViewSubmitResponse::Close) => {
                let clear = self
                    .frames
                    .last()
                    .is_some_and(|f| f.view_id == view_id && f.clear_on_close);
                if clear {
                    self.frames.clear();
                } else if self.frames.last().is_some_and(|f| f.view_id == view_id) {
                    self.frames.pop();
                } else {
                    self.frames.retain(|f| f.view_id != view_id);
                }
                if self.frames.is_empty() {
                    self.kit_subs.clear();
                    if self.dialog_open {
                        self.dialog_open = false;
                        window.close_dialog(cx);
                    }
                } else {
                    if let Some(top) = self.frames.last_mut() {
                        top.submitting = false;
                    }
                    self.observe_kits(cx);
                }
                cx.notify();
            }
            Ok(ViewSubmitResponse::Push(next_view)) => {
                self.push_view(next_view, window, cx);
            }
            Ok(ViewSubmitResponse::Update(updated_view)) => {
                self.update_view(updated_view, window, cx);
            }
            Ok(ViewSubmitResponse::Errors(errors)) => {
                let target_ix = self
                    .frames
                    .iter()
                    .rposition(|f| f.view_id == view_id)
                    .or_else(|| {
                        view_id
                            .is_empty()
                            .then(|| self.frames.len().checked_sub(1))
                            .flatten()
                    });
                if let Some(ix) = target_ix {
                    let frame = &mut self.frames[ix];
                    frame.submitting = false;
                    frame.submit_error = if errors.is_empty() {
                        Some("Please fix the highlighted fields and try again.".to_owned())
                    } else {
                        None
                    };
                    let kit = frame.kit.clone();
                    kit.update(cx, |state, cx| {
                        state.set_block_errors(errors);
                        cx.notify();
                    });
                }
                cx.notify();
            }
            Err(err) => {
                let target_ix = self
                    .frames
                    .iter()
                    .rposition(|f| f.view_id == view_id)
                    .or_else(|| {
                        view_id
                            .is_empty()
                            .then(|| self.frames.len().checked_sub(1))
                            .flatten()
                    });
                if let Some(ix) = target_ix {
                    let frame = &mut self.frames[ix];
                    frame.submitting = false;
                    frame.submit_error = Some(err);
                }
                cx.notify();
            }
        }
    }

    /// Deliver `blocks.suggestions` options into any active modal frame.
    pub(in crate::app) fn set_suggestions(
        &mut self,
        key: &str,
        options: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let mut updated = false;
        for frame in &self.frames {
            if key
                .strip_prefix(frame.ts.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
            {
                frame.kit.update(cx, |kit, cx| {
                    kit.set_suggestions(key.to_owned(), options.clone());
                    cx.notify();
                });
                updated = true;
            }
        }
        if updated {
            cx.notify();
        }
    }

    fn submit_top(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(top) = self.frames.last_mut() else {
            return;
        };
        if top.submitting {
            return;
        }
        let blocks = top.blocks();
        let ts = top.ts.clone();
        let kit = top.kit.clone();
        match crate::blockkit::collect_modal_state_errors(&ts, &blocks, &kit, cx) {
            Ok(state_values) => {
                top.submitting = true;
                top.submit_error = None;
                kit.update(cx, |state, cx| {
                    state.set_block_errors(HashMap::new());
                    cx.notify();
                });
                let view_id = top.view_id.clone();
                let hash = top.hash.clone();
                let sample = self.fixtures.sample;
                if sample {
                    self.apply_submit_result(&view_id, Ok(ViewSubmitResponse::Close), window, cx);
                } else {
                    let _ = self.app.update(cx, |app, _| {
                        app.bridge.send(CollabCommand::SubmitModal {
                            view_id,
                            hash,
                            state_values,
                        });
                    });
                    cx.notify();
                }
            }
            Err(errors) => {
                top.submit_error = None;
                kit.update(cx, |state, cx| {
                    state.set_block_errors(errors);
                    cx.notify();
                });
                cx.notify();
            }
        }
    }
}

impl Render for AppModalStack {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, fg, danger, secondary) = (
            theme.border,
            theme.muted_foreground,
            theme.foreground,
            theme.danger,
            theme.secondary,
        );
        let Some(top) = self.frames.last() else {
            return v_flex().p_4().child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Loading modal\u{2026}"),
            );
        };
        let depth = self.frames.len();
        let has_prev = depth > 1;
        let title = top.title();
        let submit_label = top.submit_label();
        let close_label = top.close_label(has_prev);
        let raw_blocks = top.blocks();
        let ts = top.ts.clone();
        let kit = top.kit.clone();
        let view_id = top.view_id.clone();
        let app_id = top.app_id.clone();
        let bot_id = top.bot_id.clone();
        let team_id = top.team_id.clone();
        let submitting = top.submitting;
        let submit_error = top.submit_error.clone();

        let app_for_links = self.app.clone();
        let links: crate::messages::LinkHandler = Rc::new(move |target, window, cx| {
            let _ = app_for_links.update(cx, |app, cx| app.open_link(target, window, cx));
        });

        let app_for_action = self.app.clone();
        let (act_bot, act_app, act_team, act_view) = (
            bot_id.clone(),
            app_id.clone(),
            team_id.clone(),
            view_id.clone(),
        );
        let on_action: crate::actions::ActionHandler = Rc::new(move |action, window, cx| {
            if let crate::actions::MessageAction::BlockAction { action, .. } = action {
                let _ = app_for_action.update(cx, |app, cx| {
                    if app.fixtures.sample {
                        crate::blockkit::log_button(&action, window, cx);
                    } else {
                        app.bridge.send(CollabCommand::BlockAction {
                            bot: act_bot.clone(),
                            app_id: act_app.clone(),
                            team: act_team.clone(),
                            action,
                            container: ActionContainer {
                                channel: app.channel.clone(),
                                message_ts: Ts::default(),
                                attachment_id: None,
                                is_ephemeral: false,
                                view_id: Some(act_view.clone()),
                            },
                        });
                    }
                });
            }
        });

        let app_for_suggest = self.app.clone();
        let (sug_bot, sug_app, sug_team, sug_view) = (
            bot_id.clone(),
            app_id.clone(),
            team_id.clone(),
            view_id.clone(),
        );
        let on_suggest: crate::blockkit::SuggestHandler = Rc::new(move |req, cx| {
            let _ = app_for_suggest.update(cx, |app, cx| {
                app.request_modal_suggestions(
                    sug_bot.clone(),
                    sug_app.clone(),
                    sug_team.clone(),
                    sug_view.clone(),
                    req,
                    cx,
                );
            });
        });

        let app_for_wf = self.app.clone();
        let on_wf: crate::blockkit::WorkflowDetailsHandler = Rc::new(move |req, window, cx| {
            let _ = app_for_wf.update(cx, |app, cx| app.open_workflow_details(req, window, cx));
        });

        let kcx = crate::blockkit::KitCx::new(&self.fixtures, Some(&kit), &ts)
            .with_links(links)
            .dispatching_actions(on_action)
            .with_suggestions(on_suggest)
            .with_workflow_details(on_wf);
        let body = crate::blockkit::render_raw_blocks(&raw_blocks, &kcx, cx);

        let header = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .pb_3()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .when(has_prev, |row| {
                        row.child(
                            Button::new("modal-back")
                                .ghost()
                                .with_size(Size::XSmall)
                                .icon(IconName::ArrowLeft)
                                .tooltip("Back to previous view")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pop_back(window, cx);
                                })),
                        )
                    })
                    .child(
                        div()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::BOLD)
                            .text_color(fg)
                            .child(title),
                    )
                    .when(has_prev, |row| {
                        row.child(
                            div()
                                .flex_none()
                                .px_1p5()
                                .py(px(1.))
                                .rounded_full()
                                .bg(secondary)
                                .text_xs()
                                .text_color(muted)
                                .child(format!("View {depth}")),
                        )
                    }),
            )
            .child(
                Button::new("modal-x-dismiss")
                    .ghost()
                    .with_size(Size::XSmall)
                    .icon(IconName::X)
                    .tooltip("Close modal")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dismiss_all(window, cx);
                    })),
            );

        let mut footer = h_flex()
            .justify_end()
            .gap_2()
            .pt_3()
            .border_t_1()
            .border_color(border)
            .child(
                preferences::form_button(
                    Button::new("modal-close").outline().label(close_label),
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if has_prev {
                        this.pop_back(window, cx);
                    } else {
                        this.dismiss_all(window, cx);
                    }
                })),
            );
        if let Some(submit_text) = submit_label {
            footer = footer.child(
                preferences::form_button(
                    Button::new("modal-submit")
                        .primary()
                        .disabled(submitting)
                        .label(if submitting {
                            "Submitting\u{2026}".to_owned()
                        } else {
                            submit_text
                        }),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.submit_top(window, cx);
                })),
            );
        }

        v_flex()
            .gap_3()
            .child(header)
            .child(
                v_flex()
                    .id("modal-blocks-body")
                    .max_h(px(440.))
                    .overflow_y_scroll()
                    .child(body),
            )
            .when_some(submit_error, |col, err| {
                col.child(
                    h_flex()
                        .items_center()
                        .gap_1p5()
                        .px_2p5()
                        .py_1p5()
                        .rounded_md()
                        .bg(danger.opacity(0.12))
                        .border_1()
                        .border_color(danger.opacity(0.35))
                        .text_xs()
                        .text_color(danger)
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .size(px(14.))
                                .text_color(danger),
                        )
                        .child(err),
                )
            })
            .child(footer)
    }
}
