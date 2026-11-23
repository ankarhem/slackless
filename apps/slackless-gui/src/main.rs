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
//! Later, the pins panel or the Activity feed, `=unreads` the All Unreads view, `=details`
//! the Slack Connect channel with its details dialog, `=notify` posts a test notification,
//! `=typing` shows two people typing in the open conversation and `=message` delivers a demo
//! direct message (notification and sound).

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
mod code_font;
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
mod hover_card;
mod huddle;
mod huddles;
mod image_viewer;
mod later;
mod lists;
mod media;
mod menu_bar;
mod messages;
mod new_message;
mod pins;
mod popout;
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
mod syntax;
mod tab_views;
mod tabs;
mod theme;
mod threads;
mod threads_cards;
mod threads_view;
mod thumbnail;
mod unreads;
mod zoom;

use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::TitleBar;
use gpui_kit::gpui::{
    Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use gpui_kit::open_window;
use slackless_core::platform::{self, IconSetter as _};

use crate::app::App;
use crate::bridge::CoreBridge;

fn main() {
    if std::env::args().any(|arg| arg == sign_in::FLAG) {
        std::process::exit(sign_in::run_helper());
    }
    let startup_urls: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| {
            arg.starts_with("slack://")
                || arg.starts_with("https://app.slack.com/")
                || arg.contains(".slack.com/archives/")
        })
        .collect();
    let sessions = sign_in::sessions();
    let zoom = zoom::Zoom::default();
    let app = Application::with_platform(zoom.platform()).with_assets(AllAssets);
    type MainWindowSlot = std::rc::Rc<
        std::cell::RefCell<
            Option<(
                gpui_kit::gpui::WeakEntity<App>,
                gpui_kit::gpui::AnyWindowHandle,
            )>,
        >,
    >;
    let main_app: MainWindowSlot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let pending_urls: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(startup_urls));
    let pending_for_open = pending_urls.clone();
    app.on_open_urls(move |urls| {
        pending_for_open.borrow_mut().extend(urls);
    });

    app.run(move |cx| {
        cx.set_global(zoom);
        // The Dock icon: an unbundled run has no bundle to take one from.
        platform::current().set_icon(include_bytes!("../icon.png"));
        gpui_kit::init(cx);
        theme::init(cx);
        menu_bar::init(cx);

        let mut live_pairs: Vec<(fixtures::Fixtures, CoreBridge)> = sessions
            .into_iter()
            .filter_map(|session| {
                let bridge = CoreBridge::start(session, cx)?;
                let snapshot = bridge.snapshot(&Default::default())?;
                Some((snapshot, bridge))
            })
            .collect();
        // Restore persisted workspace rail order (`F-CORE-003`) if present.
        if live_pairs.len() > 1 {
            if let Some(order) = live_pairs
                .iter()
                .find_map(|(_, b)| b.workspace_order())
                .filter(|o| !o.is_empty())
            {
                live_pairs.sort_by_key(|(snap, _)| {
                    let id = snap
                        .workspaces
                        .first()
                        .map(|t| t.id.as_str())
                        .unwrap_or_default();
                    order
                        .iter()
                        .position(|wanted| wanted == id)
                        .unwrap_or(usize::MAX)
                });
            }
        }
        let (fixtures, bridges) = if !live_pairs.is_empty() {
            let teams: Vec<_> = live_pairs
                .iter()
                .filter_map(|(snap, _)| snap.workspaces.first().cloned())
                .collect();
            let mut snapshots = Vec::with_capacity(live_pairs.len());
            let mut bridges = Vec::with_capacity(live_pairs.len());
            for (snap, bridge) in live_pairs {
                snapshots.push(snap);
                bridges.push(bridge);
            }
            bridges[0].activate_media(cx);
            let mut first = snapshots.remove(0);
            first.workspaces = teams;
            (first, bridges)
        } else {
            (fixtures::load(), vec![CoreBridge::offline()])
        };
        let bridge = &bridges[0];
        // Live: the Font and Slack's Color Mode (both remembered locally; Lato and `System`
        // until chosen) over the color theme it stores for the workspace, installed before the
        // first frame.
        if let Some(font) = bridge.font() {
            theme::set_font(font, cx);
        }
        // The remembered zoom, before the first frame lays out.
        if let Some(level) = bridge.zoom() {
            zoom::set_level(level, cx);
        }
        if let Some(mode) = bridge.color_mode() {
            let saved = bridge.settings();
            let palette = saved
                .as_ref()
                .map(|s| s.palette)
                .or(fixtures.palette)
                .unwrap_or(theme::Palette::Default);
            let darker_sidebars = saved.as_ref().is_some_and(|s| s.darker_sidebars);
            let window_gradient = saved.as_ref().is_none_or(|s| s.window_gradient);
            theme::activate_with(
                mode.resolve(cx.window_appearance()),
                palette,
                darker_sidebars,
                window_gradient,
                false,
                cx,
            );
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

        let main_app_for_win = main_app.clone();
        let pending_for_poll = pending_urls.clone();
        let handle = open_window(window_options, cx, move |window, cx| {
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
            let entity = App::new_multi(fixtures.clone(), bridges, window, cx);
            *main_app_for_win.borrow_mut() = Some((entity.downgrade(), window.window_handle()));
            entity
        })
        .expect("Failed to open window");
        let _ = handle;
        // Drain OS `on_open_urls` and startup CLI deep links (`slack://`, `app.slack.com/client/...`,
        // `*.slack.com/archives/...`) into the main App window.
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(120))
                    .await;
                let urls: Vec<String> = {
                    let mut guard = pending_for_poll.borrow_mut();
                    std::mem::take(&mut *guard)
                };
                if urls.is_empty() {
                    continue;
                }
                let target = main_app.borrow().clone();
                if let Some((weak, win_handle)) = target {
                    cx.update(|cx| {
                        let _ = win_handle.update(cx, |_, window, cx| {
                            window.activate_window();
                            let _ = weak.update(cx, |app, cx| {
                                for url in urls {
                                    app.handle_deep_link(&url, window, cx);
                                }
                            });
                        });
                    });
                }
            }
        })
        .detach();
    });
}
