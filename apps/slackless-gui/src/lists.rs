//! Read-only Slack List view: the conversation's List tab renders a fixture list (typed
//! columns over rows; text, person, select, date and checkbox fields) as a table
//! (docs/research/api-canvases-lists.md). Editing is deferred: cells are rendered, never
//! mutated, and the header says so.

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, Div, FontWeight, Hsla, InteractiveElement as _, ParentElement as _, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, px,
};

use crate::canvas::read_only_badge;
use crate::sidebar::team_initials;
use crate::theme;

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
#[derive(Clone, Copy)]
enum Status {
    Todo,
    InProgress,
    Blocked,
    Done,
}

enum Field {
    Text(&'static str),
    Person(&'static str),
    Select(Status),
    Date(&'static str),
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
                Text("Sign the macOS build"),
                Person("Maya Chen"),
                Select(Status::Done),
                Date("Oct 10"),
                Checkbox(true),
            ],
            vec![
                Text("Beta invite list"),
                Person("Leo Park"),
                Select(Status::InProgress),
                Date("Nov 1"),
                Checkbox(false),
            ],
            vec![
                Text("Release notes"),
                Person("Sam Rivera"),
                Select(Status::Todo),
                Date("Nov 20"),
                Checkbox(false),
            ],
            vec![
                Text("Native huddle media"),
                Person("Ana Silva"),
                Select(Status::Blocked),
                Date("TBD"),
                Checkbox(false),
            ],
            vec![
                Text("Search index migration"),
                Person("Leo Park"),
                Select(Status::Done),
                Date("Oct 28"),
                Checkbox(true),
            ],
        ],
    }
}

/// The List tab body: title row plus a horizontally and vertically scrollable table.
pub fn list_view(cx: &App) -> Stateful<Div> {
    let theme = cx.theme();
    let list = fixture();

    let header_row = list.columns.iter().fold(
        h_flex()
            .bg(theme.muted)
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD),
        |row, column| {
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

    let rows = list.rows.iter().map(|fields| {
        list.columns.iter().zip(fields).fold(
            h_flex().border_t_1().border_color(theme.border).text_sm(),
            |row, (column, field)| {
                row.child(
                    h_flex()
                        .w(px(column.width))
                        .flex_none()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(cell(field, cx)),
                )
            },
        )
    });

    let table = v_flex()
        .flex_none()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .overflow_hidden()
        .child(header_row)
        .children(rows);

    div().id("list-scroll").size_full().overflow_scroll().child(
        v_flex()
            .px_6()
            .py_5()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
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
                    .child(read_only_badge(cx)),
            )
            .child(table),
    )
}

fn cell(field: &Field, cx: &App) -> Div {
    let theme = cx.theme();
    match field {
        Field::Text(text) => div().truncate().child(*text),
        Field::Date(text) => div().text_color(theme.muted_foreground).child(*text),
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
            .child(div().truncate().child(*name)),
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
