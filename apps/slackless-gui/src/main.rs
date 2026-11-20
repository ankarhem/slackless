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
//! Later, the pins panel or the Activity feed, and `=notify` posts a test notification.

mod actions;
mod activity;
mod app;
mod avatars;
mod blockkit;
mod bookmarks;
mod bridge;
mod browser;
mod canvas;
mod compose_targets;
mod composer;
mod directory;
mod directory_rows;
mod drafts;
mod editor;
mod emoji;
mod files;
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
use gpui_kit::gpui::{WindowBounds, WindowOptions, px, size};
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
        if let Some(appearance) = fixtures.appearance {
            theme::activate_with(appearance.theme, appearance.palette, cx);
        }
        let fixtures = Arc::new(fixtures);
        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1200.), px(800.)), cx)),
            ..Default::default()
        };

        open_window(window_options, cx, move |window, cx| {
            App::new(fixtures.clone(), bridge, window, cx)
        })
        .expect("Failed to open window");
    });
}
