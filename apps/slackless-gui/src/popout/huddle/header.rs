//! Top titlebar, floating Smooth Jazz lounge music pill, and inline topic bar for Huddles.

use super::*;

impl PopoutWindow {
    pub(super) fn render_huddle_titlebar(
        &self,
        snap: &slackless_core::huddle::HuddleSessionSnapshot,
        people_count: usize,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use gpui_kit::component::Icon;

        let channel_name = self
            .fixtures
            .conversation(&snap.channel)
            .map(|c| {
                self.fixtures
                    .conversation_label(c)
                    .trim_start_matches("# ")
                    .to_owned()
            })
            .unwrap_or_else(|| self.title.clone());
        let count_text = if people_count == 1 {
            "1 person".to_owned()
        } else {
            format!("{people_count} people")
        };
        let ctrl_hover = pal.ctrl_hover;
        let ctrl_bg = pal.ctrl_bg;
        let ctrl_fg = pal.chrome_fg;

        TitleBar::new()
            .h(px(38.))
            .bg(pal.chrome_bg)
            .border_b_1()
            .border_color(pal.chrome_border)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .pr_2p5()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .justify_center()
                            .gap_1p5()
                            .child(
                                Icon::new(IconName::Headphones)
                                    .size(px(13.))
                                    .text_color(pal.chrome_muted),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(pal.chrome_fg)
                                    .child(format!("Huddle in {channel_name}")),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(pal.chrome_muted)
                                    .child(count_text),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .child(
                                h_flex()
                                    .id("popout-huddle-open-floating-panel")
                                    .h(px(24.))
                                    .px_2()
                                    .gap_1()
                                    .items_center()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(ctrl_bg)
                                    .hover(move |el| el.bg(ctrl_hover))
                                    .text_xs()
                                    .text_color(ctrl_fg)
                                    .child(
                                        Icon::new(IconName::Pin)
                                            .size(px(12.))
                                            .text_color(pal.chrome_muted),
                                    )
                                    .child("Floating bar")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some(dock) = this.huddle_dock_entity() {
                                            dock.update(cx, |d, cx| {
                                                d.request_open_floating_panel(cx);
                                            });
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id("popout-huddle-dock-back")
                                    .size(px(26.))
                                    .rounded_md()
                                    .hover(move |el| el.bg(ctrl_hover))
                                    .cursor_pointer()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::PanelLeft)
                                            .size(px(14.))
                                            .text_color(pal.chrome_muted),
                                    )
                                    .on_click(|_, window, _| {
                                        window.remove_window();
                                    }),
                            ),
                    ),
            )
    }

    pub(super) fn render_huddle_music_pill(
        &self,
        active_key: &str,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::AnyElement {
        use gpui_kit::component::Icon;

        const SONGS: [(&str, &str); 10] = [
            ("jazz", "Smooth Jazz"),
            ("waiting", "Waiting Room"),
            ("subway", "Subway Music"),
            ("whistle", "Whistle Tune"),
            ("focus", "Focus Beats"),
            ("lofi", "Lo-fi Haus"),
            ("disco", "Happy Bass"),
            ("zen", "Sound Bath"),
            ("nature", "Nature Meditation"),
            ("piano", "Piano Concerto"),
        ];

        let active_label = SONGS
            .iter()
            .find(|(k, _)| *k == active_key)
            .map_or("Smooth Jazz", |(_, name)| *name);

        let send_stop = self.send.clone();
        let pill_bg = pal.tile_pill_bg;
        let pill_fg = pal.tile_pill_fg;
        let border = pal.drawer_border;
        let hover_bg = pal.drawer_hover;
        let muted = pal.drawer_muted;
        let primary = pal.primary;
        let pop_bg = pal.popover_bg;
        let pop_fg = pal.popover_fg;
        let pop_border = pal.popover_border;
        let pop_hover = pal.popover_hover;

        let mut wrapper = v_flex()
            .absolute()
            .right(px(14.))
            .top(px(12.))
            .items_end()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        wrapper = wrapper.child(
            h_flex()
                .px_2p5()
                .py_1()
                .gap_1p5()
                .items_center()
                .rounded_lg()
                .bg(pill_bg)
                .border_1()
                .border_color(border)
                .shadow_sm()
                .child(
                    h_flex()
                        .id("popout-huddle-music-picker-btn")
                        .cursor_pointer()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .py_0p5()
                        .rounded_md()
                        .hover(move |el| el.bg(hover_bg))
                        .child(Icon::new(IconName::Music).size(px(13.)).text_color(primary))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(pill_fg)
                                .child(active_label),
                        )
                        .child(
                            Icon::new(IconName::ChevronDown)
                                .size(px(12.))
                                .text_color(muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let next = !this.huddle_music_open;
                            this.close_huddle_popovers();
                            this.huddle_music_open = next;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("popout-huddle-music-stop-btn")
                        .cursor_pointer()
                        .size(px(20.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .hover(move |el| el.bg(hover_bg))
                        .child(Icon::new(IconName::X).size(px(12.)).text_color(muted))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_music_open = false;
                            (send_stop)(Command::SetHuddleJukebox(None));
                            if this.fixtures.sample {
                                if let Some(dock) = this.huddle_dock_entity() {
                                    dock.update(cx, |d, cx| d.set_jukebox_local(None, cx));
                                }
                            }
                            cx.notify();
                        })),
                ),
        );

        if self.huddle_music_open {
            let mut menu = v_flex()
                .w(px(200.))
                .p_1p5()
                .gap_0p5()
                .rounded_xl()
                .bg(pop_bg)
                .border_1()
                .border_color(pop_border)
                .shadow_xl();

            for (key, name) in SONGS {
                let is_active = key == active_key;
                let send_pick = self.send.clone();
                let key_owned = key.to_owned();
                menu = menu.child(
                    h_flex()
                        .id(SharedString::from(format!("popout-huddle-song-{key}")))
                        .cursor_pointer()
                        .px_2p5()
                        .py_1p5()
                        .rounded_md()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .text_color(if is_active { primary } else { pop_fg })
                        .font_weight(if is_active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .hover(move |el| el.bg(pop_hover))
                        .child(name)
                        .when(is_active, |el| {
                            el.child(Icon::new(IconName::Check).size(px(12.)).text_color(primary))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.huddle_music_open = false;
                            (send_pick)(Command::SetHuddleJukebox(Some(key_owned.clone())));
                            if this.fixtures.sample {
                                if let Some(dock) = this.huddle_dock_entity() {
                                    dock.update(cx, |d, cx| {
                                        d.set_jukebox_local(Some(key_owned.clone()), cx)
                                    });
                                }
                            }
                            cx.notify();
                        })),
                );
            }
            wrapper = wrapper.child(menu);
        }

        wrapper.into_any_element()
    }

    pub(super) fn render_huddle_topic_bar(
        &self,
        topic: Option<&str>,
        pal: &HuddlePalette,
        cx: &mut Context<Self>,
    ) -> gpui_kit::gpui::Div {
        let hover = pal.drawer_hover;
        if let Some(input) = &self.huddle_topic_editing {
            return h_flex()
                .items_center()
                .gap_1p5()
                .px_1p5()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(pal.drawer_border)
                .bg(pal.drawer_bg)
                .child(
                    div()
                        .w(px(180.))
                        .text_xs()
                        .text_color(pal.drawer_fg)
                        .child(Textarea::new(input).appearance(false).bordered(false)),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-save")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(pal.primary)
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(pal.primary_fg)
                        .child("Save")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(inp) = this.huddle_topic_editing.take() {
                                let text = inp.read(cx).value().trim().to_owned();
                                (this.send)(Command::SetHuddleTopic(text.clone()));
                                if this.fixtures.sample {
                                    if let Some(dock) = this.huddle_dock_entity() {
                                        let me = this.fixtures.self_user.clone();
                                        dock.update(cx, |d, cx| {
                                            d.apply_data_event(
                                                &slackless_core::huddle::HuddleDataEvent::Topic {
                                                    user: me,
                                                    topic: text,
                                                },
                                                cx,
                                            );
                                        });
                                    }
                                }
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .id("popout-huddle-topic-cancel")
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .hover(move |el| el.bg(hover))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(pal.drawer_muted)
                        .child("✕")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.huddle_topic_editing = None;
                            cx.notify();
                        })),
                );
        }
        let initial = topic.unwrap_or_default().to_owned();
        h_flex().min_w_0().items_center().child(
            h_flex()
                .id("popout-huddle-topic-edit")
                .px_1p5()
                .py_0p5()
                .gap_1()
                .items_center()
                .rounded_md()
                .hover(move |el| el.bg(hover))
                .cursor_pointer()
                .text_xs()
                .text_color(if topic.is_some() {
                    pal.tile_pill_fg
                } else {
                    pal.drawer_muted
                })
                .child(
                    div().max_w(px(200.)).truncate().child(
                        topic
                            .map(ToOwned::to_owned)
                            .unwrap_or_else(|| "Add a topic".to_owned()),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    let init = initial.clone();
                    let input = cx.new(|cx| {
                        let mut st = TextareaState::new(window, cx)
                            .placeholder("What are you discussing?")
                            .auto_grow(1, 1);
                        st.set_value(init, window, cx);
                        st
                    });
                    input.read(cx).focus_handle(cx).focus(window, cx);
                    this.huddle_topic_editing = Some(input);
                    cx.notify();
                })),
        )
    }
}
