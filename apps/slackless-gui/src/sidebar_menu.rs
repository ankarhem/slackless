//! The workspace header's gear menu (Slack's sidebar-management menu), reduced to the items
//! that really work here: "Filter and sort" with its "Active only" toggle (the sidebar then
//! hides conversations with no activity in 30 days) and "Create a channel section" (the
//! dialog form below). Slack's remaining entries - muted-conversation visibility, list
//! cleanup, bulk management, defaults - need data or dialogs slackless does not have, so
//! they are left out rather than shown dead.
//!
//! `workspace_menu(..) -> Div` (the menu body; the sidebar mounts it under a scrim)
//! `SectionNameForm::new(&mut Window, &mut App) -> Entity<SectionNameForm>` (the dialog body)
//! `SectionNameForm::open(&mut self, &mut Window, &mut Context<Self>)` (reset + focus)
//! emits `SectionFormEvent::Create(String)` (trimmed, non-empty)

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::{
    App, AppContext as _, Context, Div, Entity, EventEmitter, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};

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
        self.name.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
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
