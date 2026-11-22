//! The macOS menu bar: Slack desktop's menus (read from its accessibility tree), each item an
//! action the app already handles, so the menus show the keymap's own shortcuts and an item
//! works the same from the menu and the keyboard. GPUI greys out an item whose action has no
//! handler in focus. Slack's items for features slackless does not have yet are left out
//! rather than shown dead. AppKit adds its own items to the Edit and Window menus
//! (Dictation, Emoji & Symbols, window tiling, the window list), as it does to Slack's.
//!
//! `init(&mut App)` registers the app-wide commands (About, Quit, Hide, the window commands);
//! `set(&MenuBar, &mut App)` installs the menus, which `App::update_menus` rebuilds when the
//! workspaces or the History menu's places change.

use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::gpui::{
    App, KeyBinding, Menu, MenuItem, OsAction, SharedString, SystemMenuType, Window, actions,
};
use slackless_core::platform::{self, AboutPanel as _, FullScreenItem as _};
use slackless_core::types::ChannelId;

use crate::app::{
    ActualSize, CreateChannelAction, FocusSearch, JumpToUnread, NavigateBack, NavigateForward,
    NewMessage, NextWorkspace, OpenActivity, OpenChannelBrowser, OpenDms, OpenDownloads,
    OpenDrafts, OpenFiles, OpenHome, OpenLater, OpenOwnProfile, OpenPeople, OpenPreferences,
    OpenRecent, OpenThreads, OpenUnreads, PreviousWorkspace, QuickSwitch, SearchConversation,
    ShowShortcuts, SignOut, SwitchWorkspace, ToggleRightPane, ToggleWorkspaceRail, ZoomIn, ZoomOut,
};

actions!(
    menu_bar,
    [
        About,
        Quit,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ToggleFullScreen,
        OpenHelpCenter,
    ]
);

/// Slack's Help Center, which its Help menu opens.
const HELP_CENTER: &str = "https://slack.com/help";

/// The menus' moving parts.
#[derive(PartialEq)]
pub struct MenuBar {
    /// Workspace names in switcher order (File > Workspace, Sign Out).
    pub workspaces: Vec<SharedString>,
    /// The History menu's places, newest first: Slack's label and the conversation.
    pub recent: Vec<(SharedString, ChannelId)>,
}

pub fn init(cx: &mut App) {
    platform::current().drop_system_full_screen_item();
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("alt-cmd-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimize, None),
        KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
    ]);
    cx.on_action(|_: &About, _| platform::current().show_about());
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &OpenHelpCenter, cx| cx.open_url(HELP_CENTER));
    cx.on_action(|_: &Minimize, cx| on_window(cx, Window::minimize_window));
    cx.on_action(|_: &Zoom, cx| on_window(cx, Window::zoom_window));
    cx.on_action(|_: &ToggleFullScreen, cx| on_window(cx, Window::toggle_fullscreen));
}

/// Run a window command on the frontmost window once the action has been dispatched: the
/// menu dispatches through that window, which cannot be updated again until it is done.
fn on_window(cx: &mut App, command: fn(&Window)) {
    cx.defer(move |cx| {
        if let Some(window) = cx.active_window() {
            // Fails only for a window closed meanwhile, which leaves nothing to do.
            let _ = window.update(cx, |_, window, _| command(window));
        }
    });
}

/// Install Slack's menus with `bar`'s workspaces and places.
pub fn set(bar: &MenuBar, cx: &mut App) {
    let workspaces = bar.workspaces.iter().enumerate();
    cx.set_menus([
        Menu::new("Slackless").items([
            MenuItem::action("About Slackless", About),
            MenuItem::separator(),
            MenuItem::action("Preferences\u{2026}", OpenPreferences),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Slackless", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new("Sign Out").items(
                    workspaces
                        .clone()
                        .map(|(ix, name)| MenuItem::action(name.clone(), SignOut(ix))),
                ),
            ),
            MenuItem::separator(),
            MenuItem::action("Quit Slackless", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Message", NewMessage),
            MenuItem::action("Create a Channel\u{2026}", CreateChannelAction),
            MenuItem::separator(),
            MenuItem::submenu(
                Menu::new("Workspace").items(
                    workspaces
                        .map(|(ix, name)| MenuItem::action(name.clone(), SwitchWorkspace(ix)))
                        .chain([
                            MenuItem::separator(),
                            MenuItem::action("Select Next Workspace", NextWorkspace),
                            MenuItem::action("Select Previous Workspace", PreviousWorkspace),
                        ]),
                ),
            ),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Search", FocusSearch),
            MenuItem::separator(),
            MenuItem::action("Find\u{2026}", SearchConversation),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Workspace Switcher", ToggleWorkspaceRail),
            MenuItem::action("Toggle Right Sidebar", ToggleRightPane),
            MenuItem::action("Toggle Full Screen", ToggleFullScreen),
            MenuItem::separator(),
            MenuItem::action("Actual Size", ActualSize),
            MenuItem::action("Zoom In", ZoomIn),
            MenuItem::action("Zoom Out", ZoomOut),
        ]),
        Menu::new("Go").items([
            MenuItem::action("Switch to Channel", QuickSwitch),
            MenuItem::action("Jump to Unread", JumpToUnread),
            MenuItem::separator(),
            MenuItem::submenu(Menu::new("History").items(back_forward())),
            MenuItem::separator(),
            MenuItem::action("Home", OpenHome),
            MenuItem::action("Threads", OpenThreads),
            MenuItem::action("All Unreads", OpenUnreads),
            MenuItem::action("All DMs", OpenDms),
            MenuItem::action("Drafts & sent", OpenDrafts),
            MenuItem::action("Activity", OpenActivity),
            MenuItem::action("Later", OpenLater),
            MenuItem::separator(),
            MenuItem::action("Channel Browser", OpenChannelBrowser),
            MenuItem::action("File Browser", OpenFiles),
            MenuItem::action("People & User Groups", OpenPeople),
            MenuItem::separator(),
            MenuItem::action("Downloads", OpenDownloads),
            MenuItem::separator(),
            MenuItem::action("Profile", OpenOwnProfile),
        ]),
        Menu::new("History").items(
            back_forward()
                .into_iter()
                .chain((!bar.recent.is_empty()).then(MenuItem::separator))
                .chain(
                    bar.recent
                        .iter()
                        .map(|(label, id)| MenuItem::action(label.clone(), OpenRecent(id.clone()))),
                ),
        ),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
            MenuItem::action("Open Help Center", OpenHelpCenter),
        ]),
    ]);
}

/// Back and Forward (greyed out with nowhere to go: the root handles them only then).
fn back_forward() -> [MenuItem; 2] {
    [
        MenuItem::action("Back", NavigateBack),
        MenuItem::action("Forward", NavigateForward),
    ]
}
