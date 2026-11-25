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

/// A column's field type; drives its header icon and how its cells render.
#[derive(Clone, Copy)]
enum FieldKind {
    Text,
    Person,
    Select,
    Date,
    Checkbox,
}

impl FieldKind {
    fn icon(self) -> IconName {
        match self {
            FieldKind::Text => IconName::Type,
            FieldKind::Person => IconName::User,
            FieldKind::Select => IconName::CircleDot,
            FieldKind::Date => IconName::Calendar,
            FieldKind::Checkbox => IconName::SquareCheck,
        }
    }
}

/// Select options, colored like Slack's option chips.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Todo,
    InProgress,
    Blocked,
    Done,
}

impl Status {
    const ALL: [Status; 4] = [
        Status::Todo,
        Status::InProgress,
        Status::Blocked,
        Status::Done,
    ];

    fn label(self) -> &'static str {
        match self {
            Status::Todo => "To do",
            Status::InProgress => "In progress",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
        }
    }

    fn color(self, cx: &App) -> Hsla {
        let theme = cx.theme();
        match self {
            Status::Todo => theme.muted_foreground,
            Status::InProgress => theme.link,
            Status::Blocked => theme.danger,
            Status::Done => theme.success,
        }
    }
}

#[derive(Clone)]
enum Field {
    Text(String),
    Person(String),
    Select(Status),
    Date(String),
    Checkbox(bool),
}

struct Column {
    name: &'static str,
    kind: FieldKind,
    width: f32,
}

struct SlackList {
    title: &'static str,
    columns: Vec<Column>,
    rows: Vec<Vec<Field>>,
}

/// The sample list every conversation shows in fixture mode.
fn fixture() -> SlackList {
    use Field::*;
    let column = |name, kind, width| Column { name, kind, width };
    SlackList {
        title: "Launch tasks",
        columns: vec![
            column("Task", FieldKind::Text, 260.),
            column("Assignee", FieldKind::Person, 170.),
            column("Status", FieldKind::Select, 140.),
            column("Due", FieldKind::Date, 100.),
            column("Shipped", FieldKind::Checkbox, 80.),
        ],
        rows: vec![
            vec![
                Text("Sign the macOS build".into()),
                Person("Maya Chen".into()),
                Select(Status::Done),
                Date("Oct 10".into()),
                Checkbox(true),
            ],
            vec![
                Text("Beta invite list".into()),
                Person("Leo Park".into()),
                Select(Status::InProgress),
                Date("Nov 1".into()),
                Checkbox(false),
            ],
            vec![
                Text("Release notes".into()),
                Person("Sam Rivera".into()),
                Select(Status::Todo),
                Date("Nov 20".into()),
                Checkbox(false),
            ],
            vec![
                Text("Native huddle media".into()),
                Person("Ana Silva".into()),
                Select(Status::Blocked),
                Date("TBD".into()),
                Checkbox(false),
            ],
            vec![
                Text("Search index migration".into()),
                Person("Leo Park".into()),
                Select(Status::Done),
                Date("Oct 28".into()),
                Checkbox(true),
            ],
        ],
    }
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

/// The List tab body: Table or Board layout, hideable fields, CSV/JSON export, inline editing,
/// and click-to-open item detail drawer (`F-COLLAB-226..229`, `F-COLLAB-234..237`, `F-COLLAB-239`).
pub fn list_view(cx: &App) -> Stateful<Div> {
    const KEY: &str = "fixture";
    let theme = cx.theme();
    let mut list = fixture();
    let state = panel_state(KEY, cx);
    if let Some(custom_rows) = state.fixture_rows.clone() {
        list.rows = custom_rows;
    }
    let default_rows = list.rows.clone();
    let visible_cols: Vec<usize> = list
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, col)| *ix == 0 || !state.hidden_cols.contains(col.name))
        .map(|(ix, _)| ix)
        .collect();
    let toggleable_cols: Vec<(String, String)> = list
        .columns
        .iter()
        .skip(1)
        .map(|c| (c.name.to_owned(), c.name.to_owned()))
        .collect();

    let body: Div = match state.layout {
        ListLayout::Table => {
            let header_row = visible_cols.iter().fold(
                h_flex()
                    .bg(theme.muted)
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD),
                |row, &col_ix| {
                    let column = &list.columns[col_ix];
                    row.child(
                        h_flex()
                            .w(px(column.width))
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .px_3()
                            .py_2()
                            .text_color(theme.muted_foreground)
                            .child(
                                Icon::new(column.kind.icon())
                                    .text_color(theme.muted_foreground)
                                    .size(px(14.)),
                            )
                            .child(column.name),
                    )
                },
            );

            let hover_bg = theme.secondary.opacity(0.45);
            let selected_bg = theme.accent.opacity(0.28);
            let rows = list.rows.iter().enumerate().map(|(row_ix, fields)| {
                let is_sel = state.selected_row == Some(row_ix);
                let base_rows = default_rows.clone();
                visible_cols.iter().fold(
                    h_flex()
                        .id(("list-fix-row", row_ix))
                        .border_t_1()
                        .border_color(theme.border)
                        .text_sm()
                        .cursor_pointer()
                        .when(is_sel, |r| r.bg(selected_bg))
                        .when(!is_sel, |r| r.hover(move |h| h.bg(hover_bg)))
                        .on_click(move |_, window, cx| {
                            mutate_panel(
                                KEY,
                                |s| {
                                    s.selected_row = if s.selected_row == Some(row_ix) {
                                        None
                                    } else {
                                        Some(row_ix)
                                    };
                                },
                                window,
                                cx,
                            );
                        }),
                    |row, &col_ix| {
                        let column = &list.columns[col_ix];
                        let cell_el = if matches!(fields[col_ix], Field::Checkbox(_)) {
                            let rows_seed = base_rows.clone();
                            div()
                                .id(SharedString::from(format!(
                                    "list-fix-chk-{row_ix}-{col_ix}"
                                )))
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    mutate_panel(
                                        KEY,
                                        |s| {
                                            let r = s
                                                .fixture_rows
                                                .get_or_insert_with(|| rows_seed.clone());
                                            if let Some(Field::Checkbox(v)) =
                                                r.get_mut(row_ix).and_then(|f| f.get_mut(col_ix))
                                            {
                                                *v = !*v;
                                            }
                                        },
                                        window,
                                        cx,
                                    );
                                })
                                .child(cell(&fields[col_ix], cx))
                                .into_any_element()
                        } else {
                            cell(&fields[col_ix], cx).into_any_element()
                        };
                        row.child(
                            h_flex()
                                .w(px(column.width))
                                .flex_none()
                                .items_center()
                                .px_3()
                                .py_2()
                                .child(cell_el),
                        )
                    },
                )
            });

            v_flex()
                .flex_none()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .child(header_row)
                .children(rows)
        }
        ListLayout::Board => {
            let hover_bg = theme.secondary.opacity(0.6);
            let ring = theme.ring;
            h_flex()
                .items_start()
                .gap_3()
                .children(Status::ALL.iter().map(|&status| {
                    let color = status.color(cx);
                    let cards: Vec<usize> = list
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, fields)| {
                            matches!(fields.get(2), Some(Field::Select(s)) if *s == status)
                        })
                        .map(|(ix, _)| ix)
                        .collect();
                    v_flex()
                        .w(px(250.))
                        .flex_none()
                        .gap_2()
                        .p_2p5()
                        .rounded_lg()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted.opacity(0.6))
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_md()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .bg(color.opacity(0.18))
                                        .text_color(color)
                                        .child(status.label()),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(cards.len().to_string()),
                                ),
                        )
                        .children(cards.into_iter().map(|row_ix| {
                            let fields = &list.rows[row_ix];
                            let is_sel = state.selected_row == Some(row_ix);
                            let mut card = v_flex()
                                .id(("list-fix-card", row_ix))
                                .gap_1p5()
                                .p_2p5()
                                .rounded_md()
                                .border_1()
                                .border_color(if is_sel { ring } else { theme.border })
                                .bg(theme.background)
                                .cursor_pointer()
                                .hover(move |c| c.bg(hover_bg))
                                .on_click(move |_, window, cx| {
                                    mutate_panel(
                                        KEY,
                                        |s| s.selected_row = Some(row_ix),
                                        window,
                                        cx,
                                    );
                                })
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(cell(&fields[0], cx)),
                                );
                            for &col_ix in &visible_cols {
                                if col_ix == 0 || col_ix == 2 {
                                    continue;
                                }
                                card = card.child(
                                    h_flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .text_xs()
                                        .child(
                                            div()
                                                .text_color(theme.muted_foreground)
                                                .child(list.columns[col_ix].name),
                                        )
                                        .child(cell(&fields[col_ix], cx)),
                                );
                            }
                            card
                        }))
                }))
        }
    };

    let detail_drawer = state
        .selected_row
        .and_then(|row_ix| list.rows.get(row_ix).map(|fields| (row_ix, fields)))
        .map(|(row_ix, fields)| {
            let rows_seed_del = default_rows.clone();
            let mut panel = v_flex()
                .w(px(320.))
                .flex_none()
                .h_full()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.background)
                .p_4()
                .gap_3()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Item details"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    Button::new("list-fix-detail-delete")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Trash)
                                        .tooltip("Delete item")
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    let r =
                                                        s.fixture_rows.get_or_insert_with(|| {
                                                            rows_seed_del.clone()
                                                        });
                                                    if row_ix < r.len() {
                                                        r.remove(row_ix);
                                                    }
                                                    s.selected_row = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .child(
                                    Button::new("list-fix-detail-close")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::X)
                                        .on_click(|_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| s.selected_row = None,
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                );
            for (col_ix, (col, field)) in list.columns.iter().zip(fields).enumerate() {
                let rows_seed = default_rows.clone();
                let control = match field {
                    Field::Select(current) => {
                        let current = *current;
                        Button::new(SharedString::from(format!(
                            "list-fix-sel-{row_ix}-{col_ix}"
                        )))
                        .outline()
                        .with_size(Size::XSmall)
                        .label(current.label())
                        .dropdown_menu(move |mut menu, _, _| {
                            for st in Status::ALL {
                                let seed = rows_seed.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(st.label())
                                        .checked(st == current)
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    let r = s
                                                        .fixture_rows
                                                        .get_or_insert_with(|| seed.clone());
                                                    if let Some(slot) = r
                                                        .get_mut(row_ix)
                                                        .and_then(|f| f.get_mut(col_ix))
                                                    {
                                                        *slot = Field::Select(st);
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
                        .into_any_element()
                    }
                    Field::Checkbox(checked) => {
                        let checked = *checked;
                        h_flex()
                            .id(SharedString::from(format!(
                                "list-fix-det-chk-{row_ix}-{col_ix}"
                            )))
                            .items_center()
                            .gap_1p5()
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                mutate_panel(
                                    KEY,
                                    |s| {
                                        let r =
                                            s.fixture_rows.get_or_insert_with(|| rows_seed.clone());
                                        if let Some(Field::Checkbox(v)) =
                                            r.get_mut(row_ix).and_then(|f| f.get_mut(col_ix))
                                        {
                                            *v = !*v;
                                        }
                                    },
                                    window,
                                    cx,
                                );
                            })
                            .child(cell(field, cx))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if checked { "Checked" } else { "Unchecked" }),
                            )
                            .into_any_element()
                    }
                    _ => h_flex()
                        .items_center()
                        .text_sm()
                        .child(cell(field, cx))
                        .into_any_element(),
                };
                panel = panel.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(
                                    Icon::new(col.kind.icon())
                                        .size(px(12.))
                                        .text_color(theme.muted_foreground),
                                )
                                .child(col.name),
                        )
                        .child(h_flex().items_center().child(control)),
                );
            }
            panel
        });

    let add_bar = state.adding.then(|| {
        state.new_item_input.clone().map(|input| {
            let input_submit = input.clone();
            let rows_seed = default_rows.clone();
            h_flex()
                .items_center()
                .gap_2()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .bg(theme.secondary.opacity(0.3))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Textarea::new(&input).appearance(false).bordered(false)),
                )
                .child(
                    Button::new("list-fix-add-save")
                        .primary()
                        .with_size(Size::XSmall)
                        .label("Add")
                        .on_click(move |_, window, cx| {
                            let text = input_submit.read(cx).value().trim().to_owned();
                            if !text.is_empty() {
                                mutate_panel(
                                    KEY,
                                    |s| {
                                        let r =
                                            s.fixture_rows.get_or_insert_with(|| rows_seed.clone());
                                        r.push(vec![
                                            Field::Text(text),
                                            Field::Person("Unassigned".into()),
                                            Field::Select(Status::Todo),
                                            Field::Date("TBD".into()),
                                            Field::Checkbox(false),
                                        ]);
                                        s.adding = false;
                                        s.new_item_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }
                        }),
                )
                .child(
                    Button::new("list-fix-add-cancel")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(|_, window, cx| {
                            mutate_panel(
                                KEY,
                                |s| {
                                    s.adding = false;
                                    s.new_item_input = None;
                                },
                                window,
                                cx,
                            );
                        }),
                )
        })
    });

    h_flex()
        .id("list-root")
        .size_full()
        .child(
            div()
                .id("list-scroll")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_scroll()
                .child(
                    v_flex()
                        .px_6()
                        .py_5()
                        .gap_4()
                        .child(
                            h_flex()
                                .items_center()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::ListChecks)
                                        .text_color(theme.success)
                                        .size(px(18.)),
                                )
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(list.title))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("{} items", list.rows.len())),
                                )
                                .child(div().flex_1())
                                .child(
                                    Button::new("list-fix-add-item")
                                        .primary()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Plus)
                                        .label("Add item")
                                        .on_click(|_, window, cx| {
                                            let input = cx.new(|cx| {
                                                TextareaState::new(window, cx)
                                                    .placeholder("New task title…")
                                            });
                                            mutate_panel(
                                                KEY,
                                                |s| {
                                                    s.adding = true;
                                                    s.new_item_input = Some(input);
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                )
                                .child(layout_switcher(KEY, state.layout, cx))
                                .child(fields_menu(KEY, toggleable_cols, &state.hidden_cols))
                                .child({
                                    let csv = fixture_to_csv(&list);
                                    let suggested =
                                        format!("{}.csv", sanitize_filename(list.title));
                                    Button::new("list-export-csv-fixture")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("CSV")
                                        .tooltip("Download list as CSV")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                csv.clone(),
                                                suggested.clone(),
                                                "CSV",
                                                window,
                                                cx,
                                            );
                                        })
                                })
                                .child({
                                    let json = fixture_to_json(&list);
                                    let suggested =
                                        format!("{}.json", sanitize_filename(list.title));
                                    Button::new("list-export-json-fixture")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("JSON")
                                        .tooltip("Download list as JSON")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                json.clone(),
                                                suggested.clone(),
                                                "JSON",
                                                window,
                                                cx,
                                            );
                                        })
                                }),
                        )
                        .children(add_bar.flatten())
                        .child(body),
                ),
        )
        .children(detail_drawer)
}

fn cell(field: &Field, cx: &App) -> Div {
    let theme = cx.theme();
    match field {
        Field::Text(text) => div().truncate().child(text.clone()),
        Field::Date(text) => div().text_color(theme.muted_foreground).child(text.clone()),
        Field::Person(name) => h_flex()
            .gap_1p5()
            .items_center()
            .min_w_0()
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .bg(theme::avatar_color(name))
                    .text_color(theme::avatar_foreground())
                    .child(team_initials(name)),
            )
            .child(div().truncate().child(name.clone())),
        Field::Select(status) => {
            let (label, color): (&str, Hsla) = match status {
                Status::Todo => ("To do", theme.muted_foreground),
                Status::InProgress => ("In progress", theme.link),
                Status::Blocked => ("Blocked", theme.danger),
                Status::Done => ("Done", theme.success),
            };
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .text_xs()
                .bg(color.opacity(0.18))
                .text_color(color)
                .child(label)
        }
        Field::Checkbox(checked) => {
            let (icon, color) = if *checked {
                (IconName::SquareCheck, theme.success)
            } else {
                (IconName::Square, theme.muted_foreground)
            };
            div().child(Icon::new(icon).text_color(color).size(px(16.)))
        }
    }
}

/// Native live editable Slack Lists view (`F-COLLAB-226..229`, `F-COLLAB-234..239`, `F-COLLAB-248`).
#[allow(clippy::too_many_arguments)]
pub fn live_list_view(
    loaded: Option<&Result<SlackListData, String>>,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    actions: ListActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let open_browser = actions.open_browser.clone();
    match loaded {
        None => v_flex()
            .id("list-loading")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("Loading list…")
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("list-loading-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Err(err)) => v_flex()
            .id("list-error")
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_8()
            .text_sm()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Could not load list"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(err.clone()),
            )
            .when_some(open_browser, |col, open| {
                col.child(
                    Button::new("list-err-browser")
                        .outline()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .label("Open in browser")
                        .on_click(move |_, window, cx| open(window, cx)),
                )
            }),
        Some(Ok(data)) => render_live_list(data, fixtures, query, search, sort, actions, cx),
    }
}

/// Group live list row indices into named lanes by the first `select` / `multi_select` column
/// (or a single `"All items"` lane when no select column exists) for Board layout (`F-COLLAB-235`, `F-COLLAB-237`).
pub(crate) fn group_live_rows(
    data: &SlackListData,
    visible_indices: &[usize],
) -> Vec<(String, String, Vec<usize>)> {
    let Some(group_col) = data
        .columns
        .iter()
        .find(|c| matches!(c.kind.as_str(), "select" | "multi_select"))
    else {
        return vec![(
            "All items".to_owned(),
            "gray".to_owned(),
            visible_indices.to_vec(),
        )];
    };
    let mut lanes: Vec<(String, String, String, Vec<usize>)> = group_col
        .choices
        .iter()
        .map(|c| {
            (
                c.value.clone(),
                c.label.clone(),
                c.color.clone(),
                Vec::new(),
            )
        })
        .collect();
    let mut uncategorized = Vec::new();
    for &row_ix in visible_indices {
        let val = row_cell(&data.rows[row_ix], group_col);
        let first_opt = match val {
            Some(SlackListCellValue::Select(opts)) => opts.first().map(String::as_str),
            _ => None,
        };
        if let Some(opt) = first_opt {
            if let Some(lane) = lanes.iter_mut().find(|(v, _, _, _)| v == opt) {
                lane.3.push(row_ix);
            } else {
                lanes.push((
                    opt.to_owned(),
                    opt.to_owned(),
                    "gray".to_owned(),
                    vec![row_ix],
                ));
            }
        } else {
            uncategorized.push(row_ix);
        }
    }
    let mut out: Vec<(String, String, Vec<usize>)> = lanes
        .into_iter()
        .map(|(_, label, color, rows)| (label, color, rows))
        .collect();
    if !uncategorized.is_empty() || out.is_empty() {
        out.push(("Uncategorized".to_owned(), "gray".to_owned(), uncategorized));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn render_live_list(
    data: &SlackListData,
    fixtures: &Fixtures,
    query: &str,
    search: Option<&Entity<TextareaState>>,
    sort: Option<(usize, bool)>,
    actions: ListActions,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let list_key = data.file_id.clone();
    let state = panel_state(&list_key, cx);
    let visible_indices: Vec<usize> = filter_archived_rows(
        filter_and_sort_rows(data, fixtures, query, sort),
        data,
        &state.archived_rows,
        state.show_archived,
    );
    let completed_to_archive = completed_row_ids(data, &state.archived_rows);
    let archived_count = data
        .rows
        .iter()
        .filter(|r| state.archived_rows.contains(&r.id))
        .count();
    let on_sort = actions.on_sort.clone();
    let open_browser = actions.open_browser.clone();
    let can_edit = actions.on_update_cell.is_some() || actions.on_create_item.is_some();

    let primary_ix = data.columns.iter().position(|c| c.is_primary).unwrap_or(0);
    let visible_cols: Vec<usize> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, col)| *ix == primary_ix || !state.hidden_cols.contains(&col.id))
        .map(|(ix, _)| ix)
        .collect();
    let toggleable_cols: Vec<(String, String)> = data
        .columns
        .iter()
        .enumerate()
        .filter(|(ix, _)| *ix != primary_ix)
        .map(|(_, c)| (c.id.clone(), c.name.clone()))
        .collect();

    let header_row = visible_cols.iter().fold(
        h_flex()
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD),
        |row, &ix| {
            let column = &data.columns[ix];
            let width = live_column_width(column);
            let active_sort = sort.filter(|(c, _)| *c == ix).map(|(_, asc)| asc);
            let on_sort_cb = on_sort.clone();
            let hover = theme.accent;
            let cell = h_flex()
                .id(("list-col-hdr", ix))
                .w(px(width))
                .flex_none()
                .items_center()
                .gap_1p5()
                .px_3()
                .py_2()
                .text_color(theme.muted_foreground)
                .when(on_sort_cb.is_some(), |el| {
                    el.cursor_pointer().hover(move |h| h.bg(hover))
                })
                .when_some(on_sort_cb, |el, cb| {
                    el.on_click(move |_, window, cx| cb(ix, window, cx))
                })
                .child(
                    Icon::new(live_column_icon(&column.kind))
                        .text_color(theme.muted_foreground)
                        .size(px(14.)),
                )
                .child(div().truncate().child(column.name.clone()))
                .when_some(active_sort, |el, asc| {
                    el.child(div().text_xs().text_color(theme.foreground).child(if asc {
                        "\u{2191}"
                    } else {
                        "\u{2193}"
                    }))
                });
            row.child(cell)
        },
    );

    let hover_bg = theme.secondary.opacity(0.45);
    let selected_bg = theme.accent.opacity(0.28);
    let rows_ui = visible_indices.iter().map(|&row_ix| {
        let rec = &data.rows[row_ix];
        let is_sel = state.selected_row == Some(row_ix);
        let is_archived = state.archived_rows.contains(&rec.id);
        let is_subtask = is_subtask_row(rec, &data.columns);
        let key_click = list_key.clone();
        let rec_id_click = rec.id.clone();
        let load_comments_row = actions.on_load_comments.clone();
        let row_el = visible_cols.iter().fold(
            h_flex()
                .id(("list-live-row", row_ix))
                .border_t_1()
                .border_color(theme.border)
                .text_sm()
                .cursor_pointer()
                .when(is_archived, |r| r.opacity(0.55))
                .when(is_sel, |r| r.bg(selected_bg))
                .when(!is_sel, |r| r.hover(move |h| h.bg(hover_bg)))
                .on_click(move |_, window, cx| {
                    let mut opened = false;
                    mutate_panel(
                        &key_click,
                        |s| {
                            if s.selected_row == Some(row_ix) {
                                s.selected_row = None;
                            } else {
                                s.selected_row = Some(row_ix);
                                opened = true;
                            }
                        },
                        window,
                        cx,
                    );
                    if opened && let Some(cb) = &load_comments_row {
                        cb(rec_id_click.clone(), window, cx);
                    }
                }),
            |row, &col_ix| {
                let column = &data.columns[col_ix];
                let width = live_column_width(column);
                let val = row_cell(rec, column);
                let is_checkbox = matches!(
                    column.kind.as_str(),
                    "checkbox" | "todo_completed" | "completed"
                );
                let cell_el = if is_checkbox {
                    if let Some(update_cb) = actions.on_update_cell.clone() {
                        let checked = matches!(val, Some(SlackListCellValue::Checkbox(true)));
                        let row_id = rec.id.clone();
                        let col_id = column.id.clone();
                        div()
                            .id(SharedString::from(format!(
                                "list-live-chk-{row_ix}-{col_ix}"
                            )))
                            .cursor_pointer()
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                update_cb(
                                    row_id.clone(),
                                    ListCellInput {
                                        column_id: col_id.clone(),
                                        value: ListCellInputValue::Checkbox(!checked),
                                    },
                                    window,
                                    cx,
                                );
                            })
                            .child(live_cell(val, column, fixtures, cx))
                            .into_any_element()
                    } else {
                        live_cell(val, column, fixtures, cx).into_any_element()
                    }
                } else {
                    live_cell(val, column, fixtures, cx).into_any_element()
                };
                row.child(
                    h_flex()
                        .w(px(width))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .py_2()
                        .when(col_ix == primary_ix && is_subtask, |c| {
                            c.pl_6().gap_1().child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("\u{21B3}"),
                            )
                        })
                        .child(cell_el),
                )
            },
        );
        // Inline archive button on subtask rows (`F-COLLAB-256`).
        if is_subtask && !is_archived {
            let key_sub = list_key.clone();
            let sub_id = rec.id.clone();
            let on_arch = actions.on_archive_item.clone();
            row_el.child(
                div().pr_2().flex_none().child(
                    Button::new(SharedString::from(format!("list-subtask-arch-{row_ix}")))
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::Archive)
                        .tooltip("Archive subtask")
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            let rid = sub_id.clone();
                            mutate_panel(
                                &key_sub,
                                |s| {
                                    s.archived_rows.insert(rid.clone());
                                },
                                window,
                                cx,
                            );
                            if let Some(cb) = &on_arch {
                                cb(sub_id.clone(), window, cx);
                            }
                        }),
                ),
            )
        } else {
            row_el
        }
    });

    let body = if data.columns.is_empty() {
        div()
            .py_8()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child("This list has no columns.")
    } else if state.layout == ListLayout::Board {
        let lanes = group_live_rows(data, &visible_indices);
        let group_col_ix = data
            .columns
            .iter()
            .position(|c| matches!(c.kind.as_str(), "select" | "multi_select"));
        let card_hover = theme.secondary.opacity(0.6);
        let ring = theme.ring;
        h_flex()
            .items_start()
            .gap_3()
            .children(
                lanes
                    .into_iter()
                    .map(|(lane_label, color_name, lane_rows)| {
                        let color = slack_choice_color(&color_name, cx);
                        v_flex()
                            .w(px(260.))
                            .flex_none()
                            .gap_2()
                            .p_2p5()
                            .rounded_lg()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.muted.opacity(0.6))
                            .child(
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_0p5()
                                            .rounded_md()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .bg(color.opacity(0.18))
                                            .text_color(color)
                                            .child(lane_label),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(lane_rows.len().to_string()),
                                    ),
                            )
                            .children(lane_rows.into_iter().map(|row_ix| {
                                let rec = &data.rows[row_ix];
                                let is_sel = state.selected_row == Some(row_ix);
                                let key_click = list_key.clone();
                                let rec_id_card = rec.id.clone();
                                let load_comments_card = actions.on_load_comments.clone();
                                let primary_col = &data.columns[primary_ix];
                                let mut card = v_flex()
                                    .id(("list-live-card", row_ix))
                                    .gap_1p5()
                                    .p_2p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(if is_sel { ring } else { theme.border })
                                    .bg(theme.background)
                                    .cursor_pointer()
                                    .hover(move |c| c.bg(card_hover))
                                    .on_click(move |_, window, cx| {
                                        mutate_panel(
                                            &key_click,
                                            |s| s.selected_row = Some(row_ix),
                                            window,
                                            cx,
                                        );
                                        if let Some(cb) = &load_comments_card {
                                            cb(rec_id_card.clone(), window, cx);
                                        }
                                    })
                                    .child(
                                        div().text_sm().font_weight(FontWeight::SEMIBOLD).child(
                                            live_cell(
                                                row_cell(rec, primary_col),
                                                primary_col,
                                                fixtures,
                                                cx,
                                            ),
                                        ),
                                    );
                                for &col_ix in &visible_cols {
                                    if col_ix == primary_ix || Some(col_ix) == group_col_ix {
                                        continue;
                                    }
                                    let col = &data.columns[col_ix];
                                    let val = row_cell(rec, col);
                                    if matches!(val, None | Some(SlackListCellValue::Empty)) {
                                        continue;
                                    }
                                    card = card.child(
                                        h_flex()
                                            .items_center()
                                            .justify_between()
                                            .gap_2()
                                            .text_xs()
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .text_color(theme.muted_foreground)
                                                    .child(col.name.clone()),
                                            )
                                            .child(live_cell(val, col, fixtures, cx)),
                                    );
                                }
                                card
                            }))
                    }),
            )
    } else if visible_indices.is_empty() {
        v_flex()
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(header_row)
            .child(
                div()
                    .px_4()
                    .py_6()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(if query.trim().is_empty() {
                        "This list has no items yet."
                    } else {
                        "No matching items."
                    }),
            )
    } else {
        v_flex()
            .flex_none()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(header_row)
            .children(rows_ui)
    };

    let detail_drawer = state
        .selected_row
        .and_then(|row_ix| data.rows.get(row_ix))
        .map(|rec| {
            let key_close = list_key.clone();
            let key_del = list_key.clone();
            let key_arch = list_key.clone();
            let row_id_del = rec.id.clone();
            let row_id_arch = rec.id.clone();
            let on_delete = actions.on_delete_item.clone();
            let on_archive = actions.on_archive_item.clone();
            let mut panel = v_flex()
                .id("list-live-detail")
                .w(px(340.))
                .flex_none()
                .h_full()
                .overflow_y_scroll()
                .border_l_1()
                .border_color(theme.border)
                .bg(theme.background)
                .p_4()
                .gap_3()
                .child(
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .pb_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Item details"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .when_some(on_archive, |row, arch| {
                                    row.child(
                                        Button::new("list-live-detail-archive")
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Archive)
                                            .tooltip("Archive item")
                                            .on_click(move |_, window, cx| {
                                                let rid = row_id_arch.clone();
                                                mutate_panel(
                                                    &key_arch,
                                                    |s| {
                                                        s.archived_rows.insert(rid.clone());
                                                        s.selected_row = None;
                                                    },
                                                    window,
                                                    cx,
                                                );
                                                arch(row_id_arch.clone(), window, cx);
                                            }),
                                    )
                                })
                                .when_some(on_delete, |row, del| {
                                    row.child(
                                        Button::new("list-live-detail-delete")
                                            .ghost()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Trash)
                                            .tooltip("Delete item")
                                            .on_click(move |_, window, cx| {
                                                mutate_panel(
                                                    &key_del,
                                                    |s| s.selected_row = None,
                                                    window,
                                                    cx,
                                                );
                                                del(row_id_del.clone(), window, cx);
                                            }),
                                    )
                                })
                                .child(
                                    Button::new("list-live-detail-close")
                                        .ghost()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::X)
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                &key_close,
                                                |s| {
                                                    s.selected_row = None;
                                                    s.editing_cell = None;
                                                },
                                                window,
                                                cx,
                                            );
                                        }),
                                ),
                        ),
                );
            for (col_ix, col) in data.columns.iter().enumerate() {
                let val = row_cell(rec, col);
                let control = if let Some(update_cb) = actions.on_update_cell.clone() {
                    let row_id = rec.id.clone();
                    let col_id = col.id.clone();
                    match col.kind.as_str() {
                        "checkbox" | "todo_completed" | "completed" => {
                            let checked = matches!(val, Some(SlackListCellValue::Checkbox(true)));
                            h_flex()
                                .id(SharedString::from(format!("list-det-chk-{col_ix}")))
                                .items_center()
                                .gap_1p5()
                                .cursor_pointer()
                                .on_click(move |_, window, cx| {
                                    update_cb(
                                        row_id.clone(),
                                        ListCellInput {
                                            column_id: col_id.clone(),
                                            value: ListCellInputValue::Checkbox(!checked),
                                        },
                                        window,
                                        cx,
                                    );
                                })
                                .child(live_cell(val, col, fixtures, cx))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(if checked { "Checked" } else { "Unchecked" }),
                                )
                                .into_any_element()
                        }
                        "select" | "multi_select" if !col.choices.is_empty() => {
                            let current = match val {
                                Some(SlackListCellValue::Select(opts)) => opts.clone(),
                                _ => Vec::new(),
                            };
                            let current_label = current
                                .first()
                                .and_then(|v| col.choices.iter().find(|c| c.value == *v))
                                .map(|c| c.label.clone())
                                .unwrap_or_else(|| "Select…".to_owned());
                            let choices = col.choices.clone();
                            Button::new(SharedString::from(format!("list-det-sel-{col_ix}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .label(current_label)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for ch in &choices {
                                        let cb = update_cb.clone();
                                        let rid = row_id.clone();
                                        let cid = col_id.clone();
                                        let opt_val = ch.value.clone();
                                        let checked = current.contains(&opt_val);
                                        menu = menu.item(
                                            PopupMenuItem::new(ch.label.clone())
                                                .checked(checked)
                                                .on_click(move |_, window, cx| {
                                                    cb(
                                                        rid.clone(),
                                                        ListCellInput {
                                                            column_id: cid.clone(),
                                                            value: ListCellInputValue::Select(
                                                                vec![opt_val.clone()],
                                                            ),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element()
                        }
                        "user" | "todo_assignee" | "assignee" => {
                            let current: Vec<String> = match val {
                                Some(SlackListCellValue::Users(u)) => {
                                    u.iter().map(ToString::to_string).collect()
                                }
                                _ => Vec::new(),
                            };
                            let label = match val {
                                Some(SlackListCellValue::Users(u)) if !u.is_empty() => u
                                    .iter()
                                    .map(|id| fixtures.user_name(id))
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                _ => "Assign…".to_owned(),
                            };
                            let users: Vec<(slackless_core::types::UserId, String)> = fixtures
                                .users
                                .iter()
                                .filter(|u| !u.deleted && !u.is_bot)
                                .take(40)
                                .map(|u| (u.id.clone(), fixtures.user_name(&u.id).to_owned()))
                                .collect();
                            Button::new(SharedString::from(format!("list-det-usr-{col_ix}")))
                                .outline()
                                .with_size(Size::XSmall)
                                .icon(IconName::User)
                                .label(label)
                                .dropdown_menu(move |mut menu, _, _| {
                                    for (uid, name) in &users {
                                        let cb = update_cb.clone();
                                        let rid = row_id.clone();
                                        let cid = col_id.clone();
                                        let uid_val = uid.clone();
                                        let checked = current.contains(&uid.to_string());
                                        menu = menu.item(
                                            PopupMenuItem::new(name.clone())
                                                .checked(checked)
                                                .on_click(move |_, window, cx| {
                                                    cb(
                                                        rid.clone(),
                                                        ListCellInput {
                                                            column_id: cid.clone(),
                                                            value: ListCellInputValue::Users(vec![
                                                                uid_val.clone(),
                                                            ]),
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        );
                                    }
                                    menu
                                })
                                .into_any_element()
                        }
                        kind if !matches!(
                            kind,
                            "created_by" | "last_edited_by" | "created_time" | "last_edited_time"
                        ) =>
                        {
                            let editing_this =
                                state.editing_cell.as_ref().and_then(|(er, ec, ed)| {
                                    (er == &row_id && ec == &col_id).then(|| ed.clone())
                                });
                            if let Some(input) = editing_this {
                                let input_save = input.clone();
                                let key_save = list_key.clone();
                                let key_cancel = list_key.clone();
                                let kind_owned = kind.to_owned();
                                v_flex()
                                    .w_full()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(theme.border)
                                            .px_2()
                                            .py_1()
                                            .child(
                                                Textarea::new(&input)
                                                    .appearance(false)
                                                    .bordered(false),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_end()
                                            .gap_1()
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "list-det-cancel-{col_ix}"
                                                )))
                                                .ghost()
                                                .with_size(Size::XSmall)
                                                .label("Cancel")
                                                .on_click(move |_, window, cx| {
                                                    mutate_panel(
                                                        &key_cancel,
                                                        |s| s.editing_cell = None,
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "list-det-save-{col_ix}"
                                                )))
                                                .primary()
                                                .with_size(Size::XSmall)
                                                .label("Save")
                                                .on_click(move |_, window, cx| {
                                                    let raw = input_save
                                                        .read(cx)
                                                        .value()
                                                        .trim()
                                                        .to_owned();
                                                    let cell_val = match kind_owned.as_str() {
                                                        "date" | "todo_due_date" | "due_date" => {
                                                            if raw.is_empty() {
                                                                ListCellInputValue::Date(Vec::new())
                                                            } else {
                                                                ListCellInputValue::Date(vec![raw])
                                                            }
                                                        }
                                                        "number" | "rating" | "currency" => {
                                                            if let Ok(n) = raw.parse::<f64>() {
                                                                ListCellInputValue::Number(vec![n])
                                                            } else {
                                                                ListCellInputValue::Text(raw)
                                                            }
                                                        }
                                                        "link" => ListCellInputValue::Link {
                                                            url: raw,
                                                            display_name: None,
                                                        },
                                                        _ => ListCellInputValue::Text(raw),
                                                    };
                                                    mutate_panel(
                                                        &key_save,
                                                        |s| s.editing_cell = None,
                                                        window,
                                                        cx,
                                                    );
                                                    update_cb(
                                                        row_id.clone(),
                                                        ListCellInput {
                                                            column_id: col_id.clone(),
                                                            value: cell_val,
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                    )
                                    .into_any_element()
                            } else {
                                let initial = match val {
                                    Some(SlackListCellValue::Text(s))
                                    | Some(SlackListCellValue::Date(s))
                                    | Some(SlackListCellValue::Number(s)) => s.clone(),
                                    Some(SlackListCellValue::Link { url, .. }) => url.clone(),
                                    _ => String::new(),
                                };
                                let key_ed = list_key.clone();
                                let rid_ed = row_id.clone();
                                let cid_ed = col_id.clone();
                                let hover = theme.secondary.opacity(0.4);
                                h_flex()
                                    .id(SharedString::from(format!("list-det-edit-{col_ix}")))
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_1p5()
                                    .py_1()
                                    .rounded_sm()
                                    .cursor_pointer()
                                    .hover(move |el| el.bg(hover))
                                    .on_click(move |_, window, cx| {
                                        let input = cx.new(|cx| {
                                            TextareaState::new(window, cx)
                                                .default_value(initial.clone())
                                        });
                                        mutate_panel(
                                            &key_ed,
                                            |s| {
                                                s.editing_cell =
                                                    Some((rid_ed.clone(), cid_ed.clone(), input));
                                            },
                                            window,
                                            cx,
                                        );
                                    })
                                    .child(live_cell(val, col, fixtures, cx))
                                    .child(
                                        Icon::new(IconName::Pencil)
                                            .size(px(12.))
                                            .text_color(theme.muted_foreground),
                                    )
                                    .into_any_element()
                            }
                        }
                        _ => h_flex()
                            .items_center()
                            .text_sm()
                            .child(live_cell(val, col, fixtures, cx))
                            .into_any_element(),
                    }
                } else {
                    h_flex()
                        .items_center()
                        .text_sm()
                        .child(live_cell(val, col, fixtures, cx))
                        .into_any_element()
                };
                panel = panel.child(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(
                                    Icon::new(live_column_icon(&col.kind))
                                        .size(px(12.))
                                        .text_color(theme.muted_foreground),
                                )
                                .child(col.name.clone()),
                        )
                        .child(h_flex().w_full().items_center().text_sm().child(control)),
                );
            }
            let child_subtasks =
                subtask_rows_for_parent(data, &rec.id, &state.archived_rows, state.show_archived);
            if !child_subtasks.is_empty() {
                let mut sub_section = v_flex()
                    .gap_1p5()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.muted_foreground)
                            .child(format!("Subtasks ({})", child_subtasks.len())),
                    );
                let primary_col = data.columns.get(primary_ix);
                for sub_ix in child_subtasks {
                    let sub_row = &data.rows[sub_ix];
                    let sub_id = sub_row.id.clone();
                    let sub_archived = state.archived_rows.contains(&sub_id);
                    let key_sub = list_key.clone();
                    let on_arch = actions.on_archive_item.clone();
                    let sub_label = match primary_col {
                        Some(col) => live_cell(row_cell(sub_row, col), col, fixtures, cx),
                        None => div().child(sub_row.id.clone()),
                    };
                    sub_section = sub_section.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_sm()
                            .bg(theme.secondary.opacity(0.35))
                            .when(sub_archived, |r| r.opacity(0.55))
                            .child(div().flex_1().min_w_0().text_xs().child(sub_label))
                            .when(!sub_archived, |r| {
                                r.child(
                                    Button::new(SharedString::from(format!(
                                        "list-det-subtask-arch-{sub_ix}"
                                    )))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::Archive)
                                    .tooltip("Archive subtask")
                                    .on_click(
                                        move |_, window, cx| {
                                            let rid = sub_id.clone();
                                            mutate_panel(
                                                &key_sub,
                                                |s| {
                                                    s.archived_rows.insert(rid.clone());
                                                },
                                                window,
                                                cx,
                                            );
                                            if let Some(cb) = &on_arch {
                                                cb(sub_id.clone(), window, cx);
                                            }
                                        },
                                    ),
                                )
                            }),
                    );
                }
                panel = panel.child(sub_section);
            }
            panel = panel.child(render_record_comments_section(
                &list_key, &rec.id, &state, fixtures, cx,
            ));
            panel
        });

    let add_bar = state.adding.then(|| {
        state.new_item_input.clone().and_then(|input| {
            let create_cb = actions.on_create_item.clone()?;
            let primary_col_id = data.columns.get(primary_ix)?.id.clone();
            let input_submit = input.clone();
            let key_save = list_key.clone();
            let key_cancel = list_key.clone();
            Some(
                h_flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&input).appearance(false).bordered(false)),
                    )
                    .child(
                        Button::new("list-live-add-save")
                            .primary()
                            .with_size(Size::XSmall)
                            .label("Add")
                            .on_click(move |_, window, cx| {
                                let text = input_submit.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    mutate_panel(
                                        &key_save,
                                        |s| {
                                            s.adding = false;
                                            s.new_item_input = None;
                                        },
                                        window,
                                        cx,
                                    );
                                    create_cb(
                                        vec![ListCellInput {
                                            column_id: primary_col_id.clone(),
                                            value: ListCellInputValue::Text(text),
                                        }],
                                        window,
                                        cx,
                                    );
                                }
                            }),
                    )
                    .child(
                        Button::new("list-live-add-cancel")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(move |_, window, cx| {
                                mutate_panel(
                                    &key_cancel,
                                    |s| {
                                        s.adding = false;
                                        s.new_item_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }),
                    ),
            )
        })
    });

    let find_bar = search.map(|input| {
        h_flex()
            .w(px(280.))
            .h(px(30.))
            .px_2p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(input).appearance(false).bordered(false)),
            )
            .when(!query.trim().is_empty(), |row| {
                let clear_input = input.clone();
                row.child(
                    Button::new("list-find-clear")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            clear_input.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                        }),
                )
            })
    });

    h_flex()
        .id("list-live-root")
        .size_full()
        .child(
            div()
                .id("list-scroll")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_scroll()
                .child(
                    v_flex()
                        .px_6()
                        .py_5()
                        .gap_4()
                        .child(
                            h_flex()
                                .items_center()
                                .flex_wrap()
                                .gap_2()
                                .child(
                                    Icon::new(IconName::ListChecks)
                                        .text_color(theme.success)
                                        .size(px(18.)),
                                )
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(
                                                    div()
                                                        .font_weight(FontWeight::SEMIBOLD)
                                                        .child(data.title.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(theme.muted_foreground)
                                                        .child(format!(
                                                            "{} of {} items",
                                                            visible_indices.len(),
                                                            data.rows.len()
                                                        )),
                                                ),
                                        )
                                        .when(!data.description.is_empty(), |col| {
                                            col.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(data.description.clone()),
                                            )
                                        }),
                                )
                                .child(div().flex_1())
                                .when(
                                    actions.on_create_item.is_some() && !data.columns.is_empty(),
                                    |row| {
                                        let key_add = list_key.clone();
                                        row.child(
                                            Button::new("list-live-add-item")
                                                .primary()
                                                .with_size(Size::XSmall)
                                                .icon(IconName::Plus)
                                                .label("Add item")
                                                .on_click(move |_, window, cx| {
                                                    let input = cx.new(|cx| {
                                                        TextareaState::new(window, cx)
                                                            .placeholder("New item title…")
                                                    });
                                                    mutate_panel(
                                                        &key_add,
                                                        |s| {
                                                            s.adding = true;
                                                            s.new_item_input = Some(input);
                                                        },
                                                        window,
                                                        cx,
                                                    );
                                                }),
                                        )
                                    },
                                )
                                .children(find_bar)
                                .child(layout_switcher(&list_key, state.layout, cx))
                                .child({
                                    let key_show = list_key.clone();
                                    let showing = state.show_archived;
                                    let label = if showing {
                                        "Hide archived".to_owned()
                                    } else if archived_count > 0 {
                                        format!("Show archived ({archived_count})")
                                    } else {
                                        "Show archived".to_owned()
                                    };
                                    Button::new("list-toggle-archived")
                                        .when(showing, |b| b.primary())
                                        .when(!showing, |b| b.outline())
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Archive)
                                        .label(label)
                                        .tooltip("Toggle showing archived items in this view")
                                        .on_click(move |_, window, cx| {
                                            mutate_panel(
                                                &key_show,
                                                |s| s.show_archived = !s.show_archived,
                                                window,
                                                cx,
                                            );
                                        })
                                })
                                .when(!completed_to_archive.is_empty(), |row| {
                                    let key_bulk = list_key.clone();
                                    let ids = completed_to_archive.clone();
                                    let on_arch = actions.on_archive_item.clone();
                                    let count = ids.len();
                                    row.child(
                                        Button::new("list-archive-completed")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Archive)
                                            .label(format!("Archive completed ({count})"))
                                            .tooltip("Archive all completed items in this view")
                                            .on_click(move |_, window, cx| {
                                                let ids_local = ids.clone();
                                                mutate_panel(
                                                    &key_bulk,
                                                    |s| {
                                                        for id in &ids_local {
                                                            s.archived_rows.insert(id.clone());
                                                        }
                                                        s.selected_row = None;
                                                    },
                                                    window,
                                                    cx,
                                                );
                                                if let Some(cb) = &on_arch {
                                                    for id in &ids {
                                                        cb(id.clone(), window, cx);
                                                    }
                                                }
                                            }),
                                    )
                                })
                                .when(!toggleable_cols.is_empty(), |row| {
                                    row.child(fields_menu(
                                        &list_key,
                                        toggleable_cols,
                                        &state.hidden_cols,
                                    ))
                                })
                                .when_some(actions.on_create_item.clone(), |row, create_cb| {
                                    let cols_meta: Vec<(String, String, String)> = data
                                        .columns
                                        .iter()
                                        .map(|c| (c.id.clone(), c.name.clone(), c.kind.clone()))
                                        .collect();
                                    let primary_col_ix = primary_ix;
                                    row.child(
                                        Button::new("list-import-csv")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::FileText)
                                            .label("Import CSV")
                                            .tooltip("Import rows from a local CSV file")
                                            .on_click(move |_, window, cx| {
                                                let cb = create_cb.clone();
                                                let cols = cols_meta.clone();
                                                let rx = cx.prompt_for_paths(
                                                    gpui_kit::gpui::PathPromptOptions {
                                                        files: true,
                                                        directories: false,
                                                        multiple: false,
                                                        prompt: Some(
                                                            "Choose CSV file to import".into(),
                                                        ),
                                                    },
                                                );
                                                window
                                                    .spawn(cx, async move |cx| {
                                                        let Ok(Ok(Some(paths))) = rx.await else {
                                                            return;
                                                        };
                                                        let Some(path) = paths.into_iter().next()
                                                        else {
                                                            return;
                                                        };
                                                        let Ok(content) =
                                                            std::fs::read_to_string(&path)
                                                        else {
                                                            return;
                                                        };
                                                        let rows = parse_csv_rows(&content);
                                                        if rows.len() < 2 || cols.is_empty() {
                                                            return;
                                                        }
                                                        let headers = &rows[0];
                                                        let mut imported = 0usize;
                                                        let _ = cx.update(|window, cx| {
                                                            for row_vals in
                                                                rows.iter().skip(1).take(100)
                                                            {
                                                                let cells = map_csv_row_to_cells(
                                                                    headers,
                                                                    row_vals,
                                                                    &cols,
                                                                    primary_col_ix,
                                                                );
                                                                if !cells.is_empty() {
                                                                    cb(cells, window, cx);
                                                                    imported += 1;
                                                                }
                                                            }
                                                            crate::actions::notify(
                                                                "CSV imported",
                                                                format!(
                                                                    "Imported {imported} row{} into list.",
                                                                    if imported == 1 { "" } else { "s" }
                                                                ),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    })
                                                    .detach();
                                            }),
                                    )
                                })
                                .child({
                                    let csv = list_to_csv(data, fixtures);
                                    let suggested =
                                        format!("{}.csv", sanitize_filename(&data.title));
                                    Button::new("list-export-csv")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("CSV")
                                        .tooltip("Download list as CSV")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                csv.clone(),
                                                suggested.clone(),
                                                "CSV",
                                                window,
                                                cx,
                                            );
                                        })
                                })
                                .child({
                                    let json = list_to_json(data, fixtures);
                                    let suggested =
                                        format!("{}.json", sanitize_filename(&data.title));
                                    Button::new("list-export-json")
                                        .outline()
                                        .with_size(Size::XSmall)
                                        .icon(IconName::Download)
                                        .label("JSON")
                                        .tooltip("Download list as JSON")
                                        .on_click(move |_, window, cx| {
                                            export_file(
                                                json.clone(),
                                                suggested.clone(),
                                                "JSON",
                                                window,
                                                cx,
                                            );
                                        })
                                })
                                .when(!can_edit, |row| row.child(read_only_badge(cx)))
                                .when_some(actions.on_set_access.clone(), |row, set_access| {
                                    row.child(
                                        Button::new("list-share-access")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::Users)
                                            .label("Share")
                                            .tooltip("Manage workspace access for this list")
                                            .dropdown_menu(move |menu, _, _| {
                                                let sa_owner = set_access.clone();
                                                let sa_read = set_access.clone();
                                                let sa_write = set_access.clone();
                                                let sa_none = set_access.clone();
                                                menu.item(
                                                    PopupMenuItem::new(
                                                        "Workspace: Owner (full control)",
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        sa_owner(
                                                            "owner".to_owned(),
                                                            window,
                                                            cx,
                                                        );
                                                    }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Workspace: Can edit")
                                                        .on_click(move |_, window, cx| {
                                                            sa_write(
                                                                "write".to_owned(),
                                                                window,
                                                                cx,
                                                            );
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Workspace: View only")
                                                        .on_click(move |_, window, cx| {
                                                            sa_read("read".to_owned(), window, cx);
                                                        }),
                                                )
                                                .item(
                                                    PopupMenuItem::new("Restrict workspace access")
                                                        .on_click(move |_, window, cx| {
                                                            sa_none("none".to_owned(), window, cx);
                                                        }),
                                                )
                                            }),
                                    )
                                })
                                .when_some(open_browser, |row, open| {
                                    row.child(
                                        Button::new("list-open-browser")
                                            .outline()
                                            .with_size(Size::XSmall)
                                            .icon(IconName::ExternalLink)
                                            .label("Open in browser")
                                            .on_click(move |_, window, cx| open(window, cx)),
                                    )
                                }),
                        )
                        .children(add_bar.flatten())
                        .child(body),
                ),
        )
        .children(detail_drawer)
}

mod helpers;
#[cfg(test)]
mod tests;

use helpers::*;
pub(crate) use helpers::{filter_and_sort_rows, parse_csv_rows};
pub use helpers::{list_to_csv, list_to_json};
