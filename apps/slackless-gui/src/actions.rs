//! Message actions shared by the transcript (`messages.rs`) and the thread panel
//! (`threads.rs`), so both columns offer one action set with one look (gpui-kit-guide
//! section 6):
//!
//! - [`hover_toolbar`]: Slack's floating bar revealed on row hover (quick reactions, add
//!   reaction, reply in thread, forward, save for later, and a "more" dropdown);
//! - [`actions_menu`]: the full menu behind "more" and the row's right-click `ContextMenu`
//!   ([`MenuOrigin`] reorders Edit and Delete to the top, as Slack's right-click does), in
//!   Slack's order (edit; mark unread, save, reminder submenu; copy link, copy message; pin,
//!   forward, reply in thread; delete), each with Slack's keyboard shortcut while the menu
//!   is open ([`bind_menu_keys`]);
//! - the `message_focus` actions: Slack's one-key shortcuts for the focused message (the
//!   transcript's last hovered one), which the root binds and routes through the same
//!   [`ActionHandler`];
//! - [`reaction_chips`]: reaction aggregates, interactive (toggle + add) when a handler is
//!   given, static otherwise;
//! - [`edit_box`]: the inline edit draft that replaces a row while it is edited;
//! - the fixture-store mutations both views apply (reactions, edits).
//!
//! Each view supplies an [`ActionHandler`] that maps a [`MessageAction`] onto its own state.

use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Action, Anchor, AnyWindowHandle, App, ClipboardItem, Context, DismissEvent, Div,
    Focusable as _, FontWeight, Global, InteractiveElement as _, KeyBinding, ParentElement as _,
    Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    actions, div, px,
};
use slackless_core::api::MessageContent;
use slackless_core::types::{ChannelId, Edited, Message, Reaction, Ts, UserId};

use crate::emoji;
use crate::fixtures::Fixtures;
use crate::later::Remind;
use gpui_kit::gpui::{self, AnyView, HighlightStyle, StyledText};

/// What the user asked to do with a message.
#[derive(Clone)]
pub enum MessageAction {
    /// Add the signed-in user's reaction (no-op if already reacted).
    React(String),
    /// Toggle the signed-in user's reaction (reaction chip click).
    ToggleReaction(String),
    /// Open the emoji picker anchored at this window position.
    PickReaction(Point<Pixels>),
    ReplyInThread,
    CopyLink,
    Forward,
    SaveForLater,
    /// Save for later with a reminder ("Remind me about this").
    RemindMe(Remind),
    /// Save for later with a user-entered custom local date/time reminder.
    RemindCustom,
    MarkUnread,
    Edit,
    Delete,
    /// Pin or unpin the message in its conversation ("Pin to conversation").
    TogglePin,
    /// Copy the message's text ("Copy message").
    CopyText,
    /// The author's avatar or name was clicked: show their profile.
    OpenProfile(UserId),
    /// A Block Kit button was pressed: the element as the message declared it plus its
    /// `block_id`, and the `id` of the legacy attachment holding it (Giphy's preview).
    BlockAction {
        action: crate::blockkit::Json,
        attachment: Option<i64>,
    },
    /// Dismiss an ephemeral ("Only visible to you") message locally.
    DismissEphemeral,
    /// One of the message's images was clicked: open the image viewer on it.
    OpenImage(crate::image_viewer::ImageOrigin),
    /// Open the durable local SQLite revision history log for this message.
    ViewHistory,
}

/// Which actions apply to one message.
#[derive(Clone, Copy)]
pub struct ActionSet {
    /// The signed-in user wrote it: Edit and Delete are offered.
    pub own: bool,
    /// Reply in thread is offered (not on a thread root shown in its own panel).
    pub reply: bool,
    /// Currently saved for later: the save action reads "Remove from saved".
    pub saved: bool,
    /// Currently pinned to the conversation: the pin action reads "Unpin from
    /// conversation".
    pub pinned: bool,
}

pub type ActionHandler = Rc<dyn Fn(MessageAction, &mut Window, &mut App)>;

/// Slack's default one-click reactions at the front of the hover bar.
pub const DEFAULT_QUICK_REACTIONS: [&str; 3] = ["white_check_mark", "eyes", "raised_hands"];

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

/// Whether `action` applies to a message with this action set: Edit and Delete apply to
/// the signed-in user's messages only, as their menu items do.
pub fn eligible(set: ActionSet, action: &MessageAction) -> bool {
    !matches!(action, MessageAction::Edit | MessageAction::Delete) || set.own
}

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

/// Slack's hover-bar buttons: 32px squares.
const BAR_BUTTON: Size = Size::Medium;

fn toolbar_button(id: String, handler: &ActionHandler, action: MessageAction) -> Button {
    let handler = handler.clone();
    Button::new(SharedString::from(id))
        .ghost()
        .with_size(BAR_BUTTON)
        .on_click(move |_, window, cx| handler(action.clone(), window, cx))
}

/// The floating hover bar of one message row; revealed while the row's `group` is hovered.
/// `key` makes element ids unique per row; `quick` are the user's one-click reactions.
pub fn hover_toolbar(
    key: &str,
    quick: &[String],
    set: ActionSet,
    handler: &ActionHandler,
    group: SharedString,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    // Like Slack's, the bar straddles the row's top edge.
    let mut bar = h_flex()
        .gap_0p5()
        .p_0p5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .shadow_sm()
        .absolute()
        .top(px(-16.))
        .right_2()
        .opacity(0.)
        .group_hover(group, |bar| bar.opacity(1.));
    for name in quick {
        let Some(glyph) = emoji::glyph(name) else {
            continue;
        };
        bar = bar.child(
            toolbar_button(
                format!("{key}-quick-{name}"),
                handler,
                MessageAction::React(name.clone()),
            )
            .label(glyph)
            .tooltip(format!(":{name}:")),
        );
    }
    let pick = handler.clone();
    bar = bar.child(
        Button::new(SharedString::from(format!("{key}-react")))
            .ghost()
            .with_size(BAR_BUTTON)
            .icon(IconName::FaceSlightlySmilingPlus)
            .tooltip("Add reaction")
            .on_click(move |event, window, cx| {
                pick(MessageAction::PickReaction(event.position()), window, cx);
            }),
    );
    if set.reply {
        bar = bar.child(
            toolbar_button(
                format!("{key}-reply"),
                handler,
                MessageAction::ReplyInThread,
            )
            .icon(IconName::MessageSquareReply)
            .tooltip("Reply in thread"),
        );
    }
    let menu_handler = handler.clone();
    bar.child(
        toolbar_button(format!("{key}-forward"), handler, MessageAction::Forward)
            .icon(IconName::Forward)
            .tooltip("Forward message"),
    )
    .child(
        toolbar_button(format!("{key}-save"), handler, MessageAction::SaveForLater)
            .icon(IconName::Bookmark)
            .tooltip(if set.saved {
                "Remove from saved"
            } else {
                "Save for later"
            }),
    )
    .child(
        Button::new(SharedString::from(format!("{key}-more")))
            .ghost()
            .with_size(BAR_BUTTON)
            .icon(IconName::EllipsisVertical)
            .tooltip("More actions")
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, window, cx| {
                actions_menu(menu, set, &menu_handler, MenuOrigin::More, window, cx)
            }),
    )
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
        history,
    ]);
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

/// The space Slack keeps under a message's reaction pills (each pill's 4px bottom margin
/// plus its bar's), less the message card's own 2px bottom padding.
pub const REACTIONS_BOTTOM: Pixels = px(6.);

/// Reaction chips under a message. With a handler, a chip toggles the signed-in user's
/// reaction and a trailing `+` opens the picker; without one the chips are static.
pub fn reaction_chips(
    key: &str,
    reactions: &[Reaction],
    fixtures: &Fixtures,
    handler: Option<&ActionHandler>,
    cx: &App,
) -> Option<Div> {
    if reactions.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let (secondary, secondary_hover, foreground) =
        (theme.secondary, theme.secondary_hover, theme.foreground);
    let (reacted_bg, reacted_fg, reacted_ring) = crate::theme::reacted_pill(cx);
    let mut row = h_flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .pt_1()
        .pb(REACTIONS_BOTTOM);
    for reaction in reactions {
        let mine = reaction.users.contains(&fixtures.self_user);
        // Slack's `.c-reaction`: a 24px capsule, filled blue and bold when it includes you.
        let chip = h_flex()
            .id(SharedString::from(format!(
                "{key}-reaction-{}",
                reaction.name
            )))
            .h(px(24.))
            .items_center()
            .gap_1()
            .px_2()
            .rounded_full()
            .text_xs()
            .when(mine, |chip| {
                chip.bg(reacted_bg)
                    .font_weight(FontWeight::BOLD)
                    .when_some(reacted_ring, |chip, ring| {
                        chip.border_1().border_color(ring)
                    })
            })
            .when(!mine, |chip| {
                chip.bg(secondary).hover(|chip| chip.bg(secondary_hover))
            })
            // Slack's reaction emoji fills a 16px box. A glyph inks about a quarter past its
            // font size (20px of ink at 16px), so it is set at 13px; an image takes the box.
            .child(emoji::emoji_element(
                &reaction.name,
                fixtures,
                if emoji::glyph(&reaction.name).is_some() {
                    px(13.)
                } else {
                    px(16.)
                },
            ))
            .child(
                div()
                    .text_color(if mine { reacted_fg } else { foreground })
                    .child(reaction.count.to_string()),
            )
            .tooltip(reaction_tooltip(
                emoji::EmojiArt::of(&reaction.name, fixtures),
                reaction_tip(reaction, fixtures, handler.is_some()),
            ))
            .when_some(handler.cloned(), |chip, handler| {
                let name = reaction.name.clone();
                chip.cursor_pointer().on_click(move |_, window, cx| {
                    handler(MessageAction::ToggleReaction(name.clone()), window, cx);
                })
            });
        row = row.child(chip);
    }
    if let Some(handler) = handler.cloned() {
        // Slack's add-reaction control is a capsule the size of a reaction.
        row = row.child(
            h_flex()
                .id(SharedString::from(format!("{key}-reaction-add")))
                .h(px(24.))
                .px_2()
                .items_center()
                .rounded_full()
                .bg(secondary)
                .hover(|pill| pill.bg(secondary_hover))
                .cursor_pointer()
                .child(
                    Icon::new(IconName::FaceSlightlySmilingPlus)
                        .size(px(16.))
                        .text_color(foreground),
                )
                .tooltip(|window, cx| Tooltip::new("Add reaction").build(window, cx))
                .on_click(move |event, window, cx| {
                    handler(MessageAction::PickReaction(event.position()), window, cx);
                }),
        );
    }
    Some(row)
}

/// Slack's reaction tooltip text (`getTip`): who reacted, then "reacted with :name:". One
/// reactor reads "You (click to remove)" for yourself on a pill that toggles; several are
/// listed "A, B, and you", ending in "others" when Slack sent only some of the reactors.
fn reaction_tip(
    reaction: &Reaction,
    fixtures: &Fixtures,
    removable: bool,
) -> (SharedString, SharedString) {
    let me = &fixtures.self_user;
    let who = match reaction.users.as_slice() {
        [only] if only == me && removable => "You (click to remove)".to_owned(),
        [only] if only == me => "You".to_owned(),
        [only] => fixtures.user_name(only).to_owned(),
        users => {
            let mut names: Vec<String> = users
                .iter()
                .enumerate()
                .map(|(ix, user)| match user == me {
                    true if ix == 0 => "You".to_owned(),
                    true => "you".to_owned(),
                    false => fixtures.user_name(user).to_owned(),
                })
                .collect();
            if reaction.count as usize > users.len() {
                names.push("others".to_owned());
            }
            match names.as_slice() {
                [one] => one.clone(),
                [one, other] => format!("{one} and {other}"),
                [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
                [] => String::new(),
            }
        }
    };
    (
        who.into(),
        format!("reacted with :{}:", reaction.name).into(),
    )
}

/// Slack's reaction tooltip (`.c-reaction__tip`): the emoji large on a white 36px tile, then
/// who reacted in bold with "reacted with :name:" muted, centred in at most 200px.
fn reaction_tooltip(
    art: emoji::EmojiArt,
    (who, what): (SharedString, SharedString),
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |window, cx| {
        let (art, who, what) = (art.clone(), who.clone(), what.clone());
        Tooltip::element(move |_, cx| {
            // A glyph inks about a quarter past its font size; an image fills the box.
            let size = match art {
                emoji::EmojiArt::Glyph(_) => px(26.),
                _ => px(32.),
            };
            let line = format!("{who} {what}");
            let subtitle = HighlightStyle {
                color: Some(cx.theme().muted_foreground),
                font_weight: Some(FontWeight::NORMAL),
                ..Default::default()
            };
            v_flex()
                .items_center()
                .max_w(px(200.))
                .py_1()
                .gap_1()
                .text_size(px(13.))
                .line_height(px(16.))
                .font_weight(FontWeight::BOLD)
                .child(
                    div()
                        .size(px(36.))
                        .p(px(2.))
                        .rounded(px(4.))
                        .bg(gpui::white())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(art.element(size)),
                )
                .child(
                    div().text_center().child(
                        StyledText::new(line).with_highlights([(
                            who.len() + 1..who.len() + 1 + what.len(),
                            subtitle,
                        )]),
                    ),
                )
        })
        .build(window, cx)
    }
}

/// Slack's "Saved for later" marker above a saved message.
pub fn saved_banner(cx: &App) -> Div {
    let warning = cx.theme().warning;
    h_flex()
        .items_center()
        .gap_1()
        .pb_0p5()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(warning)
        .child(
            Icon::new(IconName::Bookmark)
                .text_color(warning)
                .size(px(12.)),
        )
        .child("Saved for later")
}

/// Slack's "Only visible to you" marker above an ephemeral message, with an optional
/// "Dismiss" link that removes it from the transcript (`F-COLLAB-683`).
pub fn ephemeral_banner(key: &str, handler: Option<&ActionHandler>, cx: &App) -> Div {
    let theme = cx.theme();
    let (muted, link, tint) = (
        theme.muted_foreground,
        theme.link,
        theme.secondary.opacity(0.55),
    );
    h_flex()
        .items_center()
        .gap_1p5()
        .mb_1()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .bg(tint)
        .text_xs()
        .text_color(muted)
        .child(Icon::new(IconName::Eye).text_color(muted).size(px(12.)))
        .child("Only visible to you")
        .when_some(handler.cloned(), |row, handler| {
            row.child("·").child(
                div()
                    .id(SharedString::from(format!("{key}-dismiss-eph")))
                    .cursor_pointer()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(link)
                    .hover(|el| el.underline())
                    .child("Dismiss")
                    .on_click(move |_, window, cx| {
                        handler(MessageAction::DismissEphemeral, window, cx)
                    }),
            )
        })
}

/// A toast for feedback on fixture-mode actions.
pub fn notify(title: &str, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(
        Notification::new()
            .title(title.to_owned())
            .message(message)
            .autohide(true),
        cx,
    );
}

/// Copy the message's permalink (Slack's `archives/{channel}/p{ts}` shape) with a toast.
pub fn copy_link(
    fixtures: &Fixtures,
    channel: &ChannelId,
    ts: &Ts,
    window: &mut Window,
    cx: &mut App,
) {
    let link = permalink(fixtures, channel, ts);
    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
    notify("Link copied", link, window, cx);
}

/// Slack's permalink to the message `ts` in `channel`.
pub fn permalink(fixtures: &Fixtures, channel: &ChannelId, ts: &Ts) -> String {
    let domain = fixtures
        .workspaces
        .first()
        .map(|team| team.domain.as_str())
        .unwrap_or("app");
    format!(
        "https://{domain}.slack.com/archives/{channel}/p{}",
        ts.as_str().replace('.', "")
    )
}

/// Toggle `me`'s `name` reaction on `message`; an emptied aggregate is removed. Returns
/// whether `me` has the reaction afterwards.
pub fn toggle_reaction(message: &mut Message, me: &UserId, name: &str) -> bool {
    match message.reactions.iter().position(|r| r.name == name) {
        Some(index) => {
            let reaction = &mut message.reactions[index];
            let reacted = match reaction.users.iter().position(|user| user == me) {
                Some(position) => {
                    reaction.users.remove(position);
                    false
                }
                None => {
                    reaction.users.push(me.clone());
                    true
                }
            };
            reaction.count = reaction.users.len() as u32;
            if reaction.count == 0 {
                message.reactions.remove(index);
            }
            reacted
        }
        None => add_reaction(message, me, name),
    }
}

/// Add `me`'s `name` reaction unless it is already there; returns whether it was added.
pub fn add_reaction(message: &mut Message, me: &UserId, name: &str) -> bool {
    match message.reactions.iter_mut().find(|r| r.name == name) {
        Some(reaction) => {
            let added = !reaction.users.contains(me);
            if added {
                reaction.users.push(me.clone());
            }
            reaction.count = reaction.users.len() as u32;
            added
        }
        None => {
            message.reactions.push(Reaction {
                name: name.to_owned(),
                count: 1,
                users: vec![me.clone()],
            });
            true
        }
    }
}

/// Slack's "Delete message" confirmation, previewing `message`; `delete` runs only when the
/// user picks Delete.
pub fn confirm_delete(
    message: Message,
    fixtures: Arc<Fixtures>,
    window: &mut Window,
    cx: &mut App,
    delete: impl Fn(&mut App) + 'static,
) {
    let delete = Rc::new(delete);
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let delete = delete.clone();
        let preview = div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .child(crate::messages::message_row(&fixtures, &message, None, cx));
        alert
            // Slack seats it in the middle of the window, the message shown under the question.
            .centered()
            .width(px(520.))
            .title("Delete message")
            .description(
                v_flex()
                    .gap_3()
                    .child("Are you sure you want to delete this message? This cannot be undone.")
                    .child(preview),
            )
            .show_cancel(true)
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                delete(cx);
                true
            })
    });
}

/// A change the user made to a message, already shown by the view that made it, which the
/// root must post to Slack (reactions, edits, pins and deletions are not local-only in
/// live mode).
pub enum MessageChange {
    /// `add`: the user's `name` reaction was added (else removed).
    React {
        ts: Ts,
        name: String,
        add: bool,
    },
    Edit {
        ts: Ts,
        content: MessageContent,
    },
    Delete {
        ts: Ts,
    },
    /// `pinned`: the message is now pinned to its conversation (else unpinned).
    Pin {
        ts: Ts,
        pinned: bool,
    },
    /// Move the conversation's read cursor back to `ts` so the target message and everything
    /// after it become unread (`conversations.mark`).
    MarkUnread {
        ts: Ts,
    },
    /// A Block Kit button press for the app that posted the message (`blocks.actions`).
    BlockAction(crate::blockkit::BlockDispatch),
}

/// Pin or unpin `message` in `channel` (its `pinned_to` list); returns whether it is
/// pinned afterwards.
pub fn toggle_pin(message: &mut Message, channel: &ChannelId) -> bool {
    let pinned = !message.pinned_to.contains(channel);
    if pinned {
        message.pinned_to.push(channel.clone());
    } else {
        message.pinned_to.retain(|kept| kept != channel);
    }
    pinned
}

/// Apply a local edit: new rich-text blocks, mrkdwn fallback text, and the `edited` marker.
pub fn apply_edit(message: &mut Message, me: &UserId, content: MessageContent) {
    message.blocks = content.blocks;
    message.text = content.text;
    message.edited = Some(Edited {
        user: Some(me.clone()),
        ts: now_ts(),
    });
}

/// A `Ts` for "now", matching the fixture timestamp shape.
pub fn now_ts() -> Ts {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ts::new(format!("{}.{:06}", now.as_secs(), now.subsec_micros()))
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

    #[test]
    fn toggle_pin_flips_pinned_to() {
        let channel = ChannelId::from("C1");
        let other = ChannelId::from("C2");
        let mut message = Message {
            pinned_to: vec![other.clone()],
            ..Default::default()
        };

        assert!(toggle_pin(&mut message, &channel));
        assert_eq!(message.pinned_to, vec![other.clone(), channel.clone()]);

        assert!(!toggle_pin(&mut message, &channel));
        assert_eq!(message.pinned_to, vec![other.clone()]);
    }
}
