//! Standalone native GPUI window wrapper hosting [`Preferences`] (`F-CORE-011`).

use gpui_kit::component::{ActiveTheme as _, TitleBar, h_flex, v_flex};
use gpui_kit::gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, DisplayId, Entity, FontWeight,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, point, px, size,
};

use super::Preferences;

/// Standalone native OS window wrapper hosting [`Preferences`] (`F-CORE-011`) so theme,
/// font, and layout changes can be previewed side-by-side with the main workspace window.
struct PreferencesWindow {
    preferences: Entity<Preferences>,
}

impl Render for PreferencesWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                TitleBar::new()
                    .h(px(38.))
                    .bg(theme.background)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex().flex_1().h_full().pr_4().items_center().child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Preferences"),
                        ),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.preferences.clone()))
    }
}

/// Open Preferences in a standalone native GPUI window (`920 x 640`, `F-CORE-011`), centered
/// on `display_id` (the main window's display) or the primary display if `None`.
pub fn open_preferences_window(
    preferences: Entity<Preferences>,
    display_id: Option<DisplayId>,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let bounds = Bounds::centered(display_id, size(px(920.), px(640.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        titlebar: Some(TitlebarOptions {
            title: Some(SharedString::from("Preferences")),
            traffic_light_position: Some(point(px(8.), px(12.))),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, move |window, cx| {
        window.set_window_title("Preferences — slackless");
        window.activate_window();
        let prefs_close = preferences.clone();
        window.on_window_should_close(cx, move |_, cx| {
            prefs_close.update(cx, |p, cx| p.stop_mic_probe(cx));
            true
        });
        cx.new(|_| PreferencesWindow { preferences })
    })
    .ok()
    .map(|(handle, _)| handle)
}
