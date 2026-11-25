//! Rendering for `SearchView`: header tabs/pills, empty states, and per-tab result lists.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, Size, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use slackless_core::store::SearchSort;
use slackless_core::types::{ChannelId, Conversation, File, Message, Ts, User};

use super::state::{SearchEvent, SearchSource, SearchTab, SearchView};
use crate::messages::{list_stamp, message_row};

impl SearchView {
    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border, hover, foreground, link, secondary) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
            theme.link,
            theme.secondary,
        );
        let summary = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => match self.local_results.len() {
                0 => "Local · instant".to_owned(),
                1 => "1 local hit".to_owned(),
                n => format!("{n} local hits"),
            },
            (SearchTab::Messages, SearchSource::Remote) => {
                if self.remote_messages_loading && self.remote_messages.is_empty() {
                    "Searching Slack…".to_owned()
                } else if self.remote_messages_total > 0 {
                    format!("{} on Slack", self.remote_messages_total)
                } else {
                    format!("{} on Slack", self.remote_messages.len())
                }
            }
            (SearchTab::Transcripts, _) => match self.transcript_results.len() {
                0 => "Huddle transcripts · instant".to_owned(),
                1 => "1 transcript hit".to_owned(),
                n => format!("{n} transcript hits"),
            },
            (SearchTab::Files, _) => {
                if self.remote_files_loading && self.remote_files.is_empty() {
                    "Searching Slack files…".to_owned()
                } else if self.remote_files_total > 0 {
                    format!("{} files on Slack", self.remote_files_total)
                } else {
                    format!("{} files", self.remote_files.len())
                }
            }
            (SearchTab::Channels, _) => format!("{} channels", self.matching_channels().len()),
            (SearchTab::People, _) => format!("{} people", self.matching_people().len()),
            (SearchTab::Dms, _) => format!("{} DMs", self.matching_dms().len()),
        };

        let pill = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .text_xs()
                .cursor_pointer()
                .when(active, |el| {
                    el.bg(secondary)
                        .text_color(foreground)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |el| {
                    el.text_color(muted).hover(|h| h.text_color(foreground))
                })
                .child(label)
        };

        let tab_btn = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .h_full()
                .flex()
                .items_center()
                .px_1()
                .cursor_pointer()
                .text_sm()
                .text_color(if active { foreground } else { muted })
                .when(active, |t| {
                    t.font_weight(FontWeight::SEMIBOLD)
                        .border_b_2()
                        .border_color(link)
                })
                .hover(|h| h.text_color(foreground))
                .child(label)
        };

        v_flex()
            .flex_none()
            .border_b_1()
            .border_color(border)
            .child(
                h_flex()
                    .h(px(46.))
                    .px_4()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(16.)))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Results for \u{201C}{}\u{201D}", self.query)),
                    )
                    .child(div().text_xs().text_color(muted).child(summary))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("search-close")
                            .size(px(28.))
                            .flex_none()
                            .rounded_sm()
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|el| el.bg(hover))
                            .child(Icon::new(IconName::X).text_color(muted).size(px(16.)))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Close))),
                    ),
            )
            .child(
                h_flex()
                    .h(px(36.))
                    .px_4()
                    .gap_4()
                    .items_center()
                    .child(
                        h_flex()
                            .h_full()
                            .gap_4()
                            .child(
                                tab_btn(
                                    "search-tab-messages",
                                    "Messages",
                                    self.tab == SearchTab::Messages,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::Messages, cx);
                                    },
                                )),
                            )
                            .child(
                                tab_btn(
                                    "search-tab-transcripts",
                                    "Huddles",
                                    self.tab == SearchTab::Transcripts,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::Transcripts, cx);
                                    },
                                )),
                            )
                            .when(!self.fixtures.sample, |tabs| {
                                tabs.child(
                                    tab_btn(
                                        "search-tab-files",
                                        "Files",
                                        self.tab == SearchTab::Files,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, _, cx| {
                                            this.set_tab(SearchTab::Files, cx);
                                        },
                                    )),
                                )
                            })
                            .child(
                                tab_btn(
                                    "search-tab-channels",
                                    "Channels",
                                    self.tab == SearchTab::Channels,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::Channels, cx);
                                    },
                                )),
                            )
                            .child(
                                tab_btn(
                                    "search-tab-people",
                                    "People",
                                    self.tab == SearchTab::People,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_tab(SearchTab::People, cx);
                                    },
                                )),
                            )
                            .child(
                                tab_btn("search-tab-dms", "DMs", self.tab == SearchTab::Dms)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.set_tab(SearchTab::Dms, cx);
                                    })),
                            ),
                    )
                    .child(div().flex_1())
                    .when(
                        self.tab == SearchTab::Messages && !self.fixtures.sample,
                        |bar| {
                            bar.child(
                                h_flex()
                                    .items_center()
                                    .gap_1()
                                    .p(px(2.))
                                    .rounded_md()
                                    .border_1()
                                    .border_color(border)
                                    .child(
                                        pill(
                                            "search-src-local",
                                            "Local (instant)",
                                            self.source == SearchSource::Local,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.set_source(SearchSource::Local, cx);
                                            }),
                                        ),
                                    )
                                    .child(
                                        pill(
                                            "search-src-remote",
                                            "Slack (remote)",
                                            self.source == SearchSource::Remote,
                                        )
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.set_source(SearchSource::Remote, cx);
                                            }),
                                        ),
                                    ),
                            )
                        },
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                pill(
                                    "search-sort-recent",
                                    "Most recent",
                                    self.sort == SearchSort::Recent,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_sort(SearchSort::Recent, cx);
                                    },
                                )),
                            )
                            .child(
                                pill(
                                    "search-sort-relevant",
                                    "Most relevant",
                                    self.sort == SearchSort::Relevant,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.set_sort(SearchSort::Relevant, cx);
                                    },
                                )),
                            ),
                    ),
            )
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> Div {
        let (title, hint, offer_remote) = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => {
                if self.archive.is_none() {
                    (
                        "Your local archive is empty".to_owned(),
                        "Messages you open are cached locally for instant offline search.",
                        !self.fixtures.sample && !self.query.is_empty(),
                    )
                } else if let Some(error) = &self.local_error {
                    (
                        format!("Local search failed: {error}"),
                        "Check the query syntax and try again.",
                        false,
                    )
                } else if self.query.is_empty() {
                    (
                        "Search your workspace".to_owned(),
                        "Type in the search bar and press Enter. Supports in:#ch, from:@user, -exclude, \"phrase\", has:file.",
                        false,
                    )
                } else {
                    (
                        format!("No local matches for \u{201C}{}\u{201D}", self.query),
                        "Local search only checks channels you've synced. Search Slack remotely for older history, unjoined channels, and files.",
                        !self.fixtures.sample,
                    )
                }
            }
            (SearchTab::Messages, SearchSource::Remote) => {
                if self.remote_messages_loading {
                    (
                        format!("Searching Slack for \u{201C}{}\u{201D}…", self.query),
                        "Querying Slack's full server index across workspace history and channels.",
                        false,
                    )
                } else if let Some(err) = &self.remote_messages_error {
                    (
                        format!("Remote search failed: {err}"),
                        "Switch back to Local for instant offline results.",
                        false,
                    )
                } else {
                    (
                        format!("No Slack matches for \u{201C}{}\u{201D}", self.query),
                        "Try fewer words, or narrow with in:#channel, from:@user, or -exclude.",
                        false,
                    )
                }
            }
            (SearchTab::Files, _) => {
                if self.remote_files_loading {
                    (
                        format!("Searching Slack files for \u{201C}{}\u{201D}…", self.query),
                        "Searching server-indexed file names and contents.",
                        false,
                    )
                } else if let Some(err) = &self.remote_files_error {
                    (
                        format!("File search failed: {err}"),
                        "Try again in a moment.",
                        false,
                    )
                } else {
                    (
                        format!("No files matching \u{201C}{}\u{201D}", self.query),
                        "Try a filename, extension, or keyword.",
                        false,
                    )
                }
            }
            (SearchTab::Transcripts, _) => (
                format!(
                    "No huddle transcript matches for \u{201C}{}\u{201D}",
                    self.query
                ),
                "Finalized live captions and local multilingual (EN/SV) huddle transcripts are indexed here automatically.",
                false,
            ),
            (SearchTab::Channels | SearchTab::People | SearchTab::Dms, _) => {
                ("No matches".to_owned(), "Try another query.", false)
            }
        };
        let muted = cx.theme().muted_foreground;
        let tile = cx.theme().muted;
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_8()
            .child(
                div()
                    .size(px(56.))
                    .rounded_full()
                    .bg(tile)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::Search).text_color(muted).size(px(24.))),
            )
            .child(div().font_weight(FontWeight::BOLD).child(title))
            .child(div().text_sm().text_color(muted).child(hint))
            .when(offer_remote, |col| {
                col.child(
                    div().pt_2().child(
                        Button::new("search-empty-remote")
                            .primary()
                            .with_size(Size::Small)
                            .icon(IconName::Globe)
                            .label(format!(
                                "Search Slack remotely for \u{201C}{}\u{201D}",
                                self.query
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_source(SearchSource::Remote, cx);
                            })),
                    ),
                )
            })
    }

    fn message_hit_row(
        &self,
        ix: usize,
        channel: &ChannelId,
        channel_name: Option<&str>,
        message: &Message,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border, hover) = (theme.muted_foreground, theme.border, theme.accent);
        let label = self
            .fixtures
            .conversation(channel)
            .map(|c| self.fixtures.conversation_label(c))
            .or_else(|| channel_name.map(|n| format!("#{n}")))
            .unwrap_or_else(|| channel.to_string());
        let open_channel = channel.clone();
        let ts = message.ts.clone();
        let row = message_row(&self.fixtures, message, None, cx);
        v_flex()
            .id(("search-hit", ix))
            .gap_1()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(border)
            .cursor_pointer()
            .hover(|el| el.bg(hover))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{label} · {}", list_stamp(&message.ts))),
            )
            .child(row)
            .on_click(cx.listener(move |_, _, _, cx| {
                cx.emit(SearchEvent::Open {
                    channel: open_channel.clone(),
                    ts: ts.clone(),
                });
            }))
    }

    fn file_hit_row(
        &self,
        ix: usize,
        file: &File,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let (muted, border, hover, foreground) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
        );
        let title = file
            .title
            .clone()
            .or_else(|| file.name.clone())
            .unwrap_or_else(|| file.id.to_string())
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let filename = file
            .name
            .clone()
            .unwrap_or_else(|| format!("{}.bin", file.id))
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&");
        let owner = file
            .user
            .as_ref()
            .map(|u| self.fixtures.user_name(u))
            .unwrap_or("Someone");
        let kind = file
            .pretty_type
            .as_deref()
            .or(file.filetype.as_deref())
            .unwrap_or("File");
        let size = file
            .size
            .map(|b| crate::composer::format_bytes(b.max(0) as usize))
            .unwrap_or_default();
        let meta = if size.is_empty() {
            format!("{kind} · Shared by {owner}")
        } else {
            format!("{kind} · {size} · Shared by {owner}")
        };
        let download_url = file
            .url_private_download
            .clone()
            .or_else(|| file.url_private.clone());
        let permalink = file.permalink.clone();
        h_flex()
            .id(("search-file", ix))
            .items_center()
            .gap_3()
            .px_4()
            .py_2p5()
            .border_b_1()
            .border_color(border)
            .hover(|el| el.bg(hover))
            .child(
                Icon::new(IconName::FileText)
                    .text_color(muted)
                    .size(px(18.))
                    .flex_none(),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child(title),
                    )
                    .child(div().truncate().text_xs().text_color(muted).child(meta)),
            )
            .when_some(download_url, |row, url| {
                let name = filename.clone();
                row.child(
                    Button::new(("search-file-dl", ix))
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::Download)
                        .tooltip("Download")
                        .on_click(move |_, window, cx| {
                            crate::media::download(url.clone(), name.clone(), window, cx);
                        }),
                )
            })
            .when_some(permalink, |row, url| {
                row.child(
                    Button::new(("search-file-open", ix))
                        .ghost()
                        .with_size(Size::Small)
                        .icon(IconName::ExternalLink)
                        .tooltip("Open in Slack")
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                )
            })
    }

    fn render_local_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.local_results.is_empty() {
            return self.empty_state(cx).into_any_element();
        }
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let hits: Vec<_> = self
            .local_results
            .iter()
            .enumerate()
            .map(|(ix, hit)| self.message_hit_row(ix, &hit.channel, None, &hit.message, cx))
            .collect();
        let query_for_cta = self.query.clone();
        v_flex()
            .id(SharedString::from("search-results"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(hits)
            .when(!self.fixtures.sample && !query_for_cta.is_empty(), |list| {
                list.child(
                    h_flex()
                        .px_4()
                        .py_3()
                        .items_center()
                        .justify_between()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            div().text_xs().text_color(muted).child(
                                "Showing instant local results. Need older history or unjoined channels?",
                            ),
                        )
                        .child(
                            Button::new("search-footer-remote")
                                .outline()
                                .with_size(Size::Small)
                                .icon(IconName::Globe)
                                .label("Search Slack remotely")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.set_source(SearchSource::Remote, cx);
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_remote_messages(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.remote_messages.is_empty() {
            return self.empty_state(cx).into_any_element();
        }
        let hits: Vec<_> = self
            .remote_messages
            .iter()
            .enumerate()
            .map(|(ix, hit)| {
                self.message_hit_row(
                    ix,
                    &hit.channel,
                    hit.channel_name.as_deref(),
                    &hit.message,
                    cx,
                )
            })
            .collect();
        let has_more = self.remote_messages_page < self.remote_messages_pages;
        let next_page = self.remote_messages_page + 1;
        let loading = self.remote_messages_loading;
        v_flex()
            .id(SharedString::from("search-results-remote"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(hits)
            .when(has_more, |list| {
                list.child(
                    h_flex().justify_center().py_3().child(
                        Button::new("search-remote-more")
                            .outline()
                            .with_size(Size::Small)
                            .label(if loading {
                                "Loading…"
                            } else {
                                "Load more from Slack"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.remote_messages_loading {
                                    this.request_remote_messages(next_page, cx);
                                }
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_transcripts(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.transcript_results.is_empty() {
            return self.empty_state(cx).into_any_element();
        }
        let theme = cx.theme();
        let (muted, border, fg, link_color, hover, pill_bg) = (
            theme.muted_foreground,
            theme.border,
            theme.foreground,
            theme.link,
            theme.accent,
            theme.secondary.opacity(0.6),
        );
        let query_for_click = (!self.query.trim().is_empty()).then(|| self.query.clone());
        let rows: Vec<_> = self
            .transcript_results
            .iter()
            .enumerate()
            .map(|(ix, seg)| {
                let ch_label = self
                    .fixtures
                    .conversation(&seg.channel)
                    .map(|c| self.fixtures.conversation_label(c))
                    .unwrap_or_else(|| seg.channel.to_string());
                let speaker = seg
                    .speaker_id
                    .as_ref()
                    .map(|u| self.fixtures.user_name(u).to_owned())
                    .unwrap_or_else(|| "Speaker".to_owned());
                let avatar = match &seg.speaker_id {
                    Some(uid) => crate::avatars::user_chip(&self.fixtures, uid, 28.),
                    None => div()
                        .size(px(28.))
                        .rounded_md()
                        .bg(pill_bg)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Headphones)
                                .size(px(14.))
                                .text_color(muted),
                        ),
                };
                let ts = Ts::from(format!("{}.000000", seg.at_ms.div_euclid(1_000)));
                let stamp = list_stamp(&ts);
                let lang_badge = seg.lang.as_deref().map(|l| l.to_ascii_uppercase());
                let call_id = seg.call_id.clone();
                let channel = seg.channel.clone();
                let seg_id = seg.id;
                let hl_query = query_for_click.clone();
                h_flex()
                    .id(("search-transcript", ix))
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(move |el| el.bg(hover))
                    .child(div().flex_none().mt(px(2.)).child(avatar))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap_1p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .child(speaker),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!("in {ch_label} \u{b7} {stamp}")),
                                    )
                                    .when_some(lang_badge, |row, code| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .bg(link_color.opacity(0.14))
                                                .text_xs()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(link_color)
                                                .child(code),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .text_sm()
                                    .line_height(px(20.))
                                    .text_color(fg)
                                    .child(seg.text.clone()),
                            ),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::OpenTranscript {
                            call_id: call_id.clone(),
                            channel: channel.clone(),
                            highlight_segment_id: Some(seg_id),
                            highlight_query: hl_query.clone(),
                        });
                    }))
            })
            .collect();
        v_flex()
            .id(SharedString::from("search-transcripts-local"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    }

    fn render_files(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.remote_files.is_empty() {
            return self.empty_state(cx).into_any_element();
        }
        let rows: Vec<_> = self
            .remote_files
            .iter()
            .enumerate()
            .map(|(ix, file)| self.file_hit_row(ix, file, cx))
            .collect();
        let has_more = self.remote_files_page < self.remote_files_pages;
        let next_page = self.remote_files_page + 1;
        let loading = self.remote_files_loading;
        v_flex()
            .id(SharedString::from("search-files-remote"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .when(has_more, |list| {
                list.child(
                    h_flex().justify_center().py_3().child(
                        Button::new("search-files-more")
                            .outline()
                            .with_size(Size::Small)
                            .label(if loading {
                                "Loading…"
                            } else {
                                "Load more files"
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.remote_files_loading {
                                    this.request_remote_files(next_page, cx);
                                }
                            })),
                    ),
                )
            })
            .into_any_element()
    }

    fn render_channels(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, hover, fg) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
        );
        let channels: Vec<Conversation> = self.matching_channels().into_iter().cloned().collect();
        if channels.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("No matching channels"),
                )
                .into_any_element();
        }
        v_flex()
            .id(SharedString::from("search-channels-list"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(channels.into_iter().enumerate().map(|(ix, ch)| {
                let id = ch.id.clone();
                let icon = if ch.is_private {
                    IconName::Lock
                } else {
                    IconName::Hash
                };
                let name = ch.name.clone().unwrap_or_else(|| ch.id.to_string());
                let desc = ch
                    .purpose
                    .as_ref()
                    .map(|p| p.value.clone())
                    .filter(|s| !s.is_empty())
                    .or_else(|| ch.topic.as_ref().map(|t| t.value.clone()))
                    .unwrap_or_default();
                h_flex()
                    .id(("search-ch", ix))
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2p5()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(Icon::new(icon).size(px(16.)).text_color(muted))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(name),
                            )
                            .when(!desc.is_empty(), |col| {
                                col.child(div().truncate().text_xs().text_color(muted).child(desc))
                            }),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::OpenChannel(id.clone()));
                    }))
            }))
            .into_any_element()
    }

    fn render_people(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, hover, fg) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
        );
        let people: Vec<User> = self.matching_people().into_iter().cloned().collect();
        if people.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child("No matching people"),
                )
                .into_any_element();
        }
        v_flex()
            .id(SharedString::from("search-people-list"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(people.into_iter().enumerate().map(|(ix, user)| {
                let uid = user.id.clone();
                let name = self.fixtures.name_of(&user).to_owned();
                let title = user.profile.title.clone().unwrap_or_default();
                let avatar = crate::avatars::user_chip(&self.fixtures, &uid, 32.);
                h_flex()
                    .id(("search-person", ix))
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(avatar)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(fg)
                                    .child(name),
                            )
                            .when(!title.is_empty(), |col| {
                                col.child(div().truncate().text_xs().text_color(muted).child(title))
                            }),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::OpenProfile(uid.clone()));
                    }))
            }))
            .into_any_element()
    }

    fn render_dms(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, hover, fg, bg) = (
            theme.muted_foreground,
            theme.border,
            theme.accent,
            theme.foreground,
            theme.background,
        );
        let dms: Vec<Conversation> = self.matching_dms().into_iter().cloned().collect();
        if dms.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .child(div().text_sm().text_color(muted).child("No matching DMs"))
                .into_any_element();
        }
        v_flex()
            .id(SharedString::from("search-dms-list"))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(dms.into_iter().enumerate().map(|(ix, dm)| {
                let id = dm.id.clone();
                let label = self.fixtures.conversation_label(&dm);
                let avatar = crate::avatars::conversation_avatar(&self.fixtures, &dm, 28., bg, cx);
                h_flex()
                    .id(("search-dm", ix))
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .hover(|el| el.bg(hover))
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(fg)
                            .child(label),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SearchEvent::OpenChannel(id.clone()));
                    }))
            }))
            .into_any_element()
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = self.header(cx);
        let body = match (self.tab, self.source) {
            (SearchTab::Messages, SearchSource::Local) => self.render_local_messages(cx),
            (SearchTab::Messages, SearchSource::Remote) => self.render_remote_messages(cx),
            (SearchTab::Transcripts, _) => self.render_transcripts(cx),
            (SearchTab::Files, _) => self.render_files(cx),
            (SearchTab::Channels, _) => self.render_channels(cx),
            (SearchTab::People, _) => self.render_people(cx),
            (SearchTab::Dms, _) => self.render_dms(cx),
        };
        v_flex().size_full().child(header).child(body)
    }
}
