//! Editable Slack List view (`F-COLLAB-213`, `F-COLLAB-226..229`, `F-COLLAB-234..239`,
//! `F-COLLAB-248`): renders both the sample workspace's fixture list and live Slack Lists fetched
//! via `files.info` + `lists.records.list` / `slackLists.items.list` as a searchable, sortable
//! Table or Board with typed columns, inline checkbox toggling, "+ Add item", interactive detail
//! drawer editing (text, date, number, link, select, assignee, checkbox), delete item, and
//! CSV/JSON export (`docs/research/api-canvases-lists.md`).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    App, AppContext as _, Div, Entity, FontWeight, Global, Hsla, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, div, px, rgb,
};
use slackless_core::api::{
    ListCellInput, ListCellInputValue, SlackListCellValue, SlackListColumn, SlackListData,
    SlackListRow,
};

use crate::canvas::{OpenBrowserFn, read_only_badge};
use crate::fixtures::Fixtures;
use crate::sidebar::team_initials;
use crate::theme;

/// Layout mode for a Slack List view (`F-COLLAB-236`, `F-COLLAB-237`, `F-COLLAB-239`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ListLayout {
    #[default]
    Table,
    Board,
}

#[derive(Clone, Default)]
struct ListPanelState {
    layout: ListLayout,
    hidden_cols: HashSet<String>,
    selected_row: Option<usize>,
    adding: bool,
    new_item_input: Option<Entity<TextareaState>>,
    /// Currently edited `(row_id, col_id, input)` in the item detail drawer.
    editing_cell: Option<(String, String, Entity<TextareaState>)>,
    /// Mutable local rows for fixture mode.
    fixture_rows: Option<Vec<Vec<Field>>>,
    /// Optimistically archived record IDs (`F-COLLAB-255`, `F-COLLAB-256`, `F-COLLAB-257`).
    archived_rows: HashSet<String>,
    /// View-level filter showing archived items alongside active rows (`F-COLLAB-257`).
    show_archived: bool,
    /// Inline item-comment composer input inside the detail drawer (`F-COLLAB-250`, `251`).
    comment_input: Option<Entity<TextareaState>>,
    /// Locally posted comments per `(list_id, record_id)` appended alongside server comments.
    local_comments: HashMap<String, Vec<(String, String)>>,
}

/// Persistent per-list UI state (layout mode, hidden columns, open item detail drawer, inline edits).
#[derive(Default)]
struct ListUiStore {
    panels: HashMap<String, ListPanelState>,
    /// Server-fetched record comment threads keyed by `(list_id, record_id)` (`F-COLLAB-250`).
    record_comments: HashMap<(String, String), Vec<(String, String)>>,
}

impl Global for ListUiStore {}

/// Store loaded comment thread messages for a list record (`Event::ListRecordComments`).
pub fn set_record_comments(
    list_id: &str,
    record_id: &str,
    comments: Vec<(String, String)>,
    cx: &mut App,
) {
    cx.default_global::<ListUiStore>()
        .record_comments
        .insert((list_id.to_owned(), record_id.to_owned()), comments);
}

fn record_comments(list_id: &str, record_id: &str, cx: &App) -> Vec<(String, String)> {
    cx.try_global::<ListUiStore>()
        .and_then(|s| {
            s.record_comments
                .get(&(list_id.to_owned(), record_id.to_owned()))
        })
        .cloned()
        .unwrap_or_default()
}

fn panel_state(key: &str, cx: &App) -> ListPanelState {
    cx.try_global::<ListUiStore>()
        .and_then(|store| store.panels.get(key))
        .cloned()
        .unwrap_or_default()
}

fn mutate_panel(key: &str, f: impl FnOnce(&mut ListPanelState), window: &mut Window, cx: &mut App) {
    let entry = cx
        .default_global::<ListUiStore>()
        .panels
        .entry(key.to_owned())
        .or_default();
    f(entry);
    window.refresh();
}

/// Callback invoked when the user clicks a live list column header to sort by `col_idx`.
pub type SortColumnFn = Rc<dyn Fn(usize, &mut Window, &mut App)>;
/// Callback invoked when adding a new item (`lists.records.create`).
pub type CreateListItemFn = Rc<dyn Fn(Vec<ListCellInput>, &mut Window, &mut App)>;
/// Callback invoked when updating a single cell (`lists.cells.update`).
pub type UpdateListCellFn = Rc<dyn Fn(String, ListCellInput, &mut Window, &mut App)>;
/// Callback invoked when deleting, archiving, or loading comments for a row.
pub type DeleteListItemFn = Rc<dyn Fn(String, &mut Window, &mut App)>;
/// Callback invoked when updating list sharing access (`slackLists.access.set`, `F-COLLAB-243`).
pub type SetListAccessFn = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Optional interactive callbacks for a live Slack List.
#[derive(Clone, Default)]
pub struct ListActions {
    pub on_sort: Option<SortColumnFn>,
    pub on_create_item: Option<CreateListItemFn>,
    pub on_update_cell: Option<UpdateListCellFn>,
    pub on_delete_item: Option<DeleteListItemFn>,
    pub on_archive_item: Option<DeleteListItemFn>,
    pub on_set_access: Option<SetListAccessFn>,
    pub on_load_comments: Option<DeleteListItemFn>,
    pub open_browser: Option<OpenBrowserFn>,
}

fn layout_switcher(list_key: &str, active: ListLayout, cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted, active_bg, active_fg) = (
        theme.border,
        theme.muted_foreground,
        theme.secondary,
        theme.foreground,
    );
    let mk = |layout: ListLayout, label: &'static str, icon: IconName| {
        let is_active = active == layout;
        let key = list_key.to_owned();
        h_flex()
            .id(SharedString::from(format!("list-layout-{key}-{label}")))
            .items_center()
            .gap_1()
            .px_2()
            .py(px(3.))
            .rounded_sm()
            .text_xs()
            .cursor_pointer()
            .when(is_active, |b| {
                b.bg(active_bg)
                    .text_color(active_fg)
                    .font_weight(FontWeight::SEMIBOLD)
            })
            .when(!is_active, |b| b.text_color(muted))
            .child(Icon::new(icon).size(px(12.)).text_color(if is_active {
                active_fg
            } else {
                muted
            }))
            .child(label)
            .on_click(move |_, window, cx| {
                mutate_panel(&key, |s| s.layout = layout, window, cx);
            })
    };
    h_flex()
        .items_center()
        .p(px(2.))
        .gap_0p5()
        .rounded_md()
        .border_1()
        .border_color(border)
        .child(mk(ListLayout::Table, "Table", IconName::Table2))
        .child(mk(ListLayout::Board, "Board", IconName::LayoutGrid))
}

fn fields_menu(
    list_key: &str,
    columns: Vec<(String, String)>,
    hidden: &HashSet<String>,
) -> impl gpui_kit::gpui::IntoElement {
    let key = list_key.to_owned();
    let hidden = hidden.clone();
    let hidden_count = hidden.len();
    let label = if hidden_count == 0 {
        "Fields".to_owned()
    } else {
        format!("Fields ({hidden_count} hidden)")
    };
    Button::new(SharedString::from(format!("list-fields-{key}")))
        .outline()
        .with_size(Size::XSmall)
        .icon(IconName::SlidersHorizontal)
        .label(label)
        .tooltip("Show or hide list columns")
        .dropdown_menu(move |mut menu, _, _| {
            for (col_key, col_name) in &columns {
                let list_key = key.clone();
                let col_key = col_key.clone();
                let visible = !hidden.contains(&col_key);
                menu = menu.item(
                    PopupMenuItem::new(col_name.clone())
                        .checked(visible)
                        .on_click(move |_, window, cx| {
                            mutate_panel(
                                &list_key,
                                |state| {
                                    if !state.hidden_cols.remove(&col_key) {
                                        state.hidden_cols.insert(col_key.clone());
                                    }
                                },
                                window,
                                cx,
                            );
                        }),
                );
            }
            menu
        })
}

mod detail;
mod fixture;
mod helpers;
mod live;
#[cfg(test)]
mod tests;
mod toolbar;

pub use fixture::list_view;
use fixture::{Field, SlackList, Status};
use helpers::*;
pub(crate) use helpers::{filter_and_sort_rows, parse_csv_rows};
pub use helpers::{list_to_csv, list_to_json};
#[cfg(test)]
pub(crate) use live::group_live_rows;
pub use live::live_list_view;
