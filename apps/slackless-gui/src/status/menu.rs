//! Title-bar avatar profile menu and notifications pause hover submenu.

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, InteractiveText, IntoElement, ParentElement as _,
    Render, Stateful, StatefulInteractiveElement as _, Styled as _, StyledText, Subscription,
    TextRun, Window, div, px,
};

use crate::avatars::user_chip;
use crate::fixtures::Fixtures;
use crate::messages::clock;

use super::pause::{PauseEvent, PausePicker};
use super::{PAUSE_OPTIONS, SelfState};

pub enum UserMenuEvent {
    EditStatus,
    ClearStatus,
    SetAway(bool),
    /// Pause notifications until this Unix time; `None` resumes them.
    Pause(Option<i64>),
    OpenProfile,
    OpenPreferences,
    /// Preferences > Notifications, where the schedule lives.
    OpenSchedule,
    /// Slack's "Custom…" pause dialog ([`PausePicker`]).
    OpenPauseCustom,
    /// The workspace's account settings at `<domain>.slack.com`, in the browser.
    OpenAccount,
    /// The user's Downloads folder in Finder (slackless has no download manager).
    OpenDownloads,
    /// Forget this workspace's stored session and quit; the next launch signs in.
    SignOut,
}

/// The menu under the title-bar avatar.
pub struct UserMenu {
    fixtures: Arc<Fixtures>,
    state: SelfState,
    schedule: String,
    /// The Notifications row is under the pointer: its submenu is open (Slack opens it on
    /// hover). The submenu keeps it open while the pointer is over it, so the two hover
    /// flags stay independent and order-independent.
    notif_hover: bool,
    sub_hover: bool,
    /// The hovered row id: Slack turns the whole row, hints included, white on its blue
    /// highlight, which plain hover styles cannot express for the right-aligned hints.
    hovered: Option<&'static str>,
    _pause_subscription: Option<Subscription>,
}

impl EventEmitter<UserMenuEvent> for UserMenu {}

impl UserMenu {
    pub fn new(fixtures: Arc<Fixtures>, cx: &mut App) -> Entity<UserMenu> {
        cx.new(|_| UserMenu {
            fixtures,
            state: SelfState::default(),
            schedule: String::new(),
            notif_hover: false,
            sub_hover: false,
            hovered: None,
            _pause_subscription: None,
        })
    }

    pub fn set_state(&mut self, state: SelfState, schedule: String, cx: &mut Context<Self>) {
        self.state = state;
        self.schedule = schedule;
        cx.notify();
    }

    /// The menu was dismissed: drop hover state so the submenu cannot reappear stale.
    pub fn closed(&mut self, cx: &mut Context<Self>) {
        self.notif_hover = false;
        self.sub_hover = false;
        self.hovered = None;
        cx.notify();
    }

    /// Slack's "Custom…" pause dialog: pick a wall-clock time to pause until.
    pub fn open_pause_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = PausePicker::new(window, cx);
        self._pause_subscription = Some(cx.subscribe_in(
            &picker,
            window,
            |this, _, event: &PauseEvent, window, cx| {
                window.close_dialog(cx);
                this.notif_hover = false;
                this.sub_hover = false;
                if let PauseEvent::Save(until) = event {
                    cx.emit(UserMenuEvent::Pause(Some(*until)));
                }
            },
        ));
        let picker = picker.clone();
        let first_focus = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            // The dialog shell takes focus when it opens; put the caret in the time field
            // on the builder's first run only (it re-runs on rebuilds).
            if first_focus.replace(false) {
                // Split the reads from the focus call: the shared cx borrows must end first.
                let handle = picker.read(cx).text.read(cx).focus_handle(cx);
                handle.focus(window, cx);
            }
            dialog
                .title("Pause notifications")
                .w(px(360.))
                .child(picker.clone())
        });
    }
}

/// A plain menu row sized like Slack's: 28pt tall, 24pt text inset. `hot` is Slack's
/// state-driven hover: the workspace's selected-row blue with every child turned white.
fn row_base(id: &'static str, hot: bool, blue: Hsla, on_blue: Hsla) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .px_6()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_size(px(15.))
        .when(hot, |row| row.bg(blue).text_color(on_blue))
}

/// A row whose children are plain text: hover styles alone flip them white.
fn simple_row(id: impl Into<ElementId>) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(28.))
        .px_6()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .text_size(px(15.))
}

/// Slack's menu metrics (pt): the panel is ~300pt wide, the hover submenu is a ~260pt
/// panel flush to the right edge with its top level with the status box under the header
/// (20pt header top pad + 36pt avatar + 8pt + 8pt margins). Measured off Slack's menu.
const MENU_WIDTH: f32 = 300.;
const SUBMENU_TOP: f32 = 74.;
const SUBMENU_WIDTH: f32 = 260.;

/// Slack's submenu footer; the "in preferences" tail opens Preferences.
const VIP_NOTE: &str = "You can choose to always allow notifications from VIPs ";
const VIP_LINK: &str = "in preferences";

impl Render for UserMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (popover, border, muted, accent, success) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.accent,
            theme.success,
        );
        // Slack highlights the hovered row with the workspace's selected-row blue, every
        // hint on the row turned white with it.
        let (blue, on_blue) = (theme.sidebar_accent, theme.sidebar_accent_foreground);
        let link = theme.link;
        let state = self.state.clone();
        let name = self.fixtures.user_name(&self.fixtures.self_user).to_owned();
        // Slack's menu header avatar carries no presence badge; the "● Active" line below
        // the name is the only presence here (the rail avatar keeps its badge).
        let avatar = user_chip(&self.fixtures, &self.fixtures.self_user, 36.);
        let pause_open = self.notif_hover || self.sub_hover;

        let presence = if state.away { "Away" } else { "Active" };
        let header = h_flex()
            .items_center()
            .gap_3()
            .px_6()
            .pt_5()
            .pb_2()
            .child(avatar)
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .line_height(px(20.))
                            .child(name),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .when(!state.away, |dot| dot.bg(success))
                                    .when(state.away, |dot| dot.border_1().border_color(muted)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(px(16.))
                                    .text_color(muted)
                                    .child(presence),
                            ),
                    ),
            );

        let status_box = h_flex()
            .id("menu-status")
            .mx_6()
            .my_2()
            .h(px(40.))
            .items_center()
            .gap_2()
            .px_2p5()
            .rounded(px(4.))
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_size(px(15.))
            .hover(|boxel| boxel.bg(accent))
            .map(|boxel| match &state.status {
                Some(status) => boxel
                    .child(status.glyph())
                    .child(div().flex_1().truncate().child(status.text.clone()))
                    .child(
                        div()
                            .id("menu-status-clear")
                            .size(px(20.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|x| x.bg(border))
                            .child(Icon::new(IconName::X).text_color(muted).size(px(12.)))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(UserMenuEvent::ClearStatus);
                            })),
                    ),
                None => boxel
                    .text_color(muted)
                    .child(
                        Icon::new(IconName::FaceSlightlySmiling)
                            .text_color(muted)
                            .size(px(16.)),
                    )
                    .child("Update your status"),
            })
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::EditStatus)));

        let away = state.away;
        let away_row = simple_row("menu-away")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Set yourself as")
            .child(
                div()
                    .font_weight(FontWeight::BOLD)
                    .child(if away { "active" } else { "away" }),
            )
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(UserMenuEvent::SetAway(!away))));

        let notif_hot = self.hovered == Some("menu-notifications");
        let hint_color = if notif_hot { on_blue } else { muted };
        let paused = state.paused();
        let pause_label = if paused {
            state
                .paused_until
                .filter(|until| *until != i64::MAX)
                .map(|until| format!("Until {}", clock(until, None)))
                .unwrap_or_else(|| "Paused".to_owned())
        } else {
            "On".to_owned()
        };
        let notifications_row = row_base("menu-notifications", notif_hot, blue, on_blue)
            .justify_between()
            .child("Notifications")
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(hint_color)
                            .child(pause_label),
                    )
                    .child(
                        Icon::new(IconName::ChevronRight)
                            .size(px(14.))
                            .text_color(hint_color),
                    ),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.notif_hover = *hovered;
                this.hovered = hovered.then_some("menu-notifications");
                cx.notify();
            }))
            .on_click(cx.listener(|this, _, _, cx| {
                // Hover opens the submenu; a click pins it for touch input too.
                this.notif_hover = true;
                this.hovered = Some("menu-notifications");
                cx.notify();
            }));

        let prefs_hot = self.hovered == Some("menu-prefs");
        let prefs_row = row_base("menu-prefs", prefs_hot, blue, on_blue)
            .justify_between()
            .child("Preferences")
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if prefs_hot { on_blue } else { muted })
                    .child("\u{2318},"),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-prefs");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenPreferences)));

        let account_hot = self.hovered == Some("menu-account");
        let account_row = row_base("menu-account", account_hot, blue, on_blue)
            .justify_between()
            .child("Account settings")
            .child(
                Icon::new(IconName::ExternalLink)
                    .size(px(14.))
                    .text_color(if account_hot { on_blue } else { muted }),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-account");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenAccount)));

        let downloads_hot = self.hovered == Some("menu-downloads");
        let downloads_row = row_base("menu-downloads", downloads_hot, blue, on_blue)
            .justify_between()
            .child("Downloads")
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(if downloads_hot { on_blue } else { muted })
                    .child("\u{2318}\u{21e7}J"),
            )
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-downloads");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenDownloads)));

        let profile_row = simple_row("menu-profile")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Profile")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenProfile)));

        let signout_row = simple_row("menu-signout")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Sign out")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::SignOut)));

        let separator = || div().my_2().h(px(1.)).w_full().bg(border);

        let font = window.text_style().font();
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let note_text = format!("{VIP_NOTE}{VIP_LINK}.");
        let runs = vec![
            run(VIP_NOTE.len(), muted),
            run(VIP_LINK.len(), link),
            run(1, muted),
        ];
        let this = cx.entity();
        let link_range =
            std::iter::once(VIP_NOTE.len()..VIP_NOTE.len() + VIP_LINK.len()).collect::<Vec<_>>();
        let footer = div().px_6().pt_1().pb_2().text_size(px(12.)).child(
            InteractiveText::new(
                "menu-vip-preferences",
                StyledText::new(note_text).with_runs(runs),
            )
            .on_click(link_range, move |_, _, cx| {
                this.update(cx, |_, cx| cx.emit(UserMenuEvent::OpenPreferences));
            }),
        );

        let resume = paused.then(|| {
            simple_row("menu-resume")
                .hover(|row| row.bg(blue).text_color(on_blue))
                .child("Resume notifications")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::Pause(None))))
        });
        let options = PAUSE_OPTIONS
            .iter()
            .enumerate()
            .map(|(ix, (label, length))| {
                let length = *length;
                simple_row(("menu-pause-option", ix))
                    .hover(|row| row.bg(blue).text_color(on_blue))
                    .child(*label)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(UserMenuEvent::Pause(Some(length.until())));
                    }))
            });
        let custom_row = simple_row("menu-custom")
            .hover(|row| row.bg(blue).text_color(on_blue))
            .child("Custom\u{2026}")
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenPauseCustom)));
        let working_hot = self.hovered == Some("menu-working-hours");
        let schedule_row = row_base("menu-working-hours", working_hot, blue, on_blue)
            .child("Edit your working hours")
            // Slack's submenu shows no schedule text; the current schedule stays in the
            // row's accessibility label.
            .aria_label(if self.schedule.is_empty() {
                "Edit your working hours".to_owned()
            } else {
                format!(
                    "Edit your working hours. Notification schedule: {}",
                    self.schedule
                )
            })
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = hovered.then_some("menu-working-hours");
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(UserMenuEvent::OpenSchedule)));

        let submenu = pause_open.then(|| {
            v_flex()
                .id("menu-submenu")
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .absolute()
                // Flush to the menu's right edge (2pt overlap keeps the hover chain intact
                // while the pointer crosses), top level with the status box, as Slack drops
                // it.
                .left(px(MENU_WIDTH - 2.))
                .top(px(SUBMENU_TOP))
                .w(px(SUBMENU_WIDTH))
                .rounded(px(6.))
                .border_1()
                .border_color(border)
                .bg(popover)
                .shadow_lg()
                .pt_3()
                .pb_3()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.sub_hover = *hovered;
                    cx.notify();
                }))
                .child(
                    h_flex()
                        .h(px(28.))
                        .px_6()
                        .items_center()
                        .text_size(px(13.))
                        .text_color(muted)
                        .child("Pause notifications for\u{2026}"),
                )
                .children(resume)
                .children(options)
                .child(custom_row)
                .child(separator())
                .child(schedule_row)
                .child(separator())
                .child(footer)
        });

        v_flex()
            .relative()
            .w(px(MENU_WIDTH))
            .rounded(px(6.))
            .border_1()
            .border_color(border)
            .bg(popover)
            .shadow_lg()
            .pb_4()
            .child(header)
            .child(status_box)
            .child(away_row)
            .child(notifications_row)
            .child(separator())
            .child(profile_row)
            .child(prefs_row)
            .child(account_row)
            .child(separator())
            .child(downloads_row)
            .child(separator())
            .child(signout_row)
            .children(submenu)
    }
}
