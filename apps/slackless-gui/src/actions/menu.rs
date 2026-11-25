//! Message actions popup & context menus plus Slack's menu and single-key focus shortcuts.

use gpui_kit::assets::IconName;
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::gpui::{
    Action, AnyWindowHandle, App, Context, DismissEvent, Focusable as _, Global, KeyBinding,
    ParentElement as _, Styled as _, WeakEntity, Window, actions, div,
};

use super::{ActionHandler, ActionSet, MessageAction, eligible};
use crate::later::Remind;

actions!(
    message_menu,
    [
        EditMessage,
        MarkMessageUnread,
        SaveMessage,
        CopyMessageLink,
        CopyMessageText,
        DeleteMessage
    ]
);

// Slack's one-key shortcuts for the focused message: the message last hovered while no
// field holds the focus (the root binds and routes them through the transcript's
// [`ActionHandler`]). E and Delete act on the signed-in user's messages only.
actions!(
    message_focus,
    [
        FocusEditMessage,
        FocusDeleteMessage,
        FocusReplyInThread,
        FocusForwardMessage,
        FocusTogglePin,
        FocusSaveMessage,
        FocusMarkMessageUnread,
        FocusRemindMe,
        FocusReact,
    ]
);

/// Slack's shortcuts for the open message menu, shown beside its items. The kit draws menus
/// in the root's popover layer, outside any message row, so the actions are handled app-wide
/// and routed to the open menu's message ([`MenuTarget`]).
pub fn bind_menu_keys(cx: &mut App) {
    const MENU: Option<&str> = Some("PopupMenu");
    cx.bind_keys([
        KeyBinding::new("e", EditMessage, MENU),
        KeyBinding::new("u", MarkMessageUnread, MENU),
        KeyBinding::new("shift-cmd-s", SaveMessage, MENU),
        KeyBinding::new("l", CopyMessageLink, MENU),
        KeyBinding::new("cmd-c", CopyMessageText, MENU),
        KeyBinding::new("backspace", DeleteMessage, MENU),
    ]);
    cx.on_action(|_: &EditMessage, cx| shortcut(MessageAction::Edit, cx));
    cx.on_action(|_: &MarkMessageUnread, cx| shortcut(MessageAction::MarkUnread, cx));
    cx.on_action(|_: &SaveMessage, cx| shortcut(MessageAction::SaveForLater, cx));
    cx.on_action(|_: &CopyMessageLink, cx| shortcut(MessageAction::CopyLink, cx));
    cx.on_action(|_: &CopyMessageText, cx| shortcut(MessageAction::CopyText, cx));
    cx.on_action(|_: &DeleteMessage, cx| shortcut(MessageAction::Delete, cx));
}

/// The message whose actions menu opened last: the menu's shortcuts act on it while that
/// menu has focus.
struct MenuTarget {
    set: ActionSet,
    handler: ActionHandler,
    menu: WeakEntity<PopupMenu>,
    window: AnyWindowHandle,
}

impl Global for MenuTarget {}

/// Run a menu shortcut on the open menu's message, closing the menu first as a click does.
/// Edit and delete apply to the signed-in user's messages only, as their items do. The
/// dispatch holds the window, so the shortcut runs once it is released.
fn shortcut(action: MessageAction, cx: &mut App) {
    let Some(window) = cx.try_global::<MenuTarget>().map(|target| target.window) else {
        return;
    };
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            let Some(target) = cx.try_global::<MenuTarget>() else {
                return;
            };
            let Some(menu) = target.menu.upgrade() else {
                return;
            };
            if !eligible(target.set, &action) || !menu.focus_handle(cx).contains_focused(window, cx)
            {
                return;
            }
            let handler = target.handler.clone();
            menu.update(cx, |_, cx| cx.emit(DismissEvent));
            handler(action, window, cx);
        });
    });
}

fn menu_item(
    label: &str,
    icon: IconName,
    handler: &ActionHandler,
    action: MessageAction,
) -> PopupMenuItem {
    let handler = handler.clone();
    PopupMenuItem::new(label.to_owned())
        .icon(icon)
        .on_click(move |_, window, cx| handler(action.clone(), window, cx))
}

/// Where the message actions menu opens from: the row's right-click menu carries Forward
/// and Reply in thread too (no hover bar is up beside it), whereas the hover bar's "More
/// actions" dropdown omits what sits right beside it on the bar.
#[derive(Clone, Copy, PartialEq)]
pub enum MenuOrigin {
    /// The hover bar's "More actions" dropdown.
    More,
    /// The row's right-click menu.
    Context,
}

/// One row of the message actions menu, in Slack's order. [`menu_rows`] computes the pure
/// shape; [`actions_menu`] renders it.
enum MenuRow {
    Item {
        label: &'static str,
        icon: IconName,
        action: MessageAction,
        /// Slack's keyboard shortcut shown beside the item while the menu is open, whose
        /// binding dispatches it (see [`bind_menu_keys`]).
        hint: Option<Box<dyn Action>>,
    },
    /// "Add a reminder" with its presets.
    Reminders,
    /// Slack's destructive "Delete message..." item: no icon, in the danger color.
    Delete,
    Separator,
}

impl MenuRow {
    /// The row's menu label; separators have none.
    #[cfg(test)]
    fn label(&self) -> &'static str {
        match self {
            MenuRow::Item { label, .. } => label,
            MenuRow::Reminders => "Add a reminder",
            MenuRow::Delete => "Delete message...",
            MenuRow::Separator => "",
        }
    }
}

/// The message actions menu's rows in Slack's order for one message and origin: Edit and
/// Delete only on the signed-in user's messages, Reply in thread only where replies are
/// offered.
fn menu_rows(set: ActionSet, origin: MenuOrigin) -> Vec<MenuRow> {
    let edit = MenuRow::Item {
        label: "Edit message",
        icon: IconName::Pencil,
        action: MessageAction::Edit,
        hint: Some(Box::new(EditMessage)),
    };
    let mark_unread = MenuRow::Item {
        label: "Mark unread",
        icon: IconName::SquareDot,
        action: MessageAction::MarkUnread,
        hint: Some(Box::new(MarkMessageUnread)),
    };
    let save = MenuRow::Item {
        label: if set.saved {
            "Remove from saved"
        } else {
            "Add to saved"
        },
        icon: IconName::Bookmark,
        action: MessageAction::SaveForLater,
        hint: Some(Box::new(SaveMessage)),
    };
    let copy_link = MenuRow::Item {
        label: "Copy link",
        icon: IconName::Link2,
        action: MessageAction::CopyLink,
        hint: Some(Box::new(CopyMessageLink)),
    };
    let copy_text = MenuRow::Item {
        label: "Copy message",
        icon: IconName::Type,
        action: MessageAction::CopyText,
        hint: Some(Box::new(CopyMessageText)),
    };
    let pin = MenuRow::Item {
        label: if set.pinned {
            "Unpin from conversation"
        } else {
            "Pin to conversation"
        },
        icon: IconName::Pin,
        action: MessageAction::TogglePin,
        hint: None,
    };
    let add_to_list = MenuRow::Item {
        label: "Add to list…",
        icon: IconName::ListChecks,
        action: MessageAction::AddToList,
        hint: None,
    };
    let history = MenuRow::Item {
        label: "View edit history",
        icon: IconName::Clock,
        action: MessageAction::ViewHistory,
        hint: None,
    };
    let forward = MenuRow::Item {
        label: "Forward message",
        icon: IconName::Forward,
        action: MessageAction::Forward,
        hint: None,
    };
    let mut rows = Vec::new();
    if set.own {
        rows.push(edit);
        rows.push(MenuRow::Separator);
    }
    rows.extend([
        mark_unread,
        save,
        MenuRow::Reminders,
        MenuRow::Separator,
        copy_link,
        copy_text,
        MenuRow::Separator,
        pin,
        add_to_list,
        history,
    ]);
    if set.in_thread {
        rows.push(MenuRow::Item {
            label: "Also send to channel",
            icon: IconName::Send,
            action: MessageAction::SendToChannel,
            hint: None,
        });
    }
    if origin == MenuOrigin::Context {
        rows.push(forward);
        if set.reply {
            rows.push(MenuRow::Item {
                label: "Reply in thread",
                icon: IconName::MessageSquareReply,
                action: MessageAction::ReplyInThread,
                hint: None,
            });
        }
    }
    if set.own {
        rows.push(MenuRow::Separator);
        rows.push(MenuRow::Delete);
    }
    rows
}

/// The full action menu: the hover bar's "more" dropdown and the row's right-click menu
/// ([`MenuOrigin`]), in Slack's order with Slack's shortcut beside each item
/// ([`bind_menu_keys`]). Reacting lives on the hover bar only, as in Slack. `menu` is the
/// fresh popup the framework hands to the builder closure.
pub fn actions_menu(
    menu: PopupMenu,
    set: ActionSet,
    handler: &ActionHandler,
    origin: MenuOrigin,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    // The items' shortcut hints resolve against the menu's own key context, which exists
    // once the menu has painted: draw it once more after its first frame.
    cx.on_next_frame(window, |_, _, cx| cx.notify());
    let menu_entity = cx.entity().downgrade();
    cx.set_global(MenuTarget {
        set,
        handler: handler.clone(),
        menu: menu_entity,
        window: window.window_handle(),
    });
    let remind = PopupMenu::build(window, cx, |mut menu, _, _| {
        for preset in Remind::ALL {
            menu = menu.item(menu_item(
                preset.label(),
                IconName::Clock,
                handler,
                MessageAction::RemindMe(preset),
            ));
        }
        menu.separator().item(menu_item(
            "Custom\u{2026}",
            IconName::Calendar,
            handler,
            MessageAction::RemindCustom,
        ))
    });

    let mut reminders =
        Some(PopupMenuItem::submenu("Add a reminder", remind).icon(IconName::AlarmClock));
    let mut menu = menu;
    for row in menu_rows(set, origin) {
        menu = match row {
            MenuRow::Item {
                label,
                icon,
                action,
                hint,
            } => {
                let item = menu_item(label, icon, handler, action);
                menu.item(match hint {
                    Some(hint) => item.action(hint),
                    None => item,
                })
            }
            MenuRow::Reminders => menu.item(
                reminders
                    .take()
                    .expect("menu_rows carries one reminder submenu"),
            ),
            MenuRow::Delete => {
                // Slack's destructive item: no icon, in the danger color.
                let delete = handler.clone();
                menu.item(
                    PopupMenuItem::element(|_, cx| {
                        h_flex()
                            .w_full()
                            .gap_3()
                            .justify_between()
                            .child(
                                div()
                                    .text_color(crate::theme::danger_text(cx))
                                    .child("Delete message..."),
                            )
                            // The menu's hint for the backspace binding, as Slack labels that key.
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("delete"),
                            )
                    })
                    .action(Box::new(DeleteMessage))
                    .on_click(move |_, window, cx| delete(MessageAction::Delete, window, cx)),
                )
            }
            MenuRow::Separator => menu.separator(),
        };
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(own: bool) -> ActionSet {
        ActionSet {
            own,
            reply: true,
            saved: false,
            pinned: false,
            in_thread: false,
        }
    }

    fn labels(set: ActionSet, origin: MenuOrigin) -> Vec<&'static str> {
        menu_rows(set, origin).iter().map(MenuRow::label).collect()
    }

    #[test]
    fn edit_and_delete_apply_to_own_messages_only() {
        let mine = set(true);
        let theirs = set(false);
        for action in [MessageAction::Edit, MessageAction::Delete] {
            assert!(eligible(mine, &action));
            assert!(!eligible(theirs, &action));
        }
        for action in [
            MessageAction::MarkUnread,
            MessageAction::SaveForLater,
            MessageAction::TogglePin,
            MessageAction::Forward,
            MessageAction::ReplyInThread,
            MessageAction::CopyLink,
        ] {
            assert!(eligible(mine, &action));
            assert!(eligible(theirs, &action));
        }
    }

    #[test]
    fn own_menus_lead_with_edit_and_end_with_delete() {
        for origin in [MenuOrigin::Context, MenuOrigin::More] {
            let rows = labels(set(true), origin);
            assert_eq!(&rows[..2], &["Edit message", ""]);
            assert_eq!(&rows[rows.len() - 2..], &["", "Delete message..."]);
            assert_eq!(rows.iter().filter(|l| **l == "Copy link").count(), 1);
        }
    }

    #[test]
    fn other_menus_omit_edit_and_delete() {
        for origin in [MenuOrigin::Context, MenuOrigin::More] {
            let rows = labels(set(false), origin);
            assert_eq!(rows.first(), Some(&"Mark unread"));
            assert!(!rows.contains(&"Edit message"));
            assert!(!rows.contains(&"Delete message..."));
        }
    }

    #[test]
    fn context_menu_includes_bar_actions_that_more_dropdown_omits() {
        let context = labels(set(false), MenuOrigin::Context);
        assert!(context.contains(&"Forward message"));
        assert!(context.contains(&"Reply in thread"));

        let more = labels(set(false), MenuOrigin::More);
        assert!(!more.contains(&"Forward message"));
        assert!(!more.contains(&"Reply in thread"));
    }

    #[test]
    fn save_and_pin_labels_flip_with_state() {
        let mut saved = set(false);
        saved.saved = true;
        assert!(labels(saved, MenuOrigin::Context).contains(&"Remove from saved"));
        assert!(labels(set(false), MenuOrigin::Context).contains(&"Add to saved"));

        let mut pinned = set(false);
        pinned.pinned = true;
        assert!(labels(pinned, MenuOrigin::Context).contains(&"Unpin from conversation"));
        assert!(labels(set(false), MenuOrigin::Context).contains(&"Pin to conversation"));
    }

    #[test]
    fn reply_row_follows_the_action_set() {
        let mut no_replies = set(false);
        no_replies.reply = false;
        assert!(labels(set(false), MenuOrigin::Context).contains(&"Reply in thread"));
        assert!(!labels(no_replies, MenuOrigin::Context).contains(&"Reply in thread"));
    }
}
