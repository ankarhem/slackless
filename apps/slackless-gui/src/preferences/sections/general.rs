//! Preferences > Navigation, Home, Messages & media, Language & region, Accessibility, and Mark as read section views.

use super::*;

impl Preferences {
    pub(super) fn navigation(&mut self, cx: &mut Context<Self>) -> Div {
        let order = self.settings.nav_order;
        let rows: Vec<Div> = order
            .iter()
            .enumerate()
            .map(|(pos, &slot)| {
                let (id, label, flag): (&'static str, &'static str, Flag) = match slot {
                    0 => ("nav-home", "Home", |s| &mut s.show_home),
                    1 => ("nav-dms", "DMs", |s| &mut s.show_dms),
                    2 => ("nav-activity", "Activity", |s| &mut s.show_activity),
                    3 => ("nav-files", "Files", |s| &mut s.show_files),
                    _ => ("nav-later", "Later / To-dos", |s| &mut s.show_later),
                };
                let cb = self.checkbox(id, label, flag, cx);
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(cb)
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(("nav-up", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronUp)
                                    .disabled(pos == 0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos > 0 {
                                            this.settings.nav_order.swap(pos - 1, pos);
                                            this.changed(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new(("nav-down", pos))
                                    .ghost()
                                    .with_size(Size::XSmall)
                                    .icon(IconName::ChevronDown)
                                    .disabled(pos + 1 >= order.len())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if pos + 1 < this.settings.nav_order.len() {
                                            this.settings.nav_order.swap(pos, pos + 1);
                                            this.changed(cx);
                                        }
                                    })),
                            ),
                    )
            })
            .collect();
        let style = self.radios("nav-style", &TAB_STYLES, |s| &mut s.tab_style, cx);
        v_flex()
            .child(self.group(
                "Show & order navigation tabs",
                Some("Choose which tabs appear in the left rail and reorder them."),
                v_flex().gap_1().children(rows),
                cx,
            ))
            .child(self.group("Tab appearance", None, style, cx))
    }

    pub(super) fn home(&mut self, cx: &mut Context<Self>) -> Div {
        let show = self.select("home-show", &SHOW, |s| &mut s.show, cx);
        let sort = self.select("home-sort", &SORTS, |s| &mut s.sort, cx);
        let unreads = self.checkbox(
            "home-unreads",
            "Show all unreads in one section",
            |s| &mut s.unreads_section,
            cx,
        );
        let private = self.checkbox(
            "home-private",
            "Separate private channels from public ones",
            |s| &mut s.separate_private,
            cx,
        );
        let photos = self.checkbox(
            "home-photos",
            "Show profile photos next to DMs",
            |s| &mut s.profile_photos,
            cx,
        );
        let compact_rows = self.checkbox(
            "home-compact",
            "Use compact sidebar rows",
            |s| &mut s.sidebar_compact,
            cx,
        );
        let show_muted = self.checkbox(
            "home-show-muted",
            "Show muted conversations in sidebar",
            |s| &mut s.show_muted_channels,
            cx,
        );
        v_flex()
            .child(
                self.group(
                    "Show and sort",
                    Some("Change the default for how conversations show and sort in your sidebar."),
                    v_flex()
                        .gap_3()
                        .child(self.labeled("Show", show))
                        .child(self.labeled("Sort", sort)),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Organization",
                    None,
                    v_flex()
                        .gap_1()
                        .child(unreads)
                        .child(private)
                        .child(photos)
                        .child(compact_rows)
                        .child(show_muted),
                    cx,
                ),
            )
    }

    pub(super) fn messages(&mut self, cx: &mut Context<Self>) -> Div {
        let names = self.radios("msg-names", &NAMES, |s| &mut s.names, cx);
        let clock24 = self.checkbox(
            "msg-clock-24h",
            "Display times with a 24-hour clock",
            |s| &mut s.clock_24h,
            cx,
        );
        let emoji_text = self.checkbox(
            "msg-emoji-text",
            "Display emoji as plain text (:smile:)",
            |s| &mut s.emoji_text_mode,
            cx,
        );
        let enter = self.radio_rows(
            "msg-enter",
            &ENTER_OPTIONS,
            usize::from(!self.settings.enter_sends),
            |settings, ix| settings.enter_sends = ix == 0,
            cx,
        );
        let markup = self.checkbox(
            "msg-markup",
            "Format messages with markup",
            |s| &mut s.format_markup,
            cx,
        );
        let inline = self.checkbox(
            "msg-inline",
            "Show images and files uploaded to Slack",
            |s| &mut s.inline_media,
            cx,
        );
        let previews = self.checkbox(
            "msg-previews",
            "Show images and files from linked websites",
            |s| &mut s.link_previews,
            cx,
        );
        let large = self.checkbox(
            "msg-large",
            "Even if they're larger than 2 MB",
            |s| &mut s.large_files,
            cx,
        );
        let quick_toggle = self.checkbox(
            "msg-quick-reactions",
            "Show one-click reactions on messages",
            |s| &mut s.show_quick_reactions,
            cx,
        );
        let m = self.metrics();
        let border = cx.theme().input;
        let muted = cx.theme().muted_foreground;
        let bg_card = cx.theme().secondary;
        let quick_editors = self.settings.show_quick_reactions.then(|| {
            let mut row = h_flex().flex_wrap().items_center().gap_2().pl_6().pt_1();
            for ix in 0..3 {
                let code = self.settings.quick_reaction_emojis[ix].trim();
                let glyph = crate::emoji::glyph(code)
                    .or_else(|| crate::emoji::glyph(crate::actions::DEFAULT_QUICK_REACTIONS[ix]))
                    .unwrap_or("🙂");
                row = row.child(
                    h_flex()
                        .items_center()
                        .gap_0p5()
                        .pl_2p5()
                        .pr_1()
                        .h(m.height)
                        .rounded_md()
                        .bg(bg_card)
                        .border_1()
                        .border_color(border)
                        .child(div().flex_none().text_sm().child(glyph))
                        .child(
                            div().w(px(130.)).child(
                                Textarea::new(&self.quick_reaction_inputs[ix])
                                    .with_size(m.input_size)
                                    .appearance(false)
                                    .bordered(false),
                            ),
                        ),
                );
            }
            let reset = Button::new("msg-quick-reactions-reset")
                .ghost()
                .with_size(Size::XSmall)
                .label("Reset defaults")
                .on_click(cx.listener(|this, _, window, cx| {
                    let defaults = crate::actions::DEFAULT_QUICK_REACTIONS.map(String::from);
                    this.settings.quick_reaction_emojis = defaults.clone();
                    for (ix, val) in defaults.into_iter().enumerate() {
                        this.quick_reaction_inputs[ix]
                            .update(cx, |state, cx| state.set_value(val, window, cx));
                    }
                    this.changed(cx);
                }));
            v_flex().gap_1p5().child(row).child(
                h_flex()
                    .pl_6()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Enter emoji shortcodes (e.g. white_check_mark, eyes, rocket)."),
                    )
                    .child(reset),
            )
        });
        v_flex()
            .child(
                self.group(
                    "One-click reactions",
                    Some("Quickly react to messages from the hover toolbar."),
                    v_flex()
                        .gap_1p5()
                        .child(quick_toggle)
                        .children(quick_editors),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Names & timestamps",
                    None,
                    v_flex()
                        .gap_2()
                        .child(names)
                        .child(clock24)
                        .child(emoji_text),
                    cx,
                ),
            )
            .child(self.group(
                "When writing a message, press Enter to\u{2026}",
                None,
                v_flex().gap_2().child(enter).child(markup),
                cx,
            ))
            .child(
                self.group(
                    "Inline media & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(inline)
                        .child(previews)
                        .child(div().pl_6().child(large)),
                    cx,
                ),
            )
    }

    pub(super) fn language(&mut self, cx: &mut Context<Self>) -> Div {
        let language = self.select("lang", &LANGUAGES, |s| &mut s.language, cx);
        let auto = self.checkbox(
            "tz-auto",
            "Set time zone automatically",
            |s| &mut s.timezone_auto,
            cx,
        );
        let zone = self.select("tz", &ZONES, |s| &mut s.timezone, cx);
        let spell = self.checkbox(
            "spellcheck",
            "Enable spellcheck on your messages",
            |s| &mut s.spellcheck,
            cx,
        );
        v_flex()
            .child(self.group("Language", Some("Choose the language you want to use in Slack."), language, cx))
            .child(self.group("Time zone", Some("Slack uses your time zone to send summary and notification emails, and for times in your activity feeds and reminders."), v_flex().gap_2().child(auto).child(zone), cx))
            .child(self.group("Spellcheck", None, spell, cx))
    }

    pub(super) fn accessibility(&mut self, cx: &mut Context<Self>) -> Div {
        let m = self.metrics();
        let zoom = self.select_w("zoom", 148., &ZOOM, |s| &mut s.zoom, cx);
        let animate = self.checkbox(
            "a11y-animate",
            "Allow animated images and emoji",
            |s| &mut s.animate_emoji,
            cx,
        );
        let underline = self.checkbox(
            "a11y-underline",
            "Underline links to websites",
            |s| &mut s.underline_links,
            cx,
        );
        let motion = self.checkbox("a11y-motion", "Reduce motion", |s| &mut s.reduce_motion, cx);
        let transparency = self.checkbox(
            "a11y-transparency",
            "Translucent window background (macOS vibrancy)",
            |s| &mut s.window_transparency,
            cx,
        );
        let announce = self.checkbox(
            "a11y-announce",
            "Read incoming messages aloud with a screen reader",
            |s| &mut s.announce_incoming,
            cx,
        );
        let verbosity_current = self.settings.message_verbosity;
        let theme = cx.theme();
        let (border, ring) = (theme.input, theme.ring);
        let verbosity = h_flex()
            .gap_2()
            .children(MESSAGE_VERBOSITY.iter().enumerate().map(|(ix, &label)| {
                let level = ix as u8;
                let on = verbosity_current == level;
                h_flex()
                    .id(("a11y-verbosity", ix))
                    .items_center()
                    .h(m.height)
                    .px_3()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { ring } else { border })
                    .when(on, |el| el.font_weight(FontWeight::SEMIBOLD))
                    .text_sm()
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.message_verbosity = level;
                        this.changed(cx);
                    }))
            }));
        v_flex()
            .child(self.group(
                "Zoom",
                Some("Use \u{2318}+ / \u{2318}- to zoom anywhere in Slack."),
                zoom,
                cx,
            ))
            .child(
                self.group(
                    "Animation & links",
                    None,
                    v_flex()
                        .gap_1()
                        .child(animate)
                        .child(underline)
                        .child(motion)
                        .child(transparency),
                    cx,
                ),
            )
            .child(
                self.group(
                    "Screen reader",
                    Some(
                        "Choose how much metadata is announced when reading or replaying messages.",
                    ),
                    v_flex()
                        .gap_2()
                        .child(announce)
                        .child(self.labeled("Message verbosity", verbosity)),
                    cx,
                ),
            )
    }

    pub(super) fn mark_as_read(&mut self, cx: &mut Context<Self>) -> Div {
        let mode = self.radios("mark-read", &MARK_READ, |s| &mut s.mark_read, cx);
        let confirm = self.checkbox(
            "mark-confirm",
            "Prompt to confirm when marking all messages as read (Shift+Esc)",
            |s| &mut s.confirm_mark_all,
            cx,
        );
        let no_bg = self.checkbox(
            "mark-no-bg",
            "Keep messages unread while window is in the background",
            |s| &mut s.no_bg_mark_read,
            cx,
        );
        v_flex()
            .child(self.group("When I view a channel", None, mode, cx))
            .child(self.group(
                "Mark everything read",
                None,
                v_flex().gap_1().child(confirm).child(no_bg),
                cx,
            ))
    }
}
