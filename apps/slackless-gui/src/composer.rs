//! Message composer: a framed multi-line auto-growing textarea (gpui-kit-guide section 5,
//! Enter sends, Shift+Enter inserts a newline) under a mrkdwn formatting toolbar. The toolbar
//! wraps the selection in Slack mrkdwn markers (`*bold*`, `_italic_`, `~strikethrough~`,
//! `` `code` ``), inserts list/quote prefixes, or opens a `<url|label>` link shell. Below the
//! draft sits Slack's always-visible action row (attach, format, emoji, mention, clip,
//! slash) with the send button on the right. The emoji button asks the root for the emoji
//! picker ([`ComposerEvent::OpenEmojiPicker`]); picks come back through [`Composer::insert`].
//! Used by the main column and the thread panel.
//!
//! Toolbar and action-row icons come from the bundled Lucide set (`gpui_kit::assets`); no
//! text markers are needed.
//!
//! A draft that starts with `/` opens the slash-command menu above the frame (Up/Down move,
//! Tab or Enter complete, Esc dismisses); a sent `/name args` draft is emitted as
//! [`ComposerEvent::SlashCommand`] instead of a message. The chevron beside Send opens the
//! schedule menu ([`ComposerEvent::Schedule`]). The root keeps per-conversation drafts through
//! [`Composer::text`] / [`Composer::load_draft`].

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    Anchor, App, AppContext as _, ClickEvent, Context, Div, Entity, EventEmitter, Focusable as _,
    FontWeight, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Pixels,
    Point, Render, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, px, relative,
};

use crate::drafts::SendLater;
use crate::slash::{self, Invocation, SlashCommand};

pub enum ComposerEvent {
    /// Non-empty, trimmed text the user sent. The composer has already cleared itself.
    Submit(String),
    /// The emoji button was clicked at this window position.
    OpenEmojiPicker(Point<Pixels>),
    /// A `/command` draft was sent (composer already cleared).
    SlashCommand(Invocation),
    /// The draft was scheduled for later (composer already cleared).
    Schedule { text: String, when: SendLater },
}

/// What a toolbar or action-row button does.
enum FormatAction {
    /// Wrap the selection in a mrkdwn marker pair.
    Wrap(&'static str),
    /// Insert a prefix (`"• "`, `"> "`, ...) at the caret.
    Insert(&'static str),
    /// Insert a Slack link shell `<https://|label>` around the selection.
    Link,
    /// Ask for the emoji picker.
    Emoji,
    /// Window chrome: renders the control, does nothing.
    Noop,
}

pub struct Composer {
    editor: Entity<TextareaState>,
    /// A placeholder to apply on the next frame (setting it needs the window).
    pending_placeholder: Option<SharedString>,
    /// Whether the draft has content: lights up the send button.
    has_text: bool,
    /// Preferences > Messages & media: Enter sends (Shift+Enter newline) when true; when
    /// false Enter inserts a newline and Cmd+Enter sends.
    enter_sends: bool,
    /// Highlighted row of the slash-command menu.
    slash_selected: usize,
    /// Esc closed the slash menu for the current `/prefix`.
    slash_dismissed: bool,
    /// A draft to put in the editor on the next render (loading needs the window).
    pending: Option<String>,
    /// Whether the "Schedule for later" chevron is offered.
    scheduling: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(
        placeholder: SharedString,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<Composer> {
        cx.new(|cx| {
            let editor = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder(placeholder)
                    .auto_grow(1, 8)
            });
            // Default textarea behavior: Enter emits PressEnter without inserting a newline;
            // Shift+Enter inserts one. Exactly Slack's composer semantics.
            let subscriptions = vec![cx.subscribe_in(
                &editor,
                window,
                |this: &mut Composer, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter {
                        shift: false,
                        secondary,
                    } if this.enter_sends || *secondary => this.submit(window, cx),
                    InputEvent::Change => this.track_draft(cx),
                    _ => {}
                },
            )];
            Composer {
                editor,
                pending_placeholder: None,
                has_text: false,
                enter_sends: true,
                slash_selected: 0,
                slash_dismissed: false,
                pending: None,
                scheduling: true,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Prompt with `placeholder` while empty ("Message #general"); applied on the next frame.
    pub fn set_placeholder(&mut self, placeholder: SharedString, cx: &mut Context<Self>) {
        self.pending_placeholder = Some(placeholder);
        cx.notify();
    }

    /// Offer or hide the "Schedule for later" chevron (off in the thread reply composer).
    pub fn set_scheduling(&mut self, scheduling: bool, cx: &mut Context<Self>) {
        self.scheduling = scheduling;
        cx.notify();
    }

    /// The current draft, untrimmed.
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    /// Replace the draft with `text` (a saved per-conversation draft, or empty).
    pub fn load_draft(&mut self, text: String, cx: &mut Context<Self>) {
        self.has_text = !text.trim().is_empty();
        self.slash_dismissed = false;
        self.slash_selected = 0;
        self.pending = Some(text);
        cx.notify();
    }

    /// Built-ins matching the `/prefix` being typed; empty when the menu is closed.
    fn slash_menu(&self, cx: &App) -> Vec<&'static SlashCommand> {
        if self.slash_dismissed {
            return Vec::new();
        }
        slash::typing(self.editor.read(cx).value().as_ref())
            .map(slash::matches)
            .unwrap_or_default()
    }

    /// Replace the `/prefix` with the full command name and a trailing space.
    fn complete_slash(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = format!("/{name} ");
        let end = text.len();
        self.editor.update(cx, |state, cx| {
            state.set_value(text, window, cx);
            state.set_selected_range(end..end, cx);
        });
        self.has_text = true;
        self.slash_selected = 0;
    }

    /// Slash-menu keys, captured before the textarea sees them.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let menu = self.slash_menu(cx);
        if menu.is_empty() {
            return;
        }
        let count = menu.len();
        let selected = menu[self.slash_selected.min(count - 1)];
        match event.keystroke.key.as_str() {
            "down" => self.slash_selected = (self.slash_selected + 1) % count,
            "up" => self.slash_selected = (self.slash_selected + count - 1) % count,
            "tab" => self.complete_slash(selected.name, window, cx),
            "enter" if !event.keystroke.modifiers.shift => {
                // A fully typed name sends as-is; a prefix completes first.
                let typed = self.editor.read(cx).value().to_string();
                if slash::typing(&typed) == Some(selected.name) {
                    return;
                }
                self.complete_slash(selected.name, window, cx);
            }
            "escape" => self.slash_dismissed = true,
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Clear the draft and emit it as a scheduled message.
    fn schedule(&mut self, when: SendLater, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.editor
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.has_text = false;
        cx.emit(ComposerEvent::Schedule { text, when });
        cx.notify();
    }

    /// Switch between Enter-sends and Enter-inserts-a-newline (Cmd+Enter sends).
    pub fn set_enter_sends(&mut self, enter_sends: bool, cx: &mut Context<Self>) {
        self.enter_sends = enter_sends;
        self.editor
            .update(cx, |state, cx| state.set_submit_on_enter(enter_sends, cx));
    }

    fn track_draft(&mut self, cx: &mut Context<Self>) {
        let value = self.editor.read(cx).value().to_string();
        self.has_text = !value.trim().is_empty();
        if slash::typing(&value).is_none() {
            self.slash_dismissed = false;
        }
        self.slash_selected = 0;
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        self.editor
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.has_text = false;
        match slash::parse(&text) {
            Some(invocation) => cx.emit(ComposerEvent::SlashCommand(invocation)),
            None => cx.emit(ComposerEvent::Submit(text)),
        }
        cx.notify();
    }

    /// Wrap the selected draft text (or the caret) in a mrkdwn `marker` pair and keep the
    /// wrapped text selected.
    fn wrap_selection(&self, marker: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            let range = state.selected_range();
            let selected = state.selected_value().to_string();
            state.replace(format!("{marker}{selected}{marker}"), window, cx);
            let start = range.start + marker.len();
            state.set_selected_range(start..start + selected.len(), cx);
        });
    }

    /// Insert `prefix` at the caret.
    fn insert_prefix(&self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            let caret = state.selected_range().start;
            state.replace(prefix.to_owned(), window, cx);
            state.set_selected_range(caret + prefix.len()..caret + prefix.len(), cx);
        });
    }

    /// Insert a Slack link shell `<https://|label>` around the selection; the URL segment is
    /// selected so the next keystrokes replace it.
    fn insert_link(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |state, cx| {
            let range = state.selected_range();
            let selected = state.selected_value().to_string();
            state.replace(format!("<https://|{selected}>"), window, cx);
            let start = range.start + 1;
            state.set_selected_range(start..start + "https://".len(), cx);
        });
    }

    /// Insert `text` (an emoji pick) at the caret and give the draft focus back.
    pub fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_prefix(text, window, cx);
        self.editor.read(cx).focus_handle(cx).focus(window, cx);
        self.track_draft(cx);
    }

    fn apply(
        &self,
        action: &FormatAction,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            FormatAction::Wrap(marker) => self.wrap_selection(marker, window, cx),
            FormatAction::Insert(prefix) => self.insert_prefix(prefix, window, cx),
            FormatAction::Link => self.insert_link(window, cx),
            FormatAction::Emoji => cx.emit(ComposerEvent::OpenEmojiPicker(event.position())),
            FormatAction::Noop => {}
        }
    }
}

impl Composer {
    /// The chevron beside Send: Slack's "Schedule for later" menu.
    fn schedule_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        Button::new("schedule")
            .ghost()
            .with_size(Size::XSmall)
            .icon(IconName::ChevronDown)
            .tooltip("Schedule for later")
            .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
                for when in SendLater::ALL {
                    let this = this.clone();
                    menu = menu.item(
                        PopupMenuItem::new(when.label())
                            .icon(IconName::Clock)
                            .on_click(move |_, window, cx| {
                                // The composer is gone only if its view was dropped mid-click.
                                let _ = this
                                    .update(cx, |composer, cx| composer.schedule(when, window, cx));
                            }),
                    );
                }
                menu
            })
    }

    /// The slash-command menu floating above the frame.
    fn slash_menu_panel(
        &self,
        commands: &[&'static SlashCommand],
        selected: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (popover, border, muted, active, active_fg, hover) = (
            theme.popover,
            theme.border,
            theme.muted_foreground,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.accent,
        );
        let rows: Vec<_> = commands
            .iter()
            .enumerate()
            .map(|(ix, command)| {
                let on = ix == selected;
                let name = command.name;
                v_flex()
                    .id(("slash-row", ix))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(on, |row| row.bg(active).text_color(active_fg))
                    .when(!on, |row| row.hover(|el| el.bg(hover)))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("/{name}")),
                            )
                            .child(
                                div()
                                    .text_color(if on { active_fg } else { muted })
                                    .child(command.usage),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if on { active_fg } else { muted })
                            .child(command.description),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.complete_slash(name, window, cx);
                        this.editor.read(cx).focus_handle(cx).focus(window, cx);
                        cx.notify();
                    }))
            })
            .collect();
        v_flex()
            .id("slash-menu")
            .occlude()
            .absolute()
            .left(px(16.))
            .right(px(16.))
            .bottom(relative(1.))
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .gap_0p5()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(popover)
            .shadow_lg()
            .text_sm()
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(muted)
                    .child("Built-in commands \u{b7} \u{2191}\u{2193} navigate \u{b7} tab to complete \u{b7} esc to dismiss"),
            )
            .children(rows)
    }
}

/// One square toolbar/action button carrying `label`; applies `action` on click.
fn format_button(
    id: &'static str,
    label: impl IntoElement,
    action: FormatAction,
    cx: &mut Context<Composer>,
) -> Stateful<Div> {
    let muted = cx.theme().muted_foreground;
    let hover_bg = cx.theme().secondary_hover;
    div()
        .id(SharedString::from(id))
        .flex_none()
        .size(px(26.))
        .rounded_sm()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(muted)
        .hover(|el| el.bg(hover_bg))
        .child(label)
        .on_click(cx.listener(move |this, event, window, cx| {
            this.apply(&action, event, window, cx);
        }))
}

/// The thin vertical separator between action-row groups.
fn separator(color: gpui_kit::gpui::Hsla) -> Div {
    div().w(px(1.)).h(px(16.)).rounded_full().bg(color)
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(placeholder) = self.pending_placeholder.take() {
            let editor = self.editor.clone();
            window.defer(cx, move |window, cx| {
                editor.update(cx, |state, cx| {
                    state.set_placeholder(placeholder, window, cx)
                });
            });
        }
        if let Some(text) = self.pending.take() {
            let editor = self.editor.clone();
            window.defer(cx, move |window, cx| {
                editor.update(cx, |state, cx| state.set_value(text, window, cx));
            });
        }
        let slash_menu = self.slash_menu(cx);
        let slash_selected = self.slash_selected.min(slash_menu.len().saturating_sub(1));
        let theme = cx.theme();
        let frame_border = theme.input;
        let frame_bg = theme.background;
        let muted = theme.muted_foreground;
        let accent = theme.accent;
        let border = theme.border;
        let send_color = if self.has_text { theme.primary } else { muted };
        let icon = |name: IconName| Icon::new(name).text_color(muted).size(px(15.));
        let divider = || div().mx_1().w(px(1.)).h(px(16.)).bg(border);

        // Slack's grouping: text style | link | lists | quote and code.
        let toolbar = h_flex()
            .items_center()
            .gap_0p5()
            .px_1p5()
            .pt_1p5()
            .child(format_button(
                "fmt-bold",
                icon(IconName::Bold),
                FormatAction::Wrap("*"),
                cx,
            ))
            .child(format_button(
                "fmt-italic",
                icon(IconName::Italic),
                FormatAction::Wrap("_"),
                cx,
            ))
            .child(format_button(
                "fmt-strike",
                icon(IconName::Strikethrough),
                FormatAction::Wrap("~"),
                cx,
            ))
            .child(divider())
            .child(format_button(
                "fmt-link",
                icon(IconName::Link),
                FormatAction::Link,
                cx,
            ))
            .child(divider())
            .child(format_button(
                "fmt-ol",
                icon(IconName::ListOrdered),
                FormatAction::Insert("1. "),
                cx,
            ))
            .child(format_button(
                "fmt-ul",
                icon(IconName::List),
                FormatAction::Insert("• "),
                cx,
            ))
            .child(divider())
            .child(format_button(
                "fmt-quote",
                icon(IconName::Quote),
                FormatAction::Insert("> "),
                cx,
            ))
            .child(format_button(
                "fmt-code",
                icon(IconName::CodeXml),
                FormatAction::Wrap("`"),
                cx,
            ));

        let actions = h_flex()
            .items_center()
            .justify_between()
            .px_2()
            .pb_1p5()
            .pt_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_0p5()
                    .child(format_button(
                        "act-plus",
                        icon(IconName::Plus),
                        FormatAction::Noop,
                        cx,
                    ))
                    .child(format_button(
                        "act-format",
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Aa"),
                        FormatAction::Noop,
                        cx,
                    ))
                    .child(format_button(
                        "act-emoji",
                        div().text_sm().child("\u{1F642}"),
                        FormatAction::Emoji,
                        cx,
                    ))
                    .child(format_button(
                        "act-mention",
                        icon(IconName::AtSign),
                        FormatAction::Noop,
                        cx,
                    ))
                    .child(separator(border))
                    .child(format_button(
                        "act-video",
                        icon(IconName::Video),
                        FormatAction::Noop,
                        cx,
                    ))
                    .child(format_button(
                        "act-mic",
                        icon(IconName::Mic),
                        FormatAction::Noop,
                        cx,
                    ))
                    .child(separator(border))
                    .child(format_button(
                        "act-slash",
                        icon(IconName::Slash),
                        FormatAction::Insert("/"),
                        cx,
                    )),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_0p5()
                    .child(
                        div()
                            .id("send")
                            .flex_none()
                            .size(px(28.))
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .text_color(send_color)
                            .hover(|el| el.bg(accent))
                            .child(
                                Icon::new(IconName::Send)
                                    .text_color(send_color)
                                    .size(px(16.)),
                            )
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                    )
                    .when(self.scheduling, |row| row.child(self.schedule_button(cx))),
            );

        let menu = (!slash_menu.is_empty())
            .then(|| self.slash_menu_panel(&slash_menu, slash_selected, cx));

        div()
            .relative()
            .w_full()
            .px_4()
            .pb_3()
            .capture_key_down(cx.listener(Self::on_key))
            .children(menu)
            .child(
                v_flex()
                    .w_full()
                    .rounded_lg()
                    .border_1()
                    .border_color(frame_border)
                    .bg(frame_bg)
                    .child(toolbar)
                    .child(
                        div().w_full().min_w_0().px_2().py_1().child(
                            Textarea::new(&self.editor)
                                .appearance(false)
                                .bordered(false),
                        ),
                    )
                    .child(actions),
            )
    }
}
