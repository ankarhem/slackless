//! The workspace header's gear menu (Slack's sidebar-management menu), reduced to the items
//! that really work here: "Filter and sort" with its "Active only" toggle (the sidebar then
//! hides the conversations Slack marks dormant) and "Create a channel section" (the
//! dialog form below). Slack's remaining entries - muted-conversation visibility, list
//! cleanup, bulk management, defaults - need data or dialogs slackless does not have, so
//! they are left out rather than shown dead.
//!
//! This module also builds the sidebar sections' own interaction surfaces, in Slack's
//! layout (docs/research references `r5-slack-sec-*`):
//!
//! - [`section_menu`]: the section header's right-click menu and the ⋮ "More actions"
//!   menu (the same items, "Search in this section" first, the manage items inline), with
//!   the Filter / Sort submenus and the muted / dormant channel submenus.
//! - [`channel_menu`]: a channel row's right-click menu (details, copy, move between
//!   sections, notify levels, leave).
//! - [`ChannelDrag`]: the value a drag carries and Slack's drag-layer preview.
//! - The per-section list filter and sort the Filter / Sort submenus write.
//!
//! `workspace_menu(..) -> Div` (the menu body; the sidebar mounts it under a scrim)
//! `SectionNameForm::new(&mut Window, &mut App) -> Entity<SectionNameForm>` (the dialog body)
//! `SectionNameForm::open(&mut self, &mut Window, &mut Context<Self>)` (reset + focus)
//! emits `SectionFormEvent::Create(String)` (trimmed, non-empty)

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window, div,
    px,
};
use slackless_core::NotifyLevel;
use slackless_core::types::{ChannelId, ChannelSection, SectionKind};

use crate::sidebar::Sidebar;

/// One menu row: an optional leading check, a label, and the action to run.
fn menu_row(
    id: &'static str,
    label: &'static str,
    checked: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let (fg, hover) = (cx.theme().foreground, cx.theme().accent);
    let check = checked.then(|| Icon::new(IconName::Check).text_color(fg).size(px(14.)));
    h_flex()
        .id(id)
        .items_center()
        .gap_2()
        .px_3()
        .h(px(30.))
        .rounded_md()
        .text_sm()
        .text_color(fg)
        .cursor_pointer()
        .hover(|row| row.bg(hover))
        .child(label)
        .children(check)
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// The gear menu body. `on_*` run in the sidebar's context; both close the menu afterwards.
pub(crate) fn workspace_menu(
    active_only: bool,
    on_active_only: impl Fn(&mut Window, &mut App) + 'static,
    on_create_section: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let (border, muted, popover) = (
        cx.theme().border,
        cx.theme().muted_foreground,
        cx.theme().popover,
    );
    v_flex()
        .id("workspace-menu")
        .w(px(230.))
        .p_1()
        .gap_0p5()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .bg(popover)
        .shadow_md()
        .occlude()
        .child(
            div()
                .px_3()
                .pt_1()
                .pb_0p5()
                .text_xs()
                .text_color(muted)
                .child("Filter and sort"),
        )
        .child(menu_row(
            "menu-active-only",
            "Active only",
            active_only,
            on_active_only,
            cx,
        ))
        .child(div().mx_2().my_1().h(px(1.)).bg(border))
        .child(menu_row(
            "menu-create-section",
            "Create a channel section",
            false,
            on_create_section,
            cx,
        ))
}

pub enum SectionFormEvent {
    /// A trimmed, non-empty section name.
    Create(String),
}

/// The "Create a channel section" dialog body: a name field and the Create button.
pub struct SectionNameForm {
    name: Entity<TextareaState>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SectionFormEvent> for SectionNameForm {}

impl SectionNameForm {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<SectionNameForm> {
        cx.new(|cx| {
            let name = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Name your section")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe(
                &name,
                |this: &mut SectionNameForm, _, event: &InputEvent, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.create(cx);
                    }
                },
            )];
            SectionNameForm {
                name,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Clear and focus the name field; call when the dialog opens.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_value("", window, cx);
        self.name.update(cx, |state, cx| {
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    /// [`Self::open`], seeding the field with the current name (the rename dialog).
    pub fn open_editing(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_value(name, window, cx);
        self.name.update(cx, |state, cx| {
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    fn set_value(&self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.name
            .update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_owned();
        if name.is_empty() {
            return;
        }
        cx.emit(SectionFormEvent::Create(name));
    }
}

impl Render for SectionNameForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (border, muted) = (cx.theme().border, cx.theme().muted_foreground);
        v_flex()
            .id("section-form")
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .child(Icon::new(IconName::Tag).text_color(muted).size(px(14.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&self.name).appearance(false).bordered(false)),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Sections created here appear in Slack on every client."),
                    )
                    .child(
                        Button::new("section-create")
                            .primary()
                            .label("Create")
                            .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------------------------
// Section interaction: filters, sorts, menus, drag
// ---------------------------------------------------------------------------------------------

/// Slack's per-section "Filter" choice. Slack's default ("Active only") hides the
/// conversations the server marks dormant; the rest widen or narrow the section's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionFilter {
    ActiveOnly,
    Unreads,
    Mentions,
    All,
}

impl SectionFilter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            SectionFilter::ActiveOnly => "Active only",
            SectionFilter::Unreads => "Unreads",
            SectionFilter::Mentions => "Mentions",
            SectionFilter::All => "All",
        }
    }
}

/// Slack's per-section "Sort" choice (Slack also offers "Priority", which orders by
/// relevance scores slackless does not store, so it is left out rather than shown dead).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionSort {
    /// Alphabetical; the built-in default for everything but the DMs bucket.
    Alpha,
    /// Newest activity first; the DMs bucket's built-in default.
    Recency,
}

/// Everything a section's menus need, gathered once when the menu opens.
pub(crate) struct SectionMenuInput {
    /// The section's stable key: its id, or "channels" / "dms" for the sample workspace's
    /// built-in buckets.
    pub key: SharedString,
    /// The header's label (the name Slack shows).
    pub title: String,
    /// The stored section, when the sidebar has one; `None` for the sample workspace's
    /// synthesized buckets.
    pub section: Option<ChannelSection>,
    /// The section's conversations hidden by the muted rule, then by the dormant rule
    /// (the popover and the menus list exactly these).
    pub muted: Vec<ChannelId>,
    pub dormant: Vec<ChannelId>,
    /// Conversations with unread messages (the "Mark all as read" targets).
    pub unread: Vec<ChannelId>,
    /// Conversations not yet muted (the "Mute all conversations" targets).
    pub unmuted: Vec<ChannelId>,
    /// The section's current filter and sort.
    pub filter: SectionFilter,
    pub sort: SectionSort,
}

/// Whether the section is the DMs bucket (sorted by activity, no filter or sort menus).
fn is_dm_section(input: &SectionMenuInput) -> bool {
    input
        .section
        .as_ref()
        .map(|section| section.kind == SectionKind::DirectMessages)
        .unwrap_or_else(|| input.key.as_ref() == "dms")
}

/// A user-created section: the only ones Slack renames, deletes or moves channels into.
fn standard(section: Option<&ChannelSection>) -> bool {
    section
        .map(|section| section.kind == SectionKind::Standard)
        .unwrap_or(false)
}

/// One conversation row of a section menu, resolved as the menu opens.
pub(crate) struct MenuRow {
    pub id: ChannelId,
    /// The row's label ("#eng", the DM's name).
    pub label: String,
    pub private: bool,
    pub is_dm: bool,
}

/// The section header's menu: `more` builds the ⋮ "More actions" variant ("Search in this
/// section" first, the manage items inline); otherwise the right-click layout, with the
/// manage items in a "Manage section" submenu. Slack's items slackless has no backend for
/// are left out rather than shown dead.
pub(crate) fn section_menu(
    menu: PopupMenu,
    input: SectionMenuInput,
    more: bool,
    sidebar: &WeakEntity<Sidebar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let custom = standard(input.section.as_ref());
    let dm = is_dm_section(&input);
    let SectionMenuInput {
        key,
        title,
        muted,
        dormant,
        unread,
        unmuted,
        filter,
        sort,
        ..
    } = input;

    let mut menu = menu.label(title);
    if more {
        let this = sidebar.clone();
        menu = menu.item(
            PopupMenuItem::new("Search in this section")
                .icon(IconName::TextSearch)
                .on_click(move |_, window, cx| {
                    let _ = this.update(cx, |sidebar, cx| sidebar.focus_find(window, cx));
                }),
        );
    }
    let this = sidebar.clone();
    let mark = unread.clone();
    menu = menu.item(
        PopupMenuItem::new("Mark all as read")
            .icon(IconName::CheckCheck)
            .disabled(mark.is_empty())
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |sidebar, cx| sidebar.mark_section_read(&mark, cx));
            }),
    );
    // A user-created section's manage items: inline under their label in the ⋮ menu, in
    // the right-click menu's "Manage section" submenu, as in Slack. Other channel sections
    // carry only the section settings.
    if custom {
        let section = input.section.clone().expect("custom sections are stored");
        if more {
            menu = manage_section(
                menu.separator().label("Manage section"),
                &section,
                &unmuted,
                (filter, sort),
                sidebar,
                window,
                cx,
            );
        } else {
            let (manage_sidebar, manage_unmuted) = (sidebar.clone(), unmuted.clone());
            menu =
                menu.separator()
                    .submenu("Manage section", window, cx, move |menu, window, cx| {
                        manage_section(
                            menu,
                            &section,
                            &manage_unmuted,
                            (filter, sort),
                            &manage_sidebar,
                            window,
                            cx,
                        )
                    });
        }
    } else if !dm {
        menu = section_settings(
            menu.separator(),
            key.clone(),
            &unmuted,
            (filter, sort),
            sidebar,
            window,
            cx,
        );
    }
    let menu = conversation_submenu(menu, "Muted conversations", &muted, sidebar, window, cx);
    conversation_submenu(menu, "Inactive channels", &dormant, sidebar, window, cx)
}

/// A user-created section's "Manage section" group: Rename and Create a new section, the
/// section settings, then Delete section in red.
fn manage_section(
    menu: PopupMenu,
    section: &ChannelSection,
    unmuted: &[ChannelId],
    settings: (SectionFilter, SectionSort),
    sidebar: &WeakEntity<Sidebar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (section_id, section_name) = (section.id.clone(), section.name.clone());
    let (rename_sidebar, create_sidebar) = (sidebar.clone(), sidebar.clone());
    let menu = menu
        .item(PopupMenuItem::new("Rename").on_click(move |_, window, cx| {
            let _ = rename_sidebar.update(cx, |sidebar, cx| {
                sidebar.open_rename_section(section_id.clone(), section_name.clone(), window, cx)
            });
        }))
        .item(
            PopupMenuItem::new("Create a new section").on_click(move |_, window, cx| {
                let _ = create_sidebar
                    .update(cx, |sidebar, cx| sidebar.open_section_dialog(window, cx));
            }),
        );
    let menu = section_settings(
        menu.separator(),
        SharedString::from(section.id.clone()),
        unmuted,
        settings,
        sidebar,
        window,
        cx,
    );
    let (delete_sidebar, delete_section) = (sidebar.clone(), section.clone());
    menu.separator().item(
        PopupMenuItem::element(|_, cx| {
            h_flex().w_full().child(
                div()
                    .text_color(crate::theme::danger_text(cx))
                    .child("Delete section"),
            )
        })
        .on_click(move |_, window, cx| {
            let _ = delete_sidebar.update(cx, |sidebar, cx| {
                sidebar.ask_delete_section(&delete_section, window, cx)
            });
        }),
    )
}

/// A section menu's "Section settings" block: the Filter and Sort submenus and the
/// Notifications label with "Mute all conversations" (`unmuted`: the section's conversations
/// that are not muted yet).
fn section_settings(
    menu: PopupMenu,
    key: SharedString,
    unmuted: &[ChannelId],
    (filter, sort): (SectionFilter, SectionSort),
    sidebar: &WeakEntity<Sidebar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let (filter_key, filter_sidebar) = (key.clone(), sidebar.clone());
    let (sort_key, sort_sidebar) = (key.clone(), sidebar.clone());
    let mute_sidebar = sidebar.clone();
    let targets = unmuted.to_vec();
    menu.label("Section settings")
        .submenu("Filter", window, cx, move |menu, _window, cx| {
            filter_submenu(menu, &filter_key, filter, &filter_sidebar, cx)
        })
        .submenu("Sort", window, cx, move |menu, _window, cx| {
            sort_submenu(menu, sort, &sort_sidebar, &sort_key, cx)
        })
        .separator()
        .label("Notifications")
        .item(
            PopupMenuItem::new("Mute all conversations")
                .disabled(targets.is_empty())
                .on_click(move |_, _, cx| {
                    let _ =
                        mute_sidebar.update(cx, |sidebar, cx| sidebar.mute_section(&targets, cx));
                }),
        )
}

/// The "Muted conversations" / "Inactive channels" submenu: one row per hidden channel
/// (a click opens it), left out entirely when there is nothing to list.
fn conversation_submenu(
    menu: PopupMenu,
    label: &'static str,
    ids: &[ChannelId],
    sidebar: &WeakEntity<Sidebar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    if ids.is_empty() {
        return menu;
    }
    let this = sidebar.clone();
    let ids = ids.to_vec();
    menu.submenu(label, window, cx, move |menu, _window, cx| {
        // Resolve the rows' labels as the submenu opens; they re-resolve on the next open.
        let rows = this
            .upgrade()
            .map(|sidebar| sidebar.read(cx).menu_rows(&ids))
            .unwrap_or_default();
        rows.into_iter().fold(menu, |menu, row| {
            let (this, id) = (this.clone(), row.id.clone());
            menu.item(
                PopupMenuItem::element(move |_, _| channel_menu_row(&row)).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |sidebar, cx| sidebar.select_row(id.clone(), cx));
                    },
                ),
            )
        })
    })
}

fn filter_submenu(
    menu: PopupMenu,
    key: &SharedString,
    current: SectionFilter,
    sidebar: &WeakEntity<Sidebar>,
    _cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let key = key.clone();
    let active_key = key.clone();
    let active_sidebar = sidebar.clone();
    let mut menu = menu.item(
        PopupMenuItem::element(|_, cx| {
            v_flex().child("Active only").child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("New activity within the last 30 days"),
            )
        })
        .checked(current == SectionFilter::ActiveOnly)
        .on_click(move |_, _, cx| {
            let _ = active_sidebar.update(cx, |sidebar, cx| {
                sidebar.set_section_filter(active_key.clone(), SectionFilter::ActiveOnly, cx)
            });
        }),
    );
    for choice in [
        SectionFilter::Unreads,
        SectionFilter::Mentions,
        SectionFilter::All,
    ] {
        let this = sidebar.clone();
        let key = key.clone();
        menu = menu.item(
            PopupMenuItem::new(choice.label())
                .checked(choice == current)
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |sidebar, cx| {
                        sidebar.set_section_filter(key.clone(), choice, cx)
                    });
                }),
        );
    }
    menu
}

fn sort_submenu(
    menu: PopupMenu,
    current: SectionSort,
    sidebar: &WeakEntity<Sidebar>,
    key: &SharedString,
    _cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let mut menu = menu;
    for (choice, icon) in [
        (SectionSort::Alpha, IconName::ArrowDownAZ),
        (SectionSort::Recency, IconName::Clock),
    ] {
        let this = sidebar.clone();
        let key = key.clone();
        menu = menu.item(
            PopupMenuItem::new(match choice {
                SectionSort::Alpha => "A-Z",
                SectionSort::Recency => "Recency",
            })
            .icon(icon)
            .checked(choice == current)
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |sidebar, cx| {
                    sidebar.set_section_sort(key.clone(), choice, cx)
                });
            }),
        );
    }
    menu
}

/// Everything a channel row's menu needs, gathered once when the menu opens.
pub(crate) struct ChannelMenuInput {
    pub channel: ChannelId,
    /// The row's label ("#eng", the DM's name).
    pub label: String,
    /// The workspace archive URL, as the header's "Copy link" builds it.
    pub link: String,
    /// A channel (not a DM): leave applies.
    pub is_channel: bool,
    /// The custom section holding the channel, if any: (id, name).
    pub from: Option<(String, String)>,
    /// The user's custom sections to move into: (id, name, emoji).
    pub sections: Vec<(String, String, String)>,
    pub level: NotifyLevel,
}

/// A channel row's right-click menu. Slack's summarize, more options, split view and new
/// window items have no backend here, so they are left out rather than shown dead.
pub(crate) fn channel_menu(
    menu: PopupMenu,
    input: ChannelMenuInput,
    sidebar: &WeakEntity<Sidebar>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let ChannelMenuInput {
        channel,
        label,
        link,
        is_channel,
        from,
        sections,
        level,
    } = input;
    let details_sidebar = sidebar.clone();
    let details_channel = channel.clone();
    let (copy_link, copy_name) = (
        link.clone(),
        label.strip_prefix("# ").unwrap_or(&label).to_owned(),
    );
    let move_sidebar = sidebar.clone();
    let move_channel = channel.clone();
    let mut menu = menu
        .item(
            PopupMenuItem::new("Channel details").on_click(move |_, _, cx| {
                let _ = details_sidebar.update(cx, |sidebar, cx| {
                    sidebar.open_details(details_channel.clone(), cx)
                });
            }),
        )
        .submenu("Copy", window, cx, move |menu, _window, _cx| {
            let (copy_link, copy_name) = (copy_link.clone(), copy_name.clone());
            menu.item(
                PopupMenuItem::new("Copy link")
                    .icon(IconName::Link)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                            copy_link.clone(),
                        ))
                    }),
            )
            .item(
                PopupMenuItem::new("Copy name")
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                            copy_name.clone(),
                        ))
                    }),
            )
        })
        .submenu("Move channel", window, cx, move |menu, _window, _cx| {
            let channel = move_channel.clone();
            let mut menu = menu;
            for (id, name, emoji) in sections.iter().cloned() {
                let _ = emoji;
                let this = move_sidebar.clone();
                let channel = channel.clone();
                let from = from.clone().map(|(from, _)| from);
                let current = from.as_ref().map(|from| *from == id).unwrap_or(false);
                menu = menu.item(PopupMenuItem::new(name).checked(current).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |sidebar, cx| {
                            sidebar.move_channel(
                                channel.clone(),
                                from.clone(),
                                Some(id.clone()),
                                cx,
                            )
                        });
                    },
                ));
            }
            let this = move_sidebar.clone();
            let create_channel = channel.clone();
            menu = menu.item(
                PopupMenuItem::new("Create new section")
                    .icon(IconName::Plus)
                    .on_click(move |_, window, cx| {
                        let _ = this.update(cx, |sidebar, cx| {
                            sidebar.open_create_with_channel(create_channel.clone(), window, cx)
                        });
                    }),
            );
            if let Some((from, name)) = from.clone() {
                let this = move_sidebar.clone();
                let remove_channel = channel.clone();
                menu = menu.item(PopupMenuItem::new(format!("Remove from {name}")).on_click(
                    move |_, _, cx| {
                        let _ = this.update(cx, |sidebar, cx| {
                            sidebar.move_channel(
                                remove_channel.clone(),
                                Some(from.clone()),
                                None,
                                cx,
                            )
                        });
                    },
                ));
            }
            menu
        })
        .separator()
        .label("Notify you about…");
    for (choice, text) in [
        (NotifyLevel::Everything, "All new posts"),
        (NotifyLevel::Mentions, "Just mentions"),
        (NotifyLevel::Muted, "Mute and hide"),
    ] {
        let this = sidebar.clone();
        let channel = channel.clone();
        menu = menu.item(PopupMenuItem::new(text).checked(choice == level).on_click(
            move |_, _, cx| {
                let _ = this.update(cx, |sidebar, cx| {
                    sidebar.set_notify(channel.clone(), choice, cx)
                });
            },
        ));
    }
    if is_channel {
        let this = sidebar.clone();
        let channel = channel.clone();
        menu = menu.separator().item(
            PopupMenuItem::element(|_, cx| {
                h_flex().w_full().child(
                    div()
                        .text_color(crate::theme::danger_text(cx))
                        .child("Leave channel"),
                )
            })
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |sidebar, cx| sidebar.leave(channel.clone(), cx));
            }),
        );
    }
    menu
}

/// One conversation row inside a section menu: the channel's own leading glyph (a lock for
/// private channels, `#` for public ones) beside its name.
fn channel_menu_row(row: &MenuRow) -> Div {
    let leading = if row.private {
        Icon::new(IconName::Lock).size(px(14.)).into_any_element()
    } else if row.is_dm {
        div().w(px(16.)).into_any_element()
    } else {
        div()
            .w(px(16.))
            .flex()
            .justify_center()
            .child("#")
            .into_any_element()
    };
    let name = row
        .label
        .strip_prefix("# ")
        .unwrap_or(&row.label)
        .to_owned();
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(leading)
        .child(div().truncate().child(name))
}

/// A channel row dragged onto another section (drag-and-drop move): the payload the drop
/// target reads and the drag layer renders, as Slack's `.p-drag_layer` does - the row on
/// the sidebar's primary surface at 0.7 opacity, 6px radius.
#[derive(Clone)]
pub(crate) struct ChannelDrag {
    pub id: ChannelId,
    /// The row's label, with its "# " prefix.
    pub label: String,
    pub private: bool,
    pub is_dm: bool,
    /// The custom section the channel starts in, if any.
    pub from: Option<String>,
    /// The pointer, so the preview can track it.
    pub position: Point<Pixels>,
}

impl Render for ChannelDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let row = MenuRow {
            id: self.id.clone(),
            label: self.label.clone(),
            private: self.private,
            is_dm: self.is_dm,
        };
        let name = row
            .label
            .strip_prefix("# ")
            .unwrap_or(&row.label)
            .to_owned();
        div()
            .pl(self.position.x - px(80.))
            .pt(self.position.y - px(14.))
            .child(
                h_flex()
                    .h(px(28.))
                    .min_w(px(160.))
                    .max_w(px(220.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(px(6.))
                    .opacity(0.7)
                    .shadow_md()
                    .bg(theme.sidebar_primary)
                    .text_color(theme.sidebar_primary_foreground)
                    .child(channel_menu_row(&row))
                    .child(div().flex_1().min_w_0().truncate().child(name)),
            )
    }
}
