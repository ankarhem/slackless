//! Searchable Slack-style emoji & GIF picker overlay entity.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::gpui::prelude::FluentBuilder as _;
use gpui_kit::gpui::{
    AnyElement, AppContext as _, BoxShadow, Context, Entity, EventEmitter, Focusable as _,
    FontWeight, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollStrategy, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    UniformListScrollHandle, Window, div, point, px, uniform_list,
};
use slackless_core::api::GifItem;

use super::catalog::{CATALOG, Category, FREQUENT_SEED, TABS, WORK, lookup, unicode_emoji};
use super::gifs::{GIF_PRESETS, evict_gif_asset};
use super::resolve::{
    emoji_element, normalize_emoji_query, record_frequent, search_emoji_choices, toned,
    toned_choice,
};
use crate::fixtures::Fixtures;

/// Grid metrics: `COLUMNS` cells of `CELL` x `ROW` (a section title takes a row too, so the
/// grid renders lazily as a uniform list), the scrolling height and the emoji size. The width
/// adds the grid's `px_4` padding on both sides.
const COLUMNS: usize = 9;
const CELL: f32 = 36.;
const ROW: f32 = 32.;
pub(super) const GRID_HEIGHT: f32 = 300.;
const GLYPH: f32 = 22.;
const PICKER_WIDTH: f32 = COLUMNS as f32 * CELL + 32.;

/// Top-level tab of the emoji / GIF picker (Slack's `Emoji` | `GIFs` switcher).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerTab {
    Emoji,
    Gifs,
}

/// A picked emoji: the shortcode as Slack names it (tone suffix included) and its glyph;
/// `glyph` is `None` for workspace custom emoji, which only exist as images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiChoice {
    pub name: String,
    pub glyph: Option<String>,
}

pub enum EmojiPickerEvent {
    Pick(EmojiChoice),
    /// A GIF tile from `gif.featured` / `gif.search` was clicked: stage it in the composer.
    PickGif(GifItem),
    /// Request featured GIFs (`""`) or search GIFs (`query`) from Slack's `gif.*` API.
    SearchGifs(String),
    /// Run `/giphy <query>` in fixture/fallback mode.
    RunGiphy(String),
    ToneChanged(u8),
}

/// One row of the picker's grid: a section title, or up to [`COLUMNS`] emoji.
enum Row {
    Title(&'static str),
    Cells(Vec<EmojiChoice>),
}

pub struct EmojiPicker {
    pub(super) fixtures: Arc<Fixtures>,
    search: Entity<TextareaState>,
    /// Active top tab (`Emoji` or `GIFs`).
    tab: PickerTab,
    /// Whether the top `Emoji` | `GIFs` tab switcher is shown (true for composers, false for
    /// message reactions).
    show_gifs: bool,
    /// Lowercased search text for Emoji (spaces normalized to `_`).
    query: String,
    /// Raw trimmed search text for GIFs.
    pub(super) gif_query: String,
    /// Live GIF results returned by Slack's `gif.featured` / `gif.search` endpoints.
    pub(super) gif_results: Vec<GifItem>,
    /// Last query whose results are currently in `gif_results` (`None` before first load).
    pub(super) gif_loaded_query: Option<String>,
    /// Preview URLs that should have their decoded frames & GPU textures evicted on the next
    /// render (when a new search replaces old results or hover moves off an animated GIF).
    pub(super) stale_gif_urls: Vec<String>,
    /// Index of the GIF tile currently under the pointer (the only tile animating at once).
    pub(super) hovered_gif: Option<usize>,
    /// Animated `preview_url` currently decoded for `hovered_gif`, evicted as soon as hover moves.
    pub(super) active_anim_url: Option<String>,
    pub(super) gif_grid: UniformListScrollHandle,
    pub(super) gif_loading: bool,
    pub(super) gif_debounce: Option<Task<()>>,
    /// Slack skin tone, 1 (default) to 6.
    tone: u8,
    tones_open: bool,
    /// Base shortcodes, most used first: the user's `emoji_use` pref or [`FREQUENT_SEED`], each
    /// pick moved to the front.
    frequent: Vec<String>,
    /// The grid as [`EmojiPicker::rebuild`] last laid it out.
    rows: Vec<Row>,
    /// The first row of each section, with the tab it belongs to.
    sections: Vec<(Category, usize)>,
    grid: UniformListScrollHandle,
    /// The tab last clicked, highlighted until the grid is scrolled by hand: the section it
    /// jumped to may sit too low in the grid to reach the top.
    jumped: Option<Category>,
    /// Cell under the pointer, previewed in the footer.
    hovered: Option<EmojiChoice>,
    /// True when picking emoji inside a Slack Connect (`is_ext_shared`) channel (`F-COLLAB-878`).
    external_channel: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EmojiPickerEvent> for EmojiPicker {}

impl EmojiPicker {
    pub fn new(
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut gpui_kit::gpui::App,
    ) -> Entity<EmojiPicker> {
        cx.new(|cx| {
            let search = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .placeholder("Search all emoji")
                    .auto_grow(1, 1)
            });
            let subscriptions = vec![cx.subscribe_in(
                &search,
                window,
                |this: &mut EmojiPicker, search, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => {
                        let raw = search.read(cx).value().trim().to_owned();
                        match this.tab {
                            PickerTab::Emoji => {
                                let query = normalize_emoji_query(&raw);
                                if query != this.query {
                                    this.query = query;
                                    this.rebuild();
                                    cx.notify();
                                }
                            }
                            PickerTab::Gifs => {
                                if raw != this.gif_query {
                                    this.gif_query = raw;
                                    this.request_gifs(true, cx);
                                    cx.notify();
                                }
                            }
                        }
                    }
                    InputEvent::PressEnter { .. } => match this.tab {
                        PickerTab::Emoji => {
                            if let Some(first) = this.first_cell().cloned() {
                                this.pick(first, cx);
                            }
                        }
                        PickerTab::Gifs => {
                            if let Some(first) = this.gif_results.first().cloned() {
                                cx.emit(EmojiPickerEvent::PickGif(first));
                            } else {
                                let query = this.gif_query.trim().to_owned();
                                if !query.is_empty() {
                                    cx.emit(EmojiPickerEvent::RunGiphy(query));
                                } else if let Some((preset, _, _)) = GIF_PRESETS.first() {
                                    cx.emit(EmojiPickerEvent::RunGiphy((*preset).to_owned()));
                                }
                            }
                        }
                    },
                    _ => {}
                },
            )];
            let frequent = if fixtures.frequent_emoji.is_empty() {
                FREQUENT_SEED
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect()
            } else {
                fixtures.frequent_emoji.clone()
            };
            EmojiPicker {
                tone: fixtures.skin_tone,
                fixtures,
                search,
                tab: PickerTab::Emoji,
                show_gifs: false,
                query: String::new(),
                gif_query: String::new(),
                gif_results: Vec::new(),
                gif_loaded_query: None,
                stale_gif_urls: Vec::new(),
                hovered_gif: None,
                active_anim_url: None,
                gif_grid: UniformListScrollHandle::new(),
                gif_loading: false,
                gif_debounce: None,
                tones_open: false,
                frequent,
                rows: Vec::new(),
                sections: Vec::new(),
                grid: UniformListScrollHandle::new(),
                jumped: None,
                hovered: None,
                external_channel: false,
                _subscriptions: subscriptions,
            }
        })
    }

    /// Refresh workspace fixtures (e.g. after `emoji_changed` or snapshot reload) so newly added
    /// or removed custom workspace emoji appear immediately in the picker.
    pub fn set_fixtures(&mut self, fixtures: Arc<Fixtures>, cx: &mut Context<Self>) {
        self.fixtures = fixtures;
        self.rebuild();
        cx.notify();
    }

    /// Reset workspace-scoped state when switching workspaces: swap fixtures/frequent/skin tone,
    /// cancel pending GIF fetches, evict decoded GIF GPU textures, and rebuild rows.
    pub fn sync_workspace(
        &mut self,
        fixtures: Arc<Fixtures>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.release_gifs(window, cx);
        self.tone = fixtures.skin_tone;
        self.frequent = if fixtures.frequent_emoji.is_empty() {
            FREQUENT_SEED
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        } else {
            fixtures.frequent_emoji.clone()
        };
        self.fixtures = fixtures;
        self.query.clear();
        self.gif_query.clear();
        self.tones_open = false;
        self.external_channel = false;
        self.rebuild();
        cx.notify();
    }

    /// Mark whether the picker is opened for an externally shared Slack Connect channel (`F-COLLAB-878`).
    pub fn set_external_channel(&mut self, external: bool, cx: &mut Context<Self>) {
        if self.external_channel != external {
            self.external_channel = external;
            cx.notify();
        }
    }

    /// Reset to the top of the grid with an empty, focused search on `tab`.
    pub fn open_tab(
        &mut self,
        tab: PickerTab,
        show_gifs: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_gifs = show_gifs;
        self.tab = if show_gifs { tab } else { PickerTab::Emoji };
        self.query.clear();
        self.gif_query.clear();
        self.tones_open = false;
        self.rebuild();
        let placeholder = match self.tab {
            PickerTab::Emoji => "Search all emoji",
            PickerTab::Gifs => "Search all GIFs on Giphy",
        };
        self.search.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
            state.set_value("", window, cx);
        });
        self.search.read(cx).focus_handle(cx).focus(window, cx);
        if self.tab == PickerTab::Gifs {
            self.request_gifs(false, cx);
        }
        cx.notify();
    }

    fn switch_tab(&mut self, tab: PickerTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        if self.tab == PickerTab::Gifs {
            self.release_gifs(window, cx);
        }
        self.open_tab(tab, self.show_gifs, window, cx);
    }

    /// Restore locally persisted frequent emoji and/or skin tone at launch.
    pub fn restore(
        &mut self,
        frequent: Option<Vec<String>>,
        tone: Option<u8>,
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        if let Some(list) = frequent.filter(|list| !list.is_empty()) {
            self.frequent = list;
            changed = true;
        }
        if let Some(tone) = tone.filter(|t| (1..=6).contains(t)) {
            self.tone = tone;
            changed = true;
        }
        if changed {
            self.rebuild();
            cx.notify();
        }
    }

    pub fn frequent(&self) -> &[String] {
        &self.frequent
    }

    fn pick(&mut self, choice: EmojiChoice, cx: &mut Context<Self>) {
        record_frequent(&mut self.frequent, &choice.name);
        cx.emit(EmojiPickerEvent::Pick(choice));
    }

    /// Lay the grid out again, scrolled to the top: the search matches, or every section in
    /// Slack's order.
    fn rebuild(&mut self) {
        self.rows.clear();
        self.sections.clear();
        self.hovered = None;
        self.jumped = None;
        self.grid.scroll_to_item_strict(0, ScrollStrategy::Top);
        if !self.query.is_empty() {
            let found = search_emoji_choices(
                &self.query,
                self.tone,
                self.fixtures.custom_emoji.keys().map(String::as_str),
            );
            self.push_section(Category::Frequent, "Search results", found);
            return;
        }
        let frequent = self
            .frequent
            .iter()
            .filter_map(|name| self.choice(name))
            .collect();
        self.push_section(Category::Frequent, "Frequently Used", frequent);
        let work = WORK.iter().filter_map(|name| self.choice(name)).collect();
        self.push_section(Category::Frequent, "Getting Work Done", work);
        for (category, title, list) in CATALOG {
            let cells = list
                .iter()
                .filter_map(|name| lookup(name))
                .map(|emoji| toned_choice(emoji.name, emoji.emoji, self.tone))
                .collect();
            self.push_section(category, title, cells);
        }
        let custom = self.custom_choices().collect();
        self.push_section(Category::Custom, "Custom", custom);
    }

    /// Append a titled section, [`COLUMNS`] cells to a row; an empty one is left out.
    fn push_section(&mut self, category: Category, title: &'static str, cells: Vec<EmojiChoice>) {
        if cells.is_empty() {
            return;
        }
        self.sections.push((category, self.rows.len()));
        self.rows.push(Row::Title(title));
        let mut cells = cells.into_iter();
        loop {
            let row: Vec<EmojiChoice> = cells.by_ref().take(COLUMNS).collect();
            if row.is_empty() {
                break;
            }
            self.rows.push(Row::Cells(row));
        }
    }

    /// The cell for shortcode `name`: a catalog or standard emoji in the picker's skin tone, or
    /// a workspace custom emoji; `None` when it is neither (say, a deleted custom emoji).
    fn choice(&self, name: &str) -> Option<EmojiChoice> {
        if let Some(emoji) = lookup(name) {
            return Some(toned_choice(emoji.name, emoji.emoji, self.tone));
        }
        if let Some(emoji) = unicode_emoji(name) {
            return Some(toned_choice(name, emoji, self.tone));
        }
        self.fixtures.custom_emoji(name).map(|_| EmojiChoice {
            name: name.to_owned(),
            glyph: None,
        })
    }

    fn custom_choices(&self) -> impl Iterator<Item = EmojiChoice> + '_ {
        self.fixtures.custom_emoji.keys().map(|name| EmojiChoice {
            name: name.clone(),
            glyph: None,
        })
    }

    fn first_cell(&self) -> Option<&EmojiChoice> {
        self.rows.iter().find_map(|row| match row {
            Row::Cells(cells) => cells.first(),
            Row::Title(_) => None,
        })
    }

    fn cell_at(&self, row: usize, col: usize) -> Option<&EmojiChoice> {
        match self.rows.get(row)? {
            Row::Cells(cells) => cells.get(col),
            Row::Title(_) => None,
        }
    }

    /// Scroll the grid to `category`'s section, clearing a search first.
    fn jump(&mut self, category: Category, window: &mut Window, cx: &mut Context<Self>) {
        if !self.query.is_empty() {
            self.query.clear();
            self.rebuild();
            self.search
                .update(cx, |state, cx| state.set_value("", window, cx));
        }
        if let Some(&(_, row)) = self
            .sections
            .iter()
            .find(|(section, _)| *section == category)
        {
            self.grid.scroll_to_item_strict(row, ScrollStrategy::Top);
            self.jumped = Some(category);
        }
        cx.notify();
    }

    /// The tab to highlight: search while searching, the tab last clicked, else the one whose
    /// section is at the top of the grid.
    fn active_tab(&self) -> Category {
        if !self.query.is_empty() {
            return Category::Frequent;
        }
        if let Some(category) = self.jumped {
            return category;
        }
        let top = {
            let grid = self.grid.0.borrow();
            grid.deferred_scroll_to_item.map_or_else(
                || (-grid.base_handle.offset().y / px(ROW)) as usize,
                |deferred| deferred.item_index,
            )
        };
        self.sections
            .iter()
            .take_while(|(_, row)| *row <= top)
            .last()
            .map_or(Category::Frequent, |(category, _)| *category)
    }

    /// The grid rows in `range`, for the lazily drawn list.
    fn render_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (foreground, hover) = (theme.foreground, theme.secondary_hover);
        range
            .filter_map(|ix| match self.rows.get(ix)? {
                Row::Title(title) => Some(
                    div()
                        .h(px(ROW))
                        .flex()
                        .items_center()
                        .px_1()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(*title)
                        .into_any_element(),
                ),
                Row::Cells(cells) => Some(
                    h_flex()
                        .id(("emoji-row", ix))
                        .h(px(ROW))
                        .children(
                            cells
                                .iter()
                                .enumerate()
                                .map(|(col, choice)| self.cell(ix, col, choice, hover, cx)),
                        )
                        .into_any_element(),
                ),
            })
            .collect()
    }

    fn cell(
        &self,
        row: usize,
        col: usize,
        choice: &EmojiChoice,
        hover: Hsla,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let face = match &choice.glyph {
            Some(glyph) => div()
                .text_size(px(GLYPH))
                .child(glyph.clone())
                .into_any_element(),
            None => emoji_element(&choice.name, &self.fixtures, px(GLYPH)),
        };
        div()
            .id(("emoji-cell", col))
            .w(px(CELL))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .hover(|cell| cell.bg(hover))
            .child(face)
            .on_hover(cx.listener(move |this, inside: &bool, _, cx| {
                if *inside {
                    this.hovered = this.cell_at(row, col).cloned();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(choice) = this.cell_at(row, col).cloned() {
                    this.pick(choice, cx);
                }
            }))
    }
}

impl Render for EmojiPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.stale_gif_urls.is_empty() {
            let stale = std::mem::take(&mut self.stale_gif_urls);
            window.defer(cx, move |window, cx| {
                for url in stale {
                    evict_gif_asset(&url, window, cx);
                }
            });
        }
        let theme = cx.theme();
        let border = theme.border;
        let input_border = theme.input;
        let ring = theme.ring;
        let background = theme.background;
        let footer_bg = theme.muted;
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let hover = theme.secondary_hover;
        let active_bg = theme.secondary_active;

        // Top `Emoji` | `GIFs` switcher matching official Slack's picker header.
        let top_tabs = self.show_gifs.then(|| {
            let emoji_active = self.tab == PickerTab::Emoji;
            let gifs_active = self.tab == PickerTab::Gifs;
            let transparent = Hsla::transparent_black();
            let emoji_accent = crate::theme::emoji_tab_accent();
            h_flex()
                .w_full()
                .h(px(44.))
                .border_b_1()
                .border_color(border)
                .child(
                    h_flex()
                        .id("picker-tab-emoji")
                        .flex_1()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .border_b_2()
                        .border_color(if emoji_active {
                            foreground
                        } else {
                            transparent
                        })
                        .when(!emoji_active, |el| el.hover(|el| el.bg(hover.opacity(0.4))))
                        .child(
                            Icon::new(IconName::FaceSlightlySmiling)
                                .size(px(16.))
                                .text_color(if emoji_active { emoji_accent } else { muted }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if emoji_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if emoji_active { foreground } else { muted })
                                .child("Emoji"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_tab(PickerTab::Emoji, window, cx);
                        })),
                )
                .child(
                    h_flex()
                        .id("picker-tab-gifs")
                        .flex_1()
                        .h_full()
                        .items_center()
                        .justify_center()
                        .gap_1p5()
                        .cursor_pointer()
                        .border_b_2()
                        .border_color(if gifs_active { foreground } else { transparent })
                        .when(!gifs_active, |el| el.hover(|el| el.bg(hover.opacity(0.4))))
                        .child(
                            Icon::new(IconName::Sticker)
                                .size(px(16.))
                                .text_color(if gifs_active { foreground } else { muted }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if gifs_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if gifs_active { foreground } else { muted })
                                .child("GIFs"),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.switch_tab(PickerTab::Gifs, window, cx);
                        })),
                )
        });

        // The search field in Slack's focus ring: the accent border inside a soft halo.
        let m = crate::preferences::control_metrics(cx);
        let focused = self.search.read(cx).focus_handle(cx).is_focused(window);
        let search = h_flex()
            .mx_4()
            .mt_3()
            .px_1()
            .h(m.height)
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(if focused { ring } else { input_border })
            .when(focused, |field| {
                field.shadow(vec![BoxShadow {
                    color: ring.opacity(0.3),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                    inset: false,
                }])
            })
            .bg(background)
            .child(
                div().flex_1().min_w_0().child(
                    Textarea::new(&self.search)
                        .with_size(m.input_size)
                        .appearance(false)
                        .bordered(false),
                ),
            );

        if self.tab == PickerTab::Gifs {
            return v_flex()
                .id("emoji-picker")
                .occlude()
                .on_scroll(|_, _, cx| cx.stop_propagation())
                .w(px(PICKER_WIDTH))
                .rounded_lg()
                .border_1()
                .border_color(border)
                .bg(background)
                .shadow_lg()
                .text_color(foreground)
                .children(top_tabs)
                .child(search)
                .child(self.render_gifs_body(cx));
        }

        if self.rows.is_empty() && self.query.is_empty() {
            self.rebuild();
        }

        // Slack's category row: outline icons, the section in view (or the one clicked) boxed.
        let active = self.active_tab();
        let has_custom = !self.fixtures.custom_emoji.is_empty();
        let tabs = h_flex().justify_between().px_3().pt_3().children(
            TABS.iter()
                .filter(|(category, _)| *category != Category::Custom || has_custom)
                .enumerate()
                .map(|(ix, &(category, icon))| {
                    let current = category == active;
                    div()
                        .id(("emoji-tab", ix))
                        .w(px(32.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .when(current, |tab| tab.bg(active_bg))
                        .when(!current, |tab| tab.hover(|tab| tab.bg(hover)))
                        .child(Icon::new(icon).size(px(18.)).text_color(if current {
                            foreground
                        } else {
                            muted
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.jump(category, window, cx);
                        }))
                }),
        );

        let grid = if self.rows.is_empty() {
            div()
                .mt_1p5()
                .h(px(GRID_HEIGHT))
                .p_4()
                .text_sm()
                .text_color(muted)
                .child(format!("No emoji match \u{201C}{}\u{201D}", self.query))
                .into_any_element()
        } else {
            uniform_list(
                "emoji-grid",
                self.rows.len(),
                cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx)),
            )
            .mt_1p5()
            .h(px(GRID_HEIGHT))
            .px_4()
            .track_scroll(&self.grid)
            .on_scroll(cx.listener(|this, _, _, cx| {
                // Scrolling by hand hands the highlight back to the section in view.
                this.jumped = None;
                cx.notify();
            }))
            .into_any_element()
        };

        // Footer: the hovered emoji on the left, Slack's skin-tone button (or its six choices)
        // on the right.
        let preview = h_flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .gap_2()
            .when_some(self.hovered.as_ref(), |preview, choice| {
                preview
                    .child(match &choice.glyph {
                        Some(glyph) => div()
                            .text_size(px(28.))
                            .child(glyph.clone())
                            .into_any_element(),
                        None => emoji_element(&choice.name, &self.fixtures, px(28.)),
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child(format!(":{}:", choice.name)),
                    )
            });
        let hand = unicode_emoji("raised_hand");
        let skin = if self.tones_open {
            h_flex()
                .gap_0p5()
                .children((1..=6u8).map(|tone| {
                    div()
                        .id(("emoji-tone", usize::from(tone)))
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(18.))
                        .when(tone == self.tone, |choice| {
                            choice.border_1().border_color(ring)
                        })
                        .hover(|choice| choice.bg(hover))
                        .children(hand.map(|hand| toned(hand, tone)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.tone = tone;
                            this.tones_open = false;
                            this.rebuild();
                            cx.emit(EmojiPickerEvent::ToneChanged(tone));
                            cx.notify();
                        }))
                }))
                .into_any_element()
        } else {
            h_flex()
                .id("emoji-skin-tone")
                .flex_none()
                .h(px(32.))
                .px_2p5()
                .gap_1p5()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|button| button.bg(hover))
                .child(
                    div()
                        .text_size(px(18.))
                        .children(hand.map(|hand| toned(hand, self.tone))),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted)
                        .child("Skin Tone"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.tones_open = true;
                    cx.notify();
                }))
                .into_any_element()
        };
        let footer = h_flex()
            .h(px(56.))
            .px_4()
            .gap_2()
            .items_center()
            .rounded_b_lg()
            .bg(footer_bg)
            .child(preview)
            .child(skin);

        let external_notice = self.external_channel.then(|| {
            h_flex()
                .mx_4()
                .mt_2()
                .px_2()
                .py_1()
                .rounded_md()
                .border_1()
                .border_color(border)
                .bg(footer_bg)
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(muted)
                .child(
                    Icon::new(IconName::Globe)
                        .flex_none()
                        .size(px(12.))
                        .text_color(muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child("Slack Connect \u{b7} Custom workspace emoji are visible to external people here"),
                )
        });

        v_flex()
            .id("emoji-picker")
            .occlude()
            .on_scroll(|_, _, cx| cx.stop_propagation())
            .w(px(PICKER_WIDTH))
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(background)
            .shadow_lg()
            .text_color(foreground)
            .children(top_tabs)
            .child(tabs)
            .child(search)
            .children(external_notice)
            .child(grid)
            .child(footer)
    }
}
