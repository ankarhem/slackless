//! slackless desktop app (GPUI Kit).
//!
//! Runs live against the signed-in workspace: the first launch opens Slack's sign-in window
//! ([`sign_in`]), later launches reuse the stored session. Without a session (sign-in cancelled,
//! or `SLACKLESS_FIXTURES=1`) the window renders the bundled sample workspace from [`fixtures`]
//! with an offline [`bridge::CoreBridge`]. `slackless-gui --sign-in` signs in again.
//! The root view lives in [`app`]; views talk upward through event enums the root subscribes to.
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
mod catchup;
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
mod huddle_dock;
mod huddle_invite;
mod huddle_state;
mod huddle_video;
mod huddles;
mod image_viewer;
mod later;
mod lists;
mod media;
mod menu_bar;
mod messages;
mod new_message;
mod outbox;
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
mod tools_page;
mod ui;
mod unreads;
mod updater;
mod zoom;

use std::sync::Arc;

use gpui_kit::assets::AllAssets;
use gpui_kit::component::TitleBar;
use gpui_kit::gpui::{
    Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size,
};
use gpui_kit::open_window;
use slackless_platform::{self as platform, IconSetter as _};

use crate::app::App;
use crate::bridge::{COLOR_MODE, CoreBridge, FONT, SETTINGS, WINDOW_STATE, ZOOM};
use crate::preferences::AppearanceMode;

fn main() {
    slackless_core::huddle::install_default_crypto_provider();
    if std::env::args().any(|arg| arg == sign_in::FLAG) {
        std::process::exit(sign_in::run_helper());
    }
    let _log_path = platform::init_file_logging("slackless-gui");
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
    let (url_tx, mut url_rx) = futures::channel::mpsc::unbounded::<Vec<String>>();
    if !startup_urls.is_empty() {
        let _ = url_tx.unbounded_send(startup_urls);
    }
    let url_tx_open = url_tx.clone();
    app.on_open_urls(move |urls| {
        if !urls.is_empty() {
            let _ = url_tx_open.unbounded_send(urls);
        }
    });

    app.run(move |cx| {
        cx.set_global(zoom);
        // The Dock icon: an unbundled run has no bundle to take one from.
        platform::current().set_icon(include_bytes!("../icon.png"));
        gpui_kit::init(cx);
        theme::init(cx);
        menu_bar::init(cx);

        // Start bridges and read only each workspace's 1-row Team summary for the rail; build the
        // full SQLite snapshot ONLY for workspace 0 so background workspaces never slow cold launch.
        let mut live_bridges: Vec<(slackless_core::types::Team, CoreBridge)> = sessions
            .into_iter()
            .filter_map(|session| {
                let bridge = CoreBridge::start(session, cx)?;
                let team = bridge.team_summary()?;
                Some((team, bridge))
            })
            .collect();
        // Restore persisted workspace rail order (`F-CORE-003`) if present.
        if live_bridges.len() > 1
            && let Some(order) = live_bridges
                .iter()
                .find_map(|(_, b)| b.workspace_order())
                .filter(|o| !o.is_empty())
        {
            live_bridges.sort_by_key(|(team, _)| {
                order
                    .iter()
                    .position(|wanted| wanted == team.id.as_str())
                    .unwrap_or(usize::MAX)
            });
        }
        let (fixtures, bridges) = if !live_bridges.is_empty() {
            let teams: Vec<_> = live_bridges.iter().map(|(team, _)| team.clone()).collect();
            let bridges: Vec<CoreBridge> = live_bridges.into_iter().map(|(_, b)| b).collect();
            bridges[0].activate_media(cx);
            let mut first = bridges[0]
                .snapshot(&Default::default())
                .unwrap_or_else(fixtures::load);
            first.set_rail_workspaces(teams);
            (first, bridges)
        } else {
            (fixtures::load(), vec![CoreBridge::offline()])
        };
        let bridge = &bridges[0];
        let local = bridge.all_local();
        // Live: the Font and Slack's Color Mode (both remembered locally; Lato and `System`
        // until chosen) over the color theme it stores for the workspace, installed before the
        // first frame (all read from the single batched `local` map).
        if let Some(font) = FONT.read(&local) {
            theme::set_font(font, cx);
        }
        // The remembered zoom, before the first frame lays out.
        if let Some(level) = ZOOM.read(&local) {
            zoom::set_level(level, cx);
        }
        if let Some(mode) = bridge
            .store()
            .map(|_| COLOR_MODE.read(&local).unwrap_or(AppearanceMode::System))
        {
            let saved = SETTINGS.read(&local);
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
        let saved_frame = WINDOW_STATE.read(&local);
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
        // Route POSIX SIGTERM / SIGINT (e.g. `pkill`, Ctrl-C) into GPUI's normal quit path so
        // `on_app_quit` cleanly leaves any active huddle before terminating.
        {
            use futures::StreamExt as _;
            let (term_tx, mut term_rx) = futures::channel::mpsc::unbounded::<()>();
            platform::install_termination_monitor(move || {
                let _ = term_tx.unbounded_send(());
            });
            cx.spawn(async move |cx| {
                if term_rx.next().await.is_some() {
                    cx.update(|cx| cx.quit());
                }
            })
            .detach();
        }
        // Drain OS `on_open_urls` and startup CLI deep links (`slack://`, `app.slack.com/client/...`,
        // `*.slack.com/archives/...`) via an async channel (zero idle polling).
        cx.spawn(async move |cx| {
            use futures::StreamExt as _;
            while let Some(urls) = url_rx.next().await {
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
