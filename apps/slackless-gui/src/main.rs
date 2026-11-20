//! slackless desktop app (GPUI Kit).
//!
//! Runs live against the signed-in workspace: the first launch opens Slack's sign-in window
//! ([`sign_in`]), later launches reuse the stored session. Without a session (sign-in cancelled,
//! or `SLACKLESS_FIXTURES=1`) the window renders the bundled sample workspace from [`fixtures`]
//! with an offline [`bridge::CoreBridge`]. `slackless-gui --sign-in` signs in again.
//! Module layout and the public interface between views are documented in `app.rs`.
//! `SLACKLESS_GUI_DEMO=browser` / `SLACKLESS_GUI_DEMO=search:<query>` start on the channel
//! browser or a search result list; `SLACKLESS_GUI_DEMO=canvas` / `=list` start in the huddle
//! channel with the huddle bar open on its Canvas or List tab; `=prefs` / `=profile` /
//! `=status` / `=switcher` / `=menu` start on Preferences, a profile, the status picker, the
//! Cmd+K switcher or the user menu (for screenshots); `=later` / `=pins` / `=activity` on
//! Later, the pins panel or the Activity feed, `=details` the Slack Connect channel with its
//! details dialog, and `=notify` posts a test notification.

mod actions;
mod activity;
mod app;
mod avatars;
mod blockkit;
mod bookmarks;
mod bridge;
mod browser;
mod canvas;
mod channel_details;
mod compose_targets;
mod composer;
mod directory;
mod directory_rows;
mod drafts;
mod editor;
mod emoji;
mod files;
mod files_page;
mod fixtures;
mod huddle;
mod huddles;
mod later;
mod lists;
mod media;
mod messages;
mod new_message;
mod pins;
mod preferences;
mod profile;
mod search;
mod search_menu;
mod shortcuts;
mod sidebar;
mod sidebar_menu;
mod sign_in;
mod slash;
mod status;
mod switcher;
mod tab_views;
mod tabs;
mod theme;
mod threads;
mod threads_cards;
mod threads_view;
mod thumbnail;

use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::TitleBar;
use gpui_kit::gpui::{Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size};
use gpui_kit::open_window;

use crate::app::App;
use crate::bridge::CoreBridge;

fn main() {
    if std::env::args().any(|arg| arg == sign_in::FLAG) {
        std::process::exit(sign_in::run_helper());
    }
    let session = sign_in::session();
    let app = gpui_kit::application().with_assets(AllAssets);

    app.run(move |cx| {
        gpui_kit::init(cx);
        theme::init(cx);

        let live = session
            .and_then(|session| CoreBridge::start(session, cx))
            .and_then(|bridge| Some((bridge.snapshot(&Default::default())?, bridge)));
        let (fixtures, bridge) = live.unwrap_or_else(|| (fixtures::load(), CoreBridge::offline()));
        // Live: the Font and Slack's Color Mode (both remembered locally; Lato and `System`
        // until chosen) over the color theme it stores for the workspace, installed before the
        // first frame.
        if let Some(font) = bridge.font() {
            theme::set_font(font, cx);
        }
        if let Some(mode) = bridge.color_mode() {
            let palette = fixtures.palette.unwrap_or(theme::Palette::Default);
            theme::activate_with(mode.resolve(cx.window_appearance()), palette, cx);
        }
        let fixtures = Arc::new(fixtures);
        // Like Slack, the window fills the screen on first launch and later reopens with the
        // frame it was left in (`App` saves it as it changes). A saved frame is restored onto a
        // plain window: restoring onto a zoomed one lets AppKit's zoom state skew the frame.
        // The traffic lights sit inside the 40px top bar, vertically centred, as in Slack's.
        let saved_frame = bridge.window_state();
        let default_bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
        let window_options = WindowOptions {
            window_bounds: Some(if saved_frame.is_some() {
                WindowBounds::Windowed(default_bounds)
            } else {
                WindowBounds::Maximized(default_bounds)
            }),
            titlebar: Some(TitlebarOptions {
                traffic_light_position: Some(point(px(8.), px(12.))),
                ..TitleBar::title_bar_options()
            }),
            // A restored window stays hidden until its saved frame is applied (below).
            show: saved_frame.is_none(),
            ..TitleBar::window_options()
        };

        open_window(window_options, cx, move |window, cx| {
            if let Some(state) = saved_frame {
                // Restore once the window is built, then re-read its size: AppKit's resize
                // notification can miss GPUI (the viewport would keep the 1200x800 default and
                // the rest stay black until the next input).
                window.defer(cx, move |window, cx| {
                    window.restore_native_window_state(&state);
                    window.bounds_changed(cx);
                    window.activate_window();
                });
            }
            App::new(fixtures.clone(), bridge, window, cx)
        })
        .expect("Failed to open window");
    });
}
