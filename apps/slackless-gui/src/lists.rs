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
}

/// Persistent per-list UI state (layout mode, hidden columns, open item detail drawer, inline edits).
#[derive(Default)]
struct ListUiStore {
    panels: HashMap<String, ListPanelState>,
}

impl Global for ListUiStore {}

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
/// Callback invoked when deleting a row (`lists.records.delete`).
pub type DeleteListItemFn = Rc<dyn Fn(String, &mut Window, &mut App)>;

/// Optional interactive callbacks for a live Slack List.
#[derive(Clone, Default)]
pub struct ListActions {
    pub on_sort: Option<SortColumnFn>,
    pub on_create_item: Option<CreateListItemFn>,
    pub on_update_cell: Option<UpdateListCellFn>,
    pub on_delete_item: Option<DeleteListItemFn>,
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
    let visible_indices = filter_and_sort_rows(data, fixtures, query, sort);
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
        let key_click = list_key.clone();
        visible_cols.iter().fold(
            h_flex()
                .id(("list-live-row", row_ix))
                .border_t_1()
                .border_color(theme.border)
                .text_sm()
                .cursor_pointer()
                .when(is_sel, |r| r.bg(selected_bg))
                .when(!is_sel, |r| r.hover(move |h| h.bg(hover_bg)))
                .on_click(move |_, window, cx| {
                    mutate_panel(
                        &key_click,
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
                        .child(cell_el),
                )
            },
        )
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
            let row_id_del = rec.id.clone();
            let on_delete = actions.on_delete_item.clone();
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
                                .when(!toggleable_cols.is_empty(), |row| {
                                    row.child(fields_menu(
                                        &list_key,
                                        toggleable_cols,
                                        &state.hidden_cols,
                                    ))
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

fn row_cell<'a>(row: &'a SlackListRow, column: &SlackListColumn) -> Option<&'a SlackListCellValue> {
    row.cells
        .get(&column.id)
        .or_else(|| row.cells.get(&column.key))
}

fn live_column_icon(kind: &str) -> IconName {
    match kind {
        "user" | "todo_assignee" | "assignee" | "created_by" | "last_edited_by" | "vote" => {
            IconName::User
        }
        "select" | "multi_select" => IconName::CircleDot,
        "date" | "todo_due_date" | "due_date" | "created_time" | "last_edited_time" => {
            IconName::Calendar
        }
        "checkbox" | "todo_completed" | "completed" => IconName::SquareCheck,
        "link" => IconName::Link,
        "number" | "rating" | "currency" => IconName::Hash,
        _ => IconName::Type,
    }
}

fn live_column_width(column: &SlackListColumn) -> f32 {
    if column.is_primary {
        return 260.;
    }
    match column.kind.as_str() {
        "checkbox" | "todo_completed" | "completed" => 90.,
        "date" | "todo_due_date" | "due_date" | "number" | "rating" => 120.,
        "select" | "multi_select" => 150.,
        "user" | "todo_assignee" | "assignee" => 180.,
        _ => 210.,
    }
}

fn slack_choice_color(name: &str, cx: &App) -> Hsla {
    let theme = cx.theme();
    match name {
        "green" => theme.success,
        "red" => theme.danger,
        "blue" | "indigo" => theme.link,
        "yellow" | "orange" => rgb(0xd97706).into(),
        "purple" => rgb(0x7c3aed).into(),
        "pink" => rgb(0xdb2777).into(),
        "cyan" => rgb(0x0891b2).into(),
        "brown" => rgb(0x92400e).into(),
        _ => theme.muted_foreground,
    }
}

fn cell_sort_key(
    val: Option<&SlackListCellValue>,
    col: &SlackListColumn,
    fixtures: &Fixtures,
) -> String {
    match val {
        None | Some(SlackListCellValue::Empty) => String::new(),
        Some(SlackListCellValue::Text(s))
        | Some(SlackListCellValue::Date(s))
        | Some(SlackListCellValue::Number(s)) => s.to_lowercase(),
        Some(SlackListCellValue::Link { label, .. }) => label.to_lowercase(),
        Some(SlackListCellValue::Checkbox(b)) => if *b { "1" } else { "0" }.to_owned(),
        Some(SlackListCellValue::Users(users)) => users
            .iter()
            .map(|u| fixtures.user_name(u).to_lowercase())
            .collect::<Vec<_>>()
            .join(", "),
        Some(SlackListCellValue::Select(opts)) => opts
            .iter()
            .map(|v| {
                col.choices
                    .iter()
                    .find(|c| c.value == *v)
                    .map(|c| c.label.to_lowercase())
                    .unwrap_or_else(|| v.to_lowercase())
            })
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Filter live list rows by `query` across all columns and sort by optional `(col_idx, ascending)`.
pub(crate) fn filter_and_sort_rows(
    data: &SlackListData,
    fixtures: &Fixtures,
    query: &str,
    sort: Option<(usize, bool)>,
) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let mut indices: Vec<usize> =
        data.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.is_empty()
                    || data.columns.iter().any(|col| {
                        cell_sort_key(row_cell(row, col), col, fixtures).contains(&needle)
                    })
            })
            .map(|(ix, _)| ix)
            .collect();
    if let Some((col_ix, asc)) = sort {
        if let Some(col) = data.columns.get(col_ix) {
            indices.sort_by(|&a, &b| {
                let ka = cell_sort_key(row_cell(&data.rows[a], col), col, fixtures);
                let kb = cell_sort_key(row_cell(&data.rows[b], col), col, fixtures);
                if asc { ka.cmp(&kb) } else { kb.cmp(&ka) }
            });
        }
    }
    indices
}

fn live_cell(
    val: Option<&SlackListCellValue>,
    column: &SlackListColumn,
    fixtures: &Fixtures,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    match val {
        None | Some(SlackListCellValue::Empty) => {
            div().text_color(theme.muted_foreground).child("—")
        }
        Some(SlackListCellValue::Text(t)) => div().truncate().child(t.clone()),
        Some(SlackListCellValue::Date(d)) => {
            div().text_color(theme.muted_foreground).child(d.clone())
        }
        Some(SlackListCellValue::Number(n)) => {
            div().font_weight(FontWeight::MEDIUM).child(n.clone())
        }
        Some(SlackListCellValue::Checkbox(checked)) => {
            let (icon, color) = if *checked {
                (IconName::SquareCheck, theme.success)
            } else {
                (IconName::Square, theme.muted_foreground)
            };
            div().child(Icon::new(icon).text_color(color).size(px(16.)))
        }
        Some(SlackListCellValue::Link { label, url }) => {
            let href = url.clone();
            div().child(
                div()
                    .id(gpui_kit::gpui::SharedString::from(format!(
                        "list-link-{href}"
                    )))
                    .truncate()
                    .text_color(theme.link)
                    .cursor_pointer()
                    .hover(|el| el.underline())
                    .child(label.clone())
                    .on_click(move |_, _, cx| cx.open_url(&href)),
            )
        }
        Some(SlackListCellValue::Users(users)) => {
            let mut row = h_flex()
                .gap_1p5()
                .items_center()
                .min_w_0()
                .overflow_hidden();
            for uid in users.iter().take(2) {
                let name = fixtures.user_name(uid).to_owned();
                row = row.child(
                    h_flex()
                        .gap_1()
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
                                .bg(theme::avatar_color(&name))
                                .text_color(theme::avatar_foreground())
                                .child(team_initials(&name)),
                        )
                        .child(div().truncate().child(name)),
                );
            }
            if users.len() > 2 {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("+{}", users.len() - 2)),
                );
            }
            row
        }
        Some(SlackListCellValue::Select(opts)) => {
            let mut row = h_flex().gap_1().items_center().flex_wrap();
            for opt in opts {
                let choice = column.choices.iter().find(|c| c.value == *opt);
                let label = choice.map(|c| c.label.as_str()).unwrap_or(opt.as_str());
                let color = choice
                    .map(|c| slack_choice_color(&c.color, cx))
                    .unwrap_or(theme.muted_foreground);
                row = row.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .bg(color.opacity(0.18))
                        .text_color(color)
                        .child(label.to_owned()),
                );
            }
            row
        }
    }
}

fn fixture_to_csv(list: &SlackList) -> String {
    let mut out = String::new();
    for (ix, col) in list.columns.iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        push_csv_field(&mut out, col.name);
    }
    out.push('\n');
    for row in &list.rows {
        for (ix, field) in row.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            let val = match field {
                Field::Text(s) | Field::Person(s) | Field::Date(s) => s.as_str(),
                Field::Select(Status::Todo) => "To do",
                Field::Select(Status::InProgress) => "In progress",
                Field::Select(Status::Blocked) => "Blocked",
                Field::Select(Status::Done) => "Done",
                Field::Checkbox(true) => "true",
                Field::Checkbox(false) => "false",
            };
            push_csv_field(&mut out, val);
        }
        out.push('\n');
    }
    out
}

/// Serialize a live Slack List to RFC 4180 CSV (`F-COLLAB-217`), resolving user IDs and
/// select option labels cleanly.
pub fn list_to_csv(data: &SlackListData, fixtures: &Fixtures) -> String {
    let mut out = String::new();
    for (ix, col) in data.columns.iter().enumerate() {
        if ix > 0 {
            out.push(',');
        }
        push_csv_field(&mut out, &col.name);
    }
    out.push('\n');
    for row in &data.rows {
        for (ix, col) in data.columns.iter().enumerate() {
            if ix > 0 {
                out.push(',');
            }
            let val = match row_cell(row, col) {
                None | Some(SlackListCellValue::Empty) => String::new(),
                Some(SlackListCellValue::Text(s)) | Some(SlackListCellValue::Date(s)) => s.clone(),
                Some(SlackListCellValue::Link { url, label }) => {
                    if label.is_empty() || label == url {
                        url.clone()
                    } else {
                        format!("{label} ({url})")
                    }
                }
                Some(SlackListCellValue::Number(n)) => n.to_string(),
                Some(SlackListCellValue::Checkbox(checked)) => {
                    if *checked { "true" } else { "false" }.to_owned()
                }
                Some(SlackListCellValue::Users(users)) => users
                    .iter()
                    .map(|u| fixtures.user_name(u))
                    .collect::<Vec<_>>()
                    .join("; "),
                Some(SlackListCellValue::Select(opts)) => opts
                    .iter()
                    .map(|opt| {
                        col.choices
                            .iter()
                            .find(|c| c.value == *opt)
                            .map(|c| c.label.as_str())
                            .unwrap_or(opt.as_str())
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            };
            push_csv_field(&mut out, &val);
        }
        out.push('\n');
    }
    out
}

fn push_csv_field(out: &mut String, field: &str) {
    if field.contains([',', '"', '\n', '\r']) {
        out.push('"');
        for ch in field.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(field);
    }
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "slack-list".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn fixture_to_json(list: &SlackList) -> String {
    use crate::blockkit::serde_json::{Map, Value, json};
    let rows: Vec<Value> = list
        .rows
        .iter()
        .map(|fields| {
            let mut obj = Map::new();
            for (col, field) in list.columns.iter().zip(fields) {
                let val = match field {
                    Field::Text(s) | Field::Person(s) | Field::Date(s) => Value::String(s.clone()),
                    Field::Select(Status::Todo) => Value::String("To do".into()),
                    Field::Select(Status::InProgress) => Value::String("In progress".into()),
                    Field::Select(Status::Blocked) => Value::String("Blocked".into()),
                    Field::Select(Status::Done) => Value::String("Done".into()),
                    Field::Checkbox(b) => Value::Bool(*b),
                };
                obj.insert(col.name.to_owned(), val);
            }
            Value::Object(obj)
        })
        .collect();
    let payload = json!({
        "title": list.title,
        "columns": list.columns.iter().map(|c| c.name).collect::<Vec<_>>(),
        "items": rows,
    });
    crate::blockkit::serde_json::to_string_pretty(&payload).unwrap_or_default()
}

/// Serialize a live Slack List to pretty-printed JSON (`F-COLLAB-218`), resolving user IDs and
/// select option labels cleanly.
pub fn list_to_json(data: &SlackListData, fixtures: &Fixtures) -> String {
    use crate::blockkit::serde_json::{Map, Value, json};
    let items: Vec<Value> = data
        .rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            obj.insert("id".to_owned(), Value::String(row.id.clone()));
            for col in &data.columns {
                let val = match row_cell(row, col) {
                    None | Some(SlackListCellValue::Empty) => Value::Null,
                    Some(SlackListCellValue::Text(s)) | Some(SlackListCellValue::Date(s)) => {
                        Value::String(s.clone())
                    }
                    Some(SlackListCellValue::Number(n)) => n
                        .parse::<f64>()
                        .ok()
                        .and_then(crate::blockkit::serde_json::Number::from_f64)
                        .map(Value::Number)
                        .unwrap_or_else(|| Value::String(n.clone())),
                    Some(SlackListCellValue::Checkbox(b)) => Value::Bool(*b),
                    Some(SlackListCellValue::Link { label, url }) => json!({
                        "label": label,
                        "url": url,
                    }),
                    Some(SlackListCellValue::Users(users)) => Value::Array(
                        users
                            .iter()
                            .map(|u| Value::String(fixtures.user_name(u).to_owned()))
                            .collect(),
                    ),
                    Some(SlackListCellValue::Select(opts)) => Value::Array(
                        opts.iter()
                            .map(|opt| {
                                let label = col
                                    .choices
                                    .iter()
                                    .find(|c| c.value == *opt)
                                    .map(|c| c.label.as_str())
                                    .unwrap_or(opt.as_str());
                                Value::String(label.to_owned())
                            })
                            .collect(),
                    ),
                };
                obj.insert(col.name.clone(), val);
            }
            Value::Object(obj)
        })
        .collect();
    let payload = json!({
        "file_id": data.file_id,
        "title": data.title,
        "description": data.description,
        "columns": data.columns.iter().map(|c| json!({ "id": c.id, "name": c.name, "type": c.kind })).collect::<Vec<_>>(),
        "items": items,
    });
    crate::blockkit::serde_json::to_string_pretty(&payload).unwrap_or_default()
}

fn export_file(
    content: String,
    suggested: String,
    format_label: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    let dir = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let receiver = cx.prompt_for_new_path(&dir, Some(&suggested));
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(path))) = receiver.await else {
                return;
            };
            let result = std::fs::write(&path, content);
            let _ = cx.update(|window, cx| match result {
                Ok(()) => crate::actions::notify(
                    "List exported",
                    format!("Saved {format_label} to {}", path.display()),
                    window,
                    cx,
                ),
                Err(err) => crate::actions::notify("Export failed", err.to_string(), window, cx),
            });
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use slackless_core::api::SlackListChoice;
    use std::collections::HashMap;

    #[test]
    fn filters_and_sorts_live_slack_list_rows() {
        let fixtures = crate::fixtures::load();
        let data = SlackListData {
            file_id: "F1".into(),
            title: "Tasks".into(),
            description: String::new(),
            columns: vec![
                SlackListColumn {
                    id: "C1".into(),
                    key: "task".into(),
                    name: "Task".into(),
                    kind: "text".into(),
                    is_primary: true,
                    choices: Vec::new(),
                },
                SlackListColumn {
                    id: "C2".into(),
                    key: "status".into(),
                    name: "Status".into(),
                    kind: "select".into(),
                    is_primary: false,
                    choices: vec![
                        SlackListChoice {
                            value: "opt_done".into(),
                            label: "Done".into(),
                            color: "green".into(),
                        },
                        SlackListChoice {
                            value: "opt_todo".into(),
                            label: "To do".into(),
                            color: "gray".into(),
                        },
                    ],
                },
            ],
            rows: vec![
                SlackListRow {
                    id: "R1".into(),
                    cells: HashMap::from([
                        ("C1".into(), SlackListCellValue::Text("Zebra work".into())),
                        (
                            "C2".into(),
                            SlackListCellValue::Select(vec!["opt_todo".into()]),
                        ),
                    ]),
                },
                SlackListRow {
                    id: "R2".into(),
                    cells: HashMap::from([
                        ("C1".into(), SlackListCellValue::Text("Alpha launch".into())),
                        (
                            "C2".into(),
                            SlackListCellValue::Select(vec!["opt_done".into()]),
                        ),
                    ]),
                },
            ],
        };

        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "", Some((0, true))),
            vec![1, 0]
        );
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "", Some((0, false))),
            vec![0, 1]
        );
        // Query matches resolved select choice label ("done") as well as text.
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "done", None),
            vec![1]
        );
        assert_eq!(
            filter_and_sort_rows(&data, &fixtures, "zebra", None),
            vec![0]
        );

        // RFC 4180 CSV export resolves select labels and preserves column order.
        let mut csv_data = data.clone();
        csv_data.rows[0].cells.insert(
            "C1".into(),
            SlackListCellValue::Text("Zebra, \"quoted\" work".into()),
        );
        let csv = list_to_csv(&csv_data, &fixtures);
        assert_eq!(
            csv,
            "Task,Status\n\"Zebra, \"\"quoted\"\" work\",To do\nAlpha launch,Done\n"
        );

        // JSON export resolves select labels and preserves metadata + items (`F-COLLAB-218`).
        let json_str = list_to_json(&data, &fixtures);
        let parsed: crate::blockkit::serde_json::Value =
            crate::blockkit::serde_json::from_str(&json_str).expect("valid JSON");
        assert_eq!(parsed["title"], "Tasks");
        assert_eq!(parsed["items"][0]["Task"], "Zebra work");
        assert_eq!(parsed["items"][0]["Status"][0], "To do");
        assert_eq!(parsed["items"][1]["Status"][0], "Done");

        // Board lane grouping partitions rows by the first select column (`F-COLLAB-235`, `F-COLLAB-237`).
        let lanes = group_live_rows(&data, &[0, 1]);
        assert_eq!(lanes.len(), 2);
        assert_eq!(lanes[0].0, "Done");
        assert_eq!(lanes[0].2, vec![1]);
        assert_eq!(lanes[1].0, "To do");
        assert_eq!(lanes[1].2, vec![0]);
    }
}
