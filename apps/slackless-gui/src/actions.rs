//! Message actions shared by the transcript (`messages.rs`) and the thread panel
//! (`threads.rs`), so both columns offer one action set with one look (gpui-kit-guide
//! section 6):
//!
//! - [`hover_toolbar`]: Slack's floating bar revealed on row hover (quick reactions, add
//!   reaction, reply in thread, forward, save for later, and a "more" dropdown);
//! - [`actions_menu`]: the full menu behind "more" and the row's right-click `ContextMenu`,
//!   in Slack's order (edit; mark unread, save, reminder submenu; copy link, copy message;
//!   delete), each with Slack's keyboard shortcut while the menu is open ([`bind_menu_keys`]);
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
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, AnyWindowHandle, App, ClipboardItem, Context, DismissEvent, Div, Entity,
    Focusable as _, FontWeight, Global, InteractiveElement as _, KeyBinding, ParentElement as _,
    Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    actions, div, px,
};
use slackless_core::types::{ChannelId, Edited, Message, Reaction, RichTextBlock, Ts, UserId};

use crate::emoji;
use crate::fixtures::Fixtures;
use crate::later::Remind;

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
    MarkUnread,
    Edit,
    Delete,
    /// Copy the message's text ("Copy message").
    CopyText,
    /// The author's avatar or name was clicked: show their profile.
    OpenProfile(UserId),
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
            let own_only = matches!(action, MessageAction::Edit | MessageAction::Delete);
            if (own_only && !target.set.own) || !menu.focus_handle(cx).contains_focused(window, cx)
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
                actions_menu(menu, set, &menu_handler, window, cx)
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

/// The full action menu: the hover bar's "more" dropdown and the row's right-click menu, in
/// Slack's order with Slack's shortcut beside each item ([`bind_menu_keys`]). Reacting,
/// replying and forwarding live on the hover bar only, as in Slack. `menu` is the fresh popup
/// the framework hands to the builder closure.
pub fn actions_menu(
    menu: PopupMenu,
    set: ActionSet,
    handler: &ActionHandler,
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
        menu
    });

    let mut menu = menu;
    if set.own {
        menu = menu
            .item(
                menu_item(
                    "Edit message",
                    IconName::Pencil,
                    handler,
                    MessageAction::Edit,
                )
                .action(Box::new(EditMessage)),
            )
            .separator();
    }
    menu = menu
        .item(
            menu_item(
                "Mark unread",
                IconName::SquareDot,
                handler,
                MessageAction::MarkUnread,
            )
            .action(Box::new(MarkMessageUnread)),
        )
        .item(
            menu_item(
                if set.saved {
                    "Remove from saved"
                } else {
                    "Add to saved"
                },
                IconName::Bookmark,
                handler,
                MessageAction::SaveForLater,
            )
            .action(Box::new(SaveMessage)),
        )
        .item(PopupMenuItem::submenu("Add a reminder", remind).icon(IconName::AlarmClock))
        .separator()
        .item(
            menu_item(
                "Copy link",
                IconName::Link2,
                handler,
                MessageAction::CopyLink,
            )
            .action(Box::new(CopyMessageLink)),
        )
        .item(
            menu_item(
                "Copy message",
                IconName::Type,
                handler,
                MessageAction::CopyText,
            )
            .action(Box::new(CopyMessageText)),
        );
    if set.own {
        // Slack's destructive item: no icon, in the danger color.
        let delete = handler.clone();
        menu = menu.separator().item(
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
        );
    }
    menu
}

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
    let mut row = h_flex().flex_wrap().items_center().gap_1().pt_1();
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
            .child(emoji::emoji_element(&reaction.name, fixtures, px(16.)))
            .child(
                div()
                    .text_color(if mine { reacted_fg } else { foreground })
                    .child(reaction.count.to_string()),
            )
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

/// The inline edit draft replacing an edited row: a label, the framed textarea and
/// Cancel/Save buttons (Save disabled while the draft is empty).
pub fn edit_box(
    key: &str,
    draft: &Entity<TextareaState>,
    on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    on_save: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let has_text = !draft.read(cx).value().trim().is_empty();
    v_flex()
        .w_full()
        .min_w_0()
        .py_1()
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Editing message"),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .mt_1()
                .px_2()
                .py_1()
                .rounded_lg()
                .border_1()
                .border_color(theme.input)
                .bg(theme.background)
                .child(Textarea::new(draft).appearance(false).bordered(false)),
        )
        .child(
            h_flex()
                .justify_end()
                .gap_1()
                .mt_1()
                .child(
                    Button::new(SharedString::from(format!("{key}-cancel-edit")))
                        .ghost()
                        .with_size(Size::Small)
                        .label("Cancel")
                        .on_click(move |_, window, cx| on_cancel(window, cx)),
                )
                .child(
                    Button::new(SharedString::from(format!("{key}-save-edit")))
                        .primary()
                        .with_size(Size::Small)
                        .label("Save")
                        .disabled(!has_text)
                        .on_click(move |_, window, cx| on_save(window, cx)),
                ),
        )
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
/// root must post to Slack (reactions, edits and deletions are not local-only in live mode).
pub enum MessageChange {
    /// `add`: the user's `name` reaction was added (else removed).
    React {
        ts: Ts,
        name: String,
        add: bool,
    },
    Edit {
        ts: Ts,
        text: String,
    },
    Delete {
        ts: Ts,
    },
}

/// Apply a local edit: new text (re-rendered as one plain rich-text block, as the web client
/// sends it) and the `edited` marker.
pub fn apply_edit(message: &mut Message, me: &UserId, text: String) {
    if !message.blocks.is_empty() {
        message.blocks = vec![RichTextBlock::plain_text(text.clone()).into()];
    }
    message.text = text;
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
