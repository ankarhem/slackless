//! Message actions shared by the transcript (`messages.rs`) and the thread panel
//! (`threads.rs`), so both columns offer one action set with one look (gpui-kit-guide
//! section 6):
//!
//! - [`hover_toolbar`]: Slack's floating bar revealed on row hover (quick reactions, add
//!   reaction, reply in thread, forward, save for later, and a "more" dropdown);
//! - [`actions_menu`]: the full menu behind "more" and the row's right-click `ContextMenu`
//!   (add reaction submenu, reply, copy link, forward, save, remind me submenu, mark unread,
//!   edit, delete);
//! - [`reaction_chips`]: reaction aggregates, interactive (toggle + add) when a handler is
//!   given, static otherwise;
//! - [`edit_box`]: the inline edit draft that replaces a row while it is edited;
//! - the fixture-store mutations both views apply (reactions, edits).
//!
//! Each view supplies an [`ActionHandler`] that maps a [`MessageAction`] onto its own state.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::base::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Icon, Sizable as _, Size, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, ClipboardItem, Context, Div, Entity, FontWeight, InteractiveElement as _,
    ParentElement as _, Pixels, Point, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
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

/// Slack's one-click reactions at the front of the hover bar.
const QUICK_REACTIONS: [&str; 3] = ["white_check_mark", "eyes", "raised_hands"];
/// Emoji the menu's Add reaction submenu lists before "More reactions...".
const MENU_REACTIONS: [&str; 8] = [
    "thumbsup", "heart", "joy", "tada", "eyes", "fire", "pray", "rocket",
];

fn toolbar_button(id: String, handler: &ActionHandler, action: MessageAction) -> Button {
    let handler = handler.clone();
    Button::new(SharedString::from(id))
        .ghost()
        .with_size(Size::XSmall)
        .on_click(move |_, window, cx| handler(action.clone(), window, cx))
}

/// The floating hover bar of one message row; revealed while the row's `group` is hovered.
/// `key` makes element ids unique per row.
pub fn hover_toolbar(
    key: &str,
    set: ActionSet,
    handler: &ActionHandler,
    group: SharedString,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let mut bar = h_flex()
        .gap_0p5()
        .p_0p5()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .shadow_sm()
        .absolute()
        .top_0()
        .right_2()
        .opacity(0.)
        .group_hover(group, |bar| bar.opacity(1.));
    for name in QUICK_REACTIONS {
        let Some(glyph) = emoji::glyph(name) else {
            continue;
        };
        bar = bar.child(
            toolbar_button(
                format!("{key}-quick-{name}"),
                handler,
                MessageAction::React(name.to_owned()),
            )
            .label(glyph)
            .tooltip(format!(":{name}:")),
        );
    }
    let pick = handler.clone();
    bar = bar.child(
        Button::new(SharedString::from(format!("{key}-react")))
            .ghost()
            .with_size(Size::XSmall)
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
            .with_size(Size::XSmall)
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

/// The full action menu: the hover bar's "more" dropdown and the row's right-click menu.
/// `menu` is the fresh popup the framework hands to the builder closure.
pub fn actions_menu(
    menu: PopupMenu,
    set: ActionSet,
    handler: &ActionHandler,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let reactions = PopupMenu::build(window, cx, |mut menu, _, _| {
        for name in MENU_REACTIONS {
            let Some(glyph) = emoji::glyph(name) else {
                continue;
            };
            let handler = handler.clone();
            menu = menu.item(PopupMenuItem::new(format!("{glyph}  :{name}:")).on_click(
                move |_, window, cx| handler(MessageAction::React(name.to_owned()), window, cx),
            ));
        }
        let more = handler.clone();
        menu.separator().item(
            PopupMenuItem::new("More reactions...")
                .icon(IconName::Search)
                .on_click(move |event, window, cx| {
                    more(MessageAction::PickReaction(event.position()), window, cx);
                }),
        )
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

    let mut menu = menu.item(
        PopupMenuItem::submenu("Add reaction", reactions).icon(IconName::FaceSlightlySmilingPlus),
    );
    if set.reply {
        menu = menu.item(menu_item(
            "Reply in thread",
            IconName::MessageSquareReply,
            handler,
            MessageAction::ReplyInThread,
        ));
    }
    menu = menu
        .separator()
        .item(menu_item(
            "Copy link",
            IconName::Link2,
            handler,
            MessageAction::CopyLink,
        ))
        .item(menu_item(
            "Forward message",
            IconName::Forward,
            handler,
            MessageAction::Forward,
        ))
        .item(menu_item(
            if set.saved {
                "Remove from saved"
            } else {
                "Save for later"
            },
            IconName::Bookmark,
            handler,
            MessageAction::SaveForLater,
        ))
        .item(PopupMenuItem::submenu("Remind me about this", remind).icon(IconName::AlarmClock))
        .item(menu_item(
            "Mark unread",
            IconName::BellOff,
            handler,
            MessageAction::MarkUnread,
        ));
    if set.own {
        menu = menu
            .separator()
            .item(menu_item(
                "Edit message",
                IconName::Pencil,
                handler,
                MessageAction::Edit,
            ))
            .item(menu_item(
                "Delete message",
                IconName::Trash,
                handler,
                MessageAction::Delete,
            ));
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
    let (accent, link, secondary, secondary_hover, muted) = (
        theme.accent,
        theme.link,
        theme.secondary,
        theme.secondary_hover,
        theme.muted_foreground,
    );
    let mut row = h_flex().flex_wrap().items_center().gap_1().pt_1();
    for reaction in reactions {
        let mine = reaction.users.contains(&fixtures.self_user);
        let chip = h_flex()
            .id(SharedString::from(format!(
                "{key}-reaction-{}",
                reaction.name
            )))
            .items_center()
            .gap_1()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .when(mine, |chip| {
                chip.border_1().border_color(link).bg(accent.opacity(0.15))
            })
            .when(!mine, |chip| {
                chip.bg(secondary).hover(|chip| chip.bg(secondary_hover))
            })
            .child(emoji::emoji_element(&reaction.name, fixtures, px(14.)))
            .child(
                div()
                    .text_color(if mine { link } else { muted })
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
        row = row.child(
            Button::new(SharedString::from(format!("{key}-reaction-add")))
                .ghost()
                .with_size(Size::XSmall)
                .icon(IconName::FaceSlightlySmilingPlus)
                .tooltip("Add reaction")
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
    let domain = fixtures
        .workspaces
        .first()
        .map(|team| team.domain.as_str())
        .unwrap_or("app");
    let link = format!(
        "https://{domain}.slack.com/archives/{channel}/p{}",
        ts.as_str().replace('.', "")
    );
    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()));
    notify("Link copied", link, window, cx);
}

/// Toggle `me`'s `name` reaction on `message`; an emptied aggregate is removed.
pub fn toggle_reaction(message: &mut Message, me: &UserId, name: &str) {
    match message.reactions.iter().position(|r| r.name == name) {
        Some(index) => {
            let reaction = &mut message.reactions[index];
            match reaction.users.iter().position(|user| user == me) {
                Some(position) => {
                    reaction.users.remove(position);
                }
                None => reaction.users.push(me.clone()),
            }
            reaction.count = reaction.users.len() as u32;
            if reaction.count == 0 {
                message.reactions.remove(index);
            }
        }
        None => add_reaction(message, me, name),
    }
}

/// Add `me`'s `name` reaction unless it is already there.
pub fn add_reaction(message: &mut Message, me: &UserId, name: &str) {
    match message.reactions.iter_mut().find(|r| r.name == name) {
        Some(reaction) => {
            if !reaction.users.contains(me) {
                reaction.users.push(me.clone());
            }
            reaction.count = reaction.users.len() as u32;
        }
        None => message.reactions.push(Reaction {
            name: name.to_owned(),
            count: 1,
            users: vec![me.clone()],
        }),
    }
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
