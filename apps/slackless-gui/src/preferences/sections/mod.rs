//! Preferences section views (Notifications, Navigation, Home, Appearance, Messages, Language, Audio/Video, Accessibility, Mark as read, Advanced).

mod advanced;
mod appearance;
mod audio_video;
mod general;
mod notifications;
mod updates;

use super::*;

impl Preferences {
    pub(super) fn nav(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (foreground, muted, hover, active, active_fg, border) = (
            theme.foreground,
            theme.muted_foreground,
            theme.accent,
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.border,
        );
        v_flex()
            .w(px(220.))
            .flex_none()
            .h_full()
            .gap_0p5()
            .p_2()
            .border_r_1()
            .border_color(border)
            .children(Section::ALL.iter().enumerate().map(|(ix, &section)| {
                let selected = section == self.section;
                let color = if selected { active_fg } else { foreground };
                h_flex()
                    .id(("prefs-section", ix))
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(color)
                    .when(selected, |row| {
                        row.bg(active).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!selected, |row| row.hover(|el| el.bg(hover)))
                    .child(
                        Icon::new(section.icon())
                            .text_color(if selected { active_fg } else { muted })
                            .size(px(16.)),
                    )
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| this.show(section, cx)))
            }))
    }

    /// A titled group of controls, with an optional description under the title.
    fn group(
        &self,
        title: &str,
        hint: Option<&str>,
        body: impl IntoElement,
        cx: &Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let border = cx.theme().border;
        v_flex()
            .gap_2()
            .pb_5()
            .mb_5()
            .border_b_1()
            .border_color(border)
            .child(div().font_weight(FontWeight::BOLD).child(title.to_owned()))
            .children(hint.map(|hint| div().text_sm().text_color(muted).child(hint.to_owned())))
            .child(body)
    }

    fn checkbox(
        &mut self,
        id: &'static str,
        label: &'static str,
        flag: Flag,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let on = *flag(&mut self.settings);
        let theme = cx.theme();
        let (fill, fill_fg, border) = (
            theme.sidebar_accent,
            theme.sidebar_accent_foreground,
            theme.input,
        );
        h_flex()
            .id(id)
            .items_center()
            .gap_2()
            .py_0p5()
            .cursor_pointer()
            .text_sm()
            .child(
                div()
                    .size(px(16.))
                    .flex_none()
                    .rounded(px(3.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .when(on, |b| b.bg(fill).border_color(fill))
                    .when(!on, |b| b.border_color(border))
                    .when(on, |b| {
                        b.child(Icon::new(IconName::Check).text_color(fill_fg).size(px(12.)))
                    }),
            )
            .child(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                let value = flag(&mut this.settings);
                *value = !*value;
                this.changed(cx);
            }))
    }

    fn radios(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        self.radio_rows(
            id,
            options,
            selected,
            move |settings, ix| *choice(settings) = ix,
            cx,
        )
    }

    /// Radio rows for `options` with `selected` checked; `pick` stores a click.
    fn radio_rows(
        &self,
        id: &'static str,
        options: &'static [&'static str],
        selected: usize,
        pick: impl Fn(&mut Settings, usize) + Copy + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (fill, border) = (theme.sidebar_accent, theme.input);
        v_flex()
            .gap_1()
            .children(options.iter().enumerate().map(|(ix, label)| {
                let on = ix == selected;
                h_flex()
                    .id(ElementId::NamedInteger(id.into(), ix as u64))
                    .items_center()
                    .gap_2()
                    .py_0p5()
                    .cursor_pointer()
                    .text_sm()
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .rounded_full()
                            .border_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .border_color(if on { fill } else { border })
                            .when(on, |dot| {
                                dot.child(div().size(px(8.)).rounded_full().bg(fill))
                            }),
                    )
                    .child(*label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(&mut this.settings, ix);
                        this.changed(cx);
                    }))
            }))
    }

    /// A select: the current option in a bordered box; clicking expands the option list
    /// beneath it.
    fn select(
        &mut self,
        id: &'static str,
        options: &'static [&'static str],
        choice: Choice,
        cx: &mut Context<Self>,
    ) -> Div {
        let selected = *choice(&mut self.settings);
        let open = self.open_select == Some(id);
        let theme = cx.theme();
        let (border, muted, popover, hover, active) = (
            theme.input,
            theme.muted_foreground,
            theme.popover,
            theme.accent,
            theme.sidebar_accent,
        );
        let active_fg = theme.sidebar_accent_foreground;
        let trigger = h_flex()
            .id(id)
            .w(px(300.))
            .items_center()
            .justify_between()
            .px_3()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .text_sm()
            .child(options[selected])
            .child(
                Icon::new(IconName::ChevronDown)
                    .text_color(muted)
                    .size(px(14.)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_select = if this.open_select == Some(id) {
                    None
                } else {
                    Some(id)
                };
                cx.notify();
            }));
        let list = open.then(|| {
            v_flex()
                .w(px(300.))
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(popover)
                .children(options.iter().enumerate().map(|(ix, label)| {
                    let on = ix == selected;
                    div()
                        .id(ElementId::NamedInteger(
                            SharedString::from(format!("{id}-opt")),
                            ix as u64,
                        ))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .text_sm()
                        .when(on, |row| row.bg(active).text_color(active_fg))
                        .when(!on, |row| row.hover(|el| el.bg(hover)))
                        .child(*label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            *choice(&mut this.settings) = ix;
                            this.open_select = None;
                            this.changed(cx);
                        }))
                }))
        });
        v_flex().gap_1().child(trigger).children(list)
    }

    fn labeled(&self, label: &'static str, control: impl IntoElement) -> Div {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(control)
    }

    fn mono_chip(&self, text: impl Into<SharedString>, cx: &Context<Self>) -> Div {
        let theme = cx.theme();
        div()
            .px_1p5()
            .py(px(1.))
            .rounded_sm()
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .font_family(theme.mono_font_family.clone())
            .text_xs()
            .text_color(theme.foreground)
            .child(text.into())
    }

    #[allow(clippy::too_many_arguments)]
    fn status_card(
        &self,
        dot_color: gpui_kit::gpui::Hsla,
        title: impl Into<SharedString>,
        chip: Option<SharedString>,
        detail: impl Into<SharedString>,
        detail_is_err: bool,
        actions: Div,
        cx: &Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let border = theme.border;
        let card_bg = theme.secondary.opacity(0.35);

        let mut header_row = h_flex()
            .items_center()
            .gap_2()
            .flex_wrap()
            .child(div().size(px(8.)).rounded_full().flex_none().bg(dot_color))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if detail_is_err { danger } else { fg })
                    .child(title.into()),
            );
        if let Some(chip_text) = chip {
            header_row = header_row.child(self.mono_chip(chip_text, cx));
        }

        h_flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3p5()
            .py_2p5()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .child(
                v_flex().flex_1().min_w_0().gap_1().child(header_row).child(
                    div()
                        .text_xs()
                        .text_color(if detail_is_err { danger } else { muted })
                        .child(detail.into()),
                ),
            )
            .child(actions.flex_none())
    }

    pub(super) fn render_body(&mut self, cx: &mut Context<Self>) -> Div {
        match self.section {
            Section::Notifications => self.notifications(cx),
            Section::Navigation => self.navigation(cx),
            Section::Home => self.home(cx),
            Section::Appearance => self.appearance(cx),
            Section::Messages => self.messages(cx),
            Section::Language => self.language(cx),
            Section::AudioVideo => self.audio_video(cx),
            Section::Accessibility => self.accessibility(cx),
            Section::MarkAsRead => self.mark_as_read(cx),
            Section::Advanced => self.advanced(cx),
        }
    }
}
