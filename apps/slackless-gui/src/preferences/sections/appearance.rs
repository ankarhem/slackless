//! Preferences > Appearance & Themes section view.

use super::super::*;

impl Preferences {
    pub(super) fn appearance(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let (border, ring, is_dark) = (theme.border, theme.ring, theme.mode.is_dark());
        // Slack's Color Mode: three equal buttons in a row, icon beside the label, the chosen
        // one ringed in the accent.
        let mode = self.settings.mode;
        let modes = h_flex().gap_2().children(
            [
                (AppearanceMode::Light, "Light", IconName::Sun),
                (AppearanceMode::Dark, "Dark", IconName::Moon),
                (AppearanceMode::System, "System", IconName::Monitor),
            ]
            .into_iter()
            .enumerate()
            .map(|(ix, (choice, label, icon))| {
                let on = choice == mode;
                h_flex()
                    .id(("appearance-mode", ix))
                    .flex_1()
                    .h(px(38.))
                    .justify_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { ring } else { border })
                    .when(on, |button| button.shadow_xs())
                    .child(Icon::new(icon).size(px(16.)))
                    .child(div().text_sm().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.mode = choice;
                        this.changed(cx);
                    }))
            }),
        );

        let palette = self.settings.palette;
        let named =
            div()
                .flex()
                .flex_wrap()
                .gap_3()
                .children(Palette::NAMED.iter().enumerate().map(|(ix, &choice)| {
                    let (side, accent) = choice.swatch(is_dark);
                    self.swatch_card(
                        ("palette", ix),
                        choice.name(),
                        side,
                        accent,
                        choice == palette,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.palette = choice;
                        this.changed(cx);
                    }))
                }));

        let (custom_sidebar, custom_accent) = match palette {
            Palette::Custom { sidebar, accent } => (Some(sidebar), Some(accent)),
            _ => (None, None),
        };
        let swatch_row = |kind: &'static str,
                          colors: &'static [u32],
                          current: Option<usize>,
                          cx: &mut Context<Self>| {
            h_flex()
                .gap_2()
                .children(colors.iter().enumerate().map(move |(ix, &hex)| {
                    let on = current == Some(ix);
                    div()
                        .id(ElementId::NamedInteger(kind.into(), ix as u64))
                        .size(px(28.))
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::swatch_color(hex))
                        .border_2()
                        .border_color(if on { ring } else { border })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let (sidebar, accent) = match this.settings.palette {
                                Palette::Custom { sidebar, accent } => (sidebar, accent),
                                _ => (0, 0),
                            };
                            this.settings.palette = if kind == "custom-sidebar" {
                                Palette::Custom {
                                    sidebar: ix,
                                    accent,
                                }
                            } else {
                                Palette::Custom {
                                    sidebar,
                                    accent: ix,
                                }
                            };
                            this.changed(cx);
                        }))
                }))
        };
        let m = self.metrics();
        let custom_hex_row = {
            let input_border = cx.theme().input;
            let mono = cx.theme().mono_font_family.clone();
            let sb_preview = theme::parse_hex_rgb(&self.custom_sidebar_hex.read(cx).value())
                .map(theme::swatch_color);
            let ac_preview = theme::parse_hex_rgb(&self.custom_accent_hex.read(cx).value())
                .map(theme::swatch_color);
            let hex_box = |state: &Entity<TextareaState>, preview: Option<gpui_kit::gpui::Hsla>| {
                h_flex()
                    .items_center()
                    .gap_1()
                    .pl_2p5()
                    .pr_1()
                    .w(px(148.))
                    .h(m.height)
                    .rounded_md()
                    .border_1()
                    .border_color(input_border)
                    .when_some(preview, |row, col| {
                        row.child(
                            div()
                                .flex_none()
                                .size(px(14.))
                                .rounded_xs()
                                .bg(col)
                                .border_1()
                                .border_color(border),
                        )
                    })
                    .child(
                        div().flex_1().min_w_0().child(
                            Textarea::new(state)
                                .with_size(m.input_size)
                                .font_family(mono.clone())
                                .text_xs()
                                .appearance(false)
                                .bordered(false),
                        ),
                    )
            };
            h_flex()
                .items_center()
                .gap_2()
                .child(hex_box(&self.custom_sidebar_hex, sb_preview))
                .child(hex_box(&self.custom_accent_hex, ac_preview))
                .child(
                    self.btn(Button::new("custom-hex-apply").outline().label("Apply hex"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let sb =
                                theme::parse_hex_rgb(&this.custom_sidebar_hex.read(cx).value());
                            let ac = theme::parse_hex_rgb(&this.custom_accent_hex.read(cx).value());
                            if let (Some(sidebar), Some(accent)) = (sb, ac) {
                                this.settings.palette = Palette::CustomHex { sidebar, accent };
                                this.changed(cx);
                            }
                        })),
                )
        };
        let slack_theme_row = {
            v_flex().gap_2().child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        text_input_box_for(&self.slack_theme_import, true, true, m, cx)
                            .flex_1()
                            .min_w_0()
                            .max_w(px(420.)),
                    )
                    .child(
                        self.btn(Button::new("slack-theme-import").outline().label("Import"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let raw = this.slack_theme_import.read(cx).value().to_string();
                                if let Some((sidebar, accent)) =
                                    theme::parse_slack_theme_string(&raw)
                                {
                                    this.settings.palette = Palette::CustomHex { sidebar, accent };
                                    this.custom_sidebar_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(sidebar), window, cx)
                                    });
                                    this.custom_accent_hex.update(cx, |s, cx| {
                                        s.set_value(theme::format_hex_rgb(accent), window, cx)
                                    });
                                    this.changed(cx);
                                    crate::actions::notify(
                                        "Theme imported",
                                        "Applied Slack theme colors.",
                                        window,
                                        cx,
                                    );
                                } else {
                                    crate::actions::notify(
                                        "Invalid theme string",
                                        "Paste comma-separated #RRGGBB hex values from Slack.",
                                        window,
                                        cx,
                                    );
                                }
                            })),
                    )
                    .child(
                        self.btn(
                            Button::new("slack-theme-copy")
                                .outline()
                                .icon(IconName::Copy)
                                .label("Copy theme")
                                .tooltip("Copy Slack-compatible comma-separated hex theme string"),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let exported = theme::format_slack_theme_string(
                                    this.settings.palette,
                                    is_dark,
                                );
                                cx.write_to_clipboard(gpui_kit::gpui::ClipboardItem::new_string(
                                    exported.clone(),
                                ));
                                crate::actions::notify("Slack theme copied", exported, window, cx);
                            },
                        )),
                    ),
            )
        };
        let simplified = self.checkbox(
            "appearance-simplified",
            "Simplified layout (hide workspace rail and extra chrome)",
            |s| &mut s.simplified_layout,
            cx,
        );
        let custom = v_flex()
            .gap_3()
            .child(self.labeled(
                "Sidebar",
                swatch_row("custom-sidebar", &CUSTOM_SIDEBARS, custom_sidebar, cx),
            ))
            .child(self.labeled(
                "Accent",
                swatch_row("custom-accent", &CUSTOM_ACCENTS, custom_accent, cx),
            ))
            .child(self.labeled("Custom hex colors (Sidebar / Accent)", custom_hex_row))
            .child(self.labeled(
                "Share or import Slack theme (comma-separated hex)",
                slack_theme_row,
            ))
            .child(simplified);

        let gradient = self.checkbox(
            "appearance-gradient",
            "Window gradient",
            |s| &mut s.window_gradient,
            cx,
        );
        let darker = self.checkbox(
            "appearance-darker-sidebars",
            "Darker sidebars",
            |s| &mut s.darker_sidebars,
            cx,
        );
        let font = self.select("appearance-font", &theme::FONT_LABELS, |s| &mut s.font, cx);
        let density = self.radios("appearance-density", &DENSITY, |s| &mut s.density, cx);
        let skin = self.radios("appearance-skin", &SKIN_TONES, |s| &mut s.skin_tone, cx);
        let jumbo = self.checkbox(
            "appearance-jumbo",
            "Display jumbomoji",
            |s| &mut s.jumbomoji,
            cx,
        );
        let emoticons = self.checkbox(
            "appearance-emoticons",
            "Convert my typed emoticons to emoji, so :D becomes \u{1F603}",
            |s| &mut s.convert_emoticons,
            cx,
        );

        v_flex()
            .child(self.group("Font", None, font, cx))
            .child(self.group("Color Mode", Some("Choose if Slack's appearance should be light or dark, or follow your computer's settings."), modes, cx))
            .child(self.group("Density", Some("Adjust control and message spacing across the app."), density, cx))
            .child(self.group(
                "Slack themes",
                Some("Single-color themes that tint your sidebar, chrome, and highlights."),
                v_flex().gap_3().child(named).child(h_flex().gap_5().child(gradient).child(darker)),
                cx,
            ))
            .child(self.group("Custom theme", Some("Build your own: pick a sidebar and an accent color."), custom, cx))
            .child(self.group(
                "Emoji",
                None,
                v_flex().gap_2().child(self.labeled("Default skin tone", h_flex().child(skin))).child(jumbo).child(emoticons),
                cx,
            ))
    }

    /// A color-theme card: a mini sidebar + accent preview over the theme's name.
    fn swatch_card(
        &self,
        id: (&'static str, usize),
        name: &'static str,
        sidebar: gpui_kit::gpui::Hsla,
        accent: gpui_kit::gpui::Hsla,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let (border, ring, background, muted) =
            (theme.border, theme.ring, theme.background, theme.muted);
        v_flex()
            .id(id)
            .w(px(110.))
            .gap_1()
            .p_1p5()
            .rounded_md()
            .cursor_pointer()
            .border_2()
            .border_color(if selected { ring } else { border })
            .child(
                h_flex()
                    .h(px(44.))
                    .rounded_sm()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .w(px(36.))
                            .h_full()
                            .bg(sidebar)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(5.)).w_full().rounded_sm().bg(accent)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .bg(background)
                            .p_1()
                            .gap_1()
                            .child(div().h(px(4.)).w(px(40.)).rounded_sm().bg(muted))
                            .child(div().h(px(4.)).w(px(28.)).rounded_sm().bg(muted)),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .child(name),
            )
    }
}
