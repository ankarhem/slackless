//! How the conversation tabs render: a tab row entry, and the bodies of the tabs that list
//! fetched data (Files & links, a bookmark folder). Each body renders a lazily loaded list
//! ([`Loaded`], fetched each time its tab opens, see `app.rs`) with loading, error and empty
//! states; every clickable row opens its file or link in the browser, and a row with
//! nothing to open is plain (no hover).

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, Div, FontWeight, Hsla, InteractiveElement as _, ParentElement as _, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, div, px,
};
use slackless_core::api::ChannelBookmark;
use slackless_core::types::File;

use crate::files::{file_icon, human_size};
use crate::fixtures::Fixtures;

/// A lazily loaded tab's data: the items, or the error text shown in their place. While
/// the load is in flight the tab has no entry at all.
pub type Loaded<T> = Result<Vec<T>, String>;

/// One conversation tab; the active tab carries the link-colored underline.
pub fn tab(
    id: SharedString,
    label: SharedString,
    icon: IconName,
    active: bool,
    foreground: Hsla,
    muted: Hsla,
    underline: Hsla,
) -> Stateful<Div> {
    let color = if active { foreground } else { muted };
    div()
        .id(id)
        .flex_none()
        .h_full()
        .flex()
        .items_center()
        .gap_1p5()
        .px_0p5()
        .cursor_pointer()
        .text_size(px(14.))
        .text_color(color)
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .when(active, |tab| tab.border_b_2().border_color(underline))
        .hover(|el| el.text_color(foreground))
        .child(Icon::new(icon).text_color(color).size(px(14.)))
        .child(label)
}

/// A muted centered line for a tab body's loading, error and empty states.
pub fn state_line(text: &str, muted: Hsla) -> Stateful<Div> {
    v_flex()
        .id("tab-state-line")
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .text_color(muted)
        .child(text.to_owned())
}

/// One shared-file row: type tile, name, `kind · size · shared by`; opens the file's
/// permalink in the browser.
fn file_row(fixtures: &Fixtures, file: &File, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (icon, tile) = file_icon(file, cx);
    let kind = file
        .pretty_type
        .clone()
        .or_else(|| file.filetype.as_ref().map(|t| t.to_uppercase()))
        .unwrap_or_else(|| "File".to_owned());
    let meta = match file.size {
        Some(size) => format!("{kind} · {}", human_size(size)),
        None => kind,
    };
    let shared_by = file
        .user
        .as_ref()
        .map(|user| format!(" · {}", fixtures.user_name(user)))
        .unwrap_or_default();
    let name = file
        .name
        .clone()
        .or_else(|| file.title.clone())
        .unwrap_or_else(|| file.id.to_string());
    let row = h_flex()
        .id(SharedString::from(format!("file-{}", file.id)))
        .items_center()
        .gap_3()
        .mx_3()
        .my_1()
        .px_2()
        .py_1p5()
        .rounded_md()
        .child(
            div()
                .size(px(36.))
                .flex_none()
                .rounded_md()
                .flex()
                .items_center()
                .justify_center()
                .bg(tile)
                .child(
                    Icon::new(icon)
                        .text_color(crate::theme::avatar_foreground())
                        .size(px(18.)),
                ),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{meta}{shared_by}")),
                ),
        );
    let link = file
        .permalink
        .clone()
        .or_else(|| file.url_private_download.clone())
        .or_else(|| file.url_private.clone());
    match link {
        Some(link) => {
            let hover = theme.muted;
            row.cursor_pointer()
                .hover(move |el| el.bg(hover))
                .on_click(move |_, _, cx| cx.open_url(&link))
        }
        None => row,
    }
}

/// The Files & links body: the conversation's shared files, newest first.
pub fn files_view(fixtures: &Fixtures, files: Option<&Loaded<File>>, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let body = match files {
        None => state_line("Loading files…", muted),
        Some(Err(error)) => state_line(&format!("Could not load files: {error}"), muted),
        Some(Ok(files)) if files.is_empty() => state_line("No files shared here yet.", muted),
        Some(Ok(files)) => v_flex()
            .id("tab-files-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_3()
            .children(files.iter().map(|file| file_row(fixtures, file, cx))),
    };
    v_flex().flex_1().min_h_0().bg(theme.background).child(body)
}

/// One bookmark row of a folder tab; opens `link` in the browser.
fn bookmark_row(bookmark: &ChannelBookmark, link: &str, cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let (muted, hover) = (theme.muted_foreground, theme.muted);
    let host = link
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .map(|host| format!(" · {host}"))
        .unwrap_or_default();
    let link = link.to_owned();
    h_flex()
        .id(SharedString::from(format!(
            "folder-bookmark-{}",
            bookmark.id
        )))
        .items_center()
        .gap_3()
        .mx_3()
        .my_1()
        .px_2()
        .py_1p5()
        .rounded_md()
        .cursor_pointer()
        .hover(move |el| el.bg(hover))
        .child(Icon::new(IconName::Globe).text_color(muted).size(px(16.)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(bookmark.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(format!("Link{host}")),
                ),
        )
        .on_click(move |_, _, cx| cx.open_url(&link))
}

/// A folder tab's body: the link bookmarks inside the folder, bar order.
pub fn folder_view(folder_id: &str, bookmarks: Option<&Loaded<ChannelBookmark>>, cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let body = match bookmarks {
        None => state_line("Loading bookmarks…", muted),
        Some(Err(error)) => state_line(&format!("Could not load bookmarks: {error}"), muted),
        Some(Ok(bookmarks)) => {
            let entries: Vec<(&ChannelBookmark, &str)> = bookmarks
                .iter()
                .filter(|bookmark| bookmark.parent_id.as_deref() == Some(folder_id))
                .filter_map(|bookmark| {
                    let link = bookmark.link.as_deref().filter(|link| !link.is_empty())?;
                    Some((bookmark, link))
                })
                .collect();
            if entries.is_empty() {
                state_line("No bookmarks in this folder yet.", muted)
            } else {
                v_flex()
                    .id("tab-folder-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_3()
                    .children(
                        entries
                            .into_iter()
                            .map(|(bookmark, link)| bookmark_row(bookmark, link, cx)),
                    )
            }
        }
    };
    v_flex().flex_1().min_h_0().bg(theme.background).child(body)
}
