//! Two-step custom status picker and scheduled-status editor dialog.

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

use crate::emoji::glyph as emoji_glyph;
use crate::messages::clock;

use super::{
    CLEAR_AFTER, EMOJI_CHOICES, PauseLength, SUGGESTED, ScheduledStatus, StatusSource, UserStatus,
};

pub enum StatusEvent {
    /// The status to set; `None` clears it.
    Save(Option<UserStatus>),
    /// Schedule a status to activate automatically at a future time (`F-SHELL-018`).
    Schedule(ScheduledStatus),
    /// Remove a pending scheduled status by index (`F-SHELL-019`).
    RemoveScheduled(usize),
    Cancel,
}

/// The "Set a status" dialog body. Like Slack's it has two steps: an empty field over the
/// workspace's suggestions, then (once a suggestion is picked or text typed) the status with
/// its Duration select and the pause checkbox.
pub struct StatusPicker {
    emoji: String,
    /// An emoji was chosen (or loaded): until then the field shows Slack's smiley outline.
    emoji_set: bool,
    text: Entity<TextareaState>,
    has_text: bool,
    clear_after: usize,
    pause: bool,
    /// The workspace the suggestions are for ("For Norce").
    workspace: SharedString,
    /// Workspace-configured status presets (`F-SHELL-015`); falls back to [`SUGGESTED`] when empty.
    presets: Vec<(String, String, usize)>,
    /// Recently saved custom statuses in this session (newest first, max 3).
    recent: Vec<(String, String, usize)>,
    /// Upcoming scheduled statuses shown for management (`F-SHELL-019`).
    scheduled: Vec<ScheduledStatus>,
    /// Origin of the currently loaded status (automatic vs manual; `F-SHELL-020`).
    source: StatusSource,
    emoji_open: bool,
    clear_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<StatusEvent> for StatusPicker {}

impl StatusPicker {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<StatusPicker> {
        cx.new(|cx| {
            let text = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("What's your status?")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &text,
                window,
                |this: &mut StatusPicker, text, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        this.has_text = !text.read(cx).value().trim().is_empty();
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.save(cx),
                    _ => {}
                },
            )];
            StatusPicker {
                emoji: EMOJI_CHOICES[0].to_owned(),
                emoji_set: false,
                text,
                has_text: false,
                clear_after: 0,
                pause: false,
                workspace: SharedString::default(),
                presets: Vec::new(),
                recent: Vec::new(),
                scheduled: Vec::new(),
                source: StatusSource::Manual,
                emoji_open: false,
                clear_open: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Install workspace-configured suggested statuses (`team.prefs.custom_status_default_suggestions`).
    pub fn set_presets(&mut self, presets: Vec<(String, String, usize)>, cx: &mut Context<Self>) {
        self.presets = presets;
        cx.notify();
    }

    /// Sync upcoming scheduled statuses so the picker can display and cancel them (`F-SHELL-019`).
    pub fn set_scheduled(&mut self, scheduled: Vec<ScheduledStatus>, cx: &mut Context<Self>) {
        self.scheduled = scheduled;
        cx.notify();
    }

    fn effective_suggestions(&self) -> Vec<(String, String, usize)> {
        if !self.presets.is_empty() {
            self.presets.clone()
        } else {
            SUGGESTED
                .iter()
                .map(|(code, text, clear)| ((*code).to_owned(), (*text).to_owned(), *clear))
                .collect()
        }
    }

    /// Load `status` (or a blank draft) for `workspace` before the dialog opens.
    pub fn edit(
        &mut self,
        status: Option<&UserStatus>,
        workspace: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace = workspace;
        self.load(status, window, cx);
    }

    /// Put the caret in the status field (once the dialog is up).
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.text.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn load(&mut self, status: Option<&UserStatus>, window: &mut Window, cx: &mut Context<Self>) {
        let (emoji, text, clear_after, pause, source) = match status {
            Some(status) => (
                status.emoji.clone(),
                status.text.clone(),
                status.clear_after,
                status.pause,
                status.source,
            ),
            None => (
                EMOJI_CHOICES[0].to_owned(),
                String::new(),
                0,
                false,
                StatusSource::Manual,
            ),
        };
        self.emoji = emoji;
        self.emoji_set = status.is_some();
        self.clear_after = clear_after;
        self.pause = pause;
        self.source = source;
        self.has_text = !text.is_empty();
        self.emoji_open = false;
        self.clear_open = false;
        self.text
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let text = self.text.read(cx).value().trim().to_owned();
        let status = (!text.is_empty()).then(|| {
            let mut s = UserStatus {
                emoji: self.emoji.clone(),
                text: text.clone(),
                clear_after: self.clear_after,
                pause: self.pause,
                expiration: None,
                source: StatusSource::Manual,
            };
            s.expiration = s.clears_at();
            self.recent.retain(|(_, t, _)| t != &text);
            self.recent
                .insert(0, (self.emoji.clone(), text, self.clear_after));
            self.recent.truncate(3);
            s
        });
        cx.emit(StatusEvent::Save(status));
    }

    fn pick_suggestion(
        &mut self,
        emoji: String,
        text: String,
        clear_after: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load(
            Some(&UserStatus {
                emoji,
                text,
                clear_after,
                pause: false,
                expiration: None,
                source: StatusSource::Manual,
            }),
            window,
            cx,
        );
    }
}

impl Render for StatusPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let m = crate::preferences::control_metrics(cx);
        let focused = self.text.read(cx).focus_handle(cx).is_focused(window);
        let theme = cx.theme();
        let (border, muted, hover, popover, ring, accent_fg, accent) = (
            theme.input,
            theme.muted_foreground,
            theme.accent,
            theme.popover,
            theme.ring,
            theme.sidebar_accent_foreground,
            theme.sidebar_accent,
        );
        let foreground = theme.foreground;
        let glyph = emoji_glyph(self.emoji.as_str()).unwrap_or("\u{1F4AC}");
        let show_glyph = self.emoji_set || self.has_text;

        let field = h_flex()
            .items_center()
            .gap_1()
            .px_1p5()
            .h(m.height)
            .rounded_md()
            .border_1()
            .border_color(if focused { ring } else { border })
            .child(
                div()
                    .id("status-emoji")
                    .size(px(if m.compact { 22. } else { 24. }))
                    .flex_none()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .when(show_glyph, |button| button.child(glyph))
                    .when(!show_glyph, |button| {
                        button.child(
                            Icon::new(IconName::FaceSlightlySmiling)
                                .text_color(muted)
                                .size(px(16.)),
                        )
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.emoji_open = !this.emoji_open;
                        cx.notify();
                    })),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Textarea::new(&self.text)
                        .with_size(m.input_size)
                        .appearance(false)
                        .bordered(false),
                ),
            )
            .when(self.has_text, |row| {
                row.child(
                    div()
                        .id("status-clear")
                        .size(px(if m.compact { 22. } else { 24. }))
                        .flex_none()
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|el| el.bg(hover))
                        .child(
                            Icon::new(IconName::CircleX)
                                .text_color(foreground)
                                .size(px(15.)),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.load(None, window, cx))),
                )
            });

        let emoji_grid = self.emoji_open.then(|| {
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(popover)
                .children(EMOJI_CHOICES.iter().enumerate().map(|(ix, &code)| {
                    div()
                        .id(("status-emoji-choice", ix))
                        .size(px(32.))
                        .rounded_sm()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_lg()
                        .hover(|el| el.bg(hover))
                        .child(emoji_glyph(code).unwrap_or("?"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.emoji = code.to_owned();
                            this.emoji_set = true;
                            this.emoji_open = false;
                            cx.notify();
                        }))
                }))
        });

        let suggestions = (!self.has_text).then(|| {
            let workspace_list = self.effective_suggestions();
            let recent_list = self.recent.clone();
            v_flex()
                .gap_0p5()
                .when(!recent_list.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .pb_1()
                            .child("Recent"),
                    )
                    .children(recent_list.into_iter().enumerate().map(
                        |(ix, (code, text, clear))| {
                            let clear_label =
                                CLEAR_AFTER.get(clear).copied().unwrap_or(CLEAR_AFTER[0]);
                            let (c_click, t_click) = (code.clone(), text.clone());
                            h_flex()
                                .id(("status-recent", ix))
                                .items_center()
                                .gap_2()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .text_sm()
                                .hover(|el| el.bg(hover))
                                .child(emoji_glyph(&code).unwrap_or("\u{1F4AC}"))
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(text))
                                .child(
                                    div()
                                        .text_color(muted)
                                        .child(format!("\u{2014} {clear_label}")),
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.pick_suggestion(
                                        c_click.clone(),
                                        t_click.clone(),
                                        clear,
                                        window,
                                        cx,
                                    )
                                }))
                        },
                    ))
                    .child(div().h(px(6.)))
                })
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .pb_1()
                        .child(format!("For {}", self.workspace)),
                )
                .children(workspace_list.into_iter().enumerate().map(
                    |(ix, (code, text, clear))| {
                        let clear_label = CLEAR_AFTER.get(clear).copied().unwrap_or(CLEAR_AFTER[0]);
                        let (c_click, t_click) = (code.clone(), text.clone());
                        h_flex()
                            .id(("status-suggestion", ix))
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .hover(|el| el.bg(hover))
                            .child(emoji_glyph(&code).unwrap_or("\u{1F4AC}"))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(text))
                            .child(
                                div()
                                    .text_color(muted)
                                    .child(format!("\u{2014} {clear_label}")),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick_suggestion(
                                    c_click.clone(),
                                    t_click.clone(),
                                    clear,
                                    window,
                                    cx,
                                )
                            }))
                    },
                ))
        });

        // Step two: once there is a status, its Duration and the pause checkbox.
        let this = cx.entity().downgrade();
        let pause = Checkbox::new("status-pause")
            .label("Pause notifications until status is cleared")
            .checked(self.pause)
            .on_click(move |checked, _, cx| {
                this.update(cx, |picker, cx| {
                    picker.pause = *checked;
                    cx.notify();
                })
                .ok();
            });
        let clear_select = v_flex()
            .gap_1p5()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Duration"),
            )
            .child(
                h_flex()
                    .id("status-clear-after")
                    .w_full()
                    .h(m.height)
                    .items_center()
                    .justify_between()
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .cursor_pointer()
                    .text_sm()
                    .child(
                        CLEAR_AFTER
                            .get(self.clear_after)
                            .copied()
                            .unwrap_or(CLEAR_AFTER[0]),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .text_color(muted)
                            .size(px(14.)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.clear_open = !this.clear_open;
                        cx.notify();
                    })),
            )
            .when(self.clear_open, |col| {
                col.child(
                    v_flex()
                        .w_full()
                        .p_1()
                        .rounded_md()
                        .border_1()
                        .border_color(border)
                        .bg(popover)
                        .children(CLEAR_AFTER.iter().enumerate().map(|(ix, label)| {
                            let on = ix == self.clear_after;
                            div()
                                .id(("status-clear-option", ix))
                                .px_2()
                                .py_1()
                                .rounded_sm()
                                .cursor_pointer()
                                .text_sm()
                                .when(on, |row| row.bg(accent).text_color(accent_fg))
                                .when(!on, |row| row.hover(|el| el.bg(hover)))
                                .child(*label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.clear_after = ix;
                                    this.clear_open = false;
                                    cx.notify();
                                }))
                        })),
                )
            });

        let footer = h_flex()
            .justify_end()
            .gap_2()
            .pt_2()
            .child(
                crate::preferences::form_button(
                    Button::new("status-cancel").outline().label("Cancel"),
                    cx,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.emit(StatusEvent::Cancel))),
            )
            .child(
                crate::preferences::form_button(
                    Button::new("status-save")
                        .primary()
                        .label("Save")
                        .disabled(!self.has_text),
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
            );
        let schedule_row = v_flex()
            .gap_1p5()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .child("Or schedule for later"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1p5()
                    .child(
                        Button::new("status-sched-1h")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("In 1 hour")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Seconds(3_600).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    )
                    .child(
                        Button::new("status-sched-tomorrow")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("Tomorrow 9:00 AM")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Days(1).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    )
                    .child(
                        Button::new("status-sched-next-week")
                            .outline()
                            .with_size(gpui_kit::component::Size::XSmall)
                            .label("Next week 9:00 AM")
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.text.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    cx.emit(StatusEvent::Schedule(ScheduledStatus {
                                        emoji: this.emoji.clone(),
                                        text,
                                        starts_at: PauseLength::Days(7).until(),
                                        clear_after: this.clear_after,
                                        pause: this.pause,
                                    }));
                                }
                            })),
                    ),
            );
        let details = self.has_text.then(|| {
            v_flex()
                .gap_3()
                .child(clear_select)
                .child(pause)
                .child(schedule_row)
        });

        let upcoming = (!self.scheduled.is_empty()).then(|| {
            v_flex()
                .gap_1()
                .pt_1()
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child("Scheduled statuses"),
                )
                .children(self.scheduled.iter().enumerate().map(|(ix, sched)| {
                    let glyph = emoji_glyph(&sched.emoji).unwrap_or("\u{1F4AC}");
                    let when = clock(sched.starts_at, None);
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(hover.opacity(0.45))
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .child(glyph)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .child(sched.text.clone()),
                                )
                                .child(
                                    div().text_xs().text_color(muted).child(format!("· {when}")),
                                ),
                        )
                        .child(
                            Button::new(("status-sched-remove", ix))
                                .ghost()
                                .with_size(gpui_kit::component::Size::XSmall)
                                .icon(IconName::X)
                                .tooltip("Cancel scheduled status")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if ix < this.scheduled.len() {
                                        this.scheduled.remove(ix);
                                    }
                                    cx.emit(StatusEvent::RemoveScheduled(ix));
                                    cx.notify();
                                })),
                        )
                }))
        });

        let auto_source_cue = self.source.badge_label().map(|label| {
            h_flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(hover.opacity(0.45))
                .text_xs()
                .text_color(muted)
                .child(
                    Icon::new(IconName::Sparkles)
                        .size(px(13.))
                        .text_color(muted),
                )
                .child(div().flex_1().min_w_0().child(format!(
                    "{label} \u{b7} Editing replaces this automatic status with a manual one"
                )))
        });

        let visibility_cue = {
            let ws = if self.workspace.is_empty() {
                "your workspace".to_owned()
            } else {
                self.workspace.to_string()
            };
            h_flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(Icon::new(IconName::Globe).size(px(12.)).text_color(muted))
                .child(format!(
                    "Visible to everyone in {ws} and connected Slack Connect channels"
                ))
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(field)
            .children(auto_source_cue)
            .children(emoji_grid)
            .children(suggestions)
            .children(details)
            .children(upcoming)
            .child(visibility_cue)
            .child(footer)
    }
}
