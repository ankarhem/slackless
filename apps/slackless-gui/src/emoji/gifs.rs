//! GIF tab state, virtualized grid rendering, preset fallback, and GPU texture eviction.

use std::ops::Range;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, Context, FontWeight, Hsla, ImageSource, InteractiveElement as _, IntoElement as _,
    ObjectFit, ParentElement as _, ScrollStrategy, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, Window, div, img, px, rgb, uniform_list,
};
use slackless_core::api::GifItem;

use super::picker::{EmojiPicker, EmojiPickerEvent, GRID_HEIGHT};

/// Curated Giphy mood/reaction presets shown only in offline/fixture mode (when signed into a
/// live workspace, the GIFs tab loads Slack's real `gif.featured` and `gif.search` endpoints).
pub(super) const GIF_PRESETS: [(&str, &str, u32); 12] = [
    ("Celebrate", "🎉", 0x5B2C6F),
    ("Applause", "👏", 0x1B4F72),
    ("Thumbs up", "👍", 0x145A32),
    ("Laugh", "😂", 0x7D6608),
    ("Yes", "🙌", 0x0E6251),
    ("Thank you", "🙏", 0x641E16),
    ("Mind blown", "🤯", 0x4A235A),
    ("Shrug", "🤷", 0x2E4053),
    ("Popcorn", "🍿", 0x7E5109),
    ("Dancing", "💃", 0x78281F),
    ("High five", "✋", 0x1F618D),
    ("Facepalm", "🤦", 0x424949),
];

/// Filter offline GIF presets by case-insensitive substring match.
pub(super) fn filter_gif_presets(query: &str) -> Vec<(&'static str, &'static str, u32)> {
    let needle = query.trim().to_lowercase();
    GIF_PRESETS
        .iter()
        .copied()
        .filter(|(label, _, _)| needle.is_empty() || label.to_lowercase().contains(&needle))
        .collect()
}

/// Evict a single GIF URL's decoded frames and Metal textures from GPUI's window & asset cache.
pub(crate) fn evict_gif_asset(url: &str, window: &mut Window, cx: &mut gpui_kit::gpui::App) {
    let source = ImageSource::from(url.to_owned());
    if let ImageSource::Resource(ref resource) = source
        && let Some(Ok(data)) = window.get_asset::<gpui_kit::gpui::ImgResourceLoader>(resource, cx)
    {
        let _ = window.drop_image(data);
    }
    source.remove_asset(cx);
}

impl EmojiPicker {
    /// Receive GIF results from Slack's `gif.featured` / `gif.search` backend.
    pub fn set_gif_results(&mut self, query: &str, gifs: Vec<GifItem>, cx: &mut Context<Self>) {
        if query.trim() != self.gif_query.trim() {
            return;
        }
        self.hovered_gif = None;
        if let Some(anim) = self.active_anim_url.take() {
            self.stale_gif_urls.push(anim);
        }
        for old in self.gif_results.drain(..) {
            if !gifs.iter().any(|g| g.still_url == old.still_url) {
                self.stale_gif_urls.push(old.still_url);
            }
            if !gifs.iter().any(|g| g.preview_url == old.preview_url) {
                self.stale_gif_urls.push(old.preview_url);
            }
        }
        self.gif_loading = false;
        self.gif_loaded_query = Some(self.gif_query.trim().to_owned());
        self.gif_results = gifs;
        self.gif_grid.scroll_to_item_strict(0, ScrollStrategy::Top);
        cx.notify();
    }

    /// Evict all decoded GIF frames and Metal GPU textures when the picker closes or leaves
    /// the GIFs tab so memory returns to baseline and zero animation timers stay alive.
    pub fn release_gifs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.gif_debounce = None;
        self.gif_loading = false;
        self.gif_loaded_query = None;
        self.hovered_gif = None;
        if let Some(anim) = self.active_anim_url.take() {
            evict_gif_asset(&anim, window, cx);
        }
        for url in self.stale_gif_urls.drain(..) {
            evict_gif_asset(&url, window, cx);
        }
        for gif in self.gif_results.drain(..) {
            evict_gif_asset(&gif.still_url, window, cx);
            if gif.preview_url != gif.still_url {
                evict_gif_asset(&gif.preview_url, window, cx);
            }
        }
    }

    pub(super) fn request_gifs(&mut self, debounce: bool, cx: &mut Context<Self>) {
        if self.fixtures.sample {
            self.gif_loading = false;
            self.gif_debounce = None;
            return;
        }
        let query = self.gif_query.trim().to_owned();
        if self.gif_loaded_query.as_deref() == Some(query.as_str()) && !self.gif_results.is_empty()
        {
            self.gif_loading = false;
            self.gif_debounce = None;
            return;
        }
        self.gif_loading = true;
        if !debounce {
            self.gif_debounce = None;
            cx.emit(EmojiPickerEvent::SearchGifs(query));
            return;
        }
        self.gif_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let _ = this.update(cx, |picker, cx| {
                if picker.gif_query.trim() == query {
                    cx.emit(EmojiPickerEvent::SearchGifs(query));
                }
            });
        }));
    }

    /// Virtualized 3-column GIF rows: each tile renders its lightweight single-frame
    /// `still_url` (`tinygifpreview`, ~20 KB / 160 KB decoded, 0% idle CPU), and only the
    /// hovered tile overlays its animated `preview_url` (`tinygif`), evicting the previous
    /// animated texture as soon as hover moves so memory stays bounded.
    fn render_gif_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        const GIF_COLS: usize = 3;
        let secondary = cx.theme().secondary;
        let tile = px(100.);
        range
            .filter_map(|row_ix| {
                let start = row_ix * GIF_COLS;
                let slice = self.gif_results.get(start..)?;
                let items = &slice[..slice.len().min(GIF_COLS)];
                Some(
                    h_flex()
                        .id(("gif-row", row_ix))
                        .h(px(108.))
                        .pb_2()
                        .gap_2()
                        .children(items.iter().enumerate().map(|(col, gif)| {
                            let idx = start + col;
                            let picked = gif.clone();
                            let title = SharedString::from(gif.title.clone());
                            let is_hovered = self.hovered_gif == Some(idx);
                            let anim_url = gif.preview_url.clone();
                            div()
                                .id(("gif-item", idx))
                                .relative()
                                .w(tile)
                                .h(tile)
                                .rounded_md()
                                .overflow_hidden()
                                .bg(secondary)
                                .cursor_pointer()
                                .hover(|el| el.opacity(0.92))
                                .when(!gif.title.is_empty(), |el| {
                                    el.tooltip(move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(title.clone())
                                            .build(window, cx)
                                    })
                                })
                                .child(
                                    img(gif.still_url.clone())
                                        .size_full()
                                        .object_fit(ObjectFit::Cover)
                                        .rounded(px(6.)),
                                )
                                .when(is_hovered && gif.preview_url != gif.still_url, |el| {
                                    el.child(
                                        div().absolute().inset_0().child(
                                            crate::media::Picture::new(gif.preview_url.clone())
                                                .fit(ObjectFit::Cover)
                                                .radius(px(6.)),
                                        ),
                                    )
                                })
                                .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                                    if *inside {
                                        if this.hovered_gif != Some(idx) {
                                            if let Some(prev) =
                                                this.active_anim_url.replace(anim_url.clone())
                                                && prev != anim_url
                                            {
                                                this.stale_gif_urls.push(prev);
                                            }
                                            this.hovered_gif = Some(idx);
                                            cx.notify();
                                        }
                                    } else if this.hovered_gif == Some(idx) {
                                        this.hovered_gif = None;
                                        if let Some(prev) = this.active_anim_url.take() {
                                            this.stale_gif_urls.push(prev);
                                        }
                                        cx.notify();
                                    }
                                }))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(EmojiPickerEvent::PickGif(picked.clone()));
                                }))
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// The 3-column GIFs tab content, matching Slack's `"Search all GIFs on Giphy"` grid.
    pub(super) fn render_gifs_body(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, secondary, hover, foreground, muted) = (
            theme.border,
            theme.secondary,
            theme.secondary_hover,
            theme.foreground,
            theme.muted_foreground,
        );
        let tile_w = px(100.);
        let tile_h = px(100.);
        let body_h = px(GRID_HEIGHT + 86.);

        if self.gif_loading {
            return div()
                .id("gif-grid-loading")
                .h(body_h)
                .px_4()
                .pt_3()
                .pb_3()
                .flex()
                .flex_wrap()
                .content_start()
                .gap_2()
                .children((0usize..9).map(|ix| {
                    div()
                        .id(("gif-skel", ix))
                        .w(tile_w)
                        .h(tile_h)
                        .rounded_md()
                        .bg(secondary)
                }))
                .into_any_element();
        }

        if !self.gif_results.is_empty() {
            let row_count = self.gif_results.len().div_ceil(3);
            return uniform_list(
                "gif-grid",
                row_count,
                cx.processor(|this, range: Range<usize>, _, cx| this.render_gif_rows(range, cx)),
            )
            .h(body_h)
            .px_4()
            .pt_3()
            .pb_2()
            .track_scroll(&self.gif_grid)
            .into_any_element();
        }

        if !self.fixtures.sample && self.gif_loaded_query.is_some() {
            return div()
                .id("gif-grid-empty")
                .h(body_h)
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(format!("No GIFs match \u{201C}{}\u{201D}", self.gif_query))
                .into_any_element();
        }

        let needle = self.gif_query.trim().to_lowercase();
        let presets = filter_gif_presets(&self.gif_query);
        let query_banner = (!needle.is_empty()).then(|| {
            let query = self.gif_query.trim().to_owned();
            let display = SharedString::from(format!("Send /giphy \u{201C}{query}\u{201D}"));
            h_flex()
                .id("gif-run-query")
                .w_full()
                .px_3()
                .py_2()
                .mb_2()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(secondary)
                .cursor_pointer()
                .hover(|el| el.bg(hover))
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::Sticker)
                                .size(px(15.))
                                .text_color(foreground),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(foreground)
                                .child(display),
                        ),
                )
                .child(div().text_xs().text_color(muted).child("Return"))
                .on_click(cx.listener(move |_, _, _, cx| {
                    cx.emit(EmojiPickerEvent::RunGiphy(query.clone()));
                }))
        });

        v_flex()
            .id("gif-grid-presets")
            .h(body_h)
            .px_4()
            .pt_3()
            .pb_3()
            .overflow_y_scroll()
            .children(query_banner)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_2()
                    .children(
                        presets
                            .into_iter()
                            .enumerate()
                            .map(|(ix, (label, glyph, tint))| {
                                let bg = Hsla::from(rgb(tint)).opacity(if theme.mode.is_dark() {
                                    0.85
                                } else {
                                    0.9
                                });
                                let fg = Hsla::from(rgb(0xFF_FF_FF));
                                v_flex()
                                    .id(("gif-preset", ix))
                                    .w(tile_w)
                                    .h(px(74.))
                                    .rounded_md()
                                    .bg(bg)
                                    .cursor_pointer()
                                    .hover(|el| el.opacity(0.88))
                                    .items_center()
                                    .justify_center()
                                    .gap_1()
                                    .child(div().text_size(px(22.)).child(glyph))
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(fg)
                                            .child(label),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(EmojiPickerEvent::RunGiphy(label.to_owned()));
                                    }))
                            }),
                    ),
            )
            .into_any_element()
    }
}
