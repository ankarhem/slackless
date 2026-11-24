//! Always-on-top movable & closeable floating huddle control panel.
//!
//! Styled to match official Slack's floating dark control strip during screen sharing:
//! - Always floats above all apps (including across Cmd+Tab) via `NSFloatingWindowLevel + 1`
//! - Excluded from ScreenCaptureKit (`NSWindowSharingNone`) so remote viewers never see it
//! - Movable by dragging the grip handle or anywhere on the bar background
//! - Closeable via the `×` button or `Cmd+W`
//! - Simple one-click controls: Mic (mute/unmute), Camera (on/off), Screen Share, Pen (stays on
//!   across Cmd+Tab until disabled + inline color dots), and Close.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DisplayId, FocusHandle, Focusable,
    Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, div, hsla, px, rgb, rgba, size,
};

use crate::huddle_dock::HuddleDock;
use crate::menu_bar::CloseWindow;

const PANEL_HEIGHT: f32 = 54.0;
const PANEL_WIDTH_COMPACT: f32 = 258.0;
const PANEL_WIDTH_PEN: f32 = 398.0;

const PEN_HUES: [(i32, &str); 6] = [
    (0, "Red"),
    (35, "Orange"),
    (55, "Yellow"),
    (135, "Green"),
    (210, "Blue"),
    (285, "Purple"),
];

pub struct HuddleFloatingPanel {
    dock: WeakEntity<HuddleDock>,
    focus: FocusHandle,
    last_pen_open: bool,
    _subs: Vec<Subscription>,
}

impl Focusable for HuddleFloatingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl HuddleFloatingPanel {
    fn new(dock: WeakEntity<HuddleDock>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let initial_pen = dock.upgrade().is_some_and(|d| d.read(cx).draw_mode());
        let mut subs = Vec::new();
        if let Some(d) = dock.upgrade() {
            subs.push(cx.observe_in(&d, window, |this, dock_entity, window, cx| {
                let pen_on = dock_entity.read(cx).draw_mode();
                if pen_on != this.last_pen_open {
                    this.last_pen_open = pen_on;
                    let w = if pen_on {
                        PANEL_WIDTH_PEN
                    } else {
                        PANEL_WIDTH_COMPACT
                    };
                    window.resize(size(px(w), px(PANEL_HEIGHT)));
                }
                cx.notify();
            }));
        }
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                window.focus(&this.focus, cx);
            }
        }));
        Self {
            dock,
            focus,
            last_pen_open: initial_pen,
            _subs: subs,
        }
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Turn off interactive desktop pen capture when closing the floating bar so a full-screen
        // overlay never traps desktop mouse clicks with no panel visible to toggle it off.
        if let Some(dock) = self.dock.upgrade() {
            dock.update(cx, |d, cx| {
                if d.session().sharing && d.draw_mode() {
                    d.set_draw_mode(false, cx);
                }
            });
        }
        window.remove_window();
    }
}

impl Render for HuddleFloatingPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(dock) = self.dock.upgrade() else {
            window.remove_window();
            return div().id("huddle-floating-panel-empty");
        };
        let state = dock.read(cx).view_state();
        let snap = &state.session;
        let muted = snap.muted;
        let video_on = snap.video;
        let sharing = snap.sharing;
        let pen_on = state.draw_mode;
        let pen_hue = state.draw_hue;

        // Official Slack floating dark huddle bar palette (matching reference screenshot).
        let bar_bg = Hsla::from(rgb(0x151A21));
        let bar_border = Hsla::from(rgba(0xFFFFFF24));
        let active_bg = Hsla::from(rgb(0xE6E8EB));
        let active_hover = Hsla::from(rgb(0xF5F6F7));
        let active_fg = Hsla::from(rgb(0x1A1D21));
        let inactive_bg = Hsla::from(rgb(0x4B4F55));
        let inactive_hover = Hsla::from(rgb(0x5D626A));
        let white = Hsla::from(rgb(0xFFFFFF));
        let danger_bg = Hsla::from(rgb(0xE01E5A));
        let danger_hover = Hsla::from(rgb(0xEC326B));
        let pen_active_bg = Hsla::from(rgb(0x1D9BD1));
        let pen_active_hover = Hsla::from(rgb(0x2AAAE0));
        let subtle_fg = Hsla::from(rgba(0xFFFFFF88));

        let dock_mic = dock.clone();
        let mic_btn = div()
            .id("floating-huddle-mic")
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if muted { danger_bg } else { active_bg })
            .hover(move |el| el.bg(if muted { danger_hover } else { active_hover }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(if muted {
                    IconName::MicOff
                } else {
                    IconName::Mic
                })
                .size(px(17.))
                .text_color(if muted { white } else { active_fg }),
            )
            .on_click(move |_, _, cx| {
                dock_mic.update(cx, |d, cx| d.request_toggle_mute(cx));
            });

        let dock_cam = dock.clone();
        let cam_btn = div()
            .id("floating-huddle-camera")
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if video_on { active_bg } else { inactive_bg })
            .hover(move |el| {
                el.bg(if video_on {
                    active_hover
                } else {
                    inactive_hover
                })
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(if video_on {
                    IconName::Video
                } else {
                    IconName::VideoOff
                })
                .size(px(17.))
                .text_color(if video_on { active_fg } else { white }),
            )
            .on_click(move |_, _, cx| {
                dock_cam.update(cx, |d, cx| d.request_toggle_camera(cx));
            });

        let dock_share = dock.clone();
        let share_btn = div()
            .id("floating-huddle-share")
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if sharing { active_bg } else { inactive_bg })
            .hover(move |el| {
                el.bg(if sharing {
                    active_hover
                } else {
                    inactive_hover
                })
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                Icon::new(IconName::ScreenShare)
                    .size(px(17.))
                    .text_color(if sharing { active_fg } else { white }),
            )
            .on_click(move |_, _, cx| {
                dock_share.update(cx, |d, cx| d.request_toggle_screen_share(cx));
            });

        let dock_pen = dock.clone();
        let pen_btn = div()
            .id("floating-huddle-pen")
            .size(px(36.))
            .rounded(px(9.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .bg(if pen_on { pen_active_bg } else { inactive_bg })
            .hover(move |el| {
                el.bg(if pen_on {
                    pen_active_hover
                } else {
                    inactive_hover
                })
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Icon::new(IconName::Pencil).size(px(16.)).text_color(white))
            .on_click(move |_, _, cx| {
                dock_pen.update(cx, |d, cx| d.toggle_draw_mode(cx));
            });

        let swatches = pen_on.then(|| {
            h_flex()
                .items_center()
                .gap_1p5()
                .px_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(PEN_HUES.iter().enumerate().map(|(ix, &(hue, _))| {
                    let selected = pen_hue == hue;
                    let dock_hue = dock.clone();
                    div()
                        .id(("floating-pen-hue", ix))
                        .size(px(16.))
                        .rounded_full()
                        .cursor_pointer()
                        .bg(hsla(hue as f32 / 360.0, 0.88, 0.56, 1.0))
                        .border_2()
                        .border_color(if selected {
                            white
                        } else {
                            Hsla::from(rgba(0xFFFFFF33))
                        })
                        .on_click(move |_, _, cx| {
                            dock_hue.update(cx, |d, cx| d.set_draw_hue(hue, cx));
                        })
                }))
        });

        let close_btn = div()
            .id("floating-huddle-close")
            .size(px(28.))
            .rounded(px(7.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |el| el.bg(Hsla::from(rgba(0xFFFFFF1E))))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Icon::new(IconName::X).size(px(14.)).text_color(subtle_fg))
            .on_click(cx.listener(|this, _, window, cx| {
                this.close(window, cx);
            }));

        // 2×3 grip dots indicating the bar is freely draggable.
        let dot = || {
            div()
                .size(px(3.))
                .rounded_full()
                .bg(Hsla::from(rgba(0xFFFFFF4D)))
        };
        let grip = div()
            .id("floating-huddle-grip")
            .w(px(14.))
            .h(px(28.))
            .cursor_grab()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(dot())
                    .child(dot())
                    .child(dot()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(dot())
                    .child(dot())
                    .child(dot()),
            );

        div()
            .id("huddle-floating-panel-root")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                this.close(window, cx);
            }))
            .size_full()
            .p(px(2.))
            .child(
                h_flex()
                    .id("huddle-floating-panel-bar")
                    .size_full()
                    .px_2p5()
                    .gap_2()
                    .items_center()
                    .rounded(px(13.))
                    .bg(bar_bg)
                    .border_1()
                    .border_color(bar_border)
                    .on_mouse_down(MouseButton::Left, |_, window, _| {
                        window.start_window_move();
                    })
                    .child(grip)
                    .child(mic_btn)
                    .child(cam_btn)
                    .child(share_btn)
                    .child(pen_btn)
                    .when_some(swatches, |row, s| row.child(s))
                    .child(div().w(px(1.)).h(px(22.)).bg(Hsla::from(rgba(0xFFFFFF1F))))
                    .child(close_btn),
            )
    }
}

/// Open the always-on-top movable, closeable floating huddle control panel near the bottom center
/// of `display_id` (or the primary display).
pub fn open_huddle_floating_panel(
    dock: WeakEntity<HuddleDock>,
    display_id: Option<DisplayId>,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let pen_on = dock.upgrade().is_some_and(|d| d.read(cx).draw_mode());
    let w = if pen_on {
        PANEL_WIDTH_PEN
    } else {
        PANEL_WIDTH_COMPACT
    };
    let dim = size(px(w), px(PANEL_HEIGHT));
    let mut bounds = Bounds::centered(display_id, dim, cx);
    // Sit neatly near the bottom of the screen like official Slack's screen-share control bar.
    let display_bounds = display_id
        .and_then(|id| cx.displays().into_iter().find(|d| d.id() == id))
        .or_else(|| cx.primary_display())
        .map(|d| d.bounds());
    if let Some(db) = display_bounds {
        bounds.origin.y =
            (db.origin.y + db.size.height - px(PANEL_HEIGHT + 72.)).max(db.origin.y + px(24.));
    }

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        is_resizable: false,
        is_movable: true,
        is_minimizable: false,
        ..Default::default()
    };

    let handle = cx
        .open_window(options, move |window, cx| {
            window.set_window_title("Huddle Controls");
            #[cfg(target_os = "macos")]
            if let Ok(wh) = raw_window_handle::HasWindowHandle::window_handle(window) {
                if let raw_window_handle::RawWindowHandle::AppKit(appkit) = wh.as_raw() {
                    // SAFETY: GPUI's macOS window yields a valid `NSView *` during window creation.
                    unsafe {
                        slackless_core::huddle::configure_floating_panel(appkit.ns_view.as_ptr());
                    }
                }
            }
            let dock_on_close = dock.clone();
            window.on_window_should_close(cx, move |_, cx| {
                if let Some(d) = dock_on_close.upgrade() {
                    d.update(cx, |d, cx| {
                        if d.session().sharing && d.draw_mode() {
                            d.set_draw_mode(false, cx);
                        }
                    });
                }
                true
            });
            window.activate_window();
            cx.new(|cx| HuddleFloatingPanel::new(dock, window, cx))
        })
        .ok()?;
    Some(handle.into())
}
