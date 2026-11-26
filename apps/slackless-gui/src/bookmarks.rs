//! The channel bookmarks bar under the conversation tabs and its add-link dialog
//! (docs/research/api-reactions-saved.md: `bookmarks.list`, `bookmarks.add`).
//!
//! The bar shows a "N Pinned" chip that toggles the pins panel, then the conversation's
//! top-level bookmarks (links open their URL, folders open the conversation's folder tab),
//! then "Add a bookmark", which opens the add-link dialog the tab row's "+" > Link opens too.
//!
//! Fixture mode shows a fixed set per conversation kind with names drawn from the channel
//! (sample workspace only); live mode shows the channel's `bookmarks.list` once a bookmarks
//! surface has fetched it.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::TextareaState;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, ClipboardItem, Div, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
use slackless_core::types::{ChannelId, ConversationKind};

use crate::fixtures::Fixtures;

#[derive(Clone, Copy)]
enum BookmarkKind {
    Link,
    Doc,
    Folder,
    Workflow,
}

impl BookmarkKind {
    fn icon(self) -> IconName {
        match self {
            BookmarkKind::Link => IconName::Globe,
            BookmarkKind::Doc => IconName::FileText,
            BookmarkKind::Folder => IconName::Folder,
            BookmarkKind::Workflow => IconName::Zap,
        }
    }
}

/// Where clicking a bookmark goes.
#[derive(Clone)]
enum Target {
    /// Open the link in the browser.
    Url(String),
    /// Open the conversation's tab for this folder bookmark id.
    Folder(String),
    /// Run or inspect a conversation workflow trigger (`https://slack.com/shortcuts/Ft...`).
    Workflow(String),
}

#[derive(Clone)]
pub struct Bookmark {
    title: String,
    kind: BookmarkKind,
    target: Target,
}

impl Bookmark {
    /// A link bookmark opening `url` in the browser.
    pub fn link(title: String, url: String) -> Self {
        Self {
            title,
            kind: BookmarkKind::Link,
            target: Target::Url(url),
        }
    }

    /// A bookmark folder: clicking opens the conversation's tab for `folder_id`.
    pub fn folder(title: String, folder_id: String) -> Self {
        Self {
            title,
            kind: BookmarkKind::Folder,
            target: Target::Folder(folder_id),
        }
    }

    /// A channel workflow bookmark (`F-COLLAB-404`, `F-COLLAB-431`, `F-COLLAB-432`): clicking
    /// trips the workflow; right-click offers View workflow details & Copy conversation workflow URL.
    pub fn workflow(title: String, url: String) -> Self {
        Self {
            title,
            kind: BookmarkKind::Workflow,
            target: Target::Workflow(url),
        }
    }
}

/// The fixture bookmarks of `channel`: channels get a runbook, a roadmap doc and a shared
/// folder named after the channel; DMs get a single shared doc. Sample workspace only.
pub fn fixture(fixtures: &Fixtures, channel: &ChannelId) -> Vec<Bookmark> {
    let Some(conversation) = fixtures.conversation(channel).filter(|_| fixtures.sample) else {
        return Vec::new();
    };
    let name = conversation
        .name
        .clone()
        .unwrap_or_else(|| channel.to_string());
    let bookmark = |title: String, path: &str, kind| Bookmark {
        title,
        kind,
        target: Target::Url(format!("https://example.com/{name}/{path}")),
    };
    match conversation.kind() {
        ConversationKind::PublicChannel | ConversationKind::PrivateChannel => vec![
            bookmark(format!("{name} runbook"), "runbook", BookmarkKind::Link),
            bookmark("Roadmap".to_owned(), "roadmap", BookmarkKind::Doc),
            bookmark("Shared files".to_owned(), "files", BookmarkKind::Folder),
        ],
        ConversationKind::Im | ConversationKind::Mpim => {
            vec![bookmark("1:1 notes".to_owned(), "notes", BookmarkKind::Doc)]
        }
    }
}

/// A click callback of the bar.
pub type OnClick = Rc<dyn Fn(&mut Window, &mut App)>;
/// A folder chip's click callback, given the folder bookmark id.
pub type OnFolder = Rc<dyn Fn(&str, &mut Window, &mut App)>;
/// The add-link dialog's Add callback, given the title and the link.
type OnAdd = Rc<dyn Fn(String, String, &mut App)>;

/// A workflow bookmark run callback, given the workflow trigger URL.
pub type OnWorkflowRun = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// What the bar's clickable entries call back into.
pub struct BarClicks {
    /// The "N Pinned" chip: toggle the pins panel.
    pub pins: OnClick,
    /// A folder bookmark: open the conversation's tab for the folder id.
    pub folder: OnFolder,
    /// A workflow bookmark: trip `workflows.triggers.trip` for its URL.
    pub workflow_run: OnWorkflowRun,
    /// A workflow bookmark: open the Workflow Details dialog (`workflows.triggers.info`).
    pub workflow_details: crate::blockkit::WorkflowDetailsHandler,
    /// "Add a bookmark": open the add-link dialog.
    pub add: OnClick,
}

/// The bar: the pinned chip (with the count once known, highlighted while `pins_open`), the
/// bookmarks, then "Add a bookmark".
pub fn bar(
    bookmarks: Vec<Bookmark>,
    pin_count: Option<usize>,
    pins_open: bool,
    clicks: BarClicks,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (border, muted, foreground, hover, link) = (
        theme.border,
        theme.muted_foreground,
        theme.foreground,
        theme.accent,
        theme.link,
    );
    let chip = |id: SharedString| {
        h_flex()
            .id(id)
            .flex_none()
            .items_center()
            .gap_1p5()
            .px_2()
            .py_0p5()
            .rounded_md()
            .text_xs()
            .cursor_pointer()
            .hover(move |el| el.bg(hover))
    };
    let pin_color = if pins_open { link } else { muted };
    let on_pins = clicks.pins;
    let pins = chip("bookmark-pins".into())
        .when(pins_open, |chip| chip.bg(hover))
        .text_color(if pins_open { foreground } else { muted })
        .child(Icon::new(IconName::Pin).text_color(pin_color).size(px(14.)))
        .child(match pin_count {
            Some(count) => format!("{count} Pinned"),
            None => "Pinned".to_owned(),
        })
        .on_click(move |_, window, cx| on_pins(window, cx));
    let marks = bookmarks.into_iter().enumerate().map(|(ix, bookmark)| {
        let title = bookmark.title.clone();
        let chip = chip(SharedString::from(format!("bookmark-{ix}")))
            .text_color(foreground)
            .child(
                Icon::new(bookmark.kind.icon())
                    .text_color(if matches!(bookmark.kind, BookmarkKind::Workflow) {
                        link
                    } else {
                        muted
                    })
                    .size(px(14.)),
            )
            .child(bookmark.title);
        match bookmark.target {
            Target::Url(url) => chip
                .on_click(move |_, _, cx| cx.open_url(&url))
                .into_any_element(),
            Target::Folder(folder) => {
                let on_folder = clicks.folder.clone();
                chip.on_click(move |_, window, cx| on_folder(&folder, window, cx))
                    .into_any_element()
            }
            Target::Workflow(url) => {
                let on_run = clicks.workflow_run.clone();
                let on_run_menu = clicks.workflow_run.clone();
                let on_details = clicks.workflow_details.clone();
                let url_click = url.clone();
                let url_menu = url.clone();
                let url_copy = url.clone();
                let trigger_id = slackless_core::api::extract_workflow_trigger_id(&url);
                let req = crate::blockkit::WorkflowDetailsRequest {
                    trigger_id,
                    url: Some(url),
                    title,
                };
                chip.on_click(move |_, window, cx| on_run(&url_click, window, cx))
                    .context_menu(move |menu, _, _| {
                        let on_run = on_run_menu.clone();
                        let on_details = on_details.clone();
                        let url_run = url_menu.clone();
                        let url_copy = url_copy.clone();
                        let req = req.clone();
                        menu.item(
                            PopupMenuItem::new("Run workflow")
                                .icon(IconName::Zap)
                                .on_click(move |_, window, cx| on_run(&url_run, window, cx)),
                        )
                        .item(
                            PopupMenuItem::new("View workflow details")
                                .icon(IconName::Info)
                                .on_click(move |_, window, cx| on_details(req.clone(), window, cx)),
                        )
                        .item(
                            PopupMenuItem::new("Copy workflow URL")
                                .icon(IconName::Link)
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        url_copy.clone(),
                                    ));
                                    crate::actions::notify(
                                        "Workflow URL copied",
                                        "Copied conversation workflow URL to clipboard.",
                                        window,
                                        cx,
                                    );
                                }),
                        )
                    })
                    .into_any_element()
            }
        }
    });
    let on_add = clicks.add;
    h_flex()
        .flex_none()
        .items_center()
        .gap_1()
        .px_3()
        .h(px(32.))
        .border_b_1()
        .border_color(border)
        .overflow_hidden()
        .child(pins)
        .child(div().w(px(1.)).h(px(16.)).bg(border))
        .children(marks)
        .child(
            chip("bookmark-add".into())
                .text_color(muted)
                .child(Icon::new(IconName::Plus).text_color(muted).size(px(14.)))
                .child("Add a bookmark")
                .on_click(move |_, window, cx| on_add(window, cx)),
        )
}

/// One labelled input of the add-link dialog.
fn field(label: &'static str, input: &Entity<TextareaState>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        )
        .child(crate::preferences::text_input_box(input, false, cx))
}

/// The add-link dialog: link and title fields. Add calls `on_add(title, url, cx)` for a
/// non-empty link (an empty title falls back to the link) and closes the dialog.
pub fn open_add_dialog(
    window: &mut Window,
    cx: &mut App,
    on_add: impl Fn(String, String, &mut App) + 'static,
) {
    let on_add: OnAdd = Rc::new(on_add);
    let link = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("https://…")
            .auto_grow(1, 1)
    });
    let title = cx.new(|cx| {
        TextareaState::new(window, cx)
            .placeholder("Title")
            .auto_grow(1, 1)
    });
    window.open_dialog(cx, move |dialog, _, cx| {
        let (link, title, on_add) = (link.clone(), title.clone(), on_add.clone());
        dialog.title("Add a bookmark").w(px(460.)).child(
            v_flex()
                .gap_3()
                .p_4()
                .child(field("Link", &link, cx))
                .child(field("Title", &title, cx))
                .child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            crate::preferences::form_button(
                                Button::new("link-cancel").outline().label("Cancel"),
                                cx,
                            )
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            crate::preferences::form_button(
                                Button::new("link-add").primary().label("Add"),
                                cx,
                            )
                            .on_click(move |_, window, cx| {
                                let url = link.read(cx).value().trim().to_string();
                                if url.is_empty() {
                                    return;
                                }
                                let title = title.read(cx).value().trim().to_string();
                                let title = if title.is_empty() { url.clone() } else { title };
                                on_add(title, url, cx);
                                window.close_dialog(cx);
                            }),
                        ),
                ),
        )
    });
}
